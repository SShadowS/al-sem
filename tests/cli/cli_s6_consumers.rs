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
