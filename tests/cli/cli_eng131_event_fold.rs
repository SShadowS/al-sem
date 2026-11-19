//! ENG-131 (id-shape audit): an event id folds the event name with the identifier fold on
//! both sides. Before, the publisher side lowercased (`str::to_lowercase`: a word-final `Σ`
//! becomes `ς`) while the subscriber side folded (`σ`), so a publisher `"OnΒΣ"` and the
//! subscriber naming it had two event ids: the publisher read no subscriber.

use serde_json::Value;
use std::process::Command;

#[test]
fn a_publisher_whose_name_ends_in_capital_sigma_keeps_its_subscriber() {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"13113113-0000-4000-8000-000000000131","name":"E131","publisher":"t","version":"1.0.0.0","dependencies":[]}"#,
    )
    .unwrap();
    std::fs::write(
        t.path().join("src/E.al"),
        r#"codeunit 50131 "E131 Pub"
{
    [IntegrationEvent(false, false)]
    procedure "OnΒΣ"()
    begin
    end;
}

codeunit 50132 "E131 Sub"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"E131 Pub", 'OnΒΣ', '', false, false)]
    local procedure Handle()
    begin
    end;
}
"#,
    )
    .unwrap();
    assert_ne!(
        "OnΒΣ".to_lowercase(),
        al_syntax::fold_identifier("OnΒΣ"),
        "precondition: the two folds differ on this name"
    );
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(["events", "fanout"])
        .arg(t.path())
        .args(["--format", "json", "--deterministic"])
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(
        v["summary"]["totalEvents"], 1,
        "precondition: one event: {v:#}"
    );
    assert_eq!(
        v["summary"]["zeroSubscriberEvents"], 0,
        "the publisher keeps its subscriber: {v:#}"
    );
}
