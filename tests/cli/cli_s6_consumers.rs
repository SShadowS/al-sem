//! Engine-switch S6: every consumer reads the program-backed model. Each test
//! runs one consumer's production entry on a workspace stated as source text in
//! which L3 and the program engine disagree, and asserts the program engine's
//! answer: `Caller.Run6` reaches a write to "S6 Log" only through
//! `MyPage.RunModal()`. L3 reads that as a member it cannot find
//! (`MemberNotFound`, no edge); the program engine resolves it to the page's
//! `OnOpenPage`, which inserts into "S6 Log".

use std::path::Path;

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

pub(crate) fn page_run_workspace(ws: &Path) {
    write(
        &ws.join("app.json"),
        r#"{"id":"aaaa6666-0000-0000-0000-000000000006","name":"S6Ws","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{"from":50600,"to":50699}]}"#,
    );
    write(
        &ws.join("src/S6.al"),
        r#"table 50600 "S6 Log"
{
    fields { field(1; "No."; Integer) { } }
    keys { key(PK; "No.") { } }
}

page 50601 "S6 Page"
{
    trigger OnOpenPage()
    var
        Log: Record "S6 Log";
    begin
        Log.Insert();
    end;
}

codeunit 50602 "S6 Caller"
{
    procedure Run6()
    var
        MyPage: Page "S6 Page";
    begin
        MyPage.RunModal();
    end;
}
"#,
    );
}

/// S6.1 `alsem prove`: the write is reachable, so `writes-table` answers "yes".
#[test]
fn prove_follows_the_program_engines_calls() {
    let dir = tempfile::tempdir().unwrap();
    page_run_workspace(dir.path());
    let r = al_sem::engine::l5::prove::run_prove_pipeline(
        dir.path(),
        "Run6",
        "writes-table:S6 Log",
        "s6",
        true,
    )
    .expect("prove runs");
    let v: serde_json::Value = serde_json::from_str(&r.json_text).expect("json");
    let answer = v
        .pointer("/payload/result/answer")
        .cloned()
        .unwrap_or_default();
    assert_eq!(answer, "yes", "{}", r.json_text);
}

/// S6.2 `alsem digest`: the changed routine's effects include the write.
#[test]
fn digest_follows_the_program_engines_calls() {
    let dir = tempfile::tempdir().unwrap();
    page_run_workspace(dir.path());
    let r = al_sem::engine::l5::digest_cli::run_digest_pipeline(
        dir.path(),
        None,
        Some(vec!["Run6".to_string()]),
        None,
        "s6",
        true,
        None,
    )
    .expect("digest runs");
    assert!(r.json_text.contains("S6 Log"), "{}", r.json_text);
}

/// S6.3 `alsem fingerprint`: Run6's query output names the write.
#[test]
fn fingerprint_follows_the_program_engines_calls() {
    use al_sem::engine::l5::fingerprint_cli::{
        FingerprintFormat, FingerprintOptions, FingerprintOutput, run_fingerprint_pipeline,
    };
    use al_sem::engine::l5::fingerprint_query::WitnessLimit;
    let dir = tempfile::tempdir().unwrap();
    page_run_workspace(dir.path());
    let opts = FingerprintOptions {
        workspace: dir.path(),
        driver_version: "s6",
        format: FingerprintFormat::Json,
        out: None,
        shard: None,
        witness_limit: Some(WitnessLimit::Capped(3)),
        roots: None,
        routine_selectors: vec!["Run6".to_string()],
        include_inherited: true,
        is_query_requested: true,
        deterministic: true,
        strict: false,
        verbosity: "compact",
        inventory_only: false,
        no_roots_config: false,
    };
    let r = run_fingerprint_pipeline(&opts).expect("fingerprint runs");
    let FingerprintOutput::Text(text) = r.output else {
        panic!("query output is text")
    };
    assert!(
        text.contains("aaaa6666-0000-0000-0000-000000000006/table/50600"),
        "Run6's cone holds the \"S6 Log\" insert: {text}"
    );
}

/// S6.4 `alsem diff` (workspace mode): from a page that writes nothing to one that
/// inserts into "S6 Log", `Run6` gains that write through `MyPage.RunModal()`.
#[test]
fn diff_workspace_mode_follows_the_program_engines_calls() {
    use al_sem::engine::gate::diff::CoveragePolicy;
    use al_sem::engine::gate::diff::cli::{DiffCliOptions, run_diff};
    let old = tempfile::tempdir().unwrap();
    let new = tempfile::tempdir().unwrap();
    page_run_workspace(old.path());
    page_run_workspace(new.path());
    // The old page's trigger writes nothing.
    let src = old.path().join("src/S6.al");
    let text = std::fs::read_to_string(&src).unwrap();
    std::fs::write(&src, text.replace("Log.Insert();", "")).unwrap();

    let (old_s, new_s) = (
        old.path().to_string_lossy().to_string(),
        new.path().to_string_lossy().to_string(),
    );
    let out = run_diff(&DiffCliOptions {
        old_arg: &old_s,
        new_arg: &new_s,
        format: "json",
        out: None,
        coverage_policy: CoveragePolicy::Strict,
        renames_path: None,
        fail_on: None,
        strict: false,
        deterministic: true,
        driver_version: "s6",
    });
    let json = out.output.expect("diff output");
    let v: serde_json::Value = serde_json::from_str(&json).expect("json");
    let run6_changes = json.matches("Run6").count();
    assert!(run6_changes > 0, "Run6 gains the S6 Log write: {v:#}");
}

/// A workspace event whose only subscriber names it as an identifier (`OnX`, not
/// `'OnX'`): L3 drops that subscription, the program engine binds it.
fn identifier_subscriber_workspace(ws: &Path) {
    write(
        &ws.join("app.json"),
        r#"{"id":"aaaa6666-0000-0000-0000-000000000016","name":"S6Ev","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{"from":50610,"to":50619}]}"#,
    );
    write(
        &ws.join("src/Ev.al"),
        r#"codeunit 50610 "S6 Pub"
{
    [IntegrationEvent(false, false)]
    procedure OnX()
    begin
    end;
}

codeunit 50611 "S6 Sub"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"S6 Pub", OnX, '', false, false)]
    local procedure HandleX()
    begin
    end;
}
"#,
    );
}

/// S6.5 `alsem events fanout` and `events chains` see the subscriber.
#[test]
fn events_follow_the_program_engines_event_graph() {
    use al_sem::engine::gate::events::{
        EventsChainsOptions, EventsFanoutOptions, run_events_chains, run_events_fanout,
    };
    use al_sem::engine::l5::event_flow::Scope;
    let dir = tempfile::tempdir().unwrap();
    identifier_subscriber_workspace(dir.path());
    let fanout = run_events_fanout(&EventsFanoutOptions {
        workspace: dir.path(),
        format: "json",
        scope: Scope::All,
        coverage_policy: "warn",
        driver_version: "s6",
        deterministic: true,
        strict: false,
    });
    let v: serde_json::Value = serde_json::from_str(&fanout.text).expect("json");
    let counts: Vec<i64> = v
        .pointer("/entries")
        .and_then(|e| e.as_array())
        .expect("entries")
        .iter()
        .map(|e| e["directSubscriberCount"].as_i64().unwrap_or(-1))
        .collect();
    assert_eq!(counts, vec![1], "{}", fanout.text);

    let chains = run_events_chains(&EventsChainsOptions {
        workspace: dir.path(),
        format: "json",
        scope: Scope::All,
        coverage_policy: "warn",
        max_depth: None,
        max_nodes: None,
        driver_version: "s6",
        deterministic: true,
        strict: false,
    });
    assert!(
        chains.text.contains("\"kind\": \"subscriber\""),
        "OnX's chain reaches HandleX: {}",
        chains.text
    );
}
