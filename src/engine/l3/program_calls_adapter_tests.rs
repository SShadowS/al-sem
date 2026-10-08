//! Tests of `program::model::program_calls`'s adapter against L3's `resolve_calls` (engine-switch S9.4 moved them out of `src/program`; see the `#[path]` there).

use super::*;
use crate::engine::deps::app_package_zip::test_apps;
use crate::engine::l3::call_resolver::resolve_calls;

const DEP_GUID: &str = "dddddddd-b3b3-0000-0000-000000000003";

/// The adapter's output next to L3's own, over one workspace.
struct Adapted {
    calls: ResolvedCalls,
    census: SiteCensus,
    old: ResolvedCalls,
    ws: L3Workspace,
}

impl Adapted {
    fn routine(&self, name: &str) -> &L3Routine {
        let mut it = self.ws.routines.iter().filter(|r| r.name == name);
        let r = it.next().unwrap_or_else(|| panic!("no routine {name}"));
        assert!(it.next().is_none(), "routine name {name} not unique");
        r
    }
    /// The routines named `name` in the object named `object_name`.
    fn routines_in(&self, object_name: &str, name: &str) -> Vec<&L3Routine> {
        let ids: Vec<&str> = self
            .ws
            .objects
            .iter()
            .filter(|o| o.name == object_name)
            .map(|o| o.id.as_str())
            .collect();
        self.ws
            .routines
            .iter()
            .filter(|r| r.name == name && ids.contains(&r.object_id.as_str()))
            .collect()
    }
    fn site(&self, caller: &str, callee_text: &str) -> &PCallSite {
        self.routine(caller)
            .call_sites
            .iter()
            .find(|cs| cs.callee_text == callee_text)
            .unwrap_or_else(|| panic!("no site {callee_text} in {caller}"))
    }
    fn edges(&self, callsite_id: &str) -> Vec<CallEdge> {
        at(&self.calls, callsite_id)
    }
    fn bindings(&self, callsite_id: &str) -> Vec<UpgradedBinding> {
        self.calls.upgraded_bindings[callsite_id].clone()
    }
}

fn at(calls: &ResolvedCalls, callsite_id: &str) -> Vec<CallEdge> {
    calls
        .edges
        .iter()
        .filter(|e| e.callsite_id == callsite_id)
        .cloned()
        .collect()
}

fn b(i: u32, is_var: bool, res: &str) -> UpgradedBinding {
    UpgradedBinding {
        parameter_index: i,
        callee_parameter_is_var: is_var,
        binding_resolution: res.to_string(),
    }
}

/// An expected edge: `CallEdge::base` with the given fields.
fn edge(
    from: &L3Routine,
    cs: &PCallSite,
    to: Option<&L3Routine>,
    kind: DispatchKind,
    res: Resolution,
) -> CallEdge {
    let mut e = CallEdge::base(&from.id, &cs.id, &cs.operation_id);
    e.to = to.map(|t| t.id.clone());
    e.dispatch_kind = kind;
    e.resolution = res;
    e
}

/// Write `files` under a fresh workspace (with a dependency app built from
/// `dep_symbols` when given), build both models, apply `mutate` to the L3
/// model, then run the adapter and L3's own resolver.
fn adapt_with(
    files: &[(&str, &str)],
    dep_symbols: Option<&str>,
    mutate: impl FnOnce(&mut L3Workspace),
) -> Adapted {
    adapt_full(
        files,
        dep_symbols.map(|s| (s, &[][..])),
        true,
        |_| {},
        mutate,
    )
}

/// [`adapt_with`], with the dependency app's embedded `.al` source
/// (a source dependency) next to its symbols, the stage-2 switch given,
/// and `mutate_graph` applied to the program context after resolution
/// (before the adapter).
fn adapt_full(
    files: &[(&str, &str)],
    dep: Option<(&str, &[(&str, &str)])>,
    upgrade_dependency_bindings: bool,
    mutate_graph: impl FnOnce(&mut ProgramContext),
    mutate: impl FnOnce(&mut L3Workspace),
) -> Adapted {
    let dir = tempfile::tempdir().unwrap();
    let deps = if dep.is_some() {
        format!(
            r#","dependencies":[{{"id":"{DEP_GUID}","name":"B3 Dep","publisher":"Microsoft","version":"28.0.0.0"}}]"#
        )
    } else {
        String::new()
    };
    std::fs::write(
        dir.path().join("app.json"),
        format!(
            r#"{{"id":"b3b3b3b3-0000-0000-0000-000000000003","name":"B3 Adapter","publisher":"T","version":"1.0.0.0"{deps}}}"#
        ),
    )
    .unwrap();
    if let Some((symbols, sources)) = dep {
        let manifest = test_apps::manifest_xml(DEP_GUID, "B3 Dep");
        let mut entries: Vec<(&str, &[u8])> = vec![
            ("NavxManifest.xml", manifest.as_bytes()),
            ("SymbolReference.json", symbols.as_bytes()),
        ];
        entries.extend(sources.iter().map(|(p, t)| (*p, t.as_bytes())));
        let app = test_apps::build_app(&entries);
        let pk = dir.path().join(".alpackages");
        std::fs::create_dir_all(&pk).unwrap();
        std::fs::write(pk.join("Microsoft_B3 Dep_28.4.app"), app).unwrap();
    }
    for (rel, text) in files {
        let p = dir.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    let (mut ctx, report, mut l3) = build_models(dir.path()).unwrap();
    mutate_graph(&mut ctx);
    mutate(&mut l3.workspace);
    let (calls, census) =
        resolved_calls_from_program(&report, &ctx, &l3.workspace, upgrade_dependency_bindings);
    let ws = l3.workspace;
    let old = {
        let symbols = crate::program::model::symbol_table::SymbolTable::build(
            &ws.objects,
            &ws.tables,
            &ws.routines,
        );
        resolve_calls(&ws, &symbols, &[], &[])
    };
    Adapted {
        calls,
        census,
        old,
        ws,
    }
}

/// [`adapt_with`], no mutation. Every site must come from the program
/// engine: a row test that L3's fallback answered would prove nothing.
fn adapt(files: &[(&str, &str)], dep_symbols: Option<&str>) -> Adapted {
    let a = adapt_with(files, dep_symbols, |_| {});
    assert_eq!(a.census.adapter_l3_fallback_sites, 0, "{:#?}", a.census);
    a
}

const TABLE: &str = "table 50100 \"T\"\n{\n    fields\n    {\n        field(1; A; Code[20])\n        {\n            trigger OnValidate()\n            begin\n            end;\n        }\n        field(2; B; Code[20])\n        {\n            trigger OnValidate()\n            begin\n            end;\n        }\n    }\n\n    trigger OnInsert()\n    begin\n    end;\n\n    trigger OnModify()\n    begin\n    end;\n}\n";

/// Engine-switch S6.0: a resolved member call's `receiver_type` is the
/// program resolver's receiver text: a declared variable's canonical type,
/// else the resolved object rendered (implicit `Rec` -> the page's source
/// table, which L3 rendered as `Record rec`).
#[test]
fn receiver_type_comes_from_the_program_resolver() {
    let helper = "codeunit 50110 \"My Helper\"\n{\n    procedure Ping()\n    begin\n    end;\n}\n";
    let table = "table 50111 \"Rt Cust\"\n{\n    fields\n    {\n        field(1; A; Code[20]) { }\n    }\n\n    procedure Touch()\n    begin\n    end;\n}\n";
    let page = "page 50112 \"Rt Page\"\n{\n    SourceTable = \"Rt Cust\";\n\n    trigger OnOpenPage()\n    var\n        H: Codeunit \"My Helper\";\n        Tmp: Record \"Rt Cust\" temporary;\n    begin\n        H.Ping();\n        Rec.Touch();\n        Tmp.Touch();\n    end;\n}\n";
    let a = adapt(
        &[
            ("src/h.al", helper),
            ("src/t.al", table),
            ("src/p.al", page),
        ],
        None,
    );
    let recv = |text: &str| {
        let cs = a.site("OnOpenPage", text);
        a.edges(&cs.id)
            .into_iter()
            .map(|e| e.receiver_type)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        recv("H.Ping"),
        vec![Some("Codeunit \"My Helper\"".to_string())]
    );
    assert_eq!(
        recv("Rec.Touch"),
        vec![Some("Record \"Rt Cust\"".to_string())]
    );
    // A declaration's own text wins over the rendered object.
    assert_eq!(
        recv("Tmp.Touch"),
        vec![Some("Record \"Rt Cust\" temporary".to_string())]
    );
}

/// Row "Call, Exact, Routine route in workspace": a bare call is
/// `Direct`, a member call `Method` (with the receiver's declared type);
/// both `Resolved` to the L3 routine, and the record argument's binding
/// is upgraded with the callee's `var`-ness.
#[test]
fn exact_workspace_call() {
    let cu = "codeunit 50101 \"W\"\n{\n    procedure Foo(var R: Record \"T\"; I: Integer)\n    begin\n    end;\n\n    procedure Caller()\n    var\n        R: Record \"T\";\n        Other: Codeunit \"W\";\n    begin\n        Foo(R, 1);\n        Other.Foo(R, 2);\n    end;\n}\n";
    let a = adapt(&[("src/t.al", TABLE), ("src/w.al", cu)], None);
    let (caller, foo) = (a.routine("Caller"), a.routine("Foo"));
    let bare = a.site("Caller", "Foo");
    assert_eq!(
        a.edges(&bare.id),
        vec![edge(
            caller,
            bare,
            Some(foo),
            DispatchKind::Direct,
            Resolution::Resolved
        )]
    );
    assert_eq!(
        a.bindings(&bare.id),
        vec![b(0, true, "resolved"), b(1, false, "non-record-arg")]
    );
    let member = a.site("Caller", "Other.Foo");
    let mut want = edge(
        caller,
        member,
        Some(foo),
        DispatchKind::Method,
        Resolution::Resolved,
    );
    want.receiver_type = Some("Codeunit \"W\"".to_string());
    assert_eq!(a.edges(&member.id), vec![want]);
    assert_eq!(
        a.bindings(&member.id),
        vec![b(0, true, "resolved"), b(1, false, "non-record-arg")]
    );
    assert_eq!(a.census.adapter_program_sites, 2, "{:#?}", a.census);
}

/// Row "Run, Routine route in workspace": `Codeunit.Run` lands on the
/// target's `OnRun` as `CodeunitRun`, `Resolved`.
#[test]
fn run_into_workspace() {
    let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    begin\n        Codeunit.Run(Codeunit::\"Target\");\n    end;\n}\n";
    let target = "codeunit 50102 \"Target\"\n{\n    trigger OnRun()\n    begin\n    end;\n}\n";
    let a = adapt(&[("src/w.al", cu), ("src/r.al", target)], None);
    let cs = a.site("Caller", "Codeunit.Run");
    assert_eq!(
        a.edges(&cs.id),
        vec![edge(
            a.routine("Caller"),
            cs,
            Some(a.routine("OnRun")),
            DispatchKind::CodeunitRun,
            Resolution::Resolved
        )]
    );
    assert_eq!(a.bindings(&cs.id), vec![b(0, false, "non-record-arg")]);
}

/// Row "Run", member spelling: L2 reads `Page.RunModal(Page::"P")` as a
/// member call, the program engine as a run. The run kind comes from the
/// receiver keyword: `PageRun` to the page's `OnOpenPage`.
#[test]
fn member_spelled_page_run_into_workspace() {
    let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    begin\n        Page.RunModal(Page::\"P\");\n    end;\n}\n";
    let page = "page 50102 \"P\"\n{\n    trigger OnOpenPage()\n    begin\n    end;\n}\n";
    let a = adapt(&[("src/w.al", cu), ("src/p.al", page)], None);
    let cs = a.site("Caller", "Page.RunModal");
    assert!(
        matches!(cs.callee, PCallee::Member { .. }),
        "{:?}",
        cs.callee
    );
    assert_eq!(
        a.edges(&cs.id),
        vec![edge(
            a.routine("Caller"),
            cs,
            Some(a.routine("OnOpenPage")),
            DispatchKind::PageRun,
            Resolution::Resolved
        )]
    );
}

/// Row "any other Unknown": a bare call to no routine at all → to-less
/// `Unknown(_)`; L3 spells an unresolved bare call's kind `Unresolved`.
#[test]
fn other_unknown_bare_call() {
    let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        R: Record \"T\";\n    begin\n        Nothing(R);\n    end;\n}\n";
    let a = adapt(&[("src/t.al", TABLE), ("src/w.al", cu)], None);
    let cs = a.site("Caller", "Nothing");
    assert_eq!(
        a.edges(&cs.id),
        vec![edge(
            a.routine("Caller"),
            cs,
            None,
            DispatchKind::Unresolved,
            Resolution::Unknown(L3Reason::BareUnresolved)
        )]
    );
    assert_eq!(a.bindings(&cs.id), vec![b(0, false, "unresolved-callee")]);
}

/// The dependency app for the interface and dependency-callee rows:
/// interface `IDep` (`Go(var C: Record Customer)`), its dependency
/// implementer `DepImpl`, codeunit 80 `Sales-Post` (`Post(var C)`), and
/// table 18 `Customer` with a table procedure `DepProc()` and an
/// `OnInsert` trigger symbol.
const DEP_SYMBOLS: &str = r#"{"Tables":[{"Id":18,"Name":"Customer","Fields":[{"Id":1,"Name":"No.","TypeDefinition":{"Name":"Code"}}],"Methods":[{"Name":"DepProc","Parameters":[]},{"Name":"OnInsert","Parameters":[]}]}],"Interfaces":[{"Name":"IDep","Methods":[{"Name":"Go","Parameters":[{"Name":"C","IsVar":true,"TypeDefinition":{"Name":"Record","Subtype":{"Name":"Customer","Id":18}}}]}]}],"Codeunits":[{"Id":80,"Name":"Sales-Post","Methods":[{"Name":"Post","Parameters":[{"Name":"C","IsVar":true,"TypeDefinition":{"Name":"Record","Subtype":{"Name":"Customer","Id":18}}}]}]},{"Id":81,"Name":"DepImpl","ImplementedInterfaces":["IDep"],"Methods":[{"Name":"Go","Parameters":[{"Name":"C","IsVar":true,"TypeDefinition":{"Name":"Record","Subtype":{"Name":"Customer","Id":18}}}]}]}]}"#;

/// Row "Polymorphic (interface)", engine-switch S3.2: one `Interface`+`Maybe`
/// edge to the workspace implementer, `dispatch_meta` on it, and one to-less
/// `Interface`+`ExternalTarget` edge naming the dependency implementer
/// `DepImpl` (its route used to be dropped). `total_impls` counts both.
/// Bindings: ambiguous.
#[test]
fn interface_with_workspace_and_dependency_implementers() {
    let ws_impl = "codeunit 50103 \"WsImpl\" implements IDep\n{\n    procedure Go(var C: Record Customer)\n    begin\n    end;\n}\n";
    let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        X: Interface IDep;\n        C: Record Customer;\n    begin\n        X.Go(C);\n    end;\n}\n";
    let a = adapt(
        &[("src/i.al", ws_impl), ("src/w.al", cu)],
        Some(DEP_SYMBOLS),
    );
    let cs = a.site("Caller", "X.Go");
    let mut want = edge(
        a.routine("Caller"),
        cs,
        Some(a.routine("Go")),
        DispatchKind::Interface,
        Resolution::Maybe,
    );
    want.dispatch_meta = Some(DispatchMeta {
        interface_name: "IDep".to_string(),
        total_impls: 2,
        unresolved_impls: Vec::new(),
        enum_implementers: Vec::new(),
    });
    let mut dep = edge(
        a.routine("Caller"),
        cs,
        None,
        DispatchKind::Interface,
        Resolution::ExternalTarget,
    );
    dep.external_type_ref = Some(ExternalTypeRef {
        kind: "Codeunit".to_string(),
        name: "DepImpl".to_string(),
    });
    assert_eq!(a.edges(&cs.id), vec![want, dep]);
    assert_eq!(a.bindings(&cs.id), vec![b(0, false, "ambiguous")]);
    assert_eq!(a.census.adapter_routes_dropped, 0, "{:#?}", a.census);
    assert_eq!(
        a.census.adapter_interface_dependency_impls, 1,
        "{:#?}",
        a.census
    );
    // S3.3: the symbol-only implementer's routine is named, bodyless.
    let targets: Vec<_> = a
        .calls
        .external_targets
        .iter()
        .filter(|t| t.callsite_id == cs.id)
        .collect();
    assert_eq!(targets.len(), 1, "{targets:?}");
    assert!(
        targets[0].target.ends_with("/Codeunit/81::go/1"),
        "{}",
        targets[0].target
    );
    assert_eq!(
        targets[0].body,
        Some(crate::program::registry::BodyState::Bodyless)
    );
}

/// Minor 5, precondition by assignment: the workspace implementer's L3
/// declaration anchor is moved, so its program route joins no L3
/// routine. Since engine-switch S3.1 the site is one honest
/// `Unknown(NoProgramSite)` edge (it used to fall back to L3) and is counted.
#[test]
fn interface_implementer_outside_l3_is_unknown() {
    let ws_impl = "codeunit 50103 \"WsImpl\" implements IDep\n{\n    procedure Go(var C: Record Customer)\n    begin\n    end;\n}\n";
    let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        X: Interface IDep;\n        C: Record Customer;\n    begin\n        X.Go(C);\n    end;\n}\n";
    let a = adapt_with(
        &[("src/i.al", ws_impl), ("src/w.al", cu)],
        Some(DEP_SYMBOLS),
        |ws| {
            let go = ws.routines.iter_mut().find(|r| r.name == "Go").unwrap();
            go.source_anchor.start_column += 100;
        },
    );
    let cs = a.site("Caller", "X.Go");
    let got = a.edges(&cs.id);
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(
        got[0].resolution,
        Resolution::Unknown(L3Reason::NoProgramSite)
    );
    let c = &a.census;
    assert_eq!(
        (
            c.adapter_callee_outside_l3,
            c.adapter_l3_fallback_sites,
            c.adapter_program_sites
        ),
        (1, 1, 0),
        "{c:#?}"
    );
}

/// Minor 4: a run into a WORKSPACE object with no entry trigger is
/// `Opaque` with no `external_type_ref` (the program route is an ABI
/// symbol in our own app; there is no external type to name).
#[test]
fn run_into_workspace_object_without_entry_trigger() {
    let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    begin\n        Codeunit.Run(Codeunit::\"Empty\");\n    end;\n}\n";
    let empty = "codeunit 50102 \"Empty\"\n{\n}\n";
    let a = adapt(&[("src/w.al", cu), ("src/e.al", empty)], None);
    let cs = a.site("Caller", "Codeunit.Run");
    let want = edge(
        a.routine("Caller"),
        cs,
        None,
        DispatchKind::CodeunitRun,
        Resolution::Opaque,
    );
    assert_eq!(a.edges(&cs.id), vec![want.clone()]);
    assert_eq!(at(&a.old, &cs.id), vec![want], "L3 agrees");
}

/// S9.0d: a page run reaches the page's `OnOpenPage` AND each page
/// extension's; each is its own resolved edge (never an ambiguous
/// candidate set: the run calls them all).
#[test]
fn page_run_reaches_the_base_and_extension_entry_triggers() {
    let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    begin\n        Page.Run(Page::\"P\");\n    end;\n}\n";
    let page = "page 50102 \"P\"\n{\n    trigger OnOpenPage()\n    begin\n    end;\n}\n";
    let ext = "pageextension 50103 \"PExt\" extends \"P\"\n{\n    trigger OnOpenPage()\n    begin\n    end;\n}\n";
    let a = adapt(
        &[("src/w.al", cu), ("src/p.al", page), ("src/e.al", ext)],
        None,
    );
    let cs = a.site("Caller", "Page.Run");
    let edges = a.edges(&cs.id);
    assert!(
        edges.iter().all(|e| e.resolution == Resolution::Resolved),
        "{edges:?}"
    );
    let mut to: Vec<_> = edges.into_iter().filter_map(|e| e.to).collect();
    to.sort();
    to.dedup();
    assert_eq!(to.len(), 2, "both triggers, each its own edge: {to:?}");
    assert_eq!(a.census.adapter_multi_route_sites, 0);
}

/// A run through a page VARIABLE on a workspace page with no entry
/// trigger is the same shape as `Page.Run(Page::X)` on it: `PageRun`,
/// `Opaque`, no external type (the object is ours, not external).
#[test]
fn page_variable_run_into_workspace_object_without_entry_trigger() {
    let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        P: Page \"Empty\";\n    begin\n        P.RunModal();\n    end;\n}\n";
    let empty = "page 50102 \"Empty\"\n{\n}\n";
    let a = adapt(&[("src/w.al", cu), ("src/e.al", empty)], None);
    let cs = a.site("Caller", "P.RunModal");
    let want = edge(
        a.routine("Caller"),
        cs,
        None,
        DispatchKind::PageRun,
        Resolution::Opaque,
    );
    assert_eq!(a.edges(&cs.id), vec![want]);
    assert_eq!(
        a.census.adapter_external_object_receiver, 0,
        "{:#?}",
        a.census
    );
    assert_eq!(
        a.census.adapter_workspace_run_no_entry, 1,
        "{:#?}",
        a.census
    );
}

/// The same arm for a codeunit VARIABLE and a report VARIABLE (review
/// Minor 7): `CuVar.Run()` on a codeunit with no `OnRun` and `RepVar.Run()`
/// on a report with no trigger are `Opaque` runs of their own kind, with no
/// external type, and both are counted.
#[test]
fn codeunit_and_report_variable_run_into_workspace_object_without_entry_trigger() {
    let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        C: Codeunit \"EmptyCu\";\n        R: Report \"EmptyRep\";\n    begin\n        C.Run();\n        R.Run();\n    end;\n}\n";
    let empty_cu = "codeunit 50102 \"EmptyCu\"\n{\n}\n";
    let empty_rep = "report 50103 \"EmptyRep\"\n{\n}\n";
    let a = adapt(
        &[
            ("src/w.al", cu),
            ("src/c.al", empty_cu),
            ("src/r.al", empty_rep),
        ],
        None,
    );
    for (callee, kind) in [
        ("C.Run", DispatchKind::CodeunitRun),
        ("R.Run", DispatchKind::ReportRun),
    ] {
        let cs = a.site("Caller", callee);
        let want = edge(a.routine("Caller"), cs, None, kind, Resolution::Opaque);
        assert_eq!(a.edges(&cs.id), vec![want], "{callee}");
    }
    assert_eq!(
        (
            a.census.adapter_workspace_run_no_entry,
            a.census.adapter_external_object_receiver
        ),
        (2, 0),
        "{:#?}",
        a.census
    );
}

/// Row "AmbiguousOverload" into a dependency (S3.6): both overloads are
/// in a dependency, so the edge is an external call, and both candidates
/// keep their identity in `external_targets`. They used to be dropped.
#[test]
fn ambiguous_overload_in_a_dependency_keeps_its_candidates() {
    let symbols = r#"{"Codeunits":[{"Id":90,"Name":"Over","Methods":[{"Name":"F","Parameters":[{"Name":"A","TypeDefinition":{"Name":"Integer"}}]},{"Name":"F","Parameters":[{"Name":"A","TypeDefinition":{"Name":"Decimal"}}]}]}]}"#;
    let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        O: Codeunit Over;\n        V: Variant;\n    begin\n        O.F(V);\n    end;\n}\n";
    let a = adapt(&[("src/w.al", cu)], Some(symbols));
    let cs = a.site("Caller", "O.F");
    let got = a.edges(&cs.id);
    assert_eq!(got.len(), 1, "{got:#?}");
    assert_eq!(got[0].to, None);
    assert_eq!(got[0].resolution, Resolution::ExternalTarget);
    let mut targets: Vec<&str> = a
        .calls
        .external_targets
        .iter()
        .filter(|t| t.callsite_id == cs.id)
        .map(|t| t.target.as_str())
        .collect();
    targets.sort();
    assert_eq!(targets.len(), 2, "{targets:?}");
    assert!(
        targets.iter().all(|t| t.ends_with("/Codeunit/90::f/1")),
        "{targets:?}"
    );
    assert_eq!(
        a.census.adapter_ambiguous_dependency_candidates, 2,
        "{:#?}",
        a.census
    );
    assert_eq!(a.census.adapter_routes_dropped, 0, "{:#?}", a.census);
}

/// Row "AmbiguousOverload": two same-arity overloads a `Variant`
/// argument cannot pick between → to-less `Ambiguous` with both L3 ids
/// as candidates; the record binding becomes `"ambiguous"`.
#[test]
fn ambiguous_overload() {
    let cu = "codeunit 50101 \"W\"\n{\n    procedure F(var R: Record \"T\"; A: Integer)\n    begin\n    end;\n\n    procedure F(var R: Record \"T\"; A: Decimal)\n    begin\n    end;\n\n    procedure Caller()\n    var\n        R: Record \"T\";\n        V: Variant;\n    begin\n        F(R, V);\n    end;\n}\n";
    let a = adapt(&[("src/t.al", TABLE), ("src/w.al", cu)], None);
    let cs = a.site("Caller", "F");
    let mut ids: Vec<String> = a
        .routines_in("W", "F")
        .iter()
        .map(|r| r.id.clone())
        .collect();
    ids.sort();
    assert_eq!(ids.len(), 2);
    let mut want = edge(
        a.routine("Caller"),
        cs,
        None,
        DispatchKind::Direct,
        Resolution::Ambiguous,
    );
    want.candidates = Some(ids);
    assert_eq!(a.edges(&cs.id), vec![want]);
    assert_eq!(
        a.bindings(&cs.id),
        vec![b(0, false, "ambiguous"), b(1, false, "non-record-arg")]
    );
}

/// Row "DynamicOpen": a run on a runtime target → to-less `Dynamic`.
#[test]
fn dynamic_run_target() {
    let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        Id: Integer;\n    begin\n        Codeunit.Run(Id);\n    end;\n}\n";
    let a = adapt(&[("src/w.al", cu)], None);
    let cs = a.site("Caller", "Codeunit.Run");
    assert_eq!(
        a.edges(&cs.id),
        vec![edge(
            a.routine("Caller"),
            cs,
            None,
            DispatchKind::Dynamic,
            Resolution::Unknown(L3Reason::DynamicObjectRunTarget)
        )]
    );
    assert_eq!(a.bindings(&cs.id), vec![b(0, false, "non-record-arg")]);
}

/// Row "Catalog route (builtin)": to-less `Builtin`.
#[test]
fn builtin() {
    let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    begin\n        Message('x');\n    end;\n}\n";
    let a = adapt(&[("src/w.al", cu)], None);
    let cs = a.site("Caller", "Message");
    assert_eq!(
        a.edges(&cs.id),
        vec![edge(
            a.routine("Caller"),
            cs,
            None,
            DispatchKind::Builtin,
            Resolution::Builtin
        )]
    );
    assert_eq!(a.bindings(&cs.id), vec![b(0, false, "non-record-arg")]);
}

/// Row "Routine route in a dependency": a call into a dependency
/// codeunit and a dependency table procedure → to-less `ExternalTarget`
/// carrying the program object's kind and name. Bindings stay
/// `"unresolved-callee"` (stage 1).
#[test]
fn dependency_callee_is_external_target() {
    let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        SP: Codeunit \"Sales-Post\";\n        C: Record Customer;\n    begin\n        SP.Post(C);\n        C.DepProc();\n        SP.Missing();\n    end;\n}\n";
    let a = adapt(&[("src/w.al", cu)], Some(DEP_SYMBOLS));
    let caller = a.routine("Caller");
    let post = a.site("Caller", "SP.Post");
    let mut want = edge(
        caller,
        post,
        None,
        DispatchKind::Method,
        Resolution::ExternalTarget,
    );
    want.external_type_ref = Some(ExternalTypeRef {
        kind: "Codeunit".to_string(),
        name: "Sales-Post".to_string(),
    });
    assert_eq!(a.edges(&post.id), vec![want]);
    // Stage 2: `Post(var C)` is `AbiParams::Complete` (see
    // `symbol_only_dependency_bindings_by_abi_params`).
    assert_eq!(a.bindings(&post.id), vec![b(0, true, "resolved")]);
    let proc_ = a.site("Caller", "C.DepProc");
    let mut want = edge(
        caller,
        proc_,
        None,
        DispatchKind::Method,
        Resolution::ExternalTarget,
    );
    want.external_type_ref = Some(ExternalTypeRef {
        kind: "Table".to_string(),
        name: "Customer".to_string(),
    });
    assert_eq!(a.edges(&proc_.id), vec![want]);
    // L3 calls the record receiver's case `Unknown(RecordTableProcedure)`:
    // the adapter changes its uncertainty kind, and counts it apart.
    assert_eq!(
        at(&a.old, &proc_.id)[0].resolution,
        Resolution::Unknown(L3Reason::RecordTableProcedure)
    );
    // A member the dependency's ABI lacks: a member decline on a
    // dependency receiver, also a dependency callee.
    let missing = a.site("Caller", "SP.Missing");
    let mut want = edge(
        caller,
        missing,
        None,
        DispatchKind::Method,
        Resolution::ExternalTarget,
    );
    want.external_type_ref = Some(ExternalTypeRef {
        kind: "Codeunit".to_string(),
        name: "Sales-Post".to_string(),
    });
    assert_eq!(a.edges(&missing.id), vec![want]);
    // S3.3: the two exact dependency callees are named (bodyless, symbol-only);
    // the member decline (`SP.Missing`) has no routine to name.
    let named: Vec<(&str, &str, Option<crate::program::registry::BodyState>)> = a
        .calls
        .external_targets
        .iter()
        .map(|t| {
            let site = if t.callsite_id == post.id {
                "post"
            } else if t.callsite_id == proc_.id {
                "proc"
            } else {
                "other"
            };
            (site, t.target.rsplit("::").next().unwrap(), t.body)
        })
        .collect();
    let bodyless = Some(crate::program::registry::BodyState::Bodyless);
    assert_eq!(
        named,
        vec![
            ("post", "post/1", bodyless),
            ("proc", "depproc/0", bodyless)
        ]
    );
    let c = &a.census;
    assert_eq!(c.adapter_program_sites, 3, "{c:#?}");
    assert_eq!(
        (
            c.adapter_external_record_receiver,
            c.adapter_external_object_receiver,
            c.adapter_external_other,
            c.adapter_external_member_decline
        ),
        (1, 2, 0, 1),
        "{c:#?}"
    );
}

/// Stage 2 fixture: dependency table 18 `Customer` and codeunit 82
/// `DepMix` with `Mix(var A: Record Customer; B: Record Customer)` and
/// two same-arity `Ov` overloads a `Variant` cannot pick between.
const DEP_MIX: &str = r#"{"Tables":[{"Id":18,"Name":"Customer","Fields":[{"Id":1,"Name":"No.","TypeDefinition":{"Name":"Code"}}]}],"Codeunits":[{"Id":82,"Name":"DepMix","Methods":[{"Name":"Mix","Parameters":[{"Name":"A","IsVar":true,"TypeDefinition":{"Name":"Record","Subtype":{"Name":"Customer","Id":18}}},{"Name":"B","IsVar":false,"TypeDefinition":{"Name":"Record","Subtype":{"Name":"Customer","Id":18}}}]},{"Name":"Ov","Parameters":[{"Name":"A","IsVar":true,"TypeDefinition":{"Name":"Record","Subtype":{"Name":"Customer","Id":18}}},{"Name":"X","IsVar":false,"TypeDefinition":{"Name":"Integer"}}]},{"Name":"Ov","Parameters":[{"Name":"A","IsVar":true,"TypeDefinition":{"Name":"Record","Subtype":{"Name":"Customer","Id":18}}},{"Name":"X","IsVar":false,"TypeDefinition":{"Name":"Decimal"}}]},{"Name":"Dup","Parameters":[{"Name":"A","IsVar":true,"TypeDefinition":{"Name":"Record","Subtype":{"Name":"Customer","Id":18}}}]},{"Name":"Dup","Parameters":[{"Name":"A","IsVar":true,"TypeDefinition":{"Name":"Record","Subtype":{"Name":"Customer","Id":18}}}]}]}]}"#;

const MIX_CALLER: &str = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        M: Codeunit \"DepMix\";\n        A: Record Customer;\n        B: Record Customer;\n        V: Variant;\n    begin\n        M.Mix(A, B);\n        M.Ov(A, V);\n        M.Dup(A);\n    end;\n}\n";

/// The `M.Mix` site of [`MIX_CALLER`] over [`DEP_MIX`], with the stage-2
/// switch given and `abi_params` of `Mix`'s graph node replaced by
/// `params` when given (precondition by assignment).
fn mix(upgrade: bool, params: Option<AbiParams>) -> (Adapted, Vec<CallEdge>, Vec<UpgradedBinding>) {
    let a = adapt_full(
        &[("src/w.al", MIX_CALLER)],
        Some((DEP_MIX, &[][..])),
        upgrade,
        |ctx| {
            if let Some(p) = params {
                let mut hit = 0;
                for n in ctx.graph.routines.iter_mut() {
                    if n.id.name_lc == "mix" {
                        n.abi_params = p.clone();
                        hit += 1;
                    }
                }
                assert_eq!(hit, 1, "the assignment applied");
            }
        },
        |_| {},
    );
    let cs = a.site("Caller", "M.Mix").clone();
    let (edges, bindings) = (a.edges(&cs.id), a.bindings(&cs.id));
    (a, edges, bindings)
}

/// Stage 2, symbol-only dependency with `AbiParams::Complete`: the
/// bindings are upgraded exactly as for a workspace callee (`"resolved"`,
/// `var`-ness per parameter); the edge is unchanged and to-less.
/// Without the switch they stay `"unresolved-callee"`.
#[test]
fn symbol_only_dependency_bindings_by_abi_params() {
    let (a, edges, bindings) = mix(true, None);
    assert_eq!(
        bindings,
        vec![b(0, true, "resolved"), b(1, false, "resolved")]
    );
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].to, None);
    assert_eq!(edges[0].resolution, Resolution::ExternalTarget);
    assert_eq!(
        (
            a.census.adapter_dep_bindings_source,
            a.census.adapter_dep_bindings_symbol
        ),
        (0, 1),
        "{:#?}",
        a.census
    );
    let (a0, edges0, bindings0) = mix(false, None);
    assert_eq!(
        bindings0,
        vec![
            b(0, false, "unresolved-callee"),
            b(1, false, "unresolved-callee")
        ]
    );
    assert_eq!(edges0, edges, "the switch moves bindings only");
    assert_eq!(a0.census.adapter_dep_bindings_symbol, 0);
}

/// Stage 2: `AbiParams::Missing` and `CollapsedUntrusted` are not
/// trusted — the bindings stay `"unresolved-callee"`, counted apart.
#[test]
fn untrusted_abi_params_leave_bindings_unresolved() {
    let unresolved = vec![
        b(0, false, "unresolved-callee"),
        b(1, false, "unresolved-callee"),
    ];
    let (a, edges, bindings) = mix(true, Some(AbiParams::Missing));
    assert_eq!(bindings, unresolved);
    assert_eq!(edges[0].to, None);
    let c = &a.census;
    assert_eq!(
        (
            c.adapter_dep_bindings_symbol,
            c.adapter_dep_bindings_missing,
            c.adapter_dep_bindings_collapsed
        ),
        (0, 1, 0),
        "{c:#?}"
    );
    let (a, edges, bindings) = mix(true, Some(AbiParams::CollapsedUntrusted));
    assert_eq!(bindings, unresolved);
    assert_eq!(edges[0].to, None);
    let c = &a.census;
    assert_eq!(
        (
            c.adapter_dep_bindings_symbol,
            c.adapter_dep_bindings_missing,
            c.adapter_dep_bindings_collapsed
        ),
        (0, 0, 1),
        "{c:#?}"
    );
}

/// Stage 2 leaves a dependency overload set alone: an ambiguous pick
/// (`Ov` with a `Variant`) and an ABI-collapsed overload (`Dup`, two
/// identical raw entries) have no single callee, so their bindings
/// stay what stage 1 gives, and no dependency callee gets a `to`.
#[test]
fn dependency_overload_sets_keep_stage_one_bindings() {
    let with = adapt_full(
        &[("src/w.al", MIX_CALLER)],
        Some((DEP_MIX, &[][..])),
        true,
        |_| {},
        |_| {},
    );
    let without = adapt_full(
        &[("src/w.al", MIX_CALLER)],
        Some((DEP_MIX, &[][..])),
        false,
        |_| {},
        |_| {},
    );
    for callee in ["M.Ov", "M.Dup"] {
        let cs = with.site("Caller", callee);
        assert_eq!(with.bindings(&cs.id), without.bindings(&cs.id), "{callee}");
        assert_eq!(with.edges(&cs.id), without.edges(&cs.id), "{callee}");
        assert!(
            with.edges(&cs.id).iter().all(|e| e.to.is_none()),
            "{callee}"
        );
        assert!(
            with.bindings(&cs.id)
                .iter()
                .all(|x| x.binding_resolution != "resolved"),
            "{callee}: {:?}",
            with.bindings(&cs.id)
        );
    }
    // Only `M.Mix` reached stage 2: neither overload set is an exact
    // route into one dependency routine.
    let c = &with.census;
    assert_eq!(
        (
            c.adapter_dep_bindings_source,
            c.adapter_dep_bindings_symbol,
            c.adapter_dep_bindings_missing,
            c.adapter_dep_bindings_collapsed
        ),
        (0, 1, 0, 0),
        "{c:#?}"
    );
}

/// Stage 2, source dependency: the callee's `var`-ness comes from its
/// declaration (`DeclSurface`), giving the same binding strings L3 gives
/// a workspace callee. The dependency `.app` embeds its source (L3 does
/// not read dependencies); its callee stays to-less.
#[test]
fn source_dependency_bindings_by_declaration() {
    // The ABI deliberately says `R` is by value; the source says `var R`.
    // The upgrade must follow the declaration.
    let symbols = r#"{"Tables":[{"Id":70001,"Name":"DT","Fields":[{"Id":1,"Name":"A","TypeDefinition":{"Name":"Code"}}]}],"Pages":[{"Id":70002,"Name":"DP"}],"Codeunits":[{"Id":70000,"Name":"SrcDep","Methods":[{"Name":"Go","Parameters":[{"Name":"R","IsVar":false,"TypeDefinition":{"Name":"Record","Subtype":{"Name":"DT","Id":70001}}},{"Name":"S","IsVar":false,"TypeDefinition":{"Name":"Record","Subtype":{"Name":"DT","Id":70001}}}]}]}]}"#;
    let dep = "codeunit 70000 \"SrcDep\"\n{\n    procedure Go(var R: Record \"DT\"; S: Record \"DT\")\n    begin\n    end;\n}\n";
    let table =
        "table 70001 \"DT\"\n{\n    fields\n    {\n        field(1; A; Code[20]) { }\n    }\n}\n";
    let page = "page 70002 \"DP\"\n{\n    trigger OnOpenPage()\n    begin\n    end;\n}\n";
    let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        D: Codeunit \"SrcDep\";\n        R: Record \"DT\";\n        S: Record \"DT\";\n    begin\n        D.Go(R, S);\n        Page.Run(Page::\"DP\", R);\n    end;\n}\n";
    let sources = [("src/d.al", dep), ("src/t.al", table), ("src/p.al", page)];
    let run = |upgrade: bool| {
        let a = adapt_full(
            &[("src/w.al", cu)],
            Some((symbols, &sources[..])),
            upgrade,
            |_| {},
            |_| {},
        );
        let cs = a.site("Caller", "D.Go").clone();
        let (e, bs) = (a.edges(&cs.id), a.bindings(&cs.id));
        (a, e, bs)
    };
    let (a, edges, bindings) = run(true);
    // A run into the dependency page's `OnOpenPage()`: the record
    // argument sits past its (zero) parameters, so nothing changes and
    // the site is not counted as an upgrade.
    let run_site = a.site("Caller", "Page.Run");
    assert_eq!(
        a.edges(&run_site.id)[0].resolution,
        Resolution::Opaque,
        "{:?}",
        a.edges(&run_site.id)
    );
    assert_eq!(
        a.bindings(&run_site.id),
        vec![
            b(0, false, "non-record-arg"),
            b(1, false, "unresolved-callee")
        ]
    );
    assert_eq!(
        bindings,
        vec![b(0, true, "resolved"), b(1, false, "resolved")],
        "{edges:?} {:#?}",
        a.census
    );
    assert_eq!(edges.len(), 1, "{edges:?}");
    assert_eq!(edges[0].to, None);
    assert_eq!(edges[0].resolution, Resolution::ExternalTarget);
    assert_eq!(
        (
            a.census.adapter_dep_bindings_source,
            a.census.adapter_dep_bindings_symbol
        ),
        (1, 0),
        "{:#?}",
        a.census
    );
    let (_, edges0, bindings0) = run(false);
    assert_eq!(
        bindings0,
        vec![
            b(0, false, "unresolved-callee"),
            b(1, false, "unresolved-callee")
        ]
    );
    assert_eq!(edges0, edges);
}

/// Stage 2, two symbol-only shapes: a codeunit with no object number
/// (keyed by NAME, so the ABI key maps back through `ObjKey::Name`) is
/// upgraded from `AbiParams::Complete`; a run into a dependency page
/// with no entry trigger names no routine (a placeholder key), so its
/// record argument stays `"unresolved-callee"`, counted apart.
#[test]
fn name_keyed_symbol_callee_and_no_routine_run() {
    let symbols = r#"{"Tables":[{"Id":18,"Name":"Customer","Fields":[{"Id":1,"Name":"No.","TypeDefinition":{"Name":"Code"}}]}],"Pages":[{"Id":83,"Name":"NoTrig"}],"Codeunits":[{"Name":"NamedDep","Methods":[{"Name":"Nm","Parameters":[{"Name":"A","IsVar":true,"TypeDefinition":{"Name":"Record","Subtype":{"Name":"Customer","Id":18}}}]}]}]}"#;
    let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        N: Codeunit \"NamedDep\";\n        A: Record Customer;\n    begin\n        N.Nm(A);\n        Page.Run(Page::\"NoTrig\", A);\n    end;\n}\n";
    let a = adapt_full(
        &[("src/w.al", cu)],
        Some((symbols, &[][..])),
        true,
        |ctx| {
            let named = ctx.graph.routines.iter().any(|n| {
                n.id.name_lc == "nm" && n.id.object.key == ObjKey::Name("nameddep".to_string())
            });
            assert!(named, "precondition: the codeunit is keyed by name");
        },
        |_| {},
    );
    let nm = a.site("Caller", "N.Nm");
    assert_eq!(a.bindings(&nm.id), vec![b(0, true, "resolved")]);
    assert!(a.edges(&nm.id).iter().all(|e| e.to.is_none()));
    let run = a.site("Caller", "Page.Run");
    assert_eq!(
        a.bindings(&run.id),
        vec![
            b(0, false, "non-record-arg"),
            b(1, false, "unresolved-callee")
        ]
    );
    let c = &a.census;
    assert_eq!(
        (
            c.adapter_dep_bindings_symbol,
            c.adapter_dep_bindings_missing,
            c.adapter_dep_bindings_no_routine
        ),
        (1, 0, 1),
        "{c:#?}"
    );
}

/// Row "ObjectNotInGraph": an object named but absent everywhere → a
/// to-less `ExternalTarget` (a run: `Opaque`), the type ref from the L2
/// receiver type / run target.
#[test]
fn object_not_in_graph_is_external_target() {
    let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        N: Codeunit \"Nowhere\";\n    begin\n        N.Go();\n        Codeunit.Run(Codeunit::\"Nowhere\");\n    end;\n}\n";
    let a = adapt(&[("src/w.al", cu)], None);
    let caller = a.routine("Caller");
    let nowhere = || {
        Some(ExternalTypeRef {
            kind: "Codeunit".to_string(),
            name: "Nowhere".to_string(),
        })
    };
    let go = a.site("Caller", "N.Go");
    let mut want = edge(
        caller,
        go,
        None,
        DispatchKind::Method,
        Resolution::ExternalTarget,
    );
    want.external_type_ref = nowhere();
    assert_eq!(a.edges(&go.id), vec![want]);
    let run = a.site("Caller", "Codeunit.Run");
    let mut want = edge(
        caller,
        run,
        None,
        DispatchKind::CodeunitRun,
        Resolution::Opaque,
    );
    want.external_type_ref = nowhere();
    assert_eq!(a.edges(&run.id), vec![want]);
}

/// Row "ImplicitTrigger, workspace trigger route": `Insert` →
/// `OnInsert` (`Maybe`); `Validate(A)` → field A's `OnValidate` only
/// (`Resolved`); `Modify(false)` → nothing. Keyed by the record op id.
#[test]
fn implicit_trigger_to_workspace() {
    let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        R: Record \"T\";\n    begin\n        R.Insert(true);\n        R.Validate(A, 'x');\n        R.Modify(false);\n    end;\n}\n";
    let a = adapt(&[("src/t.al", TABLE), ("src/w.al", cu)], None);
    let caller = a.routine("Caller");
    let op = |name: &str| {
        caller
            .record_operations
            .iter()
            .find(|o| o.op == name)
            .unwrap()
    };
    let on_validate_a = a
        .routines_in("T", "OnValidate")
        .into_iter()
        .find(|r| r.enclosing_member.as_deref() == Some("A"))
        .unwrap();
    let trig = |o: &L3RecordOperation, to: &L3Routine, res: Resolution| {
        let mut e = CallEdge::base(&caller.id, &o.id, &o.id);
        e.to = Some(to.id.clone());
        e.dispatch_kind = DispatchKind::ImplicitTrigger;
        e.resolution = res;
        e
    };
    assert_eq!(
        a.edges(&op("Insert").id),
        vec![trig(op("Insert"), a.routine("OnInsert"), Resolution::Maybe)]
    );
    assert_eq!(
        a.edges(&op("Validate").id),
        vec![trig(op("Validate"), on_validate_a, Resolution::Resolved)]
    );
    assert_eq!(a.edges(&op("Modify").id), vec![]);
    assert!(!a.calls.upgraded_bindings.contains_key(&op("Insert").id));
    assert_eq!(a.census.adapter_program_trigger_ops, 3, "{:#?}", a.census);
    // Engine-switch S3.4: the program resolver already dropped the two
    // routes (field B's `OnValidate`, `Modify(false)`'s `OnModify`) that the
    // adapter used to filter; its agreement check finds none left.
    assert_eq!(
        a.census.adapter_trigger_routes_filtered, 0,
        "{:#?}",
        a.census
    );
}

/// A trigger edge for the op `op` of routine `caller`.
fn trigger_edge(caller: &L3Routine, op: &L3RecordOperation, to: &L3Routine) -> CallEdge {
    let mut e = CallEdge::base(&caller.id, &op.id, &op.id);
    e.to = Some(to.id.clone());
    e.dispatch_kind = DispatchKind::ImplicitTrigger;
    e.resolution = Resolution::Maybe;
    e
}

/// `Rename` (#9): BOTH engines treat `R.Rename(..)` as a record op -- L2
/// emits an `L3RecordOperation`, and the program extractor (which reads
/// the same `record_op_type` table) classifies a `RecordOp` that
/// `resolve_implicit_trigger` routes to `OnRename`. The two pair up as a
/// matched implicit trigger, the edge is `Resolved` (Rename takes no
/// RunTrigger and always fires OnRename, measured on BC 28), L3's own
/// answer agrees, and nothing counts as "beyond L3". Before #9 neither
/// engine did this, and the site was an ordinary built-in call.
#[test]
fn rename_fires_on_rename_on_both_sides() {
    let table = "table 50100 \"T\"\n{\n    fields\n    {\n        field(1; Code; Code[20]) { }\n    }\n\n    trigger OnRename()\n    begin\n    end;\n}\n";
    let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        R: Record \"T\";\n    begin\n        R.Rename('NEW');\n    end;\n}\n";
    let a = adapt(&[("src/t.al", table), ("src/w.al", cu)], None);
    let caller = a.routine("Caller");
    let op = caller
        .record_operations
        .iter()
        .find(|o| o.op == "Rename")
        .expect("L2: Rename is a record op");
    let mut want = trigger_edge(caller, op, a.routine("OnRename"));
    want.resolution = Resolution::Resolved;
    assert_eq!(a.edges(&op.id), vec![want.clone()]);
    assert_eq!(at(&a.old, &op.id), vec![want], "L3 agrees");
    let c = &a.census;
    assert_eq!(
        (c.implicit_trigger_matched, c.implicit_trigger_unmatched),
        (1, 0),
        "{c:#?}"
    );
}

/// Beyond L3: an `Insert` also fires a TableExtension's `OnInsert`. Both
/// edges are emitted, sorted by `to`.
#[test]
fn table_extension_trigger_is_emitted_beyond_l3() {
    let table = "table 50100 \"T\"\n{\n    fields\n    {\n        field(1; Code; Code[20]) { }\n    }\n\n    trigger OnInsert()\n    begin\n    end;\n}\n";
    let ext = "tableextension 50110 \"TExt\" extends \"T\"\n{\n    trigger OnInsert()\n    begin\n    end;\n}\n";
    let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        R: Record \"T\";\n    begin\n        R.Insert(true);\n    end;\n}\n";
    let a = adapt(
        &[("src/t.al", table), ("src/e.al", ext), ("src/w.al", cu)],
        None,
    );
    let caller = a.routine("Caller");
    let op = &caller.record_operations[0];
    let base = a.routines_in("T", "OnInsert");
    let extension = a.routines_in("TExt", "OnInsert");
    assert_eq!((base.len(), extension.len()), (1, 1));
    let l3 = at(&a.old, &op.id);
    assert_eq!(l3, vec![trigger_edge(caller, op, base[0])], "L3: base only");
    let mut want = vec![
        trigger_edge(caller, op, base[0]),
        trigger_edge(caller, op, extension[0]),
    ];
    want.sort_by(|x, y| x.to.cmp(&y.to));
    assert_eq!(a.edges(&op.id), want);
}

/// Row "ImplicitTrigger to dependency": a record op on a dependency
/// table → no edge, but (S3.6) the trigger it reaches keeps its identity
/// and body state in `external_targets`, under the operation's id.
#[test]
fn implicit_trigger_to_dependency_has_no_edge() {
    let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        C: Record Customer;\n    begin\n        C.Insert(true);\n    end;\n}\n";
    let a = adapt(&[("src/w.al", cu)], Some(DEP_SYMBOLS));
    let op = &a.routine("Caller").record_operations[0];
    assert_eq!(op.op, "Insert");
    assert_eq!(a.edges(&op.id), vec![]);
    assert_eq!(a.census.adapter_program_trigger_ops, 1, "{:#?}", a.census);
    assert_eq!(a.census.adapter_routes_dropped, 0, "{:#?}", a.census);
    assert_eq!(
        a.census.adapter_trigger_dependency_routes, 1,
        "{:#?}",
        a.census
    );
    let targets: Vec<_> = a
        .calls
        .external_targets
        .iter()
        .filter(|t| t.callsite_id == op.id)
        .collect();
    assert_eq!(targets.len(), 1, "{targets:?}");
    assert!(
        targets[0].target.ends_with("::oninsert/0"),
        "{}",
        targets[0].target
    );
    assert_eq!(
        targets[0].body,
        Some(crate::program::registry::BodyState::Bodyless)
    );
}

/// S9.0c: a zero-argument call may drop its `()` inside an expression too
/// (`if IsOn then`). The resolver keeps it as a call; the body walk takes the
/// same read as a call site, so the adapter joins it rather than losing it as
/// a `program_only_site`. The variable read `B` stays a read.
#[test]
fn parenless_call_in_an_expression_reaches_the_model() {
    let cu = "codeunit 50120 \"P\"
{
    procedure IsOn(): Boolean
    begin
        exit(true);
    end;

    procedure Caller()
    var
        B: Boolean;
    begin
        if IsOn then;
        B := IsOn;
        if B then;
    end;
}
";
    let a = adapt(&[("src/p.al", cu)], None);
    let sites: Vec<&str> = a
        .routine("Caller")
        .call_sites
        .iter()
        .map(|cs| cs.callee_text.as_str())
        .collect();
    assert_eq!(sites, vec!["IsOn", "IsOn"], "two parens-less calls, no `B`");
    let is_on = a.routine("IsOn").id.clone();
    for cs in &a.routine("Caller").call_sites {
        let to: Vec<Option<String>> = a.edges(&cs.id).into_iter().map(|e| e.to).collect();
        assert_eq!(to, vec![Some(is_on.clone())], "{}", cs.id);
    }
    assert_eq!(a.census.program_only_site, 0, "{:#?}", a.census);
}

/// S9.0c: a call inside a ternary or an `in` list is a body call site as well
/// as a program edge, so the adapter joins it.
#[test]
fn calls_inside_a_ternary_reach_the_model() {
    let cu = "codeunit 50121 \"Q\"
{
    procedure F(): Boolean
    begin
    end;

    procedure A(): Integer
    begin
    end;

    procedure Caller()
    var
        X: Integer;
    begin
        X := F() ? A() : 0;
        if X in [A()] then;
    end;
}
";
    let a = adapt(&[("src/q.al", cu)], None);
    let mut sites: Vec<&str> = a
        .routine("Caller")
        .call_sites
        .iter()
        .map(|cs| cs.callee_text.as_str())
        .collect();
    sites.sort_unstable();
    assert_eq!(sites, vec!["A", "A", "F"]);
    assert_eq!(a.census.program_only_site, 0, "{:#?}", a.census);
}

/// S3.5 (was ruling 1): a bare implicit-`Rec` record op is a record op to
/// the program engine too, and takes its trigger edge from it, the same
/// edge L3 gives. It passes `true`: an argless write fires no trigger
/// (measured on BC 28), so it would have no edge to compare.
#[test]
fn bare_record_op_takes_the_program_trigger_edge() {
    let table = "table 50100 \"T\"\n{\n    fields\n    {\n        field(1; Code; Code[20]) { }\n    }\n    trigger OnModify()\n    begin\n    end;\n\n    procedure P()\n    begin\n        Modify(true);\n    end;\n}\n";
    let a = adapt(&[("src/t.al", table)], None);
    let op = &a.routine("P").record_operations[0];
    let want = at(&a.old, &op.id);
    assert_eq!(want.len(), 1, "L3 gives the bare op its OnModify edge");
    assert_eq!(
        want[0].to.as_deref(),
        Some(a.routine("OnModify").id.as_str())
    );
    assert_eq!(a.edges(&op.id), want);
    assert_eq!(a.census.adapter_program_trigger_ops, 1, "{:#?}", a.census);
    assert_eq!(a.census.adapter_l3_trigger_ops, 0, "{:#?}", a.census);
}

/// S3.6, precondition by assignment: the resolver makes no call edge with
/// two routes outside an interface or overload set, so one is built by
/// giving `Foo()`'s exact edge a second route to `Bar`. The adapter keeps
/// both as an ambiguous candidate set; it used to keep the first only.
#[test]
fn multi_route_call_keeps_every_route() {
    let cu = "codeunit 50101 \"W\"\n{\n    procedure Foo()\n    begin\n    end;\n\n    procedure Bar()\n    begin\n    end;\n\n    procedure Caller()\n    begin\n        Foo();\n    end;\n}\n";
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("app.json"),
        r#"{"id":"b3b3b3b3-0000-0000-0000-000000000003","name":"B3 Adapter","publisher":"T","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/w.al"), cu).unwrap();
    let (ctx, mut report, l3) = build_models(dir.path()).unwrap();
    let bar = ctx
        .graph
        .routines
        .iter()
        .find(|n| n.id.name_lc == "bar")
        .unwrap()
        .id
        .clone();
    let ce = report
        .edges
        .iter_mut()
        .find(|ce| ce.edge.from.name_lc == "caller" && ce.edge.kind == EdgeKind::Call)
        .unwrap();
    assert_eq!(ce.edge.shape, DispatchShape::Exact, "precondition");
    let mut second = ce.edge.routes[0].clone();
    second.target = RouteTarget::Routine(bar);
    ce.edge.routes.push(second);

    let ws = &l3.workspace;
    let (calls, census) = resolved_calls_from_program(&report, &ctx, ws, true);
    let caller = ws.routines.iter().find(|r| r.name == "Caller").unwrap();
    let id_of = |n: &str| ws.routines.iter().find(|r| r.name == n).unwrap().id.clone();
    let got = at(&calls, &caller.call_sites[0].id);
    assert_eq!(got.len(), 1, "{got:#?}");
    assert_eq!(got[0].resolution, Resolution::Ambiguous);
    let mut want = vec![id_of("Foo"), id_of("Bar")];
    want.sort();
    assert_eq!(got[0].candidates, Some(want));
    assert_eq!(census.adapter_multi_route_sites, 1, "{census:#?}");
}

/// S3.5, precondition by assignment: the op is moved off its span, so it
/// pairs with no program edge. It used to keep L3's own trigger edge; it
/// now gets none.
#[test]
fn unmatched_record_op_gets_no_l3_trigger_edge() {
    let table = "table 50100 \"T\"\n{\n    fields\n    {\n        field(1; Code; Code[20]) { }\n    }\n    trigger OnModify()\n    begin\n    end;\n\n    procedure P()\n    begin\n        Modify();\n    end;\n}\n";
    let a = adapt_with(&[("src/t.al", table)], None, |ws| {
        let r = ws.routines.iter_mut().find(|r| r.name == "P").unwrap();
        r.record_operations[0].source_anchor.start_column += 100;
    });
    let op = &a.routine("P").record_operations[0];
    assert_eq!(at(&a.old, &op.id).len(), 1, "precondition: L3 has an edge");
    assert_eq!(a.edges(&op.id), vec![]);
    assert_eq!(a.census.adapter_l3_trigger_ops, 1, "{:#?}", a.census);
    assert_eq!(a.census.adapter_program_trigger_ops, 0, "{:#?}", a.census);
}

/// Ruling 2 as replaced by engine-switch S3.1, precondition by assignment:
/// the L3 site is moved off its span, so it pairs with no program edge. It
/// used to keep L3's own (resolved) edge; it is now one honest
/// `Unknown(NoProgramSite)` edge.
#[test]
fn unmatched_site_is_unknown_not_l3() {
    let cu = "codeunit 50101 \"W\"\n{\n    procedure Foo()\n    begin\n    end;\n\n    procedure Caller()\n    begin\n        Foo();\n    end;\n}\n";
    let a = adapt_with(&[("src/w.al", cu)], None, |ws| {
        let r = ws.routines.iter_mut().find(|r| r.name == "Caller").unwrap();
        r.call_sites[0].source_anchor.start_column += 100;
    });
    let cs = a.site("Caller", "Foo");
    assert!(
        at(&a.old, &cs.id).iter().any(|e| e.to.is_some()),
        "precondition: L3 resolves it"
    );
    let got = a.edges(&cs.id);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].to, None);
    assert_eq!(
        got[0].resolution,
        Resolution::Unknown(L3Reason::NoProgramSite)
    );
    assert_eq!(a.census.adapter_l3_fallback_sites, 1, "{:#?}", a.census);
    assert_eq!(a.census.adapter_program_sites, 0, "{:#?}", a.census);
}

/// Ruling 5: the adapter emits edges in `resolve_calls`'s order (call
/// sites in routine order, then trigger edges); here every edge is one
/// both engines agree on, so the two lists are identical.
#[test]
fn edge_order_matches_resolve_calls() {
    let cu = "codeunit 50101 \"W\"\n{\n    procedure Foo(var R: Record \"T\"; I: Integer)\n    begin\n        R.Insert(true);\n    end;\n\n    procedure Caller()\n    var\n        R: Record \"T\";\n    begin\n        R.Modify(true);\n        Foo(R, 1);\n        Message('x');\n        Foo(R, 2);\n    end;\n}\n";
    let a = adapt(&[("src/t.al", TABLE), ("src/w.al", cu)], None);
    assert_eq!(a.calls.edges.len(), 5);
    assert_eq!(a.calls.edges, a.old.edges);
    assert_eq!(a.calls.upgraded_bindings, a.old.upgraded_bindings);
}

/// The production path (no notes) gives the same calls as the harness
/// path (notes) on every `tests/r0-corpus` fixture: skipping the notes
/// changes nothing the detectors read.
#[test]
fn production_path_equals_the_notes_path_on_r0_corpus() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus");
    let mut compared = 0;
    for e in std::fs::read_dir(&root).unwrap() {
        let dir = e.unwrap().path();
        if !dir.join("app.json").is_file() {
            continue;
        }
        let Ok((ctx, report, l3)) = build_models(&dir) else {
            continue;
        };
        let ws = &l3.workspace;
        let prod = resolved_calls_from_program(&report, &ctx, ws, true).0;
        let harness = resolved_calls_with_notes(&report, &ctx, ws, true).0;
        assert_eq!(prod.edges, harness.edges, "{}", dir.display());
        assert_eq!(
            prod.upgraded_bindings,
            harness.upgraded_bindings,
            "{}",
            dir.display()
        );
        compared += 1;
    }
    assert!(compared > 100, "only {compared} fixtures compared");
}

/// Parity over every `tests/r0-corpus` fixture: wherever the program
/// engine and L3 resolve a site to the same workspace routine(s), the
/// adapter's edges equal L3's in `to`, `dispatch_kind`, `resolution`
/// and bindings. Prints per-fixture counts (`--nocapture`).
#[test]
fn r0_corpus_parity_where_both_resolve_the_same_routine() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus");
    let mut dirs: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.join("app.json").is_file())
        .collect();
    dirs.sort();
    // Per-site buckets: `agree` (both resolve the same workspace
    // routines, edges and bindings equal — asserted), `differing target`,
    // `L3 only` / `adapter only` (one side has a `to`), and to-less on both
    // sides with the same / a different `(dispatch kind, resolution)`.
    let mut total: std::collections::BTreeMap<&str, usize> = Default::default();
    let mut failures: Vec<String> = Vec::new();
    for dir in &dirs {
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        let Ok((ctx, report, l3)) = build_models(dir) else {
            *total.entry("skipped fixtures").or_default() += 1;
            eprintln!("parity {name}: skipped (model build failed)");
            continue;
        };
        let ws = &l3.workspace;
        // Stage 1: L3 never upgrades a dependency callee's bindings, so
        // the binding parity below holds only without stage 2.
        let (calls, census) = resolved_calls_from_program(&report, &ctx, ws, false);
        *total.entry("adapter: program sites").or_default() += census.adapter_program_sites;
        *total.entry("adapter: L3 fallback sites").or_default() += census.adapter_l3_fallback_sites;
        *total.entry("adapter: program trigger ops").or_default() +=
            census.adapter_program_trigger_ops;
        *total.entry("adapter: L3 trigger ops").or_default() += census.adapter_l3_trigger_ops;
        let symbols = crate::program::model::symbol_table::SymbolTable::build(
            &ws.objects,
            &ws.tables,
            &ws.routines,
        );
        let old = resolve_calls(ws, &symbols, &[], &[]);
        let group = |rc: &ResolvedCalls| {
            let mut m: HashMap<String, Vec<CallEdge>> = HashMap::new();
            for e in &rc.edges {
                m.entry(e.callsite_id.clone()).or_default().push(e.clone());
            }
            m
        };
        let (new_g, old_g) = (group(&calls), group(&old));
        let mut ids: Vec<(String, bool)> = Vec::new();
        for r in &ws.routines {
            ids.extend(r.call_sites.iter().map(|cs| (cs.id.clone(), true)));
            ids.extend(r.record_operations.iter().map(|o| (o.id.clone(), false)));
        }
        // The program edge behind each adapted site, for the printout.
        let j = join(&report, &ctx, ws);
        let mut program_of: HashMap<String, String> = HashMap::new();
        for (ri, r) in ws.routines.iter().enumerate() {
            let named = r
                .call_sites
                .iter()
                .enumerate()
                .map(|(ci, cs)| (cs.id.clone(), j.calls.get(&(ri, ci))))
                .chain(
                    r.record_operations
                        .iter()
                        .enumerate()
                        .map(|(oi, o)| (o.id.clone(), j.ops.get(&(ri, oi)))),
                );
            for (id, ce) in named {
                let text = ce.map_or("no program edge".to_string(), |ce| {
                    let routes: Vec<_> = ce
                        .edge
                        .routes
                        .iter()
                        .map(|r| (&r.evidence, r.receiver_tier))
                        .collect();
                    format!("{:?} {:?} {routes:?}", ce.edge.kind, ce.edge.shape)
                });
                program_of.insert(id, text);
            }
        }
        let tos = |v: &[CallEdge]| {
            let mut t: Vec<String> = v.iter().filter_map(|e| e.to.clone()).collect();
            t.sort();
            t
        };
        // The whole edge, minus the diagnostic-only fields
        // (`candidates` only feeds the projection; `unknown_method_name`
        // and `receiver_shape` only feed `aldump` breakdowns).
        // Engine-switch S3.2: `dispatch_meta` is now the program engine's own
        // WHOLE-program view (L3's counts workspace implementers only), so it
        // is not compared; the dependency-implementer edges S3.2 adds (to-less
        // `Interface`+`ExternalTarget`) have no L3 counterpart and are dropped
        // from the adapter side before comparing (`comparable` below).
        // Engine-switch S6.0: `receiver_type` is the program resolver's
        // receiver text now. For a receiver with no declaration it renders
        // the resolved object (`Record Customer`, quoted multi-word names)
        // where L3 synthesized its own (`Record rec`, unquoted), so it is
        // not compared here; `receiver_type_comes_from_the_program_resolver`
        // pins it.
        let key = |e: &CallEdge| {
            let mut e = e.clone();
            e.candidates = None;
            e.unknown_method_name = None;
            e.receiver_shape = None;
            e.dispatch_meta = None;
            e.receiver_type = None;
            e
        };
        let comparable = |e: &CallEdge| {
            !(e.dispatch_kind == DispatchKind::Interface
                && e.to.is_none()
                && e.resolution == Resolution::ExternalTarget)
        };
        let same_bindings = |id: &str, is_call: bool| {
            !is_call || calls.upgraded_bindings.get(id) == old.upgraded_bindings.get(id)
        };
        let mut here: std::collections::BTreeMap<&str, usize> = Default::default();
        for (id, is_call) in &ids {
            let mut n: Vec<CallEdge> = new_g
                .get(id)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|e| comparable(e))
                .collect();
            let mut o = old_g.get(id).cloned().unwrap_or_default();
            let (nt, ot) = (tos(&n), tos(&o));
            let bucket = match (nt.is_empty(), ot.is_empty()) {
                (true, true) => {
                    let shape = |v: &[CallEdge]| {
                        v.iter()
                            .map(|e| (e.dispatch_kind, e.resolution))
                            .collect::<Vec<_>>()
                    };
                    Some(if shape(&n) == shape(&o) {
                        "to-less, same kind+resolution"
                    } else {
                        "to-less, different kind+resolution"
                    })
                }
                (true, false) => Some("L3 only resolved"),
                (false, true) => Some("adapter only resolved"),
                (false, false) if nt != ot => Some("differing target"),
                (false, false) => None,
            };
            if bucket == Some("to-less, same kind+resolution") && !same_bindings(id, *is_call) {
                failures.push(format!(
                    "{name} {id} [to-less, same kind+resolution] bindings: adapter {:?} vs L3 {:?}",
                    calls.upgraded_bindings.get(id),
                    old.upgraded_bindings.get(id),
                ));
            }
            // Informational: same kind+resolution, but another field
            // (external_type_ref, dispatch_meta, ..) differs.
            if bucket == Some("to-less, same kind+resolution")
                && n.iter().map(key).collect::<Vec<_>>() != o.iter().map(key).collect::<Vec<_>>()
            {
                *here
                    .entry("to-less, same kind+resolution, other fields differ")
                    .or_default() += 1;
                eprintln!(
                    "  {name} [to-less same, fields differ] {id}: adapter {:?} vs L3 {:?}",
                    n.iter().map(key).collect::<Vec<_>>(),
                    o.iter().map(key).collect::<Vec<_>>()
                );
            }
            if let Some(bucket) = bucket {
                *here.entry(bucket).or_default() += 1;
                if bucket != "to-less, same kind+resolution" {
                    let show = |v: &[CallEdge]| {
                        v.iter()
                            .map(|e| (e.dispatch_kind.as_str(), e.resolution, e.to.is_some()))
                            .collect::<Vec<_>>()
                    };
                    eprintln!(
                        "  {name} [{bucket}] {id}: adapter {:?} vs L3 {:?} (program: {})",
                        show(&n),
                        show(&o),
                        program_of[id]
                    );
                }
                continue;
            }
            n.sort_by(|a, b| a.to.cmp(&b.to));
            o.sort_by(|a, b| a.to.cmp(&b.to));
            let same_edges = n.len() == o.len() && n.iter().zip(&o).all(|(x, y)| key(x) == key(y));
            if same_edges && same_bindings(id, *is_call) {
                *here.entry("agree").or_default() += 1;
            } else {
                failures.push(format!(
                    "{name} {id}: adapter {:?} / {:?} vs L3 {:?} / {:?}",
                    n.iter().map(key).collect::<Vec<_>>(),
                    calls.upgraded_bindings.get(id),
                    o.iter().map(key).collect::<Vec<_>>(),
                    old.upgraded_bindings.get(id),
                ));
            }
        }
        eprintln!("parity {name}: {here:?}");
        for (k, v) in here {
            *total.entry(k).or_default() += v;
        }
    }
    eprintln!(
        "parity TOTAL over {} fixtures: {total:?}, mismatched {}",
        dirs.len(),
        failures.len()
    );
    assert!(
        total.get("agree").copied().unwrap_or(0) > 0,
        "the corpus exercised no comparable site"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
