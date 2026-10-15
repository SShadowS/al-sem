//! #83: an object without a number gets a unique object id, `@` + its encoded,
//! folded logical name (`engine::ids::object_key`), from every producer.
//!
//! - T3: two source interfaces get distinct ids, and the R0 snapshot (P1), the
//!   model (P2), the program graph's `model_object_id` (P3) and the R1a projection
//!   (P5) agree on each literal id.
//! - T3b: two codeunits whose number does not parse get distinct object and
//!   routine ids, and the interface dispatch joins the model to the graph on them.
//! - T5: two dependency interfaces without `Id` declaring the same method no
//!   longer share an object id or routine ids.
//! - T5b: in a cross-app model, the dependency interface's graph id (P3) equals its
//!   projected `ModelObject` id (P8), whose `object_key` is the id's last segment.

use std::path::Path;

use al_sem::engine::deps::projection::project_abi_to_index;
use al_sem::engine::deps::symbol_reference::parse_symbol_reference;
use al_sem::program::model::model_object_id;
use al_sem::program::model::workspace::MODEL_INSTANCE_ID_DEFAULT as MI;

const GUID: &str = "83830000-0000-0000-0000-000000000083";
const DEP_GUID: &str = "83830000-0000-0000-0000-0000000000de";

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn app_json(dir: &Path, dependencies: &str) {
    write(
        &dir.join("app.json"),
        &format!(
            r#"{{"id":"{GUID}","name":"Ws83","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50100,"to":50199}}],"dependencies":[{dependencies}]}}"#
        ),
    );
}

/// The program graph's model ids (P3) of every object of `kind` in app `guid`.
fn graph_ids(
    graph: &al_sem::program::graph::ProgramGraph,
    kind: al_syntax::ir::ObjectKind,
    guid: &str,
) -> Vec<String> {
    let mut v: Vec<String> = graph
        .objects
        .iter()
        .filter(|o| o.id.kind == kind && graph.apps.resolve(o.id.app).guid == guid)
        .map(|o| model_object_id(graph, &o.id))
        .collect();
    v.sort();
    v
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

fn read(ws: &Path) -> std::sync::Arc<al_sem::program::inputs::InputSet> {
    al_sem::program::inputs::InputSet::read(ws, &Default::default())
}

/// T3. The second interface's name holds every byte a consumer splits on, a
/// doubled quote and a non-ASCII letter, so the encoding and the unescape are both
/// exercised.
///
/// Discrimination (2026-10-15): `model_object_id` (P3) without the unescape gives
/// `%22%22` for the doubled quote and fails the P3 assertion; restored, it passes.
#[test]
fn source_interfaces_get_distinct_ids_from_every_producer() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    app_json(ws, "");
    write(
        &ws.join("src/IEmpty.Interface.al"),
        "interface IEmpty\n{\n}\n",
    );
    write(
        &ws.join("src/Odd.Interface.al"),
        "interface \"Ø \"\"Q\"\" /:#%@ If\"\n{\n}\n",
    );
    let internal = sorted(vec![
        format!("{GUID}/Interface/@iempty"),
        format!("{GUID}/Interface/@%C3%B8%20%22q%22%20%2F%3A%23%25%40%20if"),
    ]);
    let stable: Vec<String> = internal.iter().map(|i| i.replace('/', ":")).collect();

    // P1: the R0 identity snapshot.
    let r0 = al_sem::engine::snapshot::snapshot_workspace(ws).unwrap();
    let p1 = sorted(
        r0.objects
            .iter()
            .filter(|o| o.kind == "Interface")
            .map(|o| o.stable_object_id.clone())
            .collect(),
    );
    assert_eq!(p1, stable, "P1 (R0 snapshot)");

    // P2: the model.
    let inputs = read(ws);
    let model = al_sem::program::model::program_calls::assemble_and_resolve_workspace_program(
        &inputs, MI, true,
    )
    .expect("model");
    let p2 = sorted(
        model
            .workspace
            .objects
            .iter()
            .filter(|o| o.object_type == "Interface")
            .map(|o| o.id.clone())
            .collect(),
    );
    assert_eq!(p2, internal, "P2 (model)");

    // P3: `model_object_id` of each graph node.
    let (ctx, _, _) = al_sem::program::resolve::full::build_program_with_coverage(&inputs).unwrap();
    let p3 = graph_ids(ctx.graph(), al_syntax::ir::ObjectKind::Interface, GUID);
    assert_eq!(p3, internal, "P3 (model_object_id)");

    // P5: the R1a projection.
    let l2 = al_sem::program::body::l2_workspace::project_workspace(ws).unwrap();
    let p5 = sorted(
        l2.objects
            .iter()
            .filter(|o| o.object_type == "Interface")
            .map(|o| o.stable_object_id.clone())
            .collect(),
    );
    assert_eq!(p5, stable, "P5 (R1a)");
}

/// T3b. A numbered kind whose number does not parse (it overflows `i64`) has no
/// number, so its id takes its name too, and its routines' ids with it. `"CU A"`
/// implements the interface `Caller` dispatches over, so the dispatch must find
/// it: that joins the model's id (P2) to the graph's (P3) at
/// `program_calls.rs`'s `reached`.
///
/// Discrimination (2026-10-15): the model (P2) minting its id from
/// `o.id.unwrap_or(0)` again lists `"CU A"` as not-found (and its object id and
/// routine ids collide with `"CU B"`'s); restored, it passes.
#[test]
fn codeunits_without_a_parsed_number_get_distinct_ids() {
    let cu_a = "codeunit 99999999999999999999 \"CU A\" implements IRun\n{\n    procedure Run()\n    begin\n    end;\n}\n";
    let cu_b =
        "codeunit 99999999999999999999 \"CU B\"\n{\n    procedure Run()\n    begin\n    end;\n}\n";
    // The precondition, stated in the test: the lowering gives each a Codeunit
    // with NO number (not a saturated or wrapped one).
    for src in [cu_a, cu_b] {
        let ir = al_syntax::parse(src);
        assert_eq!(ir.objects.len(), 1, "{src}");
        assert_eq!(ir.objects[0].kind, al_syntax::ir::ObjectKind::Codeunit);
        assert_eq!(ir.objects[0].id, None, "the number must not parse: {src}");
    }

    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    app_json(ws, "");
    write(
        &ws.join("src/IRun.Interface.al"),
        "interface IRun\n{\n    procedure Run();\n}\n",
    );
    write(&ws.join("src/CuA.Codeunit.al"), cu_a);
    write(&ws.join("src/CuB.Codeunit.al"), cu_b);
    write(
        &ws.join("src/Caller.Codeunit.al"),
        "codeunit 50100 Caller\n{\n    procedure Go(I: Interface IRun)\n    begin\n        I.Run();\n    end;\n}\n",
    );

    let model = al_sem::program::model::program_calls::assemble_and_resolve_workspace_program(
        &read(ws),
        MI,
        true,
    )
    .expect("model");
    let id_a = format!("{GUID}/Codeunit/@cu%20a");
    let id_b = format!("{GUID}/Codeunit/@cu%20b");
    let numberless = sorted(
        model
            .workspace
            .objects
            .iter()
            .filter(|o| o.object_type == "Codeunit" && o.object_number == 0)
            .map(|o| o.id.clone())
            .collect(),
    );
    assert_eq!(numberless, vec![id_a.clone(), id_b.clone()], "object ids");

    let run = |object_id: &str| {
        let v: Vec<_> = model
            .workspace
            .routines
            .iter()
            .filter(|r| r.object_id == object_id && r.name == "Run")
            .collect();
        assert_eq!(v.len(), 1, "one Run in {object_id}");
        v[0]
    };
    let (run_a, run_b) = (run(&id_a), run(&id_b));
    assert_ne!(run_a.id, run_b.id, "internal routine ids");
    assert_ne!(
        run_a.stable_routine_id, run_b.stable_routine_id,
        "stable routine ids"
    );
    assert!(
        run_a
            .stable_routine_id
            .starts_with(&format!("{GUID}:Codeunit:@cu%20a#")),
        "{}",
        run_a.stable_routine_id
    );

    // The dispatch over IRun reaches "CU A" and does not list it as not-found.
    let go = model
        .workspace
        .routines
        .iter()
        .find(|r| r.name == "Go")
        .expect("Caller.Go");
    let edges: Vec<_> = model
        .calls
        .edges
        .iter()
        .filter(|e| e.from == go.id)
        .collect();
    assert!(
        edges
            .iter()
            .any(|e| e.to.as_deref() == Some(run_a.id.as_str())),
        "the dispatch reaches CU A's Run: {edges:#?}"
    );
    let meta = edges
        .iter()
        .find_map(|e| e.dispatch_meta.as_ref())
        .expect("the dispatch carries its meta");
    assert_eq!(meta.total_impls, 1, "{meta:?}");
    assert!(
        meta.unresolved_impls.is_empty(),
        "no implementer is not-found: {meta:?}"
    );
}

const TWO_DEP_INTERFACES: &str = r#"{"Interfaces":[{"Name":"IFoo","Methods":[{"Name":"Bar","Parameters":[]}]},{"Name":"IQux","Methods":[{"Name":"Bar","Parameters":[]}]}]}"#;

/// T5. The real SymbolReference parser reads two interfaces without `Id` that
/// declare the same method; their projected objects and routines stay apart.
///
/// Discrimination (2026-10-15): the projection (P8) keying an ABI object on
/// `Some(o.object_number)` (no zero rule) gives both `…/Interface/0` and one pair
/// of routine ids; restored, it passes.
#[test]
fn dependency_interfaces_without_id_get_distinct_ids() {
    let abi = parse_symbol_reference(TWO_DEP_INTERFACES);
    assert!(
        abi.objects.iter().all(|o| o.object_number == 0),
        "precondition: the symbols carry no Id"
    );
    let p = project_abi_to_index(&abi, DEP_GUID, MI);
    let ids = sorted(p.objects.iter().map(|o| o.id.clone()).collect());
    assert_eq!(
        ids,
        vec![
            format!("{DEP_GUID}/Interface/@ifoo"),
            format!("{DEP_GUID}/Interface/@iqux"),
        ]
    );
    for o in &p.objects {
        assert!(o.id.ends_with(&format!("/{}", o.object_key)), "{o:?}");
    }
    assert_eq!(p.routines.len(), 2);
    assert_ne!(p.routines[0].id, p.routines[1].id, "internal routine ids");
    assert_ne!(
        p.routines[0].stable_routine_id, p.routines[1].stable_routine_id,
        "stable routine ids"
    );
}

/// T5b. A symbol-only dependency declares an interface without `Id`; the
/// workspace dispatches over it, so the cross-app model holds it. The graph node
/// (`ObjKey::Name`, P3) and the projected `ModelObject` (P8) name the same id.
///
/// Discrimination (2026-10-15): `dep_object_to_l3` setting `object_key` to `""`
/// fails the last-segment assertion; restored, it passes.
#[test]
fn dependency_interface_graph_id_equals_its_model_object_id() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    app_json(
        ws,
        &format!(r#"{{"id":"{DEP_GUID}","name":"Dep83","publisher":"probe","version":"1.0.0.0"}}"#),
    );
    write(
        &ws.join("src/W.Codeunit.al"),
        "codeunit 50101 W\n{\n    procedure Caller()\n    var\n        X: Interface IDep;\n    begin\n        X.Go();\n    end;\n}\n",
    );
    crate::symbol_app::write_symbol_app(
        &ws.join(".alpackages/probe_Dep83_1.0.0.0.app"),
        DEP_GUID,
        "Dep83",
        "1.0.0.0",
        &format!(
            r#"{{"RuntimeVersion":"13.0","Interfaces":[{{"Name":"IDep","Methods":[{{"Name":"Go","Parameters":[]}}]}}],"AppId":"{DEP_GUID}","Name":"Dep83","Publisher":"probe","Version":"1.0.0.0"}}"#
        ),
    );

    let inputs = read(ws);
    let cross = al_sem::program::model::program_calls::assemble_and_resolve_cross_app_program(
        &inputs, MI, true,
    )
    .expect("cross-app model");
    let objects: Vec<_> = cross
        .resolved
        .workspace
        .objects
        .iter()
        .filter(|o| o.object_type == "Interface" && o.app_guid == DEP_GUID)
        .collect();
    assert_eq!(objects.len(), 1, "the dependency interface is in the model");
    let model_object = objects[0];

    // The cross-app build's graph (`ResolvedProgram::build` starts from this FULL build).
    let (ctx, _, _) = al_sem::program::resolve::full::build_program_with_coverage_profiled(
        &inputs,
        al_sem::program::profile::BuildProfile::FULL,
    )
    .unwrap();
    let graph = graph_ids(ctx.graph(), al_syntax::ir::ObjectKind::Interface, DEP_GUID);
    assert_eq!(graph.len(), 1, "the dependency interface is in the graph");

    assert_eq!(graph[0], model_object.id, "P3 == P8");
    assert_eq!(model_object.object_key, "@idep");
    assert_eq!(
        model_object.id.rsplit('/').next(),
        Some(model_object.object_key.as_str()),
        "object_key is the id's last segment"
    );
}
