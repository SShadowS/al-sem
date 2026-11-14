//! Impact's relation and tier (output-validation round 2, r2 §2.1, W2): the
//! site classes, the total order, configuration as a candidate, hubs as
//! `via-hub`, canonical entries. Hand-built worlds state every precondition
//! literally; the configured-dispatch and `declaredRunnable` tests read real
//! source through a real build.

use std::path::Path;
use std::sync::Arc;

use al_syntax::ir::ObjectKind;

use super::*;
use crate::program::impact_graph::project::tests::{World, sited_of};
use crate::program::impact_graph::{ObjectFacts, RoutineFacts, RunTarget};
use crate::program::node::RoutineNodeId;
use crate::program::node_extract::Access;
use crate::program::resolve::edge::{DispatchShape, EdgeKind};
use crate::program::resolve::full::SiteFacts;
use crate::snapshot::TrustTier;

fn mask(kinds: &[&str]) -> u16 {
    kinds
        .iter()
        .map(|k| crate::engine::root_classification::root_kind_bit(k).expect("a kind"))
        .fold(0, |m, b| m | b)
}

fn kinds(w: &mut World, id: &RoutineNodeId, k: &[&str]) {
    w.facts.routines.insert(
        id.clone(),
        RoutineFacts {
            root_kinds: mask(k),
            ..Default::default()
        },
    );
}

/// A run of `target` (a `Page.Run` on a typed variable: the engine's run
/// edge) from `from`.
fn run(
    w: &mut World,
    from: &RoutineNodeId,
    target: &crate::program::node::ObjectNodeId,
    line: u32,
) {
    let ob = w.edge(from, EdgeKind::Call, DispatchShape::Exact, vec![], line);
    w.site_facts.insert(
        ob,
        SiteFacts {
            run_target: Some(target.clone()),
            ..Default::default()
        },
    );
}

/// One walk from `roots` with impact's visitor: the visitor (for its
/// per-node classes) and the walk's summary.
fn walk_up<'g>(g: &'g SitedGraph, roots: &[NodeIx], shared: &'g Shared<'g>) -> Up<'g> {
    let mut v = Up::new(g, Keep::new(true), roots, shared);
    walk_multi(
        g,
        roots,
        &WalkOptions::default(),
        &mut WalkScratch::default(),
        &mut v,
    );
    v
}

fn class_of(up: &Up<'_>, n: NodeIx) -> Population {
    up.best
        .get(&n)
        .expect("the node is reached")
        .class
        .population()
}

fn named(g: &SitedGraph, name_lc: &str) -> NodeIx {
    let ns = g.routines_named(name_lc);
    assert_eq!(ns.len(), 1, "{name_lc}: one routine");
    ns[0]
}

/// A page `Target` whose action `Post` (its `OnAction` trigger, the root)
/// is reached only by navigation: the `Card` page opens `Target`, and a
/// codeunit procedure `Runner` runs `Card` (`Page.Run`, a code run).
/// Returns the graph, the root, and the nodes `Target`, `Card`, `Runner`.
fn navigation_world() -> (SitedGraph, NodeIx, NodeIx, NodeIx, NodeIx) {
    let mut w = World::default();
    let a = w.app("W");
    let cu = w.obj(a, ObjectKind::Codeunit, 1, "Cu", TrustTier::Workspace);
    let target = w.obj(a, ObjectKind::Page, 2, "Target", TrustTier::Workspace);
    let card = w.obj(a, ObjectKind::Page, 3, "Card", TrustTier::Workspace);
    let post = w.rtn_with(&target, "OnAction", |x| {
        x.is_trigger = true;
        x.id.enclosing_member_lc = Some("post".into());
    });
    let sink = w.rtn_with(&cu, "Sink", |x| x.access = Access::Local);
    w.call(&post, &sink, 1);
    let runner = w.rtn_with(&cu, "Runner", |x| x.access = Access::Local);
    run(&mut w, &runner, &card, 2);
    w.facts.objects.insert(
        card.clone(),
        ObjectFacts {
            opens: vec![RunTarget::Object(target.clone())],
            ..Default::default()
        },
    );
    let card_open = w.trigger(&card, "OnOpenPage");
    w.call(&card_open, &sink, 3);
    kinds(&mut w, &post, &["trigger-page", "page-action"]);
    let g = sited_of(&w, a, &[]);
    let post_n = g.routine_node(&post).expect("a node");
    let obj = |g: &SitedGraph, name: &str| {
        (0..g.node_count() as u32)
            .find(|&n| {
                g.object_node(n).is_some() && g.routine_object(n).is_none() && {
                    g.object_node_name(n).is_some_and(|x| x.1 == name)
                }
            })
            .expect("an object node")
    };
    let (t, c) = (obj(&g, "Target"), obj(&g, "Card"));
    let r = g.routine_node(&runner).expect("a node");
    (g, post_n, t, c, r)
}

/// Round-2 r2 §2.1 step 1: a page action reached only by a page that opens
/// it has no code population; the opening page, the page that opens it and
/// the code that runs that page are reached by UI navigation (`local`).
///
/// DISCRIMINATION (2026-11-07): `site_bits` never setting `NAV` (every
/// lifecycle site code) fails "no code population": the three ancestors are
/// filed as code.
#[test]
fn a_page_action_reached_only_by_navigation_has_no_code_population() {
    let (g, root, target, card, runner) = navigation_world();
    let shared = Shared::new(&g, &[root].into_iter().collect());
    let up = walk_up(&g, &[root], &shared);
    for (n, what) in [(target, "Target"), (card, "Card"), (runner, "Runner")] {
        assert_eq!(class_of(&up, n), Population::Navigation, "{what}");
        assert_eq!(up.best[&n].class.tier, Tier::Local, "{what}");
    }
    let s = summary(&up, &g, &Captions::new(HashMap::new()));
    let a = &s["byRelation"]["ancestors"];
    assert_eq!(
        a["code"],
        json!({"local": 0, "viaDependency": 0}),
        "no code population: {a:#}"
    );
    assert_eq!(a["uiNavigation"]["local"], json!(3), "{a:#}");
    assert!(s["nearest"]["code"].is_null(), "{:#}", s["nearest"]);
    assert_eq!(
        s["nearest"]["uiNavigation"]["relation"],
        json!("UI navigation")
    );
}

/// Round-2 r2 §2.1 step 1: a subscriber whose publisher is raised by a code
/// call is in the code population (an event dispatch is code).
///
/// DISCRIMINATION (2026-11-07): `site_bits` setting `NAV` on
/// `Boundary::EventDispatch` fails "the publisher is code".
#[test]
fn a_subscriber_raised_by_code_is_in_the_code_population() {
    let mut w = World::default();
    let a = w.app("W");
    let cu = w.obj(a, ObjectKind::Codeunit, 1, "Cu", TrustTier::Workspace);
    let publisher = w.rtn(&cu, "OnSomething");
    let subscriber = w.rtn(&cu, "Handle");
    let caller = w.rtn_with(&cu, "Caller", |x| x.access = Access::Local);
    w.event(&publisher, &[&subscriber]);
    w.call(&caller, &publisher, 1);
    let g = sited_of(&w, a, &[]);
    let (s, p, c) = (
        g.routine_node(&subscriber).expect("a node"),
        g.routine_node(&publisher).expect("a node"),
        g.routine_node(&caller).expect("a node"),
    );
    assert!(
        g.sites_into(s)
            .iter()
            .any(|&x| g.sites()[x as usize].boundary == Boundary::EventDispatch),
        "precondition: the subscriber is reached by an event dispatch"
    );
    let shared = Shared::new(&g, &[s].into_iter().collect());
    let up = walk_up(&g, &[s], &shared);
    assert_eq!(class_of(&up, p), Population::Code, "the publisher is code");
    assert_eq!(class_of(&up, c), Population::Code, "its caller is code");
}

/// Round-2 r2 §2.1 step 1: a static background start on the path stays
/// code (the context axis labels it, not the relation).
///
/// DISCRIMINATION (2026-11-07): `site_bits` setting `NAV` on
/// `Boundary::Background` fails "the starter is code".
#[test]
fn a_background_start_on_the_path_stays_code() {
    let (g, r, _, _) = super::tests::entry_world();
    let starter = named(&g, "starter");
    assert!(
        g.sites()
            .iter()
            .any(|s| s.from == starter && s.boundary == Boundary::Background),
        "precondition: the starter's site is a background start"
    );
    let shared = Shared::new(&g, &[r].into_iter().collect());
    let up = walk_up(&g, &[r], &shared);
    assert_eq!(
        class_of(&up, starter),
        Population::Code,
        "the starter is code"
    );
}

/// A page `Pg` whose action trigger `Act` calls the root `R`; `Pg` is
/// reached only through the `h:run:page` hub, from `Y`'s dynamic page run.
/// Returns the graph, `R`, `Y` and the hub.
fn hub_world() -> (SitedGraph, NodeIx, NodeIx, NodeIx) {
    use crate::program::resolve::edge::OpenBound;
    let mut w = World::default();
    let a = w.app("W");
    let cu = w.obj(a, ObjectKind::Codeunit, 1, "Cu", TrustTier::Workspace);
    let pg = w.obj(a, ObjectKind::Page, 2, "Pg", TrustTier::Workspace);
    let r = w.rtn_with(&cu, "R", |x| x.access = Access::Local);
    let act = w.rtn_with(&pg, "OnAction", |x| {
        x.is_trigger = true;
        x.id.enclosing_member_lc = Some("act".into());
    });
    w.call(&act, &r, 1);
    let y = w.rtn_with(&cu, "Y", |x| x.access = Access::Local);
    let s = w.edge(
        &y,
        EdgeKind::Call,
        DispatchShape::DynamicOpen,
        vec![crate::program::impact_graph::project::tests::unknown_route()],
        2,
    );
    w.site_facts.insert(
        s,
        SiteFacts {
            open: vec![OpenBound::Run {
                kind: Some(ObjectKind::Page),
            }],
            ..Default::default()
        },
    );
    let g = sited_of(&w, a, &[]);
    let hub = (0..g.node_count() as u32)
        .find(|&n| g.flags(n) & flag::HUB != 0 && g.node_id(n) == "h:run:page")
        .expect("the page run hub");
    let (r, y) = (
        g.routine_node(&r).expect("a node"),
        g.routine_node(&y).expect("a node"),
    );
    (g, r, y, hub)
}

/// Round-2 r2 §2.1 steps 1-2, the mixed path: `R` <- `Act` is a code call,
/// `Act` <- `Pg` a UI-navigation step (a page's lifecycle into its action),
/// `Pg` <- the hub <- `Y` over a hub. `Y` is a candidate over a hub whose
/// path also needs navigation; every state at the hub node is `via-hub`.
///
/// DISCRIMINATION (2026-11-07): `Population::of` testing the bits before the
/// hub tier (a hub path classified by its relation) fails "Y is a candidate
/// over a hub" (it is filed as UI navigation) and "the hub is via-hub".
#[test]
fn a_mixed_path_over_navigation_and_a_hub_is_a_candidate_that_needs_navigation() {
    let (g, r, y, hub) = hub_world();
    let shared = Shared::new(&g, &[r].into_iter().collect());
    let up = walk_up(&g, &[r], &shared);
    assert_eq!(
        class_of(&up, y),
        Population::OverHub,
        "Y is a candidate over a hub"
    );
    let w = up.best[&y].class;
    assert!(w.needs_navigation(), "Y's path also needs navigation");
    assert_eq!(shown_tier(w.tier, w.ext), "via-hub");
    let wj = witness_json(&g, &w);
    assert_eq!(wj["relation"], json!("relation not established"));
    assert_eq!(wj["needsNavigation"], json!(true));
    assert_eq!(
        class_of(&up, hub),
        Population::OverHub,
        "the hub is via-hub"
    );
    let s = summary(&up, &g, &Captions::new(HashMap::new()));
    let a = &s["byRelation"]["ancestors"];
    assert_eq!(
        a["candidates"]["overHub"],
        json!(1),
        "Y; the hub node is not an ancestor: {a:#}"
    );
}

/// Round-2 r2 §2.1 step 2, the total order with the witness from the same
/// state: a node reached first by `(code, via-hub)` at 2 hops and later by
/// `(UI navigation, local)` at 5 hops takes `UI navigation, local`, and its
/// witness is the 5-hop state's. A node reached by `(code,
/// via-configuration)` and by `(UI navigation, local)` takes the established
/// `UI navigation, local` too. The states are handed to the visitor as the
/// walk would (hop order).
///
/// DISCRIMINATION (2026-11-07): `Population::of` testing the bits before
/// the hub tier fails the first assert (the 2-hop hub state is filed as
/// code and wins); `Population::of` ignoring `CONFIGURED` (a configured
/// state established) fails the second (the configured state is filed as
/// code and wins).
#[test]
fn the_class_order_puts_established_navigation_before_a_hub_or_configuration() {
    let (g, r, _, _) = super::tests::entry_world();
    let shared = Shared::new(&g, &[r].into_iter().collect());
    for (first, what) in [
        ((Tier::OnlyOpen, 0u8), "a hub path"),
        ((Tier::Local, bit::CONFIGURED), "a configured path"),
    ] {
        let mut up = Up::new(&g, Keep::new(false), &[r], &shared);
        let n: NodeIx = 1;
        let state = |tier: Tier, ext: u8| WalkState {
            node: n,
            tier,
            crossed_event: false,
            ext,
            guard: 0,
        };
        up.reach(0, state(first.0, first.1), 2, Via::Root(0), None);
        up.reach(0, state(Tier::Local, bit::NAV), 5, Via::Root(0), None);
        let b = up.best[&n];
        assert_eq!(
            (b.class.population(), b.class.tier, b.class.hops),
            (Population::Navigation, Tier::Local, 5),
            "{what}: established navigation wins, with its own witness"
        );
        assert_eq!(
            (b.tier, b.hops).1,
            if first.0 == Tier::Local { 2 } else { 5 }
        );
    }
}

/// `tests/fixtures/dependency-callers` (its README states every case), the
/// dependency loaded as a source folder.
fn dependency_callers() -> SitedGraph {
    let ws = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dependency-callers/ws");
    let inputs = Arc::new(crate::program::inputs::WorkspaceInputs::read(
        &ws,
        &Default::default(),
    ));
    let program = crate::program::resolved::ResolvedProgram::build(
        &inputs,
        crate::program::resolved::ProgramScope::CrossApp,
    )
    .expect("program");
    crate::program::impact_graph::sited_from_program(&program, &|| false).expect("not cancelled")
}

/// Round-2 r2 §2.1 step 2 on the configured-dispatch fixture: `DC
/// Approver.Approve` reaches the root only over `ExecuteResponse`'s
/// configured arm: a candidate over configuration with relation `code`,
/// counted among the candidates and not in established code. `DC
/// Both.Mixed` also has a code path: established code.
///
/// DISCRIMINATION (2026-11-07): `Population::of` ignoring `CONFIGURED` (a
/// configured state established) fails "Approve is a candidate over
/// configuration".
#[test]
fn a_configured_path_is_a_candidate_over_configuration() {
    let g = dependency_callers();
    let target = named(&g, "target");
    let approve = named(&g, "approve");
    let mixed = named(&g, "mixed");
    assert!(
        (0..g.site_count() as u32).any(|s| g.has_condition(s, "configuredDispatch")),
        "precondition: the fixture has configured-dispatch sites"
    );
    let shared = Shared::new(&g, &[target].into_iter().collect());
    let up = walk_up(&g, &[target], &shared);
    assert_eq!(
        class_of(&up, approve),
        Population::ConfiguredCode,
        "Approve is a candidate over configuration"
    );
    let w = up.best[&approve].class;
    assert_eq!(shown_tier(w.tier, w.ext), "via-configuration");
    assert_eq!(witness_json(&g, &w)["relation"], json!("code"));
    assert_eq!(witness_json(&g, &w)["established"], json!(false));
    assert_eq!(
        class_of(&up, mixed),
        Population::Code,
        "Mixed has a code path"
    );
    let s = summary(&up, &g, &Captions::new(HashMap::new()));
    let a = &s["byRelation"]["ancestors"];
    assert!(
        a["candidates"]["overConfiguration"]["code"].as_u64() >= Some(1),
        "counted among the candidates: {a:#}"
    );
}

/// A temporary workspace with one AL file.
fn one_file_workspace(al: &str) -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join(".git")).unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id": "dc0dc0dc-0000-4000-8000-0000000000aa", "name": "W2 Rules", "publisher": "probe",
            "version": "1.0.0.0", "runtime": "13.0", "idRanges": [{"from": 50100, "to": 50199}]}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/Rules.al"), al).unwrap();
    t
}

/// TLS-51: with the call-site `declaredRunnable` rule removed, nothing backs
/// up the resolver's start rule (`dataset::starts_object`): a dispatching
/// builtin it missed would run no object at all. So every builtin in
/// `DISPATCHING_BUILTINS` that runs a `Page`/`Report`/`XmlPort`/`Query`, on
/// a typed local, and a report builtin on a parameter, a global, an array
/// element and `this.`, must make a `run` site and no `declaredRunnable` one.
/// (`with R do SaveAsPdf()` is a known gap, ENG-73, so it is not listed.)
///
/// DISCRIMINATION (2026-11-14): `starts_object` without `"saveaspdf"` in the
/// report output methods fails at "ReportSaveAsPdf" (no run site); restored,
/// passes.
#[test]
fn every_dispatching_builtin_on_a_typed_receiver_is_a_run_site() {
    let mut al = String::from(
        r#"table 50150 "B Row"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { Clustered = true; } }
}

page 50151 "B Page"
{
    SourceTable = "B Row";
    trigger OnOpenPage()
    begin
        Message('p');
    end;
}

report 50152 "B Report"
{
    dataset { dataitem(Row; "B Row") { } }
    trigger OnInitReport()
    begin
        Message('r');
    end;
    trigger OnPreReport()
    begin
        Message('r');
    end;
}

xmlport 50153 "B XmlPort"
{
    schema { textelement(Root) { } }
    trigger OnPreXmlPort()
    begin
        Message('x');
    end;
}

query 50154 "B Query"
{
    elements { dataitem(Row; "B Row") { column(Code; Code) { } } }
    trigger OnBeforeOpen()
    begin
        Message('q');
    end;
}

codeunit 50155 "B Probe"
{
    var
        GR: Report "B Report";
"#,
    );
    let cases: &[(&str, &str, &str)] = &[
        ("PageRun", "V: Page \"B Page\";", "V.Run();"),
        ("PageRunModal", "V: Page \"B Page\";", "V.RunModal();"),
        ("ReportRun", "V: Report \"B Report\";", "V.Run();"),
        ("ReportRunModal", "V: Report \"B Report\";", "V.RunModal();"),
        ("ReportExecute", "V: Report \"B Report\";", "V.Execute('');"),
        ("ReportPrint", "V: Report \"B Report\";", "V.Print('');"),
        (
            "ReportRunRequestPage",
            "V: Report \"B Report\"; T: Text;",
            "T := V.RunRequestPage();",
        ),
        (
            "ReportSaveAs",
            "V: Report \"B Report\"; O: OutStream;",
            "V.SaveAs('', ReportFormat::Pdf, O);",
        ),
        (
            "ReportSaveAsExcel",
            "V: Report \"B Report\";",
            "V.SaveAsExcel('f');",
        ),
        (
            "ReportSaveAsHtml",
            "V: Report \"B Report\";",
            "V.SaveAsHtml('f');",
        ),
        (
            "ReportSaveAsPdf",
            "V: Report \"B Report\";",
            "V.SaveAsPdf('f');",
        ),
        (
            "ReportSaveAsWord",
            "V: Report \"B Report\";",
            "V.SaveAsWord('f');",
        ),
        (
            "ReportSaveAsXml",
            "V: Report \"B Report\";",
            "V.SaveAsXml('f');",
        ),
        ("XmlPortRun", "V: XmlPort \"B XmlPort\";", "V.Run();"),
        ("XmlPortImport", "V: XmlPort \"B XmlPort\";", "V.Import();"),
        ("XmlPortExport", "V: XmlPort \"B XmlPort\";", "V.Export();"),
        ("QueryOpen", "V: Query \"B Query\";", "V.Open();"),
        (
            "QuerySaveAsCsv",
            "V: Query \"B Query\";",
            "V.SaveAsCsv('f');",
        ),
        (
            "QuerySaveAsJson",
            "V: Query \"B Query\"; O: OutStream;",
            "V.SaveAsJson(O);",
        ),
        (
            "QuerySaveAsXml",
            "V: Query \"B Query\";",
            "V.SaveAsXml('f');",
        ),
        (
            "ArrayReceiver",
            "V: array[2] of Report \"B Report\";",
            "V[1].SaveAsPdf('f');",
        ),
        ("GlobalReceiver", "", "GR.SaveAsPdf('f');"),
        ("ThisGlobalReceiver", "", "this.GR.SaveAsPdf('f');"),
    ];
    for (name, vars, body) in cases {
        let var = if vars.is_empty() {
            String::new()
        } else {
            format!("    var\n        {}\n", vars.replace("; ", ";\n        "))
        };
        al.push_str(&format!(
            "    procedure {name}()\n{var}    begin\n        {body}\n    end;\n\n"
        ));
    }
    al.push_str(
        "    procedure ParamReceiver(var V: Report \"B Report\")\n    begin\n        V.SaveAsPdf('f');\n    end;\n}\n",
    );
    let t = one_file_workspace(&al);
    let ws = t.path().join("ws");
    let inputs = Arc::new(crate::program::inputs::WorkspaceInputs::read(
        &ws,
        &Default::default(),
    ));
    let program = crate::program::resolved::ResolvedProgram::build(
        &inputs,
        crate::program::resolved::ProgramScope::CrossApp,
    )
    .expect("program");
    let g = crate::program::impact_graph::sited_from_program(&program, &|| false)
        .expect("not cancelled");
    let names = cases
        .iter()
        .map(|c| c.0)
        .chain(std::iter::once("ParamReceiver"));
    for name in names {
        let from = named(&g, &al_syntax::fold_identifier(name));
        let via = |v: &str| {
            g.sites()
                .iter()
                .filter(|s| s.from == from && s.via() == v)
                .count()
        };
        assert!(via("run") >= 1, "{name}: no run site");
        assert_eq!(via("declaredRunnable"), 0, "{name}");
    }
}

/// Round-2 r2 §2.1 step 1: a dispatching builtin on a receiver typed as one
/// object (`R.RunRequestPage`) is a `run` call site and code; a `TestPage`
/// declaration is the one `declaredRunnable` rule left, a rule edge (`RULE`,
/// no call site); a plain `Page` variable declaration yields no site at all.
/// (The call-site `declaredRunnable` rule ran such builtins until ENG-19 made
/// every one a run of its object; TLS-51 removed it.)
///
/// DISCRIMINATION (2026-11-14): `site_bits` classifying a `declaredRunnable`
/// site as code (0) fails "the TestPage declaration is a rule edge".
#[test]
fn a_typed_run_is_code_and_a_test_page_declaration_is_a_rule_edge() {
    let t = one_file_workspace(
        r#"codeunit 50100 "Holder"
{
    procedure OnlyDeclares()
    var
        P: Page "My Card";
    begin
    end;

    procedure Prints()
    var
        R: Report "My Rep";
    begin
        R.RunRequestPage();
    end;
}

codeunit 50101 "My Tests"
{
    Subtype = Test;

    [Test]
    procedure DrivesPage()
    var
        TP: TestPage "My Card";
    begin
    end;
}

page 50100 "My Card"
{
    trigger OnOpenPage()
    begin
    end;
}

report 50100 "My Rep"
{
    trigger OnPreReport()
    begin
    end;
}
"#,
    );
    let ws = t.path().join("ws");
    let inputs = Arc::new(crate::program::inputs::WorkspaceInputs::read(
        &ws,
        &Default::default(),
    ));
    let program = crate::program::resolved::ResolvedProgram::build(
        &inputs,
        crate::program::resolved::ProgramScope::CrossApp,
    )
    .expect("program");
    let g = crate::program::impact_graph::sited_from_program(&program, &|| false)
        .expect("not cancelled");
    let declared = |from: NodeIx| -> Vec<SiteIx> {
        (0..g.site_count() as u32)
            .filter(|&s| {
                let st = &g.sites()[s as usize];
                st.from == from && st.via() == "declaredRunnable"
            })
            .collect()
    };
    // ENG-19: `RunRequestPage` runs the report's request page, so it is a
    // `run` site (as `SaveAsPdf` became one with B6), not `declaredRunnable`.
    let prints_from = named(&g, "prints");
    assert!(declared(prints_from).is_empty());
    let prints: Vec<SiteIx> = (0..g.site_count() as u32)
        .filter(|&s| {
            let st = &g.sites()[s as usize];
            st.from == prints_from && st.via() == "run"
        })
        .collect();
    assert_eq!(
        prints.len(),
        1,
        "precondition: RunRequestPage makes one run site"
    );
    assert!(g.sites()[prints[0] as usize].anchor.is_some());
    assert_eq!(
        site_bits(&g, prints[0]),
        0,
        "the RunRequestPage site is code"
    );
    let drives = declared(named(&g, "drivespage"));
    assert_eq!(
        drives.len(),
        1,
        "precondition: the TestPage declaration makes one site"
    );
    assert!(g.sites()[drives[0] as usize].anchor.is_none());
    assert_eq!(
        site_bits(&g, drives[0]),
        bit::RULE,
        "the TestPage declaration is a rule edge"
    );
    let holder = named(&g, "onlydeclares");
    assert!(
        g.sites().iter().all(|s| s.from != holder),
        "a plain Page declaration yields no site"
    );
}

/// A page `Card` whose action `Open` both runs `Opened` (`RunObject`) and
/// has an `OnAction` trigger; the trigger calls the root `R` and
/// `Opened.OnOpenPage` calls `R` too. Returns the graph, `R` and the
/// trigger.
fn action_world(named_actions: bool) -> (SitedGraph, NodeIx, NodeIx) {
    let mut w = World::default();
    let a = w.app("W");
    let cu = w.obj(a, ObjectKind::Codeunit, 1, "Cu", TrustTier::Workspace);
    let card = w.obj(a, ObjectKind::Page, 2, "Card", TrustTier::Workspace);
    let opened = w.obj(a, ObjectKind::Page, 3, "Opened", TrustTier::Workspace);
    let r = w.rtn_with(&cu, "R", |x| x.access = Access::Local);
    let act = w.rtn_with(&card, "OnAction", |x| {
        x.is_trigger = true;
        x.id.enclosing_member_lc = Some("open".into());
    });
    let open_t = w.trigger(&opened, "OnOpenPage");
    w.call(&act, &r, 1);
    w.call(&open_t, &r, 2);
    w.facts.objects.insert(
        card.clone(),
        ObjectFacts {
            opens: vec![RunTarget::Object(opened.clone())],
            action_runs: vec![RunTarget::Object(opened.clone())],
            action_run_names: if named_actions {
                vec![(RunTarget::Object(opened.clone()), "Open".into())]
            } else {
                Vec::new()
            },
            ..Default::default()
        },
    );
    kinds(&mut w, &act, &["trigger-page", "page-action"]);
    kinds(&mut w, &open_t, &["trigger-page"]);
    let g = sited_of(&w, a, &[]);
    let (r, act) = (
        g.routine_node(&r).expect("a node"),
        g.routine_node(&act).expect("a node"),
    );
    (g, r, act)
}

/// Round-2 r2 §2.1 step 3: a page action with both an `OnAction` trigger and
/// a `RunObject` is one entry, in the numerator and the denominator. Here the
/// trigger is reached by a code call and the action's `RunObject` by the
/// opened page's lifecycle; the program holds the action and
/// `Opened.OnOpenPage`, and the walk reaches both, once each. Without the
/// action's name (facts that do not name it) the action-run site stands for
/// the page, a second entry on both sides.
///
/// DISCRIMINATION (2026-11-07): keying the trigger node by its
/// StableRoutineId instead of its action (`EntryInventory::of`'s
/// `EntryKey::Action` arm removed) fails "the action counts once" (3 of 3).
#[test]
fn an_action_with_a_trigger_and_a_run_object_counts_once() {
    let (g, r, act) = action_world(true);
    let inv = g.entry_inventory();
    let sites: Vec<u32> = g.action_run_sites().to_vec();
    assert_eq!(sites.len(), 1, "precondition: one action-run site");
    assert_eq!(
        inv.of_site(sites[0]),
        &[inv.of_node(act).expect("the trigger is an entry")],
        "the site and the trigger are one entry"
    );
    let s = super::tests::walked(&g, r);
    let all = &s["programShare"]["all"];
    assert_eq!(
        (all["reached"].as_u64(), all["program"].as_u64()),
        (Some(2), Some(2)),
        "the action counts once: {all:#}"
    );
    assert_eq!(
        s["entriesByKind"]["code"]["page action"],
        json!(1),
        "{:#}",
        s["entriesByKind"]
    );
    // Facts that do not name the action: the site stands for the page.
    let (g, r, _) = action_world(false);
    let s = super::tests::walked(&g, r);
    let all = &s["programShare"]["all"];
    assert_eq!(
        (all["reached"].as_u64(), all["program"].as_u64()),
        (Some(3), Some(3)),
        "{all:#}"
    );
}
