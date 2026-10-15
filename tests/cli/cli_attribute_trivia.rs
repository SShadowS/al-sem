//! #80 end to end: an `[EventSubscriber]` separated from its handler by a comment,
//! a pragma or an `#if` line still binds, so `alsem digest` sees the handler's
//! `Commit()` through the event dispatch from `FireN`.
//!
//! Each workspace is stated as source text in a tempdir (like
//! `cli_s6_consumers.rs`) and run through `run_digest_pipeline` with
//! `deterministic: true` (like `cli_b_digest_differential.rs`). No golden file.

use std::collections::BTreeMap;
use std::path::Path;

use al_sem::engine::l5::digest_cli::run_digest_pipeline;

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Write `app.json` + `src/Trivia.al` into `ws`.
fn workspace(ws: &Path, al: &str) {
    write(
        &ws.join("app.json"),
        r#"{"id":"aaaa8080-0000-0000-0000-000000000080","name":"Trivia80","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{"from":50100,"to":50199}]}"#,
    );
    write(&ws.join("src/Trivia.al"), al);
}

/// Digest `fires` in a workspace holding `al`; returns, per routine display name,
/// whether its effects include `COMMIT`.
fn commit_by_routine(al: &str, fires: &[&str]) -> BTreeMap<String, bool> {
    let dir = tempfile::tempdir().unwrap();
    workspace(dir.path(), al);
    let r = run_digest_pipeline(
        dir.path(),
        None,
        Some(fires.iter().map(|s| s.to_string()).collect()),
        None,
        "issue-80",
        true,  // deterministic
        None,  // max_paths
        false, // cross-app, the CLI default (#66)
    )
    .expect("digest runs");
    let v: serde_json::Value = serde_json::from_str(&r.json_text).expect("json");
    let entries = v
        .pointer("/payload/entries")
        .and_then(|e| e.as_array())
        .unwrap_or_else(|| panic!("no payload.entries: {}", r.json_text));
    let mut out = BTreeMap::new();
    for e in entries {
        let name = e
            .pointer("/routine/display")
            .and_then(|d| d.as_str())
            .expect("routine.display")
            .to_string();
        let commit = e
            .get("effects")
            .and_then(|x| x.as_array())
            .map(|fx| {
                fx.iter()
                    .any(|f| f.get("type").and_then(|t| t.as_str()) == Some("COMMIT"))
            })
            .unwrap_or(false);
        out.insert(name, commit);
    }
    out
}

/// One `OnEN` + `FireN` per separator kind (1 line comment, 2 block comment,
/// 3 `#pragma`, 4 same-line comment, 5 `#if` around the attribute only, 6 the
/// attribute before an `#if` that wraps the handler), a no-trivia control (0),
/// and the target variant: `#if not X [EventSubscriber(…, #if X 'OnA' #else
/// 'OnB' #endif …)] #endif` binds to `OnB` only.
const MAIN: &str = r##"codeunit 50180 "Trivia Pub"
{
    procedure Fire1()
    begin
        OnE1();
    end;

    procedure Fire2()
    begin
        OnE2();
    end;

    procedure Fire3()
    begin
        OnE3();
    end;

    procedure Fire4()
    begin
        OnE4();
    end;

    procedure Fire5()
    begin
        OnE5();
    end;

    procedure Fire6()
    begin
        OnE6();
    end;

    procedure Fire0()
    begin
        OnE0();
    end;

    procedure FireA()
    begin
        OnA();
    end;

    procedure FireB()
    begin
        OnB();
    end;

    [IntegrationEvent(false, false)]
    local procedure OnE1()
    begin
    end;

    [IntegrationEvent(false, false)]
    local procedure OnE2()
    begin
    end;

    [IntegrationEvent(false, false)]
    local procedure OnE3()
    begin
    end;

    [IntegrationEvent(false, false)]
    local procedure OnE4()
    begin
    end;

    [IntegrationEvent(false, false)]
    local procedure OnE5()
    begin
    end;

    [IntegrationEvent(false, false)]
    local procedure OnE6()
    begin
    end;

    [IntegrationEvent(false, false)]
    local procedure OnE0()
    begin
    end;

    [IntegrationEvent(false, false)]
    local procedure OnA()
    begin
    end;

    [IntegrationEvent(false, false)]
    local procedure OnB()
    begin
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Trivia Pub", 'OnE1', '', false, false)]
    // line comment
    local procedure H1()
    begin
        Commit();
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Trivia Pub", 'OnE2', '', false, false)]
    /* block comment */
    local procedure H2()
    begin
        Commit();
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Trivia Pub", 'OnE3', '', false, false)]
#pragma warning disable AA0150
    local procedure H3()
    begin
        Commit();
    end;
#pragma warning restore AA0150

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Trivia Pub", 'OnE4', '', false, false)] // same-line comment
    local procedure H4()
    begin
        Commit();
    end;

#if not CLEAN99
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Trivia Pub", 'OnE5', '', false, false)]
#endif
    local procedure H5()
    begin
        Commit();
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Trivia Pub", 'OnE6', '', false, false)]
#if X
    local procedure H6(I: Integer)
    begin
        Commit();
    end;
#else
    local procedure H6(T: Text)
    begin
        Commit();
    end;
#endif

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Trivia Pub", 'OnE0', '', false, false)]
    local procedure H0()
    begin
        Commit();
    end;

#if not X
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Trivia Pub",
#if X
        'OnA',
#else
        'OnB',
#endif
        '', false, false)]
#endif
    local procedure HT()
    begin
        Commit();
    end;
}
"##;

/// The merge variant (A19's shape): `P` exists only under X and Y and reads
/// 'OnA' (no `Commit`); `H` is reached only by paths that read 'OnB'.
const MERGE: &str = r##"codeunit 50190 "Trivia Merge"
{
    procedure FireA()
    begin
        OnA();
    end;

    procedure FireB()
    begin
        OnB();
    end;

    [IntegrationEvent(false, false)]
    local procedure OnA()
    begin
    end;

    [IntegrationEvent(false, false)]
    local procedure OnB()
    begin
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Trivia Merge",
#if X and Y
        'OnA',
#else
        'OnB',
#endif
        '', false, false)]
#if X
#if Y
    local procedure P()
    begin
    end;
#endif
#endif
    local procedure H()
    begin
        Commit();
    end;
}
"##;

/// A12 (#80): every separator kind still binds the subscriber, so every `FireN`
/// reaches its handler's `Commit()`. The target variant binds `HT` to `OnB`
/// only, and the merge variant binds `H` to `OnB` only.
#[test]
fn digest_sees_a_subscriber_separated_by_trivia() {
    let fires = [
        "Fire0", "Fire1", "Fire2", "Fire3", "Fire4", "Fire5", "Fire6", "FireA", "FireB",
    ];
    let got = commit_by_routine(MAIN, &fires);
    let want: BTreeMap<String, bool> = fires
        .iter()
        .map(|f| (f.to_string(), *f != "FireA"))
        .collect();
    assert_eq!(got, want, "main workspace: routine -> has COMMIT");

    let got = commit_by_routine(MERGE, &["FireA", "FireB"]);
    let want: BTreeMap<String, bool> = [("FireA".to_string(), false), ("FireB".to_string(), true)]
        .into_iter()
        .collect();
    assert_eq!(got, want, "merge variant: routine -> has COMMIT");
}
