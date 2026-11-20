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

/// ENG-18: a RecordRef flow fixture whose call chain crosses the evaluation's
/// depth cutoff (`recref_flow::EVAL_DEPTH`, 48) from one root and not from
/// another. `Shallow` reaches `Release` 47 calls deep; `Deep` reaches it 57
/// deep (10 more calls before the shared chain `W1..W45`). `Post`'s insert
/// into `CF Log` runs only for a `CF Doc`, which a `CF Batch` never becomes.
fn depth_fixture() -> tempfile::TempDir {
    let mut src = String::from(
        r#"table 50420 "CF Batch" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50421 "CF Approval" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50422 "CF Doc" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50423 "CF Log" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
codeunit 50424 "CF Deep"
{
    // ENG-159: `Post(V)` reads `V` after the chain returns, so the root's answer needs the whole
    // chain's write-back: one evaluation nested through every call (reads at their position no
    // longer nest for an argument read before its own call).
    procedure Shallow(var Batch: Record "CF Batch")
    var
        V: Variant;
    begin
        V := Batch;
        W1(V);
        Post(V);
    end;

    procedure Deep(var Batch: Record "CF Batch")
    var
        V: Variant;
    begin
        V := Batch;
        D1(V);
        Post(V);
    end;

    local procedure Execute(var V: Variant)
    begin
        Release(V);
    end;

    local procedure Release(var V: Variant)
    var
        RecRef: RecordRef;
        TargetRecRef: RecordRef;
    begin
        RecRef.GetTable(V);
        case RecRef.Number of
            Database::"CF Approval":
                begin
                    TargetRecRef.Open(Database::"CF Doc");
                    V := TargetRecRef;
                end;
        end;
    end;

    local procedure Post(V: Variant)
    var
        Log: Record "CF Log";
        RecRef: RecordRef;
    begin
        RecRef.GetTable(V);
        case RecRef.Number of
            Database::"CF Doc":
                begin
                    Log.Code := 'X';
                    Log.Insert();
                end;
        end;
    end;
"#,
    );
    let chain = |name: &str, n: usize, last: &str| -> String {
        (1..=n)
            .map(|k| {
                let next = if k == n {
                    last.to_string()
                } else {
                    format!("{name}{}", k + 1)
                };
                format!(
                    "\n    local procedure {name}{k}(var V: Variant)\n    begin\n        {next}(V);\n    end;\n"
                )
            })
            .collect()
    };
    src.push_str(&chain("D", 10, "W1"));
    src.push_str(&chain("W", 45, "Execute"));
    src.push_str("}\n");
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("app.json"),
        r#"{"id":"e18e18e1-0000-4000-8000-0000000000e1","name":"ENG18","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("src/Deep.al"), src).unwrap();
    dir
}

/// ENG-18: a root's literal-reach answer does not depend on what else the same
/// read asked first. Asked alone, `Shallow`'s `CF Log` insert is ruled out (the
/// whole chain is within the depth cutoff). Asked after `Deep` (whose chain
/// crosses the cutoff, so a value of the shared chain is "anything" there), the
/// answer must be the same. Before the fix the flow kept one memo and budget
/// for the whole read and marked the cut-off values final, so `Shallow` read
/// `Deep`'s truncated `W` values and kept the insert.
///
/// ENG-159: the fixture has no store, so no code keeps a record of its
/// RecordRefs or Variants; the test states that ([`KeptElsewhere::Nothing`]).
/// Under production's input (any parameter may be kept, the sound rule) the
/// chain's calls may rebind `V`, every value past a call is "anything", and the
/// cutoff could not change an answer. ENG-161's escape facts prove `Nothing`
/// for such a program. The roots read `V` after the chain returns, so the
/// answer nests through every call (reads at their position, ENG-159 Part A, do
/// not nest for an argument read before its own call).
///
/// Discrimination (2026-11-20): one `EvalState` (memo and budget) shared by
/// every evaluation of the read, the pre-ENG-18 shape, fails "Shallow asked
/// after Deep answers as it does alone"; the per-evaluation state, restored,
/// passes. (Sharing only the cut-off values through the results cache stayed
/// green: nested evaluations never consult that cache.)
#[test]
fn a_roots_answer_does_not_depend_on_what_was_asked_before() {
    use crate::engine::l5::literal_reach::KeptElsewhere;
    let reach = |tree, model| LiteralReach::with_kept(tree, model, KeptElsewhere::Nothing);
    let tmp = depth_fixture();
    let (base, tree) = crate::engine::l5::digest::wave2_index_tests::tree_over(
        tmp.path(),
        crate::engine::l4::cone_derived::WitnessScope::All,
    );
    let model = &base.resolved;
    let id = |name: &str| -> String {
        model
            .workspace
            .routines
            .iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("precondition: {name} is a model routine"))
            .stable_routine_id
            .clone()
    };
    let (shallow, deep) = (id("Shallow"), id("Deep"));
    let effects =
        |root: &str| crate::engine::l5::digest::wave2_index_tests::effects_of(&tree, model, root);
    let log_insert = |effects: &[crate::engine::l5::digest::DigestEffectResult]| -> usize {
        effects
            .iter()
            .position(|e| e.effect_type == "DB_INSERT")
            .expect("precondition: the CF Log insert is in the root's effects")
    };
    let (shallow_fx, deep_fx) = (effects(&shallow), effects(&deep));
    let dead = |reach: &LiteralReach, root: &str, fx: &[_]| -> bool {
        reach
            .unreachable_effects(root, fx, &[])
            .0
            .contains(&log_insert(fx))
    };

    let alone = reach(&tree, model);
    assert!(
        dead(&alone, &shallow, &shallow_fx),
        "precondition: asked alone, Shallow's chain is within the cutoff and the insert is ruled out"
    );
    let deep_alone = reach(&tree, model);
    assert!(
        !dead(&deep_alone, &deep, &deep_fx),
        "precondition: Deep's chain crosses the cutoff, so the insert is kept there"
    );

    let batch = reach(&tree, model);
    assert!(!dead(&batch, &deep, &deep_fx), "Deep's own answer");
    assert!(
        dead(&batch, &shallow, &shallow_fx),
        "Shallow asked after Deep answers as it does alone"
    );

    // The other order (review): Shallow's exact values must not reach Deep,
    // whose own evaluation crosses the cutoff.
    let reverse = reach(&tree, model);
    assert!(
        dead(&reverse, &shallow, &shallow_fx),
        "Shallow's own answer"
    );
    assert!(
        !dead(&reverse, &deep, &deep_fx),
        "Deep asked after Shallow answers as it does alone"
    );
}

/// ENG-159 (sol): `this.G` is the global `G`. `TB Box`'s `SetLog` rebinds `G` through
/// `this.G.GetTable(L)`, so `Use`'s insert (only for a `TB Log`) is live from `BoxRoot`;
/// `TB Box2` binds `G` to a `TB Doc` only, so the same insert is ruled out (the precondition:
/// the guard can rule it out). Under production's input every global is kept from entry and may
/// hold anything, which hides the keying, so the test states [`KeptElsewhere::Nothing`] (what
/// ENG-161's escape facts compute for such a program).
///
/// Discrimination (2026-11-20): `lc` keeping a leading `this.` rules `BoxRoot`'s insert out:
/// fails; restored, passes.
#[test]
fn a_this_qualified_global_is_the_global() {
    use crate::engine::l5::literal_reach::KeptElsewhere;
    let src = r#"table 50430 "TB Doc" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
table 50431 "TB Log" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
table 50432 "TB Other" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
codeunit 50433 "TB Box"
{
    var
        G: RecordRef;

    procedure SetDoc()
    var
        D: Record "TB Doc";
    begin
        G.GetTable(D);
    end;

    procedure SetLog()
    var
        L: Record "TB Log";
    begin
        this.G.GetTable(L);
    end;

    procedure Use()
    var
        O: Record "TB Other";
    begin
        if G.Number <> Database::"TB Log" then
            exit;
        O.Insert();
    end;
}
codeunit 50434 "TB Box2"
{
    var
        G: RecordRef;

    procedure SetDoc()
    var
        D: Record "TB Doc";
    begin
        G.GetTable(D);
    end;

    procedure Use()
    var
        O: Record "TB Other";
    begin
        if G.Number <> Database::"TB Log" then
            exit;
        O.Insert();
    end;
}
codeunit 50435 "TB Mgt"
{
    procedure BoxRoot()
    var
        Box: Codeunit "TB Box";
    begin
        Box.SetDoc();
        Box.SetLog();
        Box.Use();
    end;

    procedure Box2Root()
    var
        Box: Codeunit "TB Box2";
    begin
        Box.SetDoc();
        Box.Use();
    end;
}
"#;
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(
        tmp.path().join("app.json"),
        r#"{"id":"e159e159-0000-4000-8000-000000000159","name":"ENG159","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(tmp.path().join("src/Box.al"), src).unwrap();
    let (base, tree) = crate::engine::l5::digest::wave2_index_tests::tree_over(
        tmp.path(),
        crate::engine::l4::cone_derived::WitnessScope::All,
    );
    let model = &base.resolved;
    let id = |name: &str| -> String {
        model
            .workspace
            .routines
            .iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("precondition: {name} is a model routine"))
            .stable_routine_id
            .clone()
    };
    let reach = LiteralReach::with_kept(&tree, model, KeptElsewhere::Nothing);
    let dead = |root: &str| -> bool {
        let fx = crate::engine::l5::digest::wave2_index_tests::effects_of(&tree, model, root);
        let insert = fx
            .iter()
            .position(|e| e.effect_type == "DB_INSERT")
            .expect("precondition: the TB Other insert is in the root's effects");
        reach
            .unreachable_effects(root, &fx, &[])
            .0
            .contains(&insert)
    };
    assert!(
        dead(&id("Box2Root")),
        "precondition: a global bound only to a TB Doc rules the insert out"
    );
    assert!(
        !dead(&id("BoxRoot")),
        "`this.G.GetTable(L)` rebinds the global G"
    );
}

/// TLS-22: two workers that both miss one evaluation's key (forced: the hook
/// holds each after its miss until both have missed) get the serial answer,
/// and the read keeps one answer per key. A third ask after them reads the
/// kept answer and evaluates nothing.
///
/// DISCRIMINATION (2026-11-11): skipping the `results` lookup in
/// `RecRefFlow::evaluate` makes the third ask run the hook again: fails;
/// restored, passes.
#[test]
fn racing_evaluations_keep_one_answer_per_key() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let tmp = depth_fixture();
    let (base, tree) = crate::engine::l5::digest::wave2_index_tests::tree_over(
        tmp.path(),
        crate::engine::l4::cone_derived::WitnessScope::All,
    );
    let model = &base.resolved;
    let shallow = model
        .workspace
        .routines
        .iter()
        .find(|r| r.name == "Shallow")
        .expect("precondition: Shallow is a model routine")
        .stable_routine_id
        .clone();
    let fx = crate::engine::l5::digest::wave2_index_tests::effects_of(&tree, model, &shallow);
    let ask = |reach: &LiteralReach| reach.unreachable_effects(&shallow, &fx, &[]).0;

    let serial = LiteralReach::new(&tree, model);
    let want = ask(&serial);
    let serial_keys = serial.flow.results_len();
    assert!(
        serial_keys > 0,
        "precondition: the root's search evaluates a sensitive routine"
    );

    let reach = LiteralReach::new(&tree, model);
    let misses = Arc::new(AtomicUsize::new(0));
    // The first two misses' keys (hashed), and whether a wait timed out.
    let first_two = Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));
    let timed_out = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let (misses, first_two, timed_out) = (
            Arc::clone(&misses),
            Arc::clone(&first_two),
            Arc::clone(&timed_out),
        );
        reach.flow.set_evaluate_hook(Arc::new(move |key| {
            // Only the first two misses wait, until both happened, at most
            // 10 s: a lost race fails below, never hangs.
            if misses.fetch_add(1, Ordering::SeqCst) < 2 {
                first_two.lock().unwrap().push(key);
                let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
                while misses.load(Ordering::SeqCst) < 2 {
                    if std::time::Instant::now() >= until {
                        timed_out.store(true, Ordering::SeqCst);
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
        }));
    }
    let (a, b) = std::thread::scope(|s| {
        let a = s.spawn(|| ask(&reach));
        let b = s.spawn(|| ask(&reach));
        (a.join().unwrap(), b.join().unwrap())
    });
    assert_eq!(a, want, "the first worker's answer is the serial one");
    assert_eq!(b, want, "the second worker's answer is the serial one");
    assert_eq!(
        reach.flow.results_len(),
        serial_keys,
        "one answer per key, as serially"
    );
    let raced = misses.load(Ordering::SeqCst);
    assert!(raced >= 2, "precondition: both workers missed a key");
    assert!(
        !timed_out.load(Ordering::SeqCst),
        "precondition: the two misses met (no wait timed out)"
    );
    let keys = first_two.lock().unwrap().clone();
    assert!(
        keys.len() == 2 && keys[0] == keys[1],
        "precondition: both workers missed the SAME key ({keys:?})"
    );
    assert_eq!(ask(&reach), want, "the third ask's answer");
    assert_eq!(
        misses.load(Ordering::SeqCst),
        raced,
        "the third ask reads every kept answer and evaluates nothing"
    );
}

/// ENG-14 (sol's part-1 review, round 2): a `case` whose labels come from a `#if` group.
/// Without X the case is `0: Work(); else Commit();`, so `P(1)` commits; a union read
/// (`{0, 1}` plus a real `else`) would rule the commit out for `P(1)`. The case fails
/// closed (its routine has no tree). Control: the same case with no `#if` (`0: …; 1: …;
/// else Commit()`) does rule it out for `P(1)`, so the pruning is live in this fixture.
/// Break: drop the case-group marker in `lower_case_body` (the commit is ruled out).
#[test]
fn eng14_case_labels_from_a_group_prune_nothing() {
    const SRC: &str = r#"codeunit 50345 "Eng14 Case Labels"
{
    procedure CallPlain()
    begin
        Plain(1);
    end;

    procedure CallGroup()
    begin
        Group(1);
    end;

    procedure CallLabels()
    begin
        Labels(1);
    end;

    // sol, round 3: labels that vary by build inside one branch.
    local procedure Labels(I: Integer)
    begin
        case I of
#if X
            1,
#endif
            2:
                Work();
            else
                Commit();
        end;
    end;

    local procedure Plain(I: Integer)
    begin
        case I of
            0:
                Work();
            1:
                Work();
            else
                Commit();
        end;
    end;

    local procedure Group(I: Integer)
    begin
        case I of
            0:
                Work();
#if X
            1:
                Work();
#else
            else
                Commit();
#endif
        end;
    end;

    local procedure Work() begin end;
}
"#;
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("app.json"),
        r#"{"id":"e1400000-0000-4000-8000-000000000016","name":"E14C","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("src/Case.al"), SRC).unwrap();
    let (base, tree) = crate::engine::l5::digest::wave2_index_tests::tree_over(
        dir.path(),
        crate::engine::l4::cone_derived::WitnessScope::All,
    );
    let model = &base.resolved;
    let ruled_out = |caller: &str| -> bool {
        let root = model
            .workspace
            .routines
            .iter()
            .find(|r| r.name == caller)
            .unwrap_or_else(|| panic!("precondition: {caller}"))
            .stable_routine_id
            .clone();
        let fx = crate::engine::l5::digest::wave2_index_tests::effects_of(&tree, model, &root);
        let commit = fx
            .iter()
            .position(|e| e.effect_type == "COMMIT")
            .unwrap_or_else(|| panic!("precondition: {caller} reaches the commit"));
        LiteralReach::new(&tree, model)
            .unreachable_effects(&root, &fx, &[])
            .0
            .contains(&commit)
    };
    assert!(
        ruled_out("CallPlain"),
        "precondition: with no #if, P(1) rules the else's commit out"
    );
    assert!(
        !ruled_out("CallGroup"),
        "without X the case has no label 1 and P(1) commits"
    );
    assert!(
        !ruled_out("CallLabels"),
        "without X the branch has no label 1 and P(1) commits"
    );
}
