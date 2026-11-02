//! T-incr (model), T-loc, T-refine, T-demand-transitive, T-methodid, T-world
//! (context-server design §11, stage 4): after every edit, the session's
//! program and model equal a cold build of the edited workspace.
//!
//! Each test writes a workspace with a source-bearing dependency (so the
//! dependency bodies are resolved), refreshes a session, then states an edit by
//! assignment, refreshes again, and compares the session's program (every edge
//! with its witnesses, the site facts, the parens-less calls, the meta) and its
//! model (rows, calls, bindings, events, diagnostics, everything a consumer
//! reads) with `ResolvedProgram::build` and `assemble_and_resolve_cross_app_program`
//! over the same inputs, as text.

use std::path::PathBuf;
use std::sync::Arc;

use super::*;
use crate::program::model::program_calls::assemble_and_resolve_cross_app_program;

const WS_GUID: &str = "5e550000-0000-4000-8000-000000000001";
const DEP_GUID: &str = "5e550000-0000-4000-8000-0000000000de";

/// The dependency's embedded source: a library (`A`, `B` calling `C`), an
/// event the workspace subscribes to (raised by `Raise`), an interface and a
/// dependency caller dispatching over it (the workspace implements it).
const DEP_LIB: &str = "codeunit 70000 \"Dep Lib\"\n{\n    procedure A()\n    begin\n    end;\n\n    procedure B()\n    begin\n        C();\n    end;\n\n    local procedure C()\n    begin\n    end;\n\n    procedure Raise()\n    begin\n        OnDepEvent();\n    end;\n\n    [IntegrationEvent(false, false)]\n    procedure OnDepEvent()\n    begin\n    end;\n}\n";
const DEP_I: &str = "interface \"Dep I\"\n{\n    procedure M();\n}\n";
const DEP_CALLER: &str = "codeunit 70001 \"Dep Caller\"\n{\n    procedure CallI(X: Interface \"Dep I\")\n    begin\n        X.M();\n    end;\n}\n";
const DEP_SYMBOLS: &str = r#"{"Codeunits":[{"Id":70000,"Name":"Dep Lib","Methods":[{"Name":"A","Id":1},{"Name":"B","Id":2},{"Name":"Raise","Id":3},{"Name":"OnDepEvent","Id":4}]},{"Id":70001,"Name":"Dep Caller","Methods":[{"Name":"CallI","Id":1}]}],"Interfaces":[{"Name":"Dep I","Methods":[{"Name":"M","Id":1}]}]}"#;

const CALLEE: &str = "codeunit 50100 \"Callee\"\n{\n    procedure P(X: Integer): Integer\n    begin\n        exit(X);\n    end;\n\n    procedure Q()\n    begin\n    end;\n}\n";
const CALLER: &str = "codeunit 50101 \"Caller\"\n{\n    procedure Go()\n    var\n        C: Codeunit \"Callee\";\n        D: Codeunit \"Dep Lib\";\n        N: Integer;\n    begin\n        N := C.P(1);\n        C.Q();\n        D.A();\n    end;\n}\n";
const SUB: &str = "codeunit 50102 \"Sub\"\n{\n    [EventSubscriber(ObjectType::Codeunit, Codeunit::\"Dep Lib\", 'OnDepEvent', '', false, false)]\n    local procedure Handle()\n    begin\n    end;\n}\n";
const IMPL: &str =
    "codeunit 50103 \"Impl\" implements \"Dep I\"\n{\n    procedure M()\n    begin\n    end;\n}\n";

/// A workspace on disk: `root/app.json`, `root/src/*.al`, and the dependency
/// package in `root/.alpackages`.
struct Ws {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Ws {
    fn new(files: &[(&str, &str)]) -> Ws {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("ws");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("app.json"),
            format!(
                r#"{{"id":"{WS_GUID}","name":"Session Ws","publisher":"probe","version":"1.0.0.0","dependencies":[{{"id":"{DEP_GUID}","name":"Dep App","publisher":"Microsoft","version":"1.0.0.0"}}]}}"#
            ),
        )
        .unwrap();
        let ws = Ws { _dir: dir, root };
        ws.write_dep(&[("src/DepLib.Codeunit.al", DEP_LIB)]);
        for (name, text) in files {
            ws.write(name, text);
        }
        ws
    }

    /// The dependency package: `extra` sources beside the interface and its
    /// caller.
    fn write_dep(&self, lib: &[(&str, &str)]) {
        use crate::engine::deps::app_package_zip::test_apps;
        let manifest = test_apps::manifest_xml(DEP_GUID, "Dep App");
        let mut entries: Vec<(&str, &[u8])> = vec![
            ("NavxManifest.xml", manifest.as_bytes()),
            ("SymbolReference.json", DEP_SYMBOLS.as_bytes()),
            ("src/DepI.Interface.al", DEP_I.as_bytes()),
            ("src/DepCaller.Codeunit.al", DEP_CALLER.as_bytes()),
        ];
        for (name, text) in lib {
            entries.push((name, text.as_bytes()));
        }
        let pk = self.root.join(".alpackages");
        std::fs::create_dir_all(&pk).unwrap();
        std::fs::write(
            pk.join("Microsoft_Dep App_1.0.0.0.app"),
            test_apps::build_app(&entries),
        )
        .unwrap();
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join("src").join(name)
    }

    fn write(&self, name: &str, text: &str) {
        std::fs::write(self.path(name), text).unwrap();
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.path(name)).unwrap()
    }

    /// Replace exactly one occurrence of `old` in `name` (asserted: an edit
    /// that matched nothing would prove nothing).
    fn edit(&self, name: &str, old: &str, new: &str) {
        let text = self.read(name);
        assert_eq!(text.matches(old).count(), 1, "edit of {name}: {old:?}");
        self.write(name, &text.replacen(old, new, 1));
    }

    fn remove(&self, name: &str) {
        std::fs::remove_file(self.path(name)).unwrap();
    }

    fn inputs(&self) -> Arc<WorkspaceInputs> {
        WorkspaceInputs::read(&self.root, &Default::default())
    }
}

fn program_text(p: &ResolvedProgram) -> String {
    let mut s = String::new();
    write_program(p, &mut s).unwrap();
    s
}

fn model_text(x: &CrossAppProgram) -> String {
    let mut s = String::new();
    write_model(x, &mut s).unwrap();
    s
}

/// The first line where two texts differ, with context, for a readable failure.
fn first_diff(a: &str, b: &str) -> String {
    for (i, (x, y)) in a.lines().zip(b.lines()).enumerate() {
        if x != y {
            return format!("line {i}:\n  session: {x}\n  cold:    {y}");
        }
    }
    format!(
        "one text is a prefix of the other ({} vs {} lines)",
        a.lines().count(),
        b.lines().count()
    )
}

/// One refresh, compared with a cold build of the same inputs at both levels.
/// Returns what the refresh did.
fn refresh_and_compare(s: &mut WorkspaceSession, ws: &Ws, what: &str) -> RefreshReport {
    let inputs = ws.inputs();
    let program = s.refresh_program(&inputs).expect("session program");
    let report = s.last_report().cloned().expect("a report");
    let cold = ResolvedProgram::build(&inputs, ProgramScope::CrossApp).expect("cold program");
    let (a, b) = (program_text(&program), program_text(&cold));
    assert!(
        a == b,
        "{what}: program differs from cold: {}",
        first_diff(&a, &b)
    );
    compare_model(s, &inputs, Ok(program), AssemblyNeeds::RULES, what);
    s.last_report().cloned().unwrap_or(report)
}

/// The session's model over `program` (its row cache in use), compared with
/// a cold model build of the same inputs.
fn compare_model(
    s: &mut WorkspaceSession,
    inputs: &Arc<WorkspaceInputs>,
    program: Result<ResolvedProgram, String>,
    needs: AssemblyNeeds,
    what: &str,
) {
    let model = s
        .refresh_model(program, inputs, false, needs)
        .unwrap_or_else(|e| panic!("{what}: session model: {e:?}"));
    let cold = assemble_and_resolve_cross_app_program(inputs, false, needs)
        .unwrap_or_else(|e| panic!("{what}: cold model: {e:?}"));
    let (a, b) = (model_text(&model), model_text(&cold));
    assert!(
        a == b,
        "{what}: model differs from cold: {}",
        first_diff(&a, &b)
    );
}

/// What the last refresh's model assembly took from the row cache.
fn rows(r: &RefreshReport) -> RowCacheStats {
    r.rows.expect("the model was assembled through the session")
}

fn base_ws() -> Ws {
    Ws::new(&[
        ("Callee.Codeunit.al", CALLEE),
        ("Caller.Codeunit.al", CALLER),
        ("Sub.Codeunit.al", SUB),
        ("Impl.Codeunit.al", IMPL),
    ])
}

/// The harness itself: two cold builds of one workspace give the same text
/// (so a difference below is the session's, not the build's), and the first
/// refresh is a cold build (P).
fn first_refresh_is_cold_and_the_texts_are_deterministic(mode: MemoryMode) {
    let ws = base_ws();
    let inputs = ws.inputs();
    let a = ResolvedProgram::build(&inputs, ProgramScope::CrossApp).unwrap();
    let b = ResolvedProgram::build(&inputs, ProgramScope::CrossApp).unwrap();
    assert_eq!(program_text(&a), program_text(&b));
    assert!(
        !a.body().dependency_body_edges().is_empty(),
        "precondition: the dependency bodies are resolved"
    );
    let ma = assemble_and_resolve_cross_app_program(&inputs, false, AssemblyNeeds::RULES).unwrap();
    let mb = assemble_and_resolve_cross_app_program(&inputs, false, AssemblyNeeds::RULES).unwrap();
    assert_eq!(model_text(&ma), model_text(&mb));
    let mut s = WorkspaceSession::with_mode(mode);
    let r = refresh_and_compare(&mut s, &ws, "first refresh");
    assert_eq!(r.class, ChangeClass::Package);
    assert_eq!(r.parsed, 4);
    assert_eq!(r.resolved, 4);
    assert!(r.dependency_bodies_resolved);
    // Nothing changed: B with nothing parsed or resolved.
    let r = refresh_and_compare(&mut s, &ws, "no change");
    assert_eq!(r.class, ChangeClass::BodyOnly);
    assert_eq!((r.parsed, r.resolved), (0, 0));
    assert!(!r.dependency_bodies_resolved);
    assert!(r.changed.is_empty());
}

/// T-incr B: a callee's body grows; only its file is parsed and resolved, the
/// dependency bodies are reused.
fn body_edit_reuses_every_other_resolution(mode: MemoryMode) {
    let ws = base_ws();
    let mut s = WorkspaceSession::with_mode(mode);
    refresh_and_compare(&mut s, &ws, "cold");
    ws.edit(
        "Callee.Codeunit.al",
        "        exit(X);",
        "        X += 1;\n        Q();\n        exit(X * 2);",
    );
    let r = refresh_and_compare(&mut s, &ws, "body edit");
    assert_eq!(r.class, ChangeClass::BodyOnly);
    assert_eq!(r.changed, vec!["src/Callee.Codeunit.al".to_string()]);
    assert_eq!((r.parsed, r.resolved), (1, 1));
    assert!(!r.dependency_bodies_resolved);
    assert!(
        !r.dependency_edges_copied,
        "no dependency witness moved: the dependency edges are shared, not copied"
    );
    assert!(r.copied_bytes > 0, "the reused resolutions were copied");
    // T-rows: only the edited file is projected again; the other workspace
    // files' and every dependency file's raw rows come from the cache.
    let rs = rows(&r);
    assert_eq!((rs.workspace_projected, rs.workspace_reused), (1, 3));
    assert_eq!(rs.dependency_projected, 0);
    assert!(
        rs.dependency_reused > 0,
        "precondition: dependency rows exist"
    );
    // A second body edit in the caller's own file.
    ws.edit(
        "Caller.Codeunit.al",
        "        C.Q();",
        "        C.Q();\n        C.Q();",
    );
    let r = refresh_and_compare(&mut s, &ws, "caller's own body edit");
    assert_eq!(r.class, ChangeClass::BodyOnly);
    assert_eq!((r.parsed, r.resolved), (1, 1));
}

/// T-loc: text inserted before a callee moves its declaration; the unchanged
/// workspace caller's witness, and the unchanged DEPENDENCY caller's witness to
/// the workspace implementer, follow it (re-materialized, never stale).
fn newline_before_a_callee_moves_every_witness(mode: MemoryMode) {
    let ws = base_ws();
    let mut s = WorkspaceSession::with_mode(mode);
    refresh_and_compare(&mut s, &ws, "cold");
    ws.edit(
        "Callee.Codeunit.al",
        "    procedure Q()",
        "\n\n\n    procedure Q()",
    );
    ws.edit(
        "Impl.Codeunit.al",
        "    procedure M()",
        "\n\n    procedure M()",
    );
    let r = refresh_and_compare(&mut s, &ws, "newlines");
    assert_eq!(r.class, ChangeClass::BodyOnly);
    assert!(!r.dependency_bodies_resolved);
    assert!(
        r.dependency_edges_copied,
        "the dependency caller's witness to the moved implementer changed"
    );
    assert_eq!(r.resolved, 2);
}

/// T-incr S: a declaration reorder, a signature change and a return-type
/// change each re-resolve every workspace file and the dependency bodies.
fn declaration_changes_reresolve_everything(mode: MemoryMode) {
    let ws = base_ws();
    let mut s = WorkspaceSession::with_mode(mode);
    refresh_and_compare(&mut s, &ws, "cold");
    ws.write(
        "Callee.Codeunit.al",
        "codeunit 50100 \"Callee\"\n{\n    procedure Q()\n    begin\n    end;\n\n    procedure P(X: Integer): Integer\n    begin\n        exit(X);\n    end;\n}\n",
    );
    let r = refresh_and_compare(&mut s, &ws, "reorder");
    assert_eq!(r.class, ChangeClass::Declaration);
    assert_eq!(r.resolved, 4);
    assert!(r.dependency_bodies_resolved);
    ws.edit("Callee.Codeunit.al", "P(X: Integer)", "P(X: Decimal)");
    let r = refresh_and_compare(&mut s, &ws, "signature");
    assert_eq!(r.class, ChangeClass::Declaration);
    ws.edit("Callee.Codeunit.al", "): Integer", "): Text");
    let r = refresh_and_compare(&mut s, &ws, "return type");
    assert_eq!(r.class, ChangeClass::Declaration);
    // The caller now passes 1 to a Decimal and reads a Text: still equal.
    ws.edit(
        "Callee.Codeunit.al",
        "    procedure Q()",
        "    procedure Q(Y: Integer)",
    );
    let r = refresh_and_compare(&mut s, &ws, "arity change breaks the caller's call");
    assert_eq!(r.class, ChangeClass::Declaration);
}

/// T-incr F: a file added, removed and renamed (each S).
fn file_add_remove_rename(mode: MemoryMode) {
    let ws = base_ws();
    let mut s = WorkspaceSession::with_mode(mode);
    refresh_and_compare(&mut s, &ws, "cold");
    ws.write(
        "Extra.Codeunit.al",
        "codeunit 50104 \"Extra\"\n{\n    procedure R()\n    var\n        C: Codeunit \"Callee\";\n    begin\n        C.Q();\n    end;\n}\n",
    );
    let r = refresh_and_compare(&mut s, &ws, "add");
    assert_eq!(r.class, ChangeClass::Declaration);
    assert_eq!(r.changed, vec!["src/Extra.Codeunit.al".to_string()]);
    let text = ws.read("Extra.Codeunit.al");
    ws.remove("Extra.Codeunit.al");
    ws.write("Renamed.Codeunit.al", &text);
    let r = refresh_and_compare(&mut s, &ws, "rename");
    assert_eq!(r.class, ChangeClass::Declaration);
    ws.remove("Renamed.Codeunit.al");
    let r = refresh_and_compare(&mut s, &ws, "remove");
    assert_eq!(r.class, ChangeClass::Declaration);
    assert_eq!(r.changed, vec!["src/Renamed.Codeunit.al".to_string()]);
    // A second implementer of the DEPENDENCY's interface: the dependency
    // caller's `X.M()` dispatch changes, so the dependency bodies must be
    // resolved again (§6.6: never reused across S).
    ws.write(
        "Impl2.Codeunit.al",
        "codeunit 50106 \"Impl2\" implements \"Dep I\"\n{\n    procedure M()\n    begin\n    end;\n}\n",
    );
    let r = refresh_and_compare(&mut s, &ws, "second implementer");
    assert_eq!(r.class, ChangeClass::Declaration);
    assert!(r.dependency_bodies_resolved);
}

/// T-demand-transitive: a body edit moves the workspace's dependency call from
/// `A` (W) to `B` (D1, which reaches `C`: D2), then removes it (contraction).
/// The demand is computed whole every revision, from the reused dependency
/// edges.
fn demand_expansion_and_contraction(mode: MemoryMode) {
    let ws = base_ws();
    let mut s = WorkspaceSession::with_mode(mode);
    refresh_and_compare(&mut s, &ws, "cold");
    ws.edit("Caller.Codeunit.al", "        D.A();", "        D.B();");
    let r = refresh_and_compare(&mut s, &ws, "W -> D1 -> D2");
    assert_eq!(r.class, ChangeClass::BodyOnly);
    assert!(!r.dependency_bodies_resolved);
    ws.edit("Caller.Codeunit.al", "        D.B();", "");
    let r = refresh_and_compare(&mut s, &ws, "contraction");
    assert_eq!(r.class, ChangeClass::BodyOnly);
    ws.edit(
        "Caller.Codeunit.al",
        "        C.Q();",
        "        C.Q();\n        D.Raise();",
    );
    refresh_and_compare(&mut s, &ws, "dependency raiser reached");
}

/// The dependency raises an event the workspace subscribes to: a body edit in
/// the subscriber (B), then a second subscriber (S).
fn dependency_raiser_and_subscriber_edits(mode: MemoryMode) {
    let ws = base_ws();
    let mut s = WorkspaceSession::with_mode(mode);
    refresh_and_compare(&mut s, &ws, "cold");
    ws.edit(
        "Sub.Codeunit.al",
        "    begin\n    end;",
        "    var\n        C: Codeunit \"Callee\";\n    begin\n        C.Q();\n    end;",
    );
    let r = refresh_and_compare(&mut s, &ws, "subscriber body");
    assert_eq!(r.class, ChangeClass::BodyOnly);
    ws.edit(
        "Sub.Codeunit.al",
        "}\n",
        "\n    [EventSubscriber(ObjectType::Codeunit, Codeunit::\"Dep Lib\", 'OnDepEvent', '', false, false)]\n    local procedure Handle2()\n    begin\n    end;\n}\n",
    );
    let r = refresh_and_compare(&mut s, &ws, "second subscriber");
    assert_eq!(r.class, ChangeClass::Declaration);
}

/// The event-flow edges are rerun whole every revision (§6.5). The
/// counterexample to every finer rule keyed on the PUBLISHER's file or on the
/// class: text inserted above the workspace subscriber (B, its projection
/// kept) moves its declaration, and the edge of the DEPENDENCY publisher
/// `OnDepEvent` (whose file, package and projection are unchanged) carries
/// the subscriber's span as its route witness, so the edge changes. Reusing
/// the previous revision's event-flow edges under B, or recomputing only the
/// publishers in changed files, would keep the stale span.
fn event_flow_edge_of_an_unchanged_publisher_moves_under_b(mode: MemoryMode) {
    use crate::program::resolve::edge::EdgeKind;
    let ws = base_ws();
    let edge = |ws: &Ws| {
        let p = ResolvedProgram::build(&ws.inputs(), ProgramScope::CrossApp).unwrap();
        let e: Vec<String> = p
            .body()
            .all_edges()
            .filter(|e| {
                e.edge.kind == EdgeKind::EventFlow && e.edge.from.name_lc.as_str() == "ondepevent"
            })
            .map(|e| format!("{:?}", e.edge))
            .collect();
        assert_eq!(e.len(), 1, "precondition: one edge for the publisher");
        e[0].clone()
    };
    let mut s = WorkspaceSession::with_mode(mode);
    refresh_and_compare(&mut s, &ws, "cold");
    let before = edge(&ws);
    ws.edit(
        "Sub.Codeunit.al",
        "    [EventSubscriber",
        "\n\n\n    [EventSubscriber",
    );
    assert_ne!(
        edge(&ws),
        before,
        "precondition: the unchanged publisher's edge moves with its subscriber"
    );
    let r = refresh_and_compare(&mut s, &ws, "lines above the subscriber");
    assert_eq!(r.class, ChangeClass::BodyOnly);
    assert_eq!(r.changed, vec!["src/Sub.Codeunit.al".to_string()]);
}

/// Several routines per file and a duplicate id (`#if` arms declare one
/// procedure twice): a body edit in one arm is B, the duplicate's rows stay
/// as cold builds them.
fn duplicate_ids_across_if_arms(mode: MemoryMode) {
    let dup = "codeunit 50105 \"Dup\"\n{\n#if A\n    procedure Twice()\n    var\n        C: Codeunit \"Callee\";\n    begin\n        C.Q();\n    end;\n#else\n    procedure Twice()\n    begin\n    end;\n#endif\n\n    procedure Other()\n    begin\n        Twice();\n    end;\n}\n";
    let ws = Ws::new(&[
        ("Callee.Codeunit.al", CALLEE),
        ("Caller.Codeunit.al", CALLER),
        ("Dup.Codeunit.al", dup),
    ]);
    let mut s = WorkspaceSession::with_mode(mode);
    refresh_and_compare(&mut s, &ws, "cold");
    ws.edit(
        "Dup.Codeunit.al",
        "        C.Q();",
        "        C.Q();\n        C.Q();",
    );
    let r = refresh_and_compare(&mut s, &ws, "one arm's body");
    assert_eq!(r.class, ChangeClass::BodyOnly);
    ws.edit(
        "Callee.Codeunit.al",
        "    procedure Q()",
        "\n    procedure Q()",
    );
    let r = refresh_and_compare(&mut s, &ws, "callee moves under the duplicate");
    assert_eq!(r.class, ChangeClass::BodyOnly);
}

const HOLDER: &str = "interface \"H Evaluator\"\n{\n    procedure Evaluate(): Text;\n}\ncodeunit 50311 \"H Real\" implements \"H Evaluator\"\n{\n    procedure Evaluate(): Text\n    begin\n        exit('x');\n    end;\n}\ncodeunit 50312 \"H Other\" implements \"H Evaluator\"\n{\n    procedure Evaluate(): Text\n    begin\n        exit('w');\n    end;\n}\ncodeunit 50313 \"H Holder\"\n{\n    Access = Internal;\n\n    var\n        Evaluator: Interface \"H Evaluator\";\n\n    internal procedure SetEvaluator(NewEvaluator: Interface \"H Evaluator\")\n    begin\n        Evaluator := NewEvaluator;\n    end;\n\n    internal procedure Lookup(): Text\n    begin\n        exit(Evaluator.Evaluate());\n    end;\n}\n";
const HOLDER_CALLER: &str = "codeunit 50314 \"H Caller\"\n{\n    procedure Go(): Text\n    var\n        Real: Codeunit \"H Real\";\n        Other: Codeunit \"H Other\";\n        Any: Interface \"H Evaluator\";\n        Holder: Codeunit \"H Holder\";\n    begin\n        Holder.SetEvaluator(Real);\n        exit(Holder.Lookup());\n    end;\n}\n";

/// T-refine: the holder's interface site is closed by the whole-program
/// refinement over its setter's callers. The CALLER's body moves the setter's
/// argument X -> Y -> an interface value (unprovable) -> back to X; the
/// holder's file is unchanged, its pre-refinement resolution reused, and the
/// refinement rerun every revision.
fn setter_argument_holder_refinement(mode: MemoryMode) {
    let ws = Ws::new(&[
        ("Holder.Codeunit.al", HOLDER),
        ("HolderCaller.Codeunit.al", HOLDER_CALLER),
    ]);
    let mut s = WorkspaceSession::with_mode(mode);
    refresh_and_compare(&mut s, &ws, "cold (X)");
    let site = |s: &mut WorkspaceSession| {
        let p = s.refresh_program(&ws.inputs()).unwrap();
        p.body()
            .site_facts
            .values()
            .find_map(|f| f.interface_holder.clone())
    };
    let closed_x = site(&mut s);
    assert!(
        closed_x.is_some(),
        "precondition: the holder site is closed"
    );
    for (arg, what) in [("Other", "Y"), ("Any", "unprovable"), ("Real", "back to X")] {
        let old = ws.read("HolderCaller.Codeunit.al");
        let line = old
            .lines()
            .find(|l| l.contains("Holder.SetEvaluator("))
            .unwrap()
            .to_string();
        ws.edit(
            "HolderCaller.Codeunit.al",
            &line,
            &format!("        Holder.SetEvaluator({arg});"),
        );
        let r = refresh_and_compare(&mut s, &ws, what);
        assert_eq!(r.class, ChangeClass::BodyOnly, "{what}");
        assert_eq!(r.resolved, 1, "{what}: only the caller is resolved");
    }
    assert_eq!(
        site(&mut s),
        closed_x,
        "back to X closes the site as before"
    );
}

const TEMP_PUB: &str = "table 50890 \"TP T\"\n{\n    fields { field(1; \"No.\"; Code[20]) { } }\n    keys { key(PK; \"No.\") { } }\n}\n\ncodeunit 50890 \"TP Pub\"\n{\n    [IntegrationEvent(false, false)]\n    local procedure OnX(var Rec: Record \"TP T\")\n    begin\n    end;\n\n    procedure Raise()\n    var\n        Rec: Record \"TP T\" temporary;\n    begin\n        OnX(Rec);\n    end;\n}\n";
const TEMP_SUB: &str = "codeunit 50892 \"TP Sub\"\n{\n    [EventSubscriber(ObjectType::Codeunit, Codeunit::\"TP Pub\", 'OnX', '', false, false)]\n    local procedure Handle(var Rec: Record \"TP T\")\n    begin\n        Rec.Insert();\n    end;\n}\n";

/// The temp proof of an UNCHANGED subscriber is revoked when the raiser's body
/// passes a physical record (B), and restored when it passes a temporary one
/// again: the final passes rerun over the whole model every revision.
fn temp_proof_revocation_in_an_unchanged_subscriber(mode: MemoryMode) {
    let ws = Ws::new(&[("Pub.Codeunit.al", TEMP_PUB), ("Sub.Codeunit.al", TEMP_SUB)]);
    let mut s = WorkspaceSession::with_mode(mode);
    refresh_and_compare(&mut s, &ws, "cold (proven)");
    let proven = |s: &mut WorkspaceSession| {
        let inputs = ws.inputs();
        let m = crate::program::model::program_calls::cross_app_program_from(
            s.refresh_program(&inputs),
            &inputs,
            false,
            AssemblyNeeds::BASE,
        )
        .unwrap();
        let h = m
            .resolved
            .workspace
            .routines
            .iter()
            .find(|r| r.name == "Handle")
            .unwrap()
            .clone();
        h.record_variables
            .iter()
            .filter(|rv| rv.is_parameter)
            .all(|rv| crate::program::body::features::temp_state_suppresses(Some(&rv.temp_state)))
    };
    assert!(proven(&mut s), "precondition: the proof holds");
    ws.edit(
        "Pub.Codeunit.al",
        "Record \"TP T\" temporary;",
        "Record \"TP T\";",
    );
    let r = refresh_and_compare(&mut s, &ws, "revoked");
    assert_eq!(r.class, ChangeClass::BodyOnly);
    assert!(!proven(&mut s), "a physical raise revokes the proof");
    ws.edit(
        "Pub.Codeunit.al",
        "        Rec: Record \"TP T\";",
        "        Rec: Record \"TP T\" temporary;",
    );
    refresh_and_compare(&mut s, &ws, "restored");
    assert!(proven(&mut s));
}

/// T-methodid: a `[MethodId(n)]` change in the callee's file is S, and changes
/// the cold output of an UNCHANGED caller that calls by method id
/// (`__alsem_mid_<n>`): were it classified B, the caller's reused resolution
/// would be stale.
fn method_id_change_through_a_synthesized_dispatch(mode: MemoryMode) {
    let lib = "codeunit 50120 \"Mid Lib\"\n{\n    [MethodId(42)]\n    procedure P()\n    begin\n    end;\n}\n";
    let caller = "codeunit 50121 \"Mid Caller\"\n{\n    procedure Go()\n    var\n        L: Codeunit \"Mid Lib\";\n    begin\n        L.\"__alsem_mid_42\"();\n    end;\n}\n";
    let ws = Ws::new(&[
        ("MidLib.Codeunit.al", lib),
        ("MidCaller.Codeunit.al", caller),
    ]);
    let mut s = WorkspaceSession::with_mode(mode);
    refresh_and_compare(&mut s, &ws, "cold");
    let resolved_to_p = |ws: &Ws| {
        let p = ResolvedProgram::build(&ws.inputs(), ProgramScope::CrossApp).unwrap();
        p.body().all_edges().any(|e| {
            e.edge.from.name_lc.as_str() == "go"
                && e.edge.routes.iter().any(|r| {
                    matches!(&r.target, crate::program::resolve::edge::RouteTarget::Routine(t) if t.name_lc.as_str() == "p")
                })
        })
    };
    assert!(
        resolved_to_p(&ws),
        "precondition: the synthesized call binds by id 42"
    );
    ws.edit("MidLib.Codeunit.al", "[MethodId(42)]", "[MethodId(43)]");
    assert!(!resolved_to_p(&ws), "precondition: the cold output changes");
    let r = refresh_and_compare(&mut s, &ws, "method id 42 -> 43");
    assert_eq!(r.class, ChangeClass::Declaration);
    ws.edit(
        "MidLib.Codeunit.al",
        "[MethodId(43)]",
        "[TryFunction]\n    [MethodId(42)]",
    );
    let r = refresh_and_compare(&mut s, &ws, "back to 42, now a TryFunction");
    assert_eq!(r.class, ChangeClass::Declaration);
}

const ENUM_I: &str = "interface \"EI Shape\"\n{\n    procedure Area(): Integer;\n}\ncodeunit 50131 \"EI Square\" implements \"EI Shape\"\n{\n    procedure Area(): Integer\n    begin\n        exit(4);\n    end;\n}\ncodeunit 50132 \"EI Circle\" implements \"EI Shape\"\n{\n    procedure Area(): Integer\n    begin\n        exit(3);\n    end;\n}\n";
const ENUM_E: &str = "enum 50130 \"EI Kind\" implements \"EI Shape\"\n{\n    Extensible = false;\n    value(0; Square) { Implementation = \"EI Shape\" = \"EI Square\"; }\n    value(1; Circle) { Implementation = \"EI Shape\" = \"EI Circle\"; }\n}\n";
const ENUM_USE: &str = "codeunit 50133 \"EI Use\"\n{\n    procedure Go(K: Enum \"EI Kind\"): Integer\n    var\n        S: Interface \"EI Shape\";\n    begin\n        S := K;\n        exit(S.Area());\n    end;\n}\n";

/// An enum's extensibility and implementation map (S) close or open an
/// unchanged caller's interface dispatch.
fn enum_extensibility_and_impls(mode: MemoryMode) {
    let ws = Ws::new(&[
        ("Shapes.al", ENUM_I),
        ("Kind.Enum.al", ENUM_E),
        ("Use.Codeunit.al", ENUM_USE),
    ]);
    let mut s = WorkspaceSession::with_mode(mode);
    refresh_and_compare(&mut s, &ws, "cold (closed)");
    ws.edit("Kind.Enum.al", "Extensible = false;", "Extensible = true;");
    let r = refresh_and_compare(&mut s, &ws, "extensible");
    assert_eq!(r.class, ChangeClass::Declaration);
    ws.edit("Kind.Enum.al", "Extensible = true;", "Extensible = false;");
    ws.edit("Kind.Enum.al", "= \"EI Circle\"", "= \"EI Square\"");
    let r = refresh_and_compare(&mut s, &ws, "remapped");
    assert_eq!(r.class, ChangeClass::Declaration);
}

/// P: the dependency package changes (a new procedure); the session rebuilds.
/// A `roots.config.json` change alone is B with nothing re-resolved, and the
/// model reads the new config.
fn package_change_rebuilds_and_roots_config_is_b(mode: MemoryMode) {
    let ws = base_ws();
    let mut s = WorkspaceSession::with_mode(mode);
    refresh_and_compare(&mut s, &ws, "cold");
    // The stamp is taken from mtime and length: a longer package.
    let lib2 = DEP_LIB.replace(
        "    local procedure C()",
        "    procedure D()\n    begin\n    end;\n\n    local procedure C()",
    );
    ws.write_dep(&[("src/DepLib.Codeunit.al", &lib2)]);
    ws.edit("Caller.Codeunit.al", "        D.A();", "        D.D();");
    let r = refresh_and_compare(&mut s, &ws, "package change");
    assert_eq!(r.class, ChangeClass::Package);
    assert!(r.dependency_bodies_resolved);
    std::fs::write(
        ws.root.join("roots.config.json"),
        r#"{"roots":[{"routine":"Caller.Go","kind":"api"}]}"#,
    )
    .unwrap();
    let r = refresh_and_compare(&mut s, &ws, "roots config");
    assert_eq!(r.class, ChangeClass::BodyOnly);
    assert_eq!((r.parsed, r.resolved), (0, 0));
}

/// The root `app.json` is a P input: any change rebuilds (A-S is taken
/// conservatively as P in this prefix).
fn app_json_change_is_p(mode: MemoryMode) {
    let ws = base_ws();
    let mut s = WorkspaceSession::with_mode(mode);
    refresh_and_compare(&mut s, &ws, "cold");
    let app = std::fs::read_to_string(ws.root.join("app.json")).unwrap();
    std::fs::write(
        ws.root.join("app.json"),
        app.replace(
            "\"version\":\"1.0.0.0\",\"dependencies\"",
            "\"version\":\"1.0.0.1\",\"dependencies\"",
        ),
    )
    .unwrap();
    let r = refresh_and_compare(&mut s, &ws, "app.json");
    assert_eq!(r.class, ChangeClass::Package);
}

/// An `app.json` change a per-file resolution reads (G28: an `Access =
/// Internal` interface is closed only while every `internalsVisibleTo` friend
/// is loaded): naming an unloaded friend reopens an UNCHANGED caller's
/// dispatch. Taken as P, the caller is resolved again; were it B, its reused
/// resolution would keep the closed site.
fn app_json_friend_reopens_an_internal_interface(mode: MemoryMode) {
    let i = "interface \"G28 I\"\n{\n    Access = Internal;\n    procedure M();\n}\ncodeunit 50140 \"G28 Impl\" implements \"G28 I\"\n{\n    procedure M()\n    begin\n    end;\n}\n";
    let use_ = "codeunit 50141 \"G28 Use\"\n{\n    procedure Go(X: Interface \"G28 I\")\n    begin\n        X.M();\n    end;\n}\n";
    let ws = Ws::new(&[("G28.al", i), ("Use.Codeunit.al", use_)]);
    let closed = |ws: &Ws| {
        let p = ResolvedProgram::build(&ws.inputs(), ProgramScope::CrossApp).unwrap();
        p.body()
            .site_facts
            .values()
            .any(|f| !f.interface_closed_over.is_empty())
    };
    assert!(
        closed(&ws),
        "precondition: the internal interface is closed"
    );
    let mut s = WorkspaceSession::with_mode(mode);
    refresh_and_compare(&mut s, &ws, "cold (closed)");
    let app = std::fs::read_to_string(ws.root.join("app.json")).unwrap();
    std::fs::write(
        ws.root.join("app.json"),
        app.replace(
            "\"dependencies\"",
            "\"internalsVisibleTo\":[{\"id\":\"5e550000-0000-4000-8000-0000000000ff\",\"name\":\"Friend\",\"publisher\":\"probe\"}],\"dependencies\"",
        ),
    )
    .unwrap();
    assert!(!closed(&ws), "precondition: an unloaded friend reopens it");
    let r = refresh_and_compare(&mut s, &ws, "friend named");
    assert_eq!(r.class, ChangeClass::Package);
}

/// T-loc's guard on the re-materialization itself (§6.5, round-6 NEW 2): it
/// rewrites ONLY a `SourceSpan` witness of a routine route, from the current
/// surface; a symbol-only callee's `AbiSymbol` and a source-tier miss's `None`
/// stay as they are; a `SourceSpan` whose routine the surface no longer has is
/// a failure, never a manufactured zero-span witness. Hand-stated edges.
#[test]
fn rematerialize_touches_only_source_witnesses() {
    use crate::program::resolve::edge::{Route, RouteTarget, Witness};
    use crate::program::resolve::full::rematerialize_witnesses;
    let ws = base_ws();
    let p = ResolvedProgram::build(&ws.inputs(), ProgramScope::CrossApp).unwrap();
    let surface = p.body().ctx.decl_surface();
    let mut edges: Vec<_> = p.body().all_edges().cloned().collect();
    let with_source = edges
        .iter()
        .position(|e| {
            e.edge.routes.iter().any(|r| {
                matches!(
                    (&r.target, &r.witness),
                    (RouteTarget::Routine(_), Witness::SourceSpan { .. })
                )
            })
        })
        .expect("precondition: a source witness");
    // Every witness stale: span zeroed, file renamed.
    let mut stale = edges.clone();
    for e in &mut stale {
        for r in &mut e.edge.routes {
            if let Witness::SourceSpan { file, span } = &mut r.witness {
                *file = "stale.al".to_string();
                *span = (0, 0);
            }
        }
    }
    assert!(rematerialize_witnesses(&mut stale, &surface));
    let text = |v: &[crate::program::resolve::full::ClassifiedEdge]| format!("{v:?}");
    assert_eq!(text(&stale), text(&edges), "re-materialized = cold");

    // A route whose routine the surface lacks: a failure, the witness never
    // becomes a zero-span source witness of something else.
    let route: &mut Route = edges[with_source]
        .edge
        .routes
        .iter_mut()
        .find(|r| matches!(r.witness, Witness::SourceSpan { .. }))
        .unwrap();
    if let RouteTarget::Routine(rid) = &mut route.target {
        rid.name_lc = "no such routine".into();
    }
    assert!(!rematerialize_witnesses(&mut edges, &surface));

    // A symbol-only callee (`AbiSymbol`) and a source-tier miss (`None`) on a
    // routine route are untouched, even for a routine the surface HAS (a
    // re-materialization keyed on the target alone would rewrite them into
    // source evidence). Hand-stated by assignment on a real edge.
    let mut others = vec![p.body().all_edges().nth(with_source).unwrap().clone()];
    let base = others[0]
        .edge
        .routes
        .iter()
        .find(|r| matches!(r.witness, Witness::SourceSpan { .. }))
        .unwrap()
        .clone();
    let RouteTarget::Routine(rid) = &base.target else {
        unreachable!()
    };
    assert!(
        surface.get(rid).is_some(),
        "precondition: the surface knows the routine"
    );
    let abi = Route {
        witness: Witness::AbiSymbol {
            key: crate::program::resolve::edge::AbiRoutineKey {
                app: rid.object.app,
                object_type: "Codeunit".to_string(),
                object_number: 1,
                object_name_lc: "x".to_string(),
                routine_name_lc: rid.name_lc.to_string(),
                params_count: rid.params_count,
                param_type_fp: 0,
                routine_kind: crate::program::resolve::edge::AbiRoutineKind::Procedure,
                event_kind: crate::program::resolve::edge::AbiEventKind::None,
            },
        },
        ..base.clone()
    };
    let none = Route {
        witness: Witness::None,
        ..base
    };
    others[0].edge.routes = vec![abi, none];
    let before = text(&others);
    assert!(rematerialize_witnesses(&mut others, &surface));
    assert_eq!(text(&others), before);
}

const SYM_GUID: &str = "5e550000-0000-4000-8000-0000000000ab";
const SYM_SYMBOLS: &str = r#"{"Codeunits":[{"Id":71000,"Name":"Sym Lib","Methods":[{"Name":"S","Id":1},{"Name":"T","Id":2,"Parameters":[{"Name":"N","TypeDefinition":{"Name":"Integer"}}]}]}],"Tables":[{"Id":71001,"Name":"Sym T","Fields":[{"Id":1,"Name":"No.","TypeDefinition":{"Name":"Code","Subtype":{"Name":"20"}}}]}]}"#;
const SYM_CALLER: &str = "codeunit 50150 \"Sym Caller\"\n{\n    procedure Go()\n    var\n        L: Codeunit \"Sym Lib\";\n        R: Record \"Sym T\";\n    begin\n        L.S();\n        R.FindFirst();\n    end;\n}\n";

/// `base_ws` plus a SYMBOL-ONLY dependency (a package with a
/// `SymbolReference.json` and no source), called by a workspace file, so the
/// model holds an ABI block.
fn ws_with_symbol_only_dependency() -> Ws {
    use crate::engine::deps::app_package_zip::test_apps;
    let ws = base_ws();
    ws.write("SymCaller.Codeunit.al", SYM_CALLER);
    std::fs::write(
        ws.root.join("app.json"),
        format!(
            r#"{{"id":"{WS_GUID}","name":"Session Ws","publisher":"probe","version":"1.0.0.0","dependencies":[{{"id":"{DEP_GUID}","name":"Dep App","publisher":"Microsoft","version":"1.0.0.0"}},{{"id":"{SYM_GUID}","name":"Sym App","publisher":"Microsoft","version":"1.0.0.0"}}]}}"#
        ),
    )
    .unwrap();
    let manifest = test_apps::manifest_xml(SYM_GUID, "Sym App");
    std::fs::write(
        ws.root
            .join(".alpackages")
            .join("Microsoft_Sym App_1.0.0.0.app"),
        test_apps::build_app(&[
            ("NavxManifest.xml", manifest.as_bytes()),
            ("SymbolReference.json", SYM_SYMBOLS.as_bytes()),
        ]),
    )
    .unwrap();
    ws
}

/// The ABI rows of the symbol-only dependency in the model (`dep:` units are
/// the source-bearing dependency's; the workspace's are `ws:`).
fn abi_routine_rows(x: &CrossAppProgram) -> usize {
    x.resolved
        .workspace
        .routines
        .iter()
        .filter(|r| r.app_guid.eq_ignore_ascii_case(SYM_GUID))
        .count()
}

/// T-rows (§6.8 L3/L4): the ABI block of a symbol-only dependency is reused
/// across B and S, and reprojected when the assembly needs change (a
/// symbol-only row's `rule_drafts` / `card_text` follow the needs). Every
/// step equals cold.
fn abi_block_is_reused_while_its_inputs_hold(mode: MemoryMode) {
    let ws = ws_with_symbol_only_dependency();
    let mut s = WorkspaceSession::with_mode(mode);
    let r = refresh_and_compare(&mut s, &ws, "cold");
    assert!(!rows(&r).abi_reused, "the first assembly builds the block");
    let inputs = ws.inputs();
    let m = assemble_and_resolve_cross_app_program(&inputs, false, AssemblyNeeds::BASE).unwrap();
    assert!(
        abi_routine_rows(&m) > 0,
        "precondition: the model holds the symbol-only dependency's rows"
    );
    ws.edit(
        "SymCaller.Codeunit.al",
        "        L.S();",
        "        L.S();\n        L.S();",
    );
    let r = refresh_and_compare(&mut s, &ws, "body edit");
    assert_eq!(r.class, ChangeClass::BodyOnly);
    assert!(rows(&r).abi_reused);
    ws.edit(
        "SymCaller.Codeunit.al",
        "    procedure Go()",
        "    procedure Go2()",
    );
    let r = refresh_and_compare(&mut s, &ws, "declaration edit");
    assert_eq!(r.class, ChangeClass::Declaration);
    assert!(rows(&r).abi_reused);
    // The needs change: BASE, then EXPLAIN, then BASE again, each through the
    // session's cache and compared with cold.
    for (needs, reused) in [
        (AssemblyNeeds::BASE, false),
        (AssemblyNeeds::EXPLAIN, false),
        (AssemblyNeeds::EXPLAIN, true),
        (AssemblyNeeds::BASE, false),
    ] {
        let inputs = ws.inputs();
        let p = s.refresh_program(&inputs);
        compare_model(&mut s, &inputs, p, needs, &format!("{needs:?}"));
        let rs = rows(s.last_report().unwrap());
        assert_eq!(rs.abi_reused, reused, "{needs:?}");
        if reused {
            assert_eq!(rs.workspace_projected, 0, "{needs:?}");
        } else {
            assert_eq!(rs.workspace_reused, 0, "{needs:?}: every file reprojected");
        }
    }
}

/// T-rows: the ABI block's other input, the package's `SymbolReference`, is
/// not in its key; a changed symbol-only package is P, and the session drops
/// its kept rows with the rest of its retained state, so the block is built
/// again from the new package.
fn symbol_only_package_change_rebuilds_the_abi_block(mode: MemoryMode) {
    use crate::engine::deps::app_package_zip::test_apps;
    let ws = ws_with_symbol_only_dependency();
    let mut s = WorkspaceSession::with_mode(mode);
    refresh_and_compare(&mut s, &ws, "cold");
    let more = SYM_SYMBOLS.replace(
        r#"{"Name":"S","Id":1}"#,
        r#"{"Name":"S","Id":1},{"Name":"S2","Id":3}"#,
    );
    assert_ne!(more, SYM_SYMBOLS, "precondition: the package changes");
    let manifest = test_apps::manifest_xml(SYM_GUID, "Sym App");
    std::fs::write(
        ws.root
            .join(".alpackages")
            .join("Microsoft_Sym App_1.0.0.0.app"),
        test_apps::build_app(&[
            ("NavxManifest.xml", manifest.as_bytes()),
            ("SymbolReference.json", more.as_bytes()),
        ]),
    )
    .unwrap();
    let r = refresh_and_compare(&mut s, &ws, "symbol-only package change");
    assert_eq!(r.class, ChangeClass::Package);
    assert!(!rows(&r).abi_reused);
}

/// T-rows: a file's PARENS-LESS calls are an input of its rows that its own
/// text does not decide. `L.V` (no parentheses) in an unchanged file is a call
/// while `Val` declares a procedure `V`; renaming it (S) leaves the file's
/// text and parse as they were but drops the read from the program's
/// parens-less calls, so the file's kept rows must not be reused.
fn parenless_calls_of_an_unchanged_file_change_under_s(mode: MemoryMode) {
    let val = "codeunit 50109 \"Val\"\n{\n    procedure V(): Integer\n    begin\n        exit(1);\n    end;\n}\n";
    let reader = "codeunit 50107 \"Reader\"\n{\n    procedure Go()\n    var\n        L: Codeunit \"Val\";\n        N: Integer;\n    begin\n        N := L.V;\n    end;\n}\n";
    let ws = Ws::new(&[("Val.Codeunit.al", val), ("Reader.Codeunit.al", reader)]);
    let parenless = |ws: &Ws| {
        let p = ResolvedProgram::build(&ws.inputs(), ProgramScope::CrossApp).unwrap();
        p.body()
            .parenless_calls
            .iter()
            .filter(|((_, path), _)| path == "src/Reader.Codeunit.al")
            .map(|(_, ids)| ids.len())
            .sum::<usize>()
    };
    assert_eq!(
        parenless(&ws),
        1,
        "precondition: `L.V` is a parens-less call"
    );
    let mut s = WorkspaceSession::with_mode(mode);
    refresh_and_compare(&mut s, &ws, "cold");
    ws.edit("Val.Codeunit.al", "    procedure V()", "    procedure V2()");
    assert_eq!(parenless(&ws), 0, "precondition: the rename drops the call");
    let r = refresh_and_compare(&mut s, &ws, "callee renamed");
    assert_eq!(r.class, ChangeClass::Declaration);
    assert!(
        !r.changed.contains(&"src/Reader.Codeunit.al".to_string()),
        "precondition: the reader's text is unchanged"
    );
    assert_eq!(rows(&r).workspace_projected, 2, "callee and reader");
    ws.edit("Val.Codeunit.al", "    procedure V2()", "    procedure V()");
    let r = refresh_and_compare(&mut s, &ws, "callee restored");
    assert_eq!(rows(&r).workspace_projected, 2);
}

/// T-rows: a file added before the others in the file table shifts every
/// later file's `FileIx` (each routine row's `file`, half of its sites'
/// keys). Their kept rows are reused re-stamped with the current index (the
/// one thing `project_ir` writes the index into), never with the old one, and
/// the dependency files' rows (numbered after the workspace's) likewise.
/// Equal to cold at each step.
fn file_ix_shift_restamps_later_files(mode: MemoryMode) {
    let ws = base_ws();
    let mut s = WorkspaceSession::with_mode(mode);
    refresh_and_compare(&mut s, &ws, "cold");
    let ix = |ws: &Ws, path: &str| {
        let p = ResolvedProgram::build(&ws.inputs(), ProgramScope::CrossApp).unwrap();
        let ctx = &p.body().ctx;
        ctx.file_ix(ctx.primary_app_ref, path)
    };
    let before = ix(&ws, "src/Sub.Codeunit.al");
    ws.write(
        "AAA.Codeunit.al",
        "codeunit 50108 \"AAA\"\n{\n    procedure R()\n    begin\n    end;\n}\n",
    );
    assert_ne!(
        ix(&ws, "src/Sub.Codeunit.al"),
        before,
        "precondition: the added file shifts a later file's index"
    );
    let r = refresh_and_compare(&mut s, &ws, "file added first");
    assert_eq!(r.class, ChangeClass::Declaration);
    let rs = rows(&r);
    assert_eq!(
        (rs.workspace_projected, rs.workspace_reused),
        (1, 4),
        "only the added file is projected; the shifted ones are re-stamped"
    );
    assert_eq!(rs.dependency_projected, 0);
    ws.remove("AAA.Codeunit.al");
    let r = refresh_and_compare(&mut s, &ws, "file removed");
    assert_eq!(rows(&r).workspace_projected, 0);
}

/// The dependency files of [`base_ws`]'s package (`DepI`, `DepCaller`,
/// `DepLib`).
const DEP_FILES: usize = 3;

/// One refresh as `refresh_and_compare`, keeping the session's model alive
/// while the probe is read: how many probed dependency trees exist, and how
/// many are still alive with the revision's model held.
fn refresh_holding_the_model(
    s: &mut WorkspaceSession,
    ws: &Ws,
    what: &str,
) -> (RefreshReport, usize) {
    let r = refresh_and_compare(s, ws, what);
    let inputs = ws.inputs();
    let program = s.refresh_program(&inputs);
    let model = s
        .refresh_model(program, &inputs, false, AssemblyNeeds::RULES)
        .expect("session model");
    let (_, alive) = s.probed_dependency_trees();
    drop(model);
    (r, alive)
}

/// `--low-mem` (context-server design §7): the session holds no dependency
/// tree between revisions, not even while a revision's model is alive, and
/// still answers every case cold-equal. P parses every dependency file (the
/// tier is built from that parse), S parses every one again (every body is
/// re-resolved), B parses none, a demand expansion parses exactly the file
/// whose demanded rows changed, and evicted rows are projected again from a
/// fresh parse. The default mode, probed the same way, holds them (so the
/// probe can see a retained tree).
fn low_mem_retains_no_dependency_tree(mode: MemoryMode) {
    let low = mode == MemoryMode::LowMem;
    let ws = base_ws();
    let mut s = WorkspaceSession::with_mode(mode);
    s.probe_dependency_trees();
    let (r, alive) = refresh_holding_the_model(&mut s, &ws, "cold");
    assert_eq!(r.class, ChangeClass::Package);
    assert_eq!(r.dependency_files_parsed, DEP_FILES);
    let (seen, _) = s.probed_dependency_trees();
    assert!(seen >= DEP_FILES, "precondition: the probe saw the trees");
    if !low {
        assert!(alive >= DEP_FILES, "the default mode keeps the trees");
        return;
    }
    assert_eq!(alive, 0, "P: no dependency tree outlives the build");

    // B: nothing parsed, every dependency row reused.
    ws.edit(
        "Callee.Codeunit.al",
        "        exit(X);",
        "        X += 1;\n        exit(X);",
    );
    let (r, alive) = refresh_holding_the_model(&mut s, &ws, "body edit");
    assert_eq!(r.class, ChangeClass::BodyOnly);
    assert_eq!(r.dependency_files_parsed, 0);
    assert_eq!(rows(&r).dependency_parsed, 0);
    assert_eq!(rows(&r).dependency_projected, 0);
    assert_eq!(alive, 0);

    // B with a demand expansion (W -> D1 -> D2): only `DepLib`'s demanded
    // routines change, so only it is parsed and projected again.
    ws.edit("Caller.Codeunit.al", "        D.A();", "        D.B();");
    let (r, alive) = refresh_holding_the_model(&mut s, &ws, "demand expansion");
    assert_eq!(r.class, ChangeClass::BodyOnly);
    assert_eq!(r.dependency_files_parsed, 0);
    assert_eq!(
        (rows(&r).dependency_parsed, rows(&r).dependency_projected),
        (1, 1)
    );
    assert_eq!(alive, 0);

    // S: every dependency body re-resolved, so every file parsed again; the
    // rows, keyed by text, are reused.
    ws.edit("Callee.Codeunit.al", "P(X: Integer)", "P(X: Decimal)");
    let (r, alive) = refresh_holding_the_model(&mut s, &ws, "signature");
    assert_eq!(r.class, ChangeClass::Declaration);
    assert!(r.dependency_bodies_resolved);
    assert_eq!(r.dependency_files_parsed, DEP_FILES);
    assert_eq!(rows(&r).dependency_parsed, 0, "the build holds the trees");
    assert_eq!(alive, 0);

    // The rows evicted (§7): the next B parses and projects every file.
    assert_eq!(s.evict_dependency_rows(), DEP_FILES);
    let (r, alive) = refresh_holding_the_model(&mut s, &ws, "after eviction");
    assert_eq!(r.class, ChangeClass::BodyOnly);
    assert_eq!(
        (rows(&r).dependency_parsed, rows(&r).dependency_projected),
        (DEP_FILES, DEP_FILES)
    );
    assert_eq!(alive, 0);
}

/// One session refresh, then the cold comparison: the program build's
/// embedded-source extractions of the workspace's packages are counted
/// around the session's own refresh only (the cold builds extract through
/// their own caches).
fn refresh_counting_extractions(
    s: &mut WorkspaceSession,
    ws: &Ws,
    what: &str,
) -> (RefreshReport, usize) {
    use crate::snapshot::provider::extract_log::extractions_under;
    let inputs = ws.inputs();
    let before = extractions_under(&ws.root);
    let program = s.refresh_program(&inputs).expect("session program");
    let extracted = extractions_under(&ws.root) - before;
    let report = s.last_report().cloned().expect("a report");
    let cold = ResolvedProgram::build(&inputs, ProgramScope::CrossApp).expect("cold program");
    let (a, b) = (program_text(&program), program_text(&cold));
    assert!(
        a == b,
        "{what}: program differs from cold: {}",
        first_diff(&a, &b)
    );
    compare_model(s, &inputs, Ok(program), AssemblyNeeds::RULES, what);
    (s.last_report().cloned().unwrap_or(report), extracted)
}

/// The dependency source text is extracted once per package, in both modes:
/// a B or S refresh shares the kept text through the session's `DepCache` at
/// the same `.app` stamp (the default mode used to extract it again on every
/// refresh, about 0.3 s on CDO), and a changed package is P, which extracts
/// the new text. Every step equals a cold build.
fn dependency_text_is_extracted_once_per_package(mode: MemoryMode) {
    let ws = base_ws();
    let mut s = WorkspaceSession::with_mode(mode);
    let (r, n) = refresh_counting_extractions(&mut s, &ws, "cold");
    assert_eq!(r.class, ChangeClass::Package);
    assert_eq!(n, 1, "precondition: the cold refresh extracts the text");

    ws.edit(
        "Callee.Codeunit.al",
        "        exit(X);",
        "        X += 1;\n        exit(X);",
    );
    let (r, n) = refresh_counting_extractions(&mut s, &ws, "body edit");
    assert_eq!(r.class, ChangeClass::BodyOnly);
    assert_eq!(n, 0, "B: the kept text is shared");

    ws.edit("Callee.Codeunit.al", "P(X: Integer)", "P(X: Decimal)");
    let (r, n) = refresh_counting_extractions(&mut s, &ws, "signature");
    assert_eq!(r.class, ChangeClass::Declaration);
    assert_eq!(n, 0, "S: the kept text is shared");

    // A changed package (a longer image: a new stamp) is P: its new text is
    // extracted, and the next B shares that.
    let lib2 = DEP_LIB.replace(
        "    local procedure C()",
        "    procedure D()\n    begin\n    end;\n\n    local procedure C()",
    );
    ws.write_dep(&[("src/DepLib.Codeunit.al", &lib2)]);
    let (r, n) = refresh_counting_extractions(&mut s, &ws, "package change");
    assert_eq!(r.class, ChangeClass::Package);
    assert_eq!(n, 1, "P: the new package's text is extracted");
    ws.edit("Caller.Codeunit.al", "        D.A();", "        D.D();");
    let (r, n) = refresh_counting_extractions(&mut s, &ws, "body edit after P");
    assert_eq!(r.class, ChangeClass::BodyOnly);
    assert_eq!(n, 0, "B after P: the new text is shared");
}

/// Every session test above, in both memory modes: each step's program and
/// model equal a cold build of the same inputs whichever mode the session
/// keeps (`--low-mem` re-parses what the default keeps).
macro_rules! in_both_modes {
    ($($name:ident),* $(,)?) => {
        mod default_mode {
            $(#[test]
            fn $name() {
                super::$name(super::MemoryMode::Default);
            })*
        }
        mod low_mem {
            $(#[test]
            fn $name() {
                super::$name(super::MemoryMode::LowMem);
            })*
        }
    };
}

in_both_modes!(
    first_refresh_is_cold_and_the_texts_are_deterministic,
    body_edit_reuses_every_other_resolution,
    newline_before_a_callee_moves_every_witness,
    declaration_changes_reresolve_everything,
    file_add_remove_rename,
    demand_expansion_and_contraction,
    dependency_raiser_and_subscriber_edits,
    event_flow_edge_of_an_unchanged_publisher_moves_under_b,
    duplicate_ids_across_if_arms,
    setter_argument_holder_refinement,
    temp_proof_revocation_in_an_unchanged_subscriber,
    method_id_change_through_a_synthesized_dispatch,
    enum_extensibility_and_impls,
    package_change_rebuilds_and_roots_config_is_b,
    app_json_change_is_p,
    app_json_friend_reopens_an_internal_interface,
    abi_block_is_reused_while_its_inputs_hold,
    symbol_only_package_change_rebuilds_the_abi_block,
    parenless_calls_of_an_unchanged_file_change_under_s,
    file_ix_shift_restamps_later_files,
    low_mem_retains_no_dependency_tree,
    dependency_text_is_extracted_once_per_package,
);
