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
//! - T6: the full snapshot's identity table and contracts keep both interfaces.
//! - T7: with a numbered and a numberless codeunit, the identity table stays
//!   ordinal and the contracts stay `locale_compare`.
//! - T8: `alsem diff` of a workspace that adds a second interface (or ControlAddIn)
//!   reports only its `object-added`, never a critical change on the old one.
//! - T9: GitHub #245's repro (`tests/fixtures/ws-diff-interface-added`), through
//!   `alsem fingerprint` and a diff of the two snapshot files.

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

/// T3's workspace: two interfaces and one ControlAddIn, none numbered. The second
/// interface's name holds every byte a consumer splits on, a doubled quote and a
/// non-ASCII letter.
fn t3_workspace(ws: &Path) {
    app_json(ws, "");
    write(
        &ws.join("src/IEmpty.Interface.al"),
        "interface IEmpty\n{\n}\n",
    );
    write(
        &ws.join("src/Odd.Interface.al"),
        "interface \"Ø \"\"Q\"\" /:#%@ If\"\n{\n}\n",
    );
    write(
        &ws.join("src/MyAddIn.ControlAddIn.al"),
        "controladdin \"My Add In\"\n{\n}\n",
    );
}

/// The internal ids of T3's two interfaces, `IEmpty` first.
fn t3_interface_ids() -> Vec<String> {
    vec![
        format!("{GUID}/Interface/@iempty"),
        format!("{GUID}/Interface/@%C3%B8%20%22q%22%20%2F%3A%23%25%40%20if"),
    ]
}

/// The full capability snapshot (`compose_full_snapshot`, the tree `prove`,
/// `fingerprint` and `diff` serialize) of a single-app workspace, as JSON.
fn full_snapshot(ws: &Path) -> serde_json::Value {
    use al_sem::engine::l5::snapshot_full::{
        FullSnapshotOptions, compose_full_snapshot, serialize_json,
    };
    let inputs = al_sem::program::inputs::InputSet::read(
        ws,
        &al_sem::program::inputs::InputNeeds {
            fingerprint_rows: true,
            ..Default::default()
        },
    );
    let model = al_sem::program::model::program_calls::assemble_and_resolve_workspace_program(
        &inputs, MI, true,
    )
    .expect("model");
    let opts = FullSnapshotOptions {
        inputs: &inputs,
        driver_version: "t83",
        deterministic: true,
        roots_config_ignored: false,
        universe: al_sem::engine::l5::snapshot_full::SnapshotUniverse::SingleApp,
    };
    serde_json::from_str(&serialize_json(&compose_full_snapshot(&model, &opts))).unwrap()
}

/// The identity table's `(stableId, displayName)` pairs whose id starts with
/// `prefix`, in table order.
fn identities_with(snap: &serde_json::Value, prefix: &str) -> Vec<(String, String)> {
    let ids = snap["identities"]["stableIds"].as_array().unwrap();
    let names = snap["identities"]["displayNames"].as_array().unwrap();
    assert_eq!(ids.len(), names.len(), "parallel arrays");
    ids.iter()
        .zip(names)
        .map(|(i, n)| (i.as_str().unwrap(), n.as_str().unwrap()))
        .filter(|(i, _)| i.starts_with(prefix))
        .map(|(i, n)| (i.to_string(), n.to_string()))
        .collect()
}

/// The object contracts' `stableId`s that start with `prefix`, in output order.
fn contract_ids_with(snap: &serde_json::Value, prefix: &str) -> Vec<String> {
    snap["contractFacts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] == "object")
        .map(|c| c["stableId"].as_str().unwrap().to_string())
        .filter(|i| i.starts_with(prefix))
        .collect()
}

/// T6. The two interfaces of T3's workspace both survive the full-snapshot path:
/// the identity table (a map keyed by `stableId`) and the contracts each hold
/// both, with distinct ids and each interface's own name.
///
/// Discrimination (2026-10-15): `object_key(None, …)` returning `"0"` again gives
/// both interfaces `…:Interface:0`, the identity table keeps one row and the
/// assertion fails; restored, it passes.
#[test]
fn full_snapshot_keeps_both_numberless_interfaces() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    t3_workspace(ws);
    let snap = full_snapshot(ws);
    let stable: Vec<String> = t3_interface_ids()
        .iter()
        .map(|i| i.replace('/', ":"))
        .collect();

    assert_eq!(
        sorted_pairs(identities_with(&snap, &format!("{GUID}:Interface:"))),
        sorted_pairs(vec![
            (stable[0].clone(), "IEmpty".to_string()),
            // The display name is the model's `name`, the logical name: the
            // source's doubled quote is one quote (#249).
            (stable[1].clone(), "Ø \"Q\" /:#%@ If".to_string()),
        ]),
        "identity table"
    );
    assert_eq!(
        sorted(contract_ids_with(&snap, &format!("{GUID}:Interface:"))),
        sorted(stable),
        "contracts"
    );
}

fn sorted_pairs(mut v: Vec<(String, String)>) -> Vec<(String, String)> {
    v.sort();
    v
}

/// T7. A numbered and a numberless codeunit in one app: the identity table is
/// ordinal (`…:Codeunit:1` before `…:Codeunit:@a`, since `1` < `@` by byte), the
/// contracts are `locale_compare` (`@` ranks before digits, so `…:@a` first).
/// Each output keeps its own order contract; nothing relies on the two agreeing.
///
/// Discrimination (2026-10-15): sorting the identity table with `locale_compare`
/// puts `…:@a` first and fails the identity-table assertion; sorting the
/// contracts ordinally puts `…:1` first and fails the contracts assertion; each
/// restored, it passes.
#[test]
fn mixed_object_keys_keep_each_outputs_order() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    app_json(ws, "");
    write(
        &ws.join("src/A.Codeunit.al"),
        "codeunit 99999999999999999999 \"A\"\n{\n}\n",
    );
    write(&ws.join("src/B.Codeunit.al"), "codeunit 1 \"B\"\n{\n}\n");
    let snap = full_snapshot(ws);
    let numbered = format!("{GUID}:Codeunit:1");
    let numberless = format!("{GUID}:Codeunit:@a");

    assert_eq!(
        identities_with(&snap, &format!("{GUID}:Codeunit:")),
        vec![
            (numbered.clone(), "B".to_string()),
            (numberless.clone(), "A".to_string()),
        ],
        "identity table: ordinal"
    );
    assert_eq!(
        contract_ids_with(&snap, &format!("{GUID}:Codeunit:")),
        vec![numberless, numbered],
        "contracts: locale_compare"
    );
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
    t3_workspace(ws);
    let internal = sorted(t3_interface_ids());
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

    // ControlAddIn is the other numberless kind: the same literal id from every
    // producer that projects it.
    let cai = format!("{GUID}/ControlAddIn/@my%20add%20in");
    let cai_stable = cai.replace('/', ":");
    let p1c: Vec<_> = r0
        .objects
        .iter()
        .filter(|o| o.kind == "ControlAddIn")
        .map(|o| o.stable_object_id.clone())
        .collect();
    assert_eq!(p1c, vec![cai_stable.clone()], "P1 ControlAddIn");
    let p2c: Vec<_> = model
        .workspace
        .objects
        .iter()
        .filter(|o| o.object_type == "ControlAddIn")
        .map(|o| o.id.clone())
        .collect();
    assert_eq!(p2c, vec![cai.clone()], "P2 ControlAddIn");
    assert_eq!(
        graph_ids(ctx.graph(), al_syntax::ir::ObjectKind::ControlAddIn, GUID),
        vec![cai],
        "P3 ControlAddIn"
    );
    let p5c: Vec<_> = l2
        .objects
        .iter()
        .filter(|o| o.object_type == "ControlAddIn")
        .map(|o| o.stable_object_id.clone())
        .collect();
    assert_eq!(p5c, vec![cai_stable], "P5 ControlAddIn");

    // The model's `object_key` is the id's last segment, for every source object.
    for o in &model.workspace.objects {
        assert_eq!(
            o.id.rsplit('/').next(),
            Some(o.object_key.as_str()),
            "{o:?}"
        );
    }
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
    for o in &model.workspace.objects {
        assert_eq!(
            o.id.rsplit('/').next(),
            Some(o.object_key.as_str()),
            "{o:?}"
        );
    }

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

/// #250's dependency symbols: interfaces `A""B` (logical; JSON `A\"\"B`) and
/// `IPlain` without `Id`, codeunit 50200 `C""D` and codeunit 50201 `Impl`
/// implementing both interfaces.
const DEP_250_SYMBOLS: &str = r##"{"RuntimeVersion":"13.0","Interfaces":[{"Name":"A\"\"B","Methods":[{"Name":"Go","Parameters":[]}]},{"Name":"IPlain","Methods":[{"Name":"Go","Parameters":[]}]}],"Codeunits":[{"Id":50200,"Name":"C\"\"D","Methods":[{"Name":"Run2","Parameters":[]}]},{"Id":50201,"Name":"Impl","ImplementedInterfaces":["A\"\"B","IPlain"],"Methods":[{"Name":"Go","Parameters":[]}]}],"AppId":"83830000-0000-0000-0000-0000000000de","Name":"Dep83","Publisher":"probe","Version":"1.0.0.0"}"##;

fn dep_250_entry() -> String {
    format!(r#"{{"id":"{DEP_GUID}","name":"Dep83","publisher":"probe","version":"1.0.0.0"}}"#)
}

/// T1 (#250). A symbol-only dependency declares `interface "A""""B"` (logical
/// `A""B`, no `Id`). Its graph key is the folded logical name `a""b`, and its
/// graph id (P3) equals its projected `ModelObject` id (P8), both keeping both
/// quotes: `@a%22%22b`.
///
/// Discrimination (2026-10-23): `ObjKey::model_key` unescaping again gives P3
/// `@a%22b` and fails; restored, it passes.
#[test]
fn dependency_interface_with_escaped_quotes_graph_id_equals_its_model_object_id() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    app_json(ws, &dep_250_entry());
    write(
        &ws.join("src/W.Codeunit.al"),
        "codeunit 50101 W\n{\n    procedure Caller()\n    var\n        X: Interface \"A\"\"\"\"B\";\n        P: Interface IPlain;\n        C: Codeunit \"C\"\"\"\"D\";\n    begin\n        X.Go();\n        P.Go();\n        C.Run2();\n    end;\n}\n",
    );
    crate::symbol_app::write_symbol_app(
        &ws.join(".alpackages/probe_Dep83_1.0.0.0.app"),
        DEP_GUID,
        "Dep83",
        "1.0.0.0",
        DEP_250_SYMBOLS,
    );

    let inputs = read(ws);
    let cross = al_sem::program::model::program_calls::assemble_and_resolve_cross_app_program(
        &inputs, MI, true,
    )
    .expect("cross-app model");
    let model_object = cross
        .resolved
        .workspace
        .objects
        .iter()
        .find(|o| o.object_type == "Interface" && o.app_guid == DEP_GUID && o.name == "A\"\"B")
        .expect("the dependency interface A\"\"B is in the model");

    let (ctx, _, _) = al_sem::program::resolve::full::build_program_with_coverage_profiled(
        &inputs,
        al_sem::program::profile::BuildProfile::FULL,
    )
    .unwrap();
    let graph = ctx.graph();
    let node = graph
        .objects
        .iter()
        .find(|o| {
            o.id.kind == al_syntax::ir::ObjectKind::Interface
                && graph.apps.resolve(o.id.app).guid == DEP_GUID
                && &*o.name == "A\"\"B"
        })
        .expect("the dependency interface A\"\"B is in the graph");
    assert_eq!(
        node.id.key,
        al_sem::program::node::ObjKey::Name("a\"\"b".into()),
        "the graph key is the folded logical name"
    );
    let expected = format!("{DEP_GUID}/Interface/@a%22%22b");
    assert_eq!(model_object_id(graph, &node.id), expected, "P3");
    assert_eq!(model_object.id, expected, "P8");
    assert_eq!(
        model_object.id.rsplit('/').next(),
        Some(model_object.object_key.as_str()),
        "object_key is the id's last segment"
    );
}

/// T2's caller: one call through `X: Interface "A""""B"`, one on
/// `C: Codeunit "C""""D"`.
const T2_CALLER: &str = "codeunit 50101 W\n{\n    procedure Caller()\n    var\n        X: Interface \"A\"\"\"\"B\";\n        C: Codeunit \"C\"\"\"\"D\";\n    begin\n        X.Go();\n        C.Run2();\n    end;\n}\n";
const T2_IFACE: &str = "interface \"A\"\"\"\"B\"\n{\n    procedure Go();\n}\n";

/// T2 asserts on one workspace: the source interface `A""""B` of app `guid` has
/// model id `…/Interface/@a%22%22b`, `X.Go()` reaches `Impl.Go` (codeunit `impl`)
/// and `C.Run2()` reaches codeunit `cd`'s `Run2`.
fn assert_t2(ws: &Path, guid: &str, impl_no: i64, cd_no: i64) {
    use al_sem::program::node::ObjKey;
    use al_sem::program::resolve::edge::RouteTarget;

    let inputs = read(ws);
    let (ctx, _, _) = al_sem::program::resolve::full::build_program_with_coverage_profiled(
        &inputs,
        al_sem::program::profile::BuildProfile::FULL,
    )
    .unwrap();
    assert_eq!(
        graph_ids(ctx.graph(), al_syntax::ir::ObjectKind::Interface, guid)
            .iter()
            .filter(|id| id.ends_with("/@a%22%22b"))
            .collect::<Vec<_>>(),
        vec![&format!("{guid}/Interface/@a%22%22b")],
        "the source interface keeps its model id"
    );

    let (_g, edges, _primary) =
        al_sem::program::resolve::full::resolve_full_program_for_export(ws).unwrap();
    let reaches = |no: i64, name_lc: &str| {
        edges.iter().any(|e| {
            &*e.edge.from.name_lc == "caller"
                && e.edge.routes.iter().any(|r| {
                    matches!(&r.target, RouteTarget::Routine(t)
                        if t.object.key == ObjKey::Id(no) && &*t.name_lc == name_lc)
                })
        })
    };
    assert!(reaches(impl_no, "go"), "X.Go() reaches Impl.Go");
    assert!(reaches(cd_no, "run2"), "C.Run2() reaches C\"\"D.Run2");
}

/// T2 (#250), the control. Where both sides are source (S2), or the dependency
/// ships its source (S3, tier `EmbeddedSource`), the source interface `A""""B`
/// keeps its model id `@a%22%22b`, and both escaped-name calls resolve.
///
/// Discrimination (2026-10-23): `ObjKey::from_source` keying on `fold(raw)` (no
/// unescape, `model_key` unchanged) gives `@a%22%22%22%22b` and fails; so does a
/// second unescape in `model_key` (`@a%22b`); restored, it passes.
#[test]
fn source_objects_with_escaped_quotes_keep_ids_and_resolution() {
    // S2: both sides source.
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    app_json(ws, "");
    write(&ws.join("src/AB.Interface.al"), T2_IFACE);
    write(
        &ws.join("src/CD.Codeunit.al"),
        "codeunit 50103 \"C\"\"\"\"D\"\n{\n    procedure Run2()\n    begin\n    end;\n}\n",
    );
    write(
        &ws.join("src/Impl.Codeunit.al"),
        "codeunit 50102 Impl implements \"A\"\"\"\"B\"\n{\n    procedure Go()\n    begin\n    end;\n}\n",
    );
    write(&ws.join("src/W.Codeunit.al"), T2_CALLER);
    assert_t2(ws, GUID, 50102, 50103);

    // S3: the dependency ships its source.
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    app_json(ws, &dep_250_entry());
    write(&ws.join("src/W.Codeunit.al"), T2_CALLER);
    crate::symbol_app::write_source_app(
        &ws.join(".alpackages/probe_Dep83_1.0.0.0.app"),
        DEP_GUID,
        "Dep83",
        "1.0.0.0",
        DEP_250_SYMBOLS,
        &[
            ("src/AB.Interface.al", T2_IFACE),
            (
                "src/IPlain.Interface.al",
                "interface IPlain\n{\n    procedure Go();\n}\n",
            ),
            (
                "src/CD.Codeunit.al",
                "codeunit 50200 \"C\"\"\"\"D\"\n{\n    procedure Run2()\n    begin\n    end;\n}\n",
            ),
            (
                "src/Impl.Codeunit.al",
                "codeunit 50201 Impl implements \"A\"\"\"\"B\", IPlain\n{\n    procedure Go()\n    begin\n    end;\n}\n",
            ),
        ],
        "",
    );
    let inputs = read(ws);
    let (ctx, _, _) = al_sem::program::resolve::full::build_program_with_coverage_profiled(
        &inputs,
        al_sem::program::profile::BuildProfile::FULL,
    )
    .unwrap();
    assert!(
        ctx.graph().objects.iter().any(|o| {
            ctx.graph().apps.resolve(o.id.app).guid == DEP_GUID
                && o.tier == al_sem::snapshot::TrustTier::EmbeddedSource
        }),
        "precondition: the dependency is read from its embedded source"
    );
    assert_t2(ws, DEP_GUID, 50201, 50200);
}

/// `alsem diff <old ws> <new ws> --format json`: the report's findings.
fn diff_findings(old: &Path, new: &Path) -> Vec<serde_json::Value> {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("diff")
        .arg(old)
        .arg(new)
        .args(["--deterministic", "--format", "json"])
        .env("RUST_LOG", "off")
        .output()
        .expect("run alsem diff");
    assert!(
        out.status.success(),
        "alsem diff exit {:?}; stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "diff json: {e}; stdout={stdout}; stderr={}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    report["payload"]["findings"].as_array().unwrap().clone()
}

/// T8 (the CentralGauge regression): the old workspace has one numberless object
/// `IFoo` with procedure `Run()`; the new one keeps it unchanged and ADDS a second,
/// `IBar`, with the same `Run()`. Both orders of the two files are run: with a
/// shared `…:<Kind>:0` id the two object contracts collided, the diff index kept
/// the last row, and when `IBar`'s file sorted last `diff` reported a critical
/// `procedure-signature-changed` on `…:<Kind>:0` (named `IBar`) for an untouched
/// `IFoo`. When it sorted first, `IBar`'s addition was not reported at all.
/// Now the only finding is `IBar`'s `object-added` (info).
fn adding_a_numberless_object_is_not_a_break(kind: &str, file_kind: &str) {
    let decl = |name: &str| format!("{kind} \"{name}\"\n{{\n    procedure Run();\n}}\n");
    // `Zz` (the critical case) first, so a regression reports it.
    for bar_file in [
        format!("src/Zz.IBar.{file_kind}.al"),
        format!("src/Aa.IBar.{file_kind}.al"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old");
        let new = dir.path().join("new");
        for ws in [&old, &new] {
            app_json(ws, "");
            write(&ws.join(format!("src/IFoo.{file_kind}.al")), &decl("IFoo"));
        }
        write(&new.join(&bar_file), &decl("IBar"));

        let findings = diff_findings(&old, &new);
        let got: Vec<(&str, &str, &str)> = findings
            .iter()
            .map(|f| {
                (
                    f["kind"].as_str().unwrap_or(""),
                    f["severity"].as_str().unwrap_or(""),
                    f["subject"]["newStableId"].as_str().unwrap_or(""),
                )
            })
            .collect();
        let bar_id = format!("{GUID}:{file_kind}:@ibar");
        assert_eq!(
            got,
            vec![("object-added", "info", bar_id.as_str())],
            "{bar_file}: findings {findings:#?}"
        );
    }
}

/// T8, Interface.
///
/// Red on master `3a2d5923` (2026-10-23): `procedure-signature-changed`,
/// critical, on `…:Interface:0`. Discrimination (2026-10-23): `object_key(None, …)`
/// returning `"0"` again fails this test; restored, it passes.
#[test]
fn diff_adding_an_interface_is_not_a_signature_change() {
    adding_a_numberless_object_is_not_a_break("interface", "Interface");
}

/// T8, ControlAddIn (red on master the same way, on `…:ControlAddIn:0`).
#[test]
fn diff_adding_a_controladdin_is_not_a_signature_change() {
    adding_a_numberless_object_is_not_a_break("controladdin", "ControlAddIn");
}

/// T9: GitHub #245's repro, as CentralGauge runs it (`tests/fixtures/
/// ws-diff-interface-added`): `alsem fingerprint` of `start/` and `final/`, then
/// `alsem diff start.json final.json`. `final/` adds "Repro Transport" beside
/// "Repro Strategy". The two interfaces carry distinct stable ids, and the diff
/// reports only the new one's `object-added`.
///
/// Red on master `3a2d5923` (2026-10-23): both fingerprints hold the one id
/// `…:Interface:0`, and the diff's only finding is a critical
/// `procedure-signature-changed` named "Repro Transport". Discrimination
/// (2026-10-23): `object_key(None, …)` returning `"0"` again fails this test;
/// restored, it passes.
#[test]
fn diff_issue_245_interface_added_fixture() {
    const APP: &str = "11111111-2222-4333-8444-555555555555";
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ws-diff-interface-added");
    let out_dir = tempfile::tempdir().unwrap();
    let fingerprint = |side: &str| {
        let out = out_dir.path().join(format!("{side}.json"));
        let run = std::process::Command::new(env!("CARGO_BIN_EXE_alsem"))
            .arg("fingerprint")
            .arg(fixture.join(side))
            .args(["--format", "json", "--deterministic", "--out"])
            .arg(&out)
            .env("RUST_LOG", "off")
            .output()
            .expect("run alsem fingerprint");
        assert!(
            run.status.success(),
            "fingerprint {side}: {}",
            String::from_utf8_lossy(&run.stderr)
        );
        let text = std::fs::read_to_string(&out).unwrap();
        (
            out,
            serde_json::from_str::<serde_json::Value>(&text).unwrap(),
        )
    };
    let (start, _) = fingerprint("start");
    let (final_, final_json) = fingerprint("final");

    let strategy = format!("{APP}:Interface:@repro%20strategy");
    let transport = format!("{APP}:Interface:@repro%20transport");
    let mut interface_ids: Vec<String> = final_json["payload"]["contractFacts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] == "object")
        .filter_map(|c| c["stableId"].as_str())
        .filter(|i| i.contains(":Interface:"))
        .map(str::to_string)
        .collect();
    interface_ids.sort();
    assert_eq!(
        interface_ids,
        vec![strategy, transport.clone()],
        "distinct ids"
    );

    let findings = diff_findings(&start, &final_);
    let got: Vec<(&str, &str, &str, &str)> = findings
        .iter()
        .map(|f| {
            (
                f["kind"].as_str().unwrap_or(""),
                f["severity"].as_str().unwrap_or(""),
                f["subject"]["displayName"].as_str().unwrap_or(""),
                f["subject"]["newStableId"].as_str().unwrap_or(""),
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![(
            "object-added",
            "info",
            "Repro Transport",
            transport.as_str()
        )],
        "findings {findings:#?}"
    );
}
