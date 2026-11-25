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
