//! ENG-165 unit tests: the site parser and the control-flow graph's order
//! rules. The question's answers are pinned end to end by
//! `tests/cli/cli_eng165_commit_between.rs`.

use super::*;

#[test]
fn site_point_parses_both_forms() {
    let p = |s: &str| SitePoint::parse("--after", s);
    assert_eq!(
        p("f.al:12").unwrap(),
        SitePoint {
            file: "f.al".into(),
            line: 12,
            column: None
        }
    );
    assert_eq!(
        p("f.al:12:5").unwrap(),
        SitePoint {
            file: "f.al".into(),
            line: 12,
            column: Some(5)
        }
    );
    assert_eq!(
        p("C:/x/f.al:12").unwrap(),
        SitePoint {
            file: "C:/x/f.al".into(),
            line: 12,
            column: None
        }
    );
    assert_eq!(
        p("C:/x/f.al:12:5").unwrap(),
        SitePoint {
            file: "C:/x/f.al".into(),
            line: 12,
            column: Some(5)
        }
    );
    assert_eq!(
        p("dep:App:src/f.al:7").unwrap(),
        SitePoint {
            file: "dep:App:src/f.al".into(),
            line: 7,
            column: None
        }
    );
    assert!(p("f.al").is_err());
    assert!(p("f.al:0").is_err());
    assert!(p(":3").is_err());
}

fn node(kind: &str) -> PCFNNode {
    PCFNNode {
        kind: kind.to_string(),
        operation_id: None,
        callsite_id: None,
        condition_guard: None,
        condition_leaves: None,
        children: None,
        else_children: None,
        rare: None,
        source_range: None,
        flow: None,
    }
}

fn op(id: &str) -> PCFNNode {
    let mut n = node("op");
    n.operation_id = Some(id.to_string());
    n
}

fn with(kind: &str, children: Vec<PCFNNode>) -> PCFNNode {
    let mut n = node(kind);
    n.children = Some(children);
    n
}

/// The region from after `a` to before `b` (`b` a sure cut).
fn region(tree: &PCFNNode, a: &str, b: &str, sure: bool) -> Vec<String> {
    let g = Cfg::of(tree);
    let an = g.nodes_of(&[a].into_iter().collect());
    let bn = g.nodes_of(&[b].into_iter().collect());
    let starts: Vec<usize> = an.iter().flat_map(|&n| g.succs(n)).collect();
    let target: HashSet<usize> = bn.iter().copied().collect();
    let stop = if sure { target.clone() } else { HashSet::new() };
    let f = g.forward(&starts, &stop, false);
    let bk = g.backward(&bn, &stop);
    let mut out: Vec<String> = (0..f.len())
        .filter(|&n| f[n] && bk[n] && (!sure || !target.contains(&n)))
        .flat_map(|n| g.ids[n].iter().map(|s| s.to_string()))
        .collect();
    out.sort();
    out
}

#[test]
fn sibling_arm_is_out_and_a_loop_back_edge_is_in() {
    // if X then A else C; B
    let mut iff = node("if");
    iff.children = Some(vec![op("A")]);
    iff.else_children = Some(vec![op("C")]);
    let t = with("block", vec![iff, op("B")]);
    assert_eq!(region(&t, "A", "B", true), Vec::<String>::new());
    // repeat B; A; C until …  → C is between A and the next B
    let t = with(
        "block",
        vec![with("repeat", vec![op("B"), op("A"), op("C")])],
    );
    assert_eq!(region(&t, "A", "B", true), vec!["C".to_string()]);
    // for: A; B; C  → the cut at a sure B leaves C out
    let t = with("block", vec![with("for", vec![op("A"), op("B"), op("C")])]);
    assert_eq!(region(&t, "A", "B", true), Vec::<String>::new());
    // for: A; if F then B; C  → B not sure: C is in (loop-carried)
    let mut maybe = node("if");
    maybe.children = Some(vec![op("B")]);
    let t = with("block", vec![with("for", vec![op("A"), maybe, op("C")])]);
    let r = region(&t, "A", "B", false);
    assert!(r.contains(&"C".to_string()), "{r:?}");
}

#[test]
fn exit_and_error_arms_never_reach_b_and_break_leaves_the_loop() {
    // A; if F then begin C; exit end; B
    let mut iff = node("if");
    iff.children = Some(vec![op("C"), node("exit")]);
    let t = with("block", vec![op("A"), iff, op("B")]);
    assert_eq!(region(&t, "A", "B", true), Vec::<String>::new());
    // while: A; if F then begin C; break end; B   (B only in the loop)
    let mut iff = node("if");
    iff.children = Some(vec![op("C"), node("break")]);
    let t = with("block", vec![with("while", vec![op("A"), iff, op("B")])]);
    assert_eq!(region(&t, "A", "B", true), Vec::<String>::new());
    // while: A; if F then begin C; break end; …  B after the loop: C is in,
    // and so is A itself (a later pass of the loop runs it again before B).
    let mut iff = node("if");
    iff.children = Some(vec![op("C"), node("break")]);
    let t = with("block", vec![with("while", vec![op("A"), iff]), op("B")]);
    assert_eq!(
        region(&t, "A", "B", true),
        vec!["A".to_string(), "C".to_string()]
    );
}

// ---------------------------------------------------------------------------
// The guards whose real precondition needs a huge graph or a routine without a
// tree (review r2 item 5): each knob states the precondition by assignment and
// the answer goes through prove's own job, so the guard's branch is pinned.
// ---------------------------------------------------------------------------

/// `Root` calls `Work`: A (`Send`), a call that commits nothing, B (the log
/// `Insert`), then a `Commit` after B. So may-commit is `yes` and the honest
/// commit-between answer is `no`.
const KNOB_AL: &str = r#"table 50700 "K Log"
{
    fields { field(1; N; Integer) { } }
    keys { key(PK; N) { Clustered = true; } }
}

table 50701 "K Outbox"
{
    fields { field(1; N; Integer) { } }
    keys { key(PK; N) { Clustered = true; } }
}

codeunit 50702 "K"
{
    procedure Root()
    begin
        Work();
    end;

    local procedure Work()
    var
        L: Record "K Log";
    begin
        Send();
        Helper();
        L.Insert();
        Commit();
    end;

    local procedure Helper()
    begin
        Noop();
    end;

    local procedure Noop()
    begin
    end;

    local procedure Send()
    var
        O: Record "K Outbox";
    begin
        O.Insert();
    end;
}
"#;

fn knob_answer(knobs: Knobs) -> (String, Vec<String>) {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(
        tmp.path().join("app.json"),
        r#"{"id":"16516516-0000-4000-8000-0000000c0165","name":"KnobWs","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{"from":50700,"to":50799}]}"#,
    )
    .unwrap();
    std::fs::write(tmp.path().join("src/K.al"), KNOB_AL).unwrap();
    // `Send();` and `L.Insert();` in `Work` (1-based lines of KNOB_AL).
    let line = |needle: &str| {
        KNOB_AL
            .lines()
            .position(|l| l.trim() == needle)
            .expect("fixture line")
            + 1
    };
    let mut slice = crate::engine::l5::prove_slice::SliceRequest::commit_between(
        Some(&format!("K.al:{}", line("Send();"))),
        Some(&format!("K.al:{}", line("L.Insert();"))),
        false,
        None,
    )
    .unwrap();
    slice.between.as_mut().unwrap().knobs = knobs;
    let needs = crate::program::inputs::WorkspaceNeeds {
        build_only: false,
        fingerprint_rows: true,
    };
    let inputs = crate::program::inputs::WorkspaceInputs::read(tmp.path(), &needs);
    let job = crate::engine::l5::prove::ProveJob {
        detectors: crate::engine::l5::detectors::default_detectors(),
        inputs: &inputs,
        routine_selector: "K::Root",
        question_text: "commit-between",
        alsem_ver: "test",
        deterministic: true,
        single_app: true,
        closed_world: false,
        assume_no_dependents: false,
        human: false,
        detail: true,
        out: None,
        slice: Some(slice),
        surface: Default::default(),
    };
    let r = crate::engine::batch::run_doc(&job)
        .one::<Result<crate::engine::l5::prove::ProveRunResult, String>>()
        .unwrap()
        .unwrap_or_else(|e| panic!("prove failed: {e}"));
    let v: serde_json::Value = serde_json::from_str(&r.json_text).unwrap();
    let answer = v["payload"]["result"]["answer"]
        .as_str()
        .unwrap()
        .to_string();
    let kinds = v["payload"]["result"]["blockedBy"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|b| b["kind"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    (answer, kinds)
}

fn assert_blocked(knobs: Knobs, kind: &str) {
    let (answer, kinds) = knob_answer(knobs);
    assert_eq!(answer, "unknown", "{kinds:?}");
    assert!(kinds.iter().any(|k| k == kind), "{kind} not in {kinds:?}");
}

#[test]
fn knob_fixture_answers_no_without_knobs() {
    // Precondition of the guard tests below.
    let (answer, kinds) = knob_answer(Knobs::default());
    assert_eq!(answer, "no", "{kinds:?}");
}

#[test]
fn guard_cone_walk_capped() {
    assert_blocked(
        Knobs {
            cone_cap: Some(0),
            ..Knobs::default()
        },
        "cone-walk-capped",
    );
}

#[test]
fn guard_route_cap() {
    assert_blocked(
        Knobs {
            route_budget: Some(0),
            ..Knobs::default()
        },
        "route-cap",
    );
}

#[test]
fn guard_route_graph_stopped() {
    assert_blocked(
        Knobs {
            search_stopped: true,
            ..Knobs::default()
        },
        "route-graph-stopped",
    );
}

#[test]
fn guard_cone_truncated() {
    assert_blocked(
        Knobs {
            cone_truncated: true,
            ..Knobs::default()
        },
        "cone-truncated",
    );
}

#[test]
fn guard_analysis_gaps() {
    assert_blocked(
        Knobs {
            analysis_gaps: true,
            ..Knobs::default()
        },
        "analysis-gaps",
    );
}

#[test]
fn guard_site_routine_no_tree() {
    assert_blocked(
        Knobs {
            no_tree: Some("Work".to_string()),
            ..Knobs::default()
        },
        "site-routine-no-tree",
    );
}
