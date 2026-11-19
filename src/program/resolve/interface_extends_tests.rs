//! ENG-142: interface inheritance (`interface IBaz extends IBar, NS.IQux`), pinned
//! through whole-program resolution (`resolve_full_program`), so each test reaches
//! the production call site, not only the helper. alc 18.0.41 fixes the meaning
//! (`/work/.al-sem/probes/grm05-iface`): `Z.Bar()` on an `IBaz` receiver
//! compiles, `B := Z` and `B := <codeunit implementing IBaz>` compile for
//! `B: Interface IBar`, and an `IBaz` implementer without `Bar` fails AL0582.
//!
//! Mutations (2026-11-19, each restored, green):
//! - `inherit_implementers` not called -> `IBar`/`IQux` calls reach no
//!   implementer (`a_call_through_a_base_reaches_every_derived_implementer`,
//!   `a_dependency_interfaces_bases_come_from_its_symbols`, red);
//! - its walk one level deep (no `stack.extend`) -> the grand-derived `C Deep`
//!   is missing (red);
//! - `interface_own_routine_node` without the base search -> `Take(O.Bar())`
//!   stays an ambiguous overload (`a_base_member_types_an_argument`, red; with
//!   implementers present the same break stayed GREEN, since `arg_dispatch`
//!   falls back to their agreeing return types, hence the implementer-less
//!   `IOrphan`);
//! - the ExtendedInterfaces read dropped -> the dependency case (red);
//! - the recovered-parse guard in `node_extract` dropped -> the broken file's
//!   bases count (`a_recovered_parse_gives_no_bases`, red).

use crate::program::node::ObjKey;
use crate::program::resolve::edge::{DispatchShape, RouteTarget};
use crate::program::resolve::full::{ProgramReport, resolve_full_program};

const APP_JSON: &str = r#"{"id":"14214214-0000-0000-0000-000000000001","name":"Ifx142","publisher":"probe","version":"1.0.0.0","runtime":"15.0","idRanges":[{"from":50100,"to":50199}]}"#;

const IFACES: &str = r#"namespace Probe.Ifaces;

interface IBar
{
    procedure Bar(): Integer;
}

interface IQux
{
    procedure Qux(): Integer;
}

interface IBaz extends IBar, Probe.Ifaces.IQux
{
    procedure Baz(): Integer;
}

interface IDeep extends IBaz
{
    procedure Deep(): Integer;
}

interface IOrphan extends IBar
{
    procedure Orphan(): Integer;
}
"#;

/// A codeunit `name` implementing `iface`, with every member of the chain.
fn implementer(id: u32, name: &str, iface: &str) -> String {
    format!(
        r#"namespace Probe.Ifaces;

codeunit {id} "{name}" implements {iface}
{{
    procedure Bar(): Integer begin exit(1); end;
    procedure Qux(): Integer begin exit(2); end;
    procedure Baz(): Integer begin exit(3); end;
    procedure Deep(): Integer begin exit(4); end;
}}
"#
    )
}

/// The caller: `Calls` with `vars` and `body`.
fn caller(vars: &str, body: &str) -> String {
    format!(
        "namespace Probe.Ifaces;\n\ncodeunit 50120 \"Caller\"\n{{\n    procedure Calls()\n    var\n{vars}\n    begin\n{body}\n    end;\n}}\n"
    )
}

fn write(files: &[(&str, String)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for (rel, text) in files {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

fn report_of(files: &[(&str, String)]) -> ProgramReport {
    let dir = write(files);
    resolve_full_program(dir.path()).expect("resolves")
}

/// The routine targets (`<object key>.<routine>`) of the interface (polymorphic)
/// site in `Caller.Calls` whose called member is `member_lc`, sorted.
fn interface_targets(report: &ProgramReport, member_lc: &str) -> Vec<String> {
    let mut out: Vec<String> = report
        .edges
        .iter()
        .filter(|c| {
            c.edge.from.name_lc.as_ref() == "calls" && c.edge.shape == DispatchShape::Polymorphic
        })
        .flat_map(|c| c.edge.routes.iter())
        .filter_map(|r| match &r.target {
            RouteTarget::Routine(id) if id.name_lc.as_ref() == member_lc => {
                let key = match &id.object.key {
                    ObjKey::Id(n) => n.to_string(),
                    ObjKey::Name(s) => s.to_string(),
                };
                Some(format!("{key}.{}", id.name_lc))
            }
            _ => None,
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

fn base_workspace(caller_text: String) -> Vec<(&'static str, String)> {
    vec![
        ("app.json", APP_JSON.into()),
        ("src/Ifaces.al", IFACES.into()),
        ("src/ImplBaz.al", implementer(50101, "C Baz", "IBaz")),
        ("src/ImplDeep.al", implementer(50102, "C Deep", "IDeep")),
        ("src/ImplBar.al", implementer(50103, "C Bar", "IBar")),
        ("src/Caller.al", caller_text),
    ]
}

/// A call through `IBar` reaches its own implementer, the implementer of
/// `IBaz extends IBar`, and of `IDeep extends IBaz` (transitive); a call
/// through `IQux` (a namespace-qualified base) reaches the `IBaz` and `IDeep`
/// implementers, not `C Bar`; a call through `IBaz` does not reach `C Bar`
/// (a base's implementer is not a derived one).
#[test]
fn a_call_through_a_base_reaches_every_derived_implementer() {
    let report = report_of(&base_workspace(caller(
        "        B: Interface IBar;\n        Q: Interface IQux;\n        Z: Interface IBaz;\n        N: Integer;",
        "        N := B.Bar();\n        N := Q.Qux();\n        N := Z.Baz();",
    )));
    assert_eq!(
        interface_targets(&report, "bar"),
        ["50101.bar", "50102.bar", "50103.bar"]
    );
    assert_eq!(
        interface_targets(&report, "qux"),
        ["50101.qux", "50102.qux"]
    );
    assert_eq!(
        interface_targets(&report, "baz"),
        ["50101.baz", "50102.baz"]
    );
}

/// `Take(O.Bar())`, `Take` overloaded on `Integer` and `Text`, `O: Interface
/// IOrphan` (`extends IBar`, no implementer in the program: they may live in
/// an app not loaded), `Bar` declared on the base `IBar` only: the argument is
/// typed by the base member's declared return type (`Integer`), so the call
/// picks `Take(N: Integer)` (`arg_dispatch`'s interface-signature path).
/// Without the base search nothing types the argument (no implementer's
/// signature to fall back on) and the overload stays ambiguous. (With
/// implementers, `arg_dispatch` also agrees their return types, which AL makes
/// equal, so that case cannot tell the search's work.)
#[test]
fn a_base_member_types_an_argument() {
    let caller_text = "namespace Probe.Ifaces;\n\ncodeunit 50120 \"Caller\"\n{\n    procedure Calls()\n    var\n        O: Interface IOrphan;\n    begin\n        Take(O.Bar());\n    end;\n\n    procedure Take(N: Integer)\n    begin\n    end;\n\n    procedure Take(T: Text)\n    begin\n    end;\n}\n";
    let report = report_of(&base_workspace(caller_text.into()));
    let take: Vec<_> = report
        .edges
        .iter()
        .filter(|c| {
            c.edge.from.name_lc.as_ref() == "calls"
                && c.edge.routes.iter().any(|r| {
                    matches!(&r.target, RouteTarget::Routine(id) if id.name_lc.as_ref() == "take")
                })
        })
        .collect();
    assert_eq!(take.len(), 1, "one Take site");
    assert_ne!(take[0].edge.shape, DispatchShape::AmbiguousOverload);
    assert_eq!(take[0].edge.routes.len(), 1, "{:?}", take[0].edge.routes);
    let picked = report
        .edges
        .iter()
        .flat_map(|c| c.edge.routes.iter())
        .filter_map(|r| match &r.target {
            RouteTarget::Routine(id) if id.name_lc.as_ref() == "take" => Some(id.clone()),
            _ => None,
        })
        .next()
        .unwrap();
    // The `(Integer)` overload's fingerprint, from the same declaration parsed alone.
    let one = al_syntax::parse(
        "codeunit 1 X\n{\n    procedure Take(N: Integer)\n    begin\n    end;\n}\n",
    );
    let integer_fp =
        crate::program::sig_fp::source_param_sig_fp(&one.objects[0].routines[0].params);
    assert_ne!(integer_fp, 0);
    assert_eq!(
        picked.sig_fp, integer_fp,
        "O.Bar() is an Integer: {picked:?}"
    );
}

/// An interface in a file that did not parse clean gives no bases (its list may
/// be misread), so its implementers are not added to them.
#[test]
fn a_recovered_parse_gives_no_bases() {
    let mut files = base_workspace(caller(
        "        B: Interface IBar;\n        N: Integer;",
        "        N := B.Bar();",
    ));
    files.push((
        "src/Broken.al",
        "namespace Probe.Ifaces;\n\ninterface IBroken extends IBar\n{\n    procedure Broken(): Integer;\n}\n\ncodeunit 50130 \"Junk\"\n{\n    procedure (\n"
            .into(),
    ));
    files.push((
        "src/ImplBroken.al",
        implementer(50104, "C Broken", "IBroken"),
    ));
    let report = report_of(&files);
    let bar = interface_targets(&report, "bar");
    assert!(
        !bar.contains(&"50104.bar".to_string()),
        "a recovered file's extends list is not trusted: {bar:?}"
    );
    assert!(bar.contains(&"50101.bar".to_string()), "{bar:?}");
}

/// An `extends` cycle (alc refuses it; a broken workspace may hold it) ends:
/// each interface of the cycle lists the other's implementer once.
#[test]
fn an_extends_cycle_ends() {
    let ifaces = "namespace Probe.Ifaces;\n\ninterface IA extends IB\n{\n    procedure Bar(): Integer;\n}\n\ninterface IB extends IA\n{\n    procedure Bar(): Integer;\n}\n";
    let report = report_of(&[
        ("app.json", APP_JSON.into()),
        ("src/Cycle.al", ifaces.into()),
        ("src/ImplA.al", implementer(50101, "C A", "IA")),
        ("src/ImplB.al", implementer(50102, "C B", "IB")),
        (
            "src/Caller.al",
            caller(
                "        A: Interface IA;\n        N: Integer;",
                "        N := A.Bar();",
            ),
        ),
    ]);
    assert_eq!(
        interface_targets(&report, "bar"),
        ["50101.bar", "50102.bar"]
    );
}

const DEP_GUID: &str = "14214214-0000-0000-0000-0000000000df";

/// A dependency's interfaces arrive as symbols only: their bases come from the
/// symbols' `ExtendedInterfaces` (alc's own output shape). A workspace call
/// through the dependency's base reaches the dependency's implementer of the
/// derived interface.
#[test]
fn a_dependency_interfaces_bases_come_from_its_symbols() {
    let go = r#"{"Name":"Go","Parameters":[]}"#;
    let symbols = format!(
        r#"{{"RuntimeVersion":"15.0","Interfaces":[{{"Name":"IDepBase","Methods":[{go}]}},{{"Name":"IDepDer","ExtendedInterfaces":["IDepBase"],"Methods":[]}}],"Codeunits":[{{"Id":70001,"Name":"Dep Impl","ImplementedInterfaces":["IDepDer"],"Methods":[{go}]}}]}}"#
    );
    let manifest = format!(
        r#"<?xml version="1.0" encoding="utf-8"?><Package xmlns="http://schemas.microsoft.com/navx/2015/manifest"><App Id="{DEP_GUID}" Name="Dep142" Publisher="probe" Version="1.0.0.0" Runtime="15.0" /></Package>"#
    );
    let app = crate::engine::deps::app_package_zip::test_apps::build_app(&[
        ("NavxManifest.xml", manifest.as_bytes()),
        ("SymbolReference.json", symbols.as_bytes()),
    ]);
    let app_json = format!(
        r#"{{"id":"14214214-0000-0000-0000-000000000002","name":"Ws142","publisher":"probe","version":"1.0.0.0","runtime":"15.0","idRanges":[{{"from":50100,"to":50199}}],"dependencies":[{{"id":"{DEP_GUID}","name":"Dep142","publisher":"probe","version":"1.0.0.0"}}]}}"#
    );
    let dir = write(&[
        ("app.json", app_json),
        (
            "src/Caller.al",
            "codeunit 50120 \"Caller\"\n{\n    procedure Calls()\n    var\n        B: Interface IDepBase;\n    begin\n        B.Go();\n    end;\n}\n".into(),
        ),
    ]);
    let pkgs = dir.path().join(".alpackages");
    std::fs::create_dir_all(&pkgs).unwrap();
    std::fs::write(pkgs.join("probe_Dep142_1.0.0.0.app"), app).unwrap();
    let report = resolve_full_program(dir.path()).expect("resolves");
    assert!(
        report.edges.iter().any(|c| {
            c.edge.from.name_lc.as_ref() == "calls"
                && c.edge.shape == DispatchShape::Polymorphic
                && c.edge.routes.iter().any(|r| {
                    r.target_object()
                        .is_some_and(|o| o.key == ObjKey::Id(70001))
                })
        }),
        "B.Go() reaches the dependency's IDepDer implementer"
    );
}
