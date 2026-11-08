//! r3 E2: the publisher keep rule's cases a workspace fixture cannot state
//! through the CLI: a loaded subscriber whose body is not available.

use crate::engine::l5::literal_reach::LiteralReach;

const SOURCE: &str = r#"table 50340 "FK T" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
codeunit 50341 "FK Pub"
{
    procedure Root()
    var
        T: Record "FK T";
        U: Boolean;
    begin
        U := true;
        OnKeep(U);
        if not U then
            T.Insert();
    end;

    [IntegrationEvent(false, false)]
    local procedure OnKeep(var Value: Boolean)
    begin
    end;
}
codeunit 50342 "FK Sub"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"FK Pub", 'OnKeep', '', false, false)]
    local procedure OnlyReads(var Value: Boolean)
    var
        T: Record "FK T";
    begin
        if Value then
            T.Modify();
    end;
}
"#;

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("app.json"),
        r#"{"id":"e2e2e2e2-0000-4000-8000-0000000000f2","name":"FK","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("src/Keep.al"), SOURCE).unwrap();
    dir
}

/// Whether `Root`'s `T.Insert()` (behind `if not U`) can run, with the
/// subscriber's body available or not.
fn insert_live(subscriber_body: bool) -> bool {
    let tmp = fixture();
    let (mut base, tree) = crate::engine::l5::digest::wave2_index_tests::tree_over(
        tmp.path(),
        crate::engine::l4::cone_derived::WitnessScope::All,
    );
    let sub = base
        .resolved
        .workspace
        .routines
        .iter_mut()
        .find(|r| r.name == "OnlyReads")
        .expect("precondition: the subscriber is a model routine");
    assert!(sub.body_available, "precondition: a source subscriber");
    sub.body_available = subscriber_body;
    let model = &base.resolved;
    let root = model
        .workspace
        .routines
        .iter()
        .find(|r| r.name == "Root")
        .unwrap();
    let op = root
        .record_operations
        .iter()
        .find(|o| o.op.eq_ignore_ascii_case("insert"))
        .expect("precondition: Root's Insert")
        .id
        .clone();
    let reach = LiteralReach::new(&tree.typed_edges, &tree.capability_facts, model);
    reach.live_in_root(&root.stable_routine_id, &op)
}

/// A loaded subscriber that only reads the parameter lets the publisher keep
/// the value; the same subscriber with its body not available does not
/// (nothing proves it unable to change it). Break: skipping the body check
/// (the second case stays dead).
#[test]
fn an_unavailable_subscriber_body_keeps_nothing() {
    assert!(!insert_live(true), "a reading subscriber keeps U = true");
    assert!(insert_live(false), "an unavailable body keeps nothing");
}
