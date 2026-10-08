//! R4-F — ROOT CLASSIFICATION stable-projection differential.
//!
//! For each committed al-sem golden under
//! `tests/r4f-goldens/<fixture>.rootclass.golden.json`, run the Rust source-only
//! L0→L3 pass (`assemble_and_resolve_workspace_with_program_calls(...)`, which now classifies
//! AST roots + overlays `<workspace>/roots.config.json`) over the matching
//! `tests/r0-corpus/<fixture>` workspace, project it to the stable
//! RootClassification form (`project_r4f_root_classifications`), pretty-serialize
//! it (serde_json pretty + trailing newline — the exact on-disk golden form), and
//! assert BYTE-equality.
//!
//! ## Anti-degenerate
//!
//! - `ws-d51-jobqueue` MUST carry a classification whose `kinds` contain
//!   `"job-queue-entrypoint"` (the roots.config overlay path).
//! - `ws-txn-d47-event-pos` MUST carry an `"event-subscriber"` kind (the AST path).

use std::path::PathBuf;

use al_sem::engine::root_classification::{
    R4FRootClassProjection, project_r4f_root_classifications,
};
use al_sem::program::model::program_calls::assemble_and_resolve_workspace_with_program_calls;

use crate::regen;

/// The R4-F root-classification corpus (mirrors al-sem
/// `scripts/r4f-root-classification-projection.ts` `R4F_ROOT_CLASS_FIXTURES`).
const FIXTURES: &[&str] = &[
    "ws-d51-jobqueue",
    "ws-d51-pos",
    "ws-d51-neg",
    "ws-d50-pos",
    "ws-d50-neg",
    "ws-txn-d47-event-pos",
    "ws-txn-d47-pos-http-nocommit",
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn goldens_dir() -> PathBuf {
    repo_root().join("tests").join("r4f-goldens")
}

fn corpus_dir() -> PathBuf {
    repo_root().join("tests").join("r0-corpus")
}

/// `r4f-goldens/manifest.json` documents this specific sub-family (its
/// `fixtures` entries carry `classificationCount`, the root-classification
/// projection's own field) and was read by no test (Task T0.6 — a silently
/// deleted `FIXTURES` entry would pass unnoticed). Checks `>=`, not `==`: the
/// manifest is a frozen al-sem-era provenance floor, not a live inventory.
#[test]
fn manifest_fixture_count_floor() {
    let manifest_path = goldens_dir().join("manifest.json");
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&manifest_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", manifest_path.display())),
    )
    .unwrap_or_else(|e| panic!("{} not valid JSON: {e}", manifest_path.display()));
    let claimed = manifest
        .get("fixtureCount")
        .and_then(|v| v.as_u64())
        .expect("manifest missing fixtureCount") as usize;
    assert!(
        FIXTURES.len() >= claimed,
        "r4f-goldens/manifest.json claims fixtureCount={claimed} but FIXTURES only has {} \
         entries — a fixture may have been silently dropped",
        FIXTURES.len()
    );
}

/// Pretty-serialize + trailing newline — the exact on-disk golden form (matches
/// `r4_differential::pretty_with_newline`).
fn pretty_with_newline(proj: &R4FRootClassProjection) -> String {
    let mut s = serde_json::to_string_pretty(proj).expect("serialize R4-F projection");
    s.push('\n');
    s
}

/// Run the Rust source-only L0→L3 pass + stable projection for one fixture.
fn run_rust(fixture: &str) -> R4FRootClassProjection {
    let fixture_dir = corpus_dir().join(fixture);
    assert!(
        fixture_dir.is_dir(),
        "R4-F golden for {fixture} has no matching in-repo fixture at {} (offline corpus incomplete)",
        fixture_dir.display()
    );
    match assemble_and_resolve_workspace_with_program_calls(&fixture_dir) {
        Some(resolved) => project_r4f_root_classifications(&resolved, fixture),
        None => R4FRootClassProjection {
            fixture_name: fixture.to_string(),
            classification_count: 0,
            classifications: vec![],
        },
    }
}

#[test]
fn r4f_root_classifications_match_goldens() {
    for fixture in FIXTURES {
        let golden_path = goldens_dir().join(format!("{fixture}.rootclass.golden.json"));

        let projection = run_rust(fixture);
        let rust_text = pretty_with_newline(&projection);

        // REGEN path (Task T0.6 — this family previously had none). When
        // `REGEN_TEMP_GOLDENS=1`, write the ENGINE output straight to the golden
        // file instead of comparing — the goldens are Rust-owned baselines (TS
        // oracle retired). `pretty_with_newline` already produces the exact
        // on-disk form the assert path below reads.
        if regen::regen_mode() {
            std::fs::write(&golden_path, &rust_text)
                .unwrap_or_else(|e| panic!("regen write {}: {e}", golden_path.display()));
            eprintln!(
                "REGEN r4f root-classification golden: {}",
                golden_path.display()
            );
            continue;
        }

        let golden_text = std::fs::read_to_string(&golden_path)
            .unwrap_or_else(|e| panic!("cannot read R4-F golden {}: {e}", golden_path.display()));

        assert_eq!(
            rust_text,
            golden_text,
            "R4-F ACCEPTANCE GATE: {fixture} did NOT byte-match its golden ({})",
            golden_path.display()
        );
    }
}

/// #42 / #19: EVERY corpus fixture's root classifications in one golden, one
/// line per classification (`<fixture>\t<compact JSON>`). The per-fixture
/// family above observes only the fixtures listed in `FIXTURES`; a classification
/// change on any other fixture (the four `ws-policy-api-*` API pages, most
/// codeunit `OnRun` roots) used to move NO golden, so "goldens green" said nothing
/// about it. Fixtures are discovered from the corpus directory, so a new fixture
/// is observed automatically; a fixture with no classification still gets a
/// `(none)` line, so dropping all of a fixture's roots is visible too.
#[test]
fn root_classifications_whole_corpus_summary() {
    let mut fixtures: Vec<String> = std::fs::read_dir(corpus_dir())
        .expect("read r0-corpus")
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    fixtures.sort();
    assert!(
        fixtures.len() > 100,
        "corpus discovery found only {}",
        fixtures.len()
    );

    let mut text = String::new();
    for fixture in &fixtures {
        let projection = run_rust(fixture);
        if projection.classifications.is_empty() {
            text.push_str(&format!("{fixture}\t(none)\n"));
        }
        for c in &projection.classifications {
            let line = serde_json::to_string(c).expect("serialize classification");
            text.push_str(&format!("{fixture}\t{line}\n"));
        }
    }

    let golden_path = goldens_dir().join("corpus.rootclass.summary.txt");
    if regen::regen_mode() {
        std::fs::write(&golden_path, &text)
            .unwrap_or_else(|e| panic!("regen write {}: {e}", golden_path.display()));
        eprintln!(
            "REGEN whole-corpus rootclass summary: {}",
            golden_path.display()
        );
        return;
    }
    let golden = std::fs::read_to_string(&golden_path).unwrap_or_else(|e| {
        panic!(
            "cannot read {}: {e} -- mint it with REGEN_TEMP_GOLDENS=1",
            golden_path.display()
        )
    });
    if golden.replace("\r\n", "\n") != text {
        let (g, t): (Vec<&str>, Vec<&str>) = (golden.lines().collect(), text.lines().collect());
        let gone: Vec<&&str> = g.iter().filter(|l| !t.contains(l)).take(10).collect();
        let new: Vec<&&str> = t.iter().filter(|l| !g.contains(l)).take(10).collect();
        panic!(
            "root classifications moved (regenerate with REGEN_TEMP_GOLDENS=1 if intended)\n\
             GONE:\n{gone:#?}\nNEW:\n{new:#?}"
        );
    }
}

#[test]
fn anti_degenerate_jobqueue_has_job_queue_entrypoint() {
    let projection = run_rust("ws-d51-jobqueue");
    assert!(
        projection
            .classifications
            .iter()
            .any(|c| c.kinds.iter().any(|k| k == "job-queue-entrypoint")),
        "ws-d51-jobqueue must produce a job-queue-entrypoint classification (config overlay path)"
    );
}

#[test]
fn anti_degenerate_event_pos_has_event_subscriber() {
    let projection = run_rust("ws-txn-d47-event-pos");
    assert!(
        projection
            .classifications
            .iter()
            .any(|c| c.kinds.iter().any(|k| k == "event-subscriber")),
        "ws-txn-d47-event-pos must produce an event-subscriber classification (AST path)"
    );
}
