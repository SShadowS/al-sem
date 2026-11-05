//! The two caller relations agree where they are defined to
//! (`tests/fixtures/caller-relations`, its README states each case).
//!
//! The solve graph (`query touches` up, `DbEffectQuery::ancestors`, the
//! detectors' caller walks) holds the code-call relation; R0's reverse walk
//! holds the may-execute relation. On local code the first is R0 restricted to
//! code-call sites ([`super::CodeCalls`]); the second adds the routines that
//! open a page whose user-started triggers (an action, a part) reach the root.
//! Measured on CDO 2026-11-05 (`WaitForMailLimit`): solve 964 local callers,
//! R0 1,030, R0 over code-call sites 964, the same set.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use super::{CodeCalls, ConeVisitor, Tier, Via, WalkOptions, WalkScratch, WalkState, walk_one};
use crate::engine::l4::effect_query_cli::{QuerySubstrate, query_view};
use crate::program::inputs::WorkspaceInputs;

/// Every routine a walk reaches by a `Local` path, by stable id.
#[derive(Default)]
struct LocalNodes(BTreeSet<u32>);

impl ConeVisitor for LocalNodes {
    fn reach(&mut self, _: u32, s: WalkState, _: u32, _: Via, _: Option<WalkState>) {
        if s.tier == Tier::Local {
            self.0.insert(s.node);
        }
    }
}

/// `Object::Routine` per stable id, from the model.
fn names(model: &crate::program::model::workspace::Model) -> HashMap<String, String> {
    let obj: HashMap<&str, &str> = model
        .workspace
        .objects
        .iter()
        .map(|o| (o.id.as_str(), o.name.as_str()))
        .collect();
    model
        .workspace
        .routines
        .iter()
        .map(|r| {
            (
                r.stable_routine_id.clone(),
                format!(
                    "{}::{}",
                    obj.get(r.object_id.as_str()).unwrap_or(&"?"),
                    r.name
                ),
            )
        })
        .collect()
}

#[test]
fn solve_callers_equal_the_reverse_walk_over_code_call_sites() {
    let ws = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/caller-relations/ws");
    let inputs = Arc::new(WorkspaceInputs::read(&ws, &Default::default()));

    // The solve graph's transitive callers, as `query touches` up reads them.
    let view = query_view(&inputs, false).expect("view");
    let substrate = QuerySubstrate::of(&view);
    let model = substrate.resolved();
    let query = substrate.query();
    let name_of = names(model);
    let target = model
        .workspace
        .routines
        .iter()
        .find(|r| r.name == "Target")
        .expect("CR Target.Target");
    let rix = query.routine_ix(&target.id).expect("a solve row");
    let solve: BTreeSet<String> = query
        .ancestors(rix)
        .into_iter()
        .map(|(_, a)| {
            let id = query.bundle().routine_id(a);
            let r = model
                .workspace
                .routines
                .iter()
                .find(|r| r.id == id)
                .expect("a model row");
            name_of[&r.stable_routine_id].clone()
        })
        .collect();

    // R0 over the sited graph of the same workspace.
    let program = crate::program::resolved::ResolvedProgram::build(
        &inputs,
        crate::program::resolved::ProgramScope::CrossApp,
    )
    .expect("program");
    let g = super::super::sited_from_program(&program, &|| false)
        .unwrap_or_else(|_| panic!("the sited graph build was cancelled"));
    let root = g.nodes_with_stable_id(&target.stable_routine_id)[0];
    let local_names = |set: &BTreeSet<u32>| -> BTreeSet<String> {
        set.iter()
            .filter(|&&n| n != root)
            .filter_map(|&n| g.stable_id(n))
            .map(|sid| name_of[&sid].clone())
            .collect()
    };
    let mut code = LocalNodes::default();
    walk_one(
        &g,
        root,
        &WalkOptions::default(),
        &mut WalkScratch::default(),
        &mut CodeCalls(&mut code),
    );
    let mut all = LocalNodes::default();
    walk_one(
        &g,
        root,
        &WalkOptions::default(),
        &mut WalkScratch::default(),
        &mut all,
    );
    let code = local_names(&code.0);
    let all = local_names(&all.0);

    // The solve's set, stated by hand (README): every code-call caller.
    let expected: BTreeSet<String> = [
        "CR Direct::CallIt",
        "CR Runnable::OnRun",
        "CR Runner::RunIt",
        "CR Publisher::OnSomething",
        "CR Subscriber::HandleSomething",
        "CR Raiser::RaiseIt",
        "CR Entry::OnModify",
        "CR Modifier::ModifyIt",
        "CR Lifecycle Page::OnOpenPage",
        "CR Openers::OpenLifecycle",
        "CR Extension::OnOpenPage",
        "CR Extension::OnAction",
        "CR Openers::OpenExtended",
        "CR Action Page::OnAction",
        "CR Part Page::OnOpenPage",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(solve, expected, "the solve graph's callers");
    // The defined relation: on local code, solve == R0 over code-call sites.
    assert_eq!(
        code, solve,
        "R0 over code-call sites against the solve graph"
    );
    // The may-execute relation adds exactly the openers of pages whose
    // user-started triggers reach the root.
    let extra: BTreeSet<&str> = all.difference(&solve).map(String::as_str).collect();
    assert_eq!(
        extra,
        BTreeSet::from(["CR Openers::OpenAction", "CR Openers::OpenWithPart"]),
        "R0's may-execute callers beyond the code-call relation"
    );
    assert!(solve.is_subset(&all));
}
