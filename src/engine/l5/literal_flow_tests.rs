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
    let reach = LiteralReach::new(&tree, model);
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

/// r3 §7 R1: two publishers each keep a flag a helper branches on, so the
/// helper is entered with other bindings once a keep is dropped (states the
/// search with every keep followed never met), and the commit behind both
/// flags is ruled out only by the keeps.
const TWO_KEPT_FLAGS: &str = r#"codeunit 50343 "KG Two"
{
    procedure Start()
    var
        H1: Boolean;
        H2: Boolean;
    begin
        H1 := true;
        OnFirst(H1);
        Mid(H1);
        H2 := true;
        OnSecond(H2);
        Mid(H2);
    end;

    local procedure Mid(Flag: Boolean)
    begin
        if not Flag then
            Log();
    end;

    local procedure Log()
    begin
        Commit();
    end;

    [IntegrationEvent(false, false)]
    local procedure OnFirst(var Value: Boolean)
    begin
    end;

    [IntegrationEvent(false, false)]
    local procedure OnSecond(var Value: Boolean)
    begin
    end;
}
"#;

/// r3 §7 R1: replaying the search with every keep followed under a keep
/// choice ([`LiteralReach::dead_under`]) rules out exactly the rows the
/// search and judge under that choice rule out (of those the full search
/// rules out), for every choice of the root's keeps. Hand-stated: the
/// fixture's choices step into helper states the full search never met,
/// and at least one choice makes a ruled-out row live.
///
/// DISCRIMINATION (2026-11-09): `dead_under` reusing every state's recorded
/// successors (`same` always true) fails the equality under `None`; not
/// stepping the new states (`q_new` never filled) fails it too; restored,
/// passes. Both breaks also fail the `cli_prove_flow_values::r1_` tests.
#[test]
fn a_replayed_search_rules_out_what_the_search_does() {
    use super::{ALL, Keeps};
    use std::collections::HashSet;
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("app.json"),
        r#"{"id":"e2e2e2e2-0000-4000-8000-0000000000f3","name":"KG","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("src/Two.al"), TWO_KEPT_FLAGS).unwrap();
    let (base, tree) = crate::engine::l5::digest::wave2_index_tests::tree_over(
        dir.path(),
        crate::engine::l4::cone_derived::WitnessScope::All,
    );
    let model = &base.resolved;
    let start = model
        .workspace
        .routines
        .iter()
        .find(|r| r.name == "Start")
        .expect("precondition: Start")
        .stable_routine_id
        .clone();
    let effects = crate::engine::l5::digest::compute_digest_effects_cli(
        &tree,
        model,
        Some(std::slice::from_ref(&start)),
        false,
    )
    .remove(0)
    .effects;
    let per_site: Vec<&str> = crate::engine::l5::digest::SITE_ENUMERATED.to_vec();
    let reach = LiteralReach::new(&tree, model);
    let (&root, _) = reach.by_stable.get_key_value(start.as_str()).unwrap();
    let reached = reach.search(root, ALL).expect("precondition: not capped");
    let (dead, _) = reach.judge(&reached, &effects, &per_site, ALL);
    assert!(!dead.is_empty(), "precondition: a row ruled out");
    let mut keeps: Vec<u32> = reached
        .1
        .iter()
        .flat_map(|(r, bs)| bs.iter().flat_map(|b| reach.keeps_at(r, b)))
        .collect();
    keeps.sort_unstable();
    keeps.dedup();
    assert_eq!(keeps.len(), 2, "precondition: both publishers' keeps");
    let g = reach.keep_graph(root).expect("precondition: not capped");
    let mut choices: Vec<Keeps> = vec![Keeps::All, Keeps::None];
    for mask in 0..(1u32 << keeps.len()) {
        let set: Vec<u32> = (0..keeps.len())
            .filter(|&j| mask & (1 << j) != 0)
            .map(|j| keeps[j])
            .collect();
        choices.push(Keeps::Only(set.into()));
    }
    let mut rows: Vec<usize> = dead.iter().copied().collect();
    rows.sort_unstable();
    let mut shell = (reached.0.clone(), std::collections::HashMap::new());
    let (mut fresh, mut revived) = (false, false);
    for keep in &choices {
        let r = reach.search(root, keep).expect("not capped");
        fresh |=
            r.1.iter()
                .any(|(f, bs)| bs.iter().any(|b| !g.index.contains_key(&(*f, b.clone()))));
        let want: HashSet<usize> = reach
            .judge(&r, &effects, &per_site, keep)
            .0
            .intersection(&dead)
            .copied()
            .collect();
        revived |= want != dead;
        let got = reach.dead_under(&g, &mut shell, &rows, keep, &effects, &per_site);
        assert_eq!(got, Some(want), "under {keep:?}");
    }
    assert!(
        fresh,
        "precondition: a choice meets states the full search did not"
    );
    assert!(revived, "precondition: a choice makes a ruled-out row live");
}
