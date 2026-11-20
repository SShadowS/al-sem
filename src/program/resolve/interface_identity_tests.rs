//! ENG-152: interfaces by identity, through whole-program resolution
//! (`resolve_full_program`), so each test reaches the production call site.
//! The resolver-level cases (one visible app, several, none) are
//! `resolver::tests::interface_dispatch_reaches_only_the_named_interfaces_implementers`.

use crate::program::node::ObjKey;
use crate::program::resolve::edge::{DispatchShape, RouteTarget};
use crate::program::resolve::full::{ProgramReport, resolve_full_program};

const WS_GUID: &str = "15215215-0000-0000-0000-000000000001";
const A_GUID: &str = "15215215-0000-0000-0000-0000000000a1";
const B_GUID: &str = "15215215-0000-0000-0000-0000000000b1";

/// A dependency `.app`: its symbols and any embedded `.al` source.
fn app(guid: &str, name: &str, symbols: &str, sources: &[(&str, &str)]) -> Vec<u8> {
    let manifest = format!(
        r#"<?xml version="1.0" encoding="utf-8"?><Package xmlns="http://schemas.microsoft.com/navx/2015/manifest"><App Id="{guid}" Name="{name}" Publisher="probe" Version="1.0.0.0" Runtime="15.0" /></Package>"#
    );
    let mut entries: Vec<(&str, &[u8])> = vec![
        ("NavxManifest.xml", manifest.as_bytes()),
        ("SymbolReference.json", symbols.as_bytes()),
    ];
    for (path, text) in sources {
        entries.push((path, text.as_bytes()));
    }
    crate::engine::deps::app_package_zip::test_apps::build_app(&entries)
}

/// A workspace depending on `deps` (`(guid, name)`), with `files` and the
/// packages `apps` (`(file name, bytes)`), resolved whole.
fn report(
    deps: &[(&str, &str)],
    files: &[(&str, &str)],
    apps: Vec<(String, Vec<u8>)>,
) -> ProgramReport {
    let dir = tempfile::tempdir().expect("tempdir");
    let deps_json: Vec<String> = deps
        .iter()
        .map(|(g, n)| {
            format!(r#"{{"id":"{g}","name":"{n}","publisher":"probe","version":"1.0.0.0"}}"#)
        })
        .collect();
    std::fs::write(
        dir.path().join("app.json"),
        format!(
            r#"{{"id":"{WS_GUID}","name":"Ws152","publisher":"probe","version":"1.0.0.0","runtime":"15.0","idRanges":[{{"from":50100,"to":50199}}],"dependencies":[{}]}}"#,
            deps_json.join(",")
        ),
    )
    .unwrap();
    for (rel, text) in files {
        let p = dir.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    let pkgs = dir.path().join(".alpackages");
    std::fs::create_dir_all(&pkgs).unwrap();
    for (name, bytes) in apps {
        std::fs::write(pkgs.join(name), bytes).unwrap();
    }
    resolve_full_program(dir.path()).expect("resolves")
}

/// The object keys every route of the interface (polymorphic) sites in
/// routine `from_lc` targets, sorted.
fn targets(report: &ProgramReport, from_lc: &str) -> Vec<String> {
    let mut out: Vec<String> = report
        .edges
        .iter()
        .filter(|c| {
            c.edge.from.name_lc.as_ref() == from_lc && c.edge.shape == DispatchShape::Polymorphic
        })
        .flat_map(|c| c.edge.routes.iter())
        .filter_map(|r| match &r.target {
            RouteTarget::Unresolved => None,
            _ => r.target_object().map(|o| match o.key {
                ObjKey::Id(n) => n.to_string(),
                ObjKey::Name(s) => s.to_string(),
            }),
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// A symbol reference qualified by another app's guid keeps that app: package
/// B declares its OWN `Dep Iface` and implements app A's (`#<A>#"Dep Iface"`,
/// alc's shape), which extends `IBase`. A workspace call through A's `IBase`
/// reaches B's codeunit (an implementer of a derived interface implements the
/// base, ENG-142). Dropping the guid read B's reference as B's own
/// `Dep Iface`, which has no base: the codeunit was missed.
///
/// Mutation (2026-11-20): the symbol reader's `abi_interface_app` answering
/// `None` (the guid dropped, the reference inferred as B's own) -> no route to
/// 70201 (red); restored, green.
#[test]
fn a_qualified_symbol_reference_keeps_its_app() {
    let go = r#"{"Name":"Go","Parameters":[]}"#;
    let a_dashless = A_GUID.replace('-', "");
    let a_symbols = format!(
        r#"{{"RuntimeVersion":"15.0","Interfaces":[{{"Name":"IBase","Methods":[{go}]}},{{"Name":"Dep Iface","ExtendedInterfaces":["IBase"],"Methods":[]}}]}}"#
    );
    let b_symbols = format!(
        r##"{{"RuntimeVersion":"15.0","Interfaces":[{{"Name":"Dep Iface","Methods":[]}}],"Codeunits":[{{"Id":70201,"Name":"B Impl","ImplementedInterfaces":["#{a_dashless}#\"Dep Iface\""],"Methods":[{{"Id":1,"Name":"Go","Parameters":[]}}]}}]}}"##
    );
    let caller = "codeunit 50120 \"Caller\"\n{\n    procedure Calls()\n    var\n        X: Interface IBase;\n    begin\n        X.Go();\n    end;\n}\n";
    let r = report(
        &[(A_GUID, "DepA152"), (B_GUID, "DepB152")],
        &[("src/Caller.al", caller)],
        vec![
            (
                "probe_DepA152_1.0.0.0.app".into(),
                app(A_GUID, "DepA152", &a_symbols, &[]),
            ),
            (
                "probe_DepB152_1.0.0.0.app".into(),
                app(B_GUID, "DepB152", &b_symbols, &[]),
            ),
        ],
    );
    assert_eq!(targets(&r, "calls"), ["70201"]);
}

/// A callee's interface return type binds in the CALLEE's app. Dependency A
/// declares `IDup`, a `Factory` whose `Make()` returns it, and an implementer;
/// dependency B (which A does not see) declares its own `IDup` and
/// implementer; the workspace sees both. `F.Make().Go()` in the workspace
/// reaches only A's implementer: `Make`'s `IDup` is A's. Bound from the
/// caller's app, `IDup` names both A's and B's (several visible:
/// over-approximated) and B's implementer was reached too.
///
/// Mutation (2026-11-20): `ReceiverType::declared_in` returning the receiver
/// unchanged (scoped at the caller) -> 70202 reached too (red); restored, green.
#[test]
fn a_return_type_binds_in_the_callees_app() {
    let dep_src = "interface IDup\n{\n    procedure Go();\n}\n\ncodeunit 70101 \"Factory\"\n{\n    procedure Make(): Interface IDup\n    var\n        I: Codeunit \"Dep Impl\";\n    begin\n        exit(I);\n    end;\n}\n\ncodeunit 70102 \"Dep Impl\" implements IDup\n{\n    procedure Go()\n    begin\n    end;\n}\n";
    let a_symbols = r#"{"RuntimeVersion":"15.0","Interfaces":[{"Name":"IDup","Methods":[{"Name":"Go","Parameters":[]}]}],"Codeunits":[{"Id":70101,"Name":"Factory","Methods":[{"Id":1,"Name":"Make","Parameters":[],"ReturnTypeDefinition":{"Name":"Interface","Subtype":{"Name":"IDup"}}}]},{"Id":70102,"Name":"Dep Impl","ImplementedInterfaces":["IDup"],"Methods":[{"Id":1,"Name":"Go","Parameters":[]}]}]}"#;
    let b_symbols = r#"{"RuntimeVersion":"15.0","Interfaces":[{"Name":"IDup","Methods":[{"Name":"Go","Parameters":[]}]}],"Codeunits":[{"Id":70202,"Name":"B Impl","ImplementedInterfaces":["IDup"],"Methods":[{"Id":1,"Name":"Go","Parameters":[]}]}]}"#;
    let caller = "codeunit 50120 \"Caller\"\n{\n    procedure Calls()\n    var\n        F: Codeunit Factory;\n    begin\n        F.Make().Go();\n    end;\n}\n";
    let r = report(
        &[(A_GUID, "DepA152"), (B_GUID, "DepB152")],
        &[("src/Caller.al", caller)],
        vec![
            (
                "probe_DepA152_1.0.0.0.app".into(),
                app(A_GUID, "DepA152", a_symbols, &[("src/Dep.al", dep_src)]),
            ),
            (
                "probe_DepB152_1.0.0.0.app".into(),
                app(B_GUID, "DepB152", b_symbols, &[]),
            ),
        ],
    );
    assert_eq!(targets(&r, "calls"), ["70102"]);
}
