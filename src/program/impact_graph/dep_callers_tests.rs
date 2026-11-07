//! Callers through dependency code on `tests/fixtures/dependency-callers`
//! (its README states every case), with the dependency loaded both as a
//! source folder and as a package: design r2 invariants 1-5.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::*;
use crate::engine::l4::effect_query_cli::{QuerySubstrate, query_view};
use crate::program::impact_graph::sited_from_program;
use crate::program::inputs::WorkspaceInputs;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dependency-callers")
}

/// The two ways the dependency is loaded.
#[derive(Clone, Copy, Debug)]
enum Form {
    /// `ws/al-sem.dependencies.json` -> `../dep` (trust tier local source).
    SourceFolder,
    /// A `.app` with `dep/`'s source embedded, in the workspace's `.alpackages`.
    Package,
}

const DEP_GUID: &str = "437dbf0e-84ff-417a-965d-ed2bb9650972";

/// A `.app`: the 40-byte NAVX header, then a zip of the manifest, the symbols
/// and the embedded source (as `tests/common/symbol_app.rs` writes one).
fn write_package(path: &Path, sources: &[(String, String)]) {
    let manifest = format!(
        r#"<?xml version="1.0" encoding="utf-8"?><Package xmlns="http://schemas.microsoft.com/navx/2015/manifest"><App Id="{DEP_GUID}" Name="Base Application" Publisher="Microsoft" Version="1.0.0.0" Runtime="13.0" /></Package>"#
    );
    let symbols = format!(
        r#"{{"RuntimeVersion":"13.0","AppId":"{DEP_GUID}","Name":"Base Application","Publisher":"Microsoft","Version":"1.0.0.0"}}"#
    );
    let mut bytes = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut bytes);
        let opts =
            zip::write::SimpleFileOptions::default().last_modified_time(zip::DateTime::default());
        zip.start_file("NavxManifest.xml", opts).unwrap();
        zip.write_all(manifest.as_bytes()).unwrap();
        zip.start_file("SymbolReference.json", opts).unwrap();
        zip.write_all(symbols.as_bytes()).unwrap();
        for (entry, text) in sources {
            zip.start_file(entry.as_str(), opts).unwrap();
            zip.write_all(text.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }
    let mut out = vec![0u8; 40];
    out.extend_from_slice(bytes.get_ref());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, out).unwrap();
}

/// The fixture's workspace in `form`: the committed folder, or a copy whose
/// dependency is a package. The temp dir (if any) must outlive the use.
fn workspace(form: Form) -> (PathBuf, Option<tempfile::TempDir>) {
    match form {
        Form::SourceFolder => (fixture().join("ws"), None),
        Form::Package => {
            let t = tempfile::tempdir().unwrap();
            // A `.git` marker stops the package discovery's ancestor walk.
            std::fs::create_dir_all(t.path().join(".git")).unwrap();
            let ws = t.path().join("ws");
            std::fs::create_dir_all(ws.join("src")).unwrap();
            for f in ["app.json", "src/Callers.al"] {
                std::fs::copy(fixture().join("ws").join(f), ws.join(f)).unwrap();
            }
            let sources: Vec<(String, String)> = ["Engine.al", "Workflow.al"]
                .iter()
                .map(|f| {
                    (
                        format!("src/{f}"),
                        std::fs::read_to_string(fixture().join("dep/src").join(f)).unwrap(),
                    )
                })
                .collect();
            write_package(
                &ws.join(".alpackages/Microsoft_Base Application_1.0.0.0.app"),
                &sources,
            );
            (ws, Some(t))
        }
    }
}

/// What a test reads off one form: the sited graph, the root's nodes, the
/// answer, and the solve graph's workspace callers of the root (stable ids).
struct World {
    g: SitedGraph,
    roots: Vec<NodeIx>,
    answer: Answer,
    solve_workspace: BTreeSet<String>,
    _tmp: Option<tempfile::TempDir>,
}

fn world(form: Form) -> World {
    let (ws, tmp) = workspace(form);
    let inputs = Arc::new(WorkspaceInputs::read(&ws, &Default::default()));
    let view = query_view(&inputs, false).expect("view");
    let substrate = QuerySubstrate::of(&view);
    let model = substrate.resolved();
    let query = substrate.query();
    let target = model
        .workspace
        .routines
        .iter()
        .find(|r| r.name == "Target")
        .expect("DC Target.Target");
    let ws_guid = target.app_guid.clone();
    let rix = query.routine_ix(&target.id).expect("a solve row");
    let solve_workspace: BTreeSet<String> = query
        .ancestors(rix)
        .into_iter()
        .filter_map(|(_, a)| {
            let id = query.bundle().routine_id(a);
            let r = model.workspace.routines.iter().find(|r| r.id == id)?;
            (r.app_guid == ws_guid).then(|| r.stable_routine_id.clone())
        })
        .collect();
    let program = crate::program::resolved::ResolvedProgram::build(
        &inputs,
        crate::program::resolved::ProgramScope::CrossApp,
    )
    .expect("program");
    let g = sited_from_program(&program, &|| false).expect("not cancelled");
    let roots = g.nodes_with_stable_id(&target.stable_routine_id);
    assert!(!roots.is_empty(), "{form:?}: the root has a node");
    let answer = callers_through_dependency(&g, &roots, &WalkOptions::default());
    World {
        g,
        roots,
        answer,
        solve_workspace,
        _tmp: tmp,
    }
}

/// `Codeunit 52804::start`-style names of the listed callers in `tier`.
fn listed(w: &World, tier: DepTier) -> Vec<String> {
    w.answer
        .callers
        .iter()
        .filter(|c| c.tier == tier)
        .map(|c| w.g.display(c.node))
        .collect()
}

fn caller<'a>(w: &'a World, display: &str) -> &'a DepCaller {
    w.answer
        .callers
        .iter()
        .find(|c| w.g.display(c.node) == display)
        .unwrap_or_else(|| panic!("{display} is listed"))
}

fn check_form(form: Form) {
    let w = world(form);
    assert!(w.answer.limited.is_none(), "{form:?}: no cap");
    // Invariant 5 (first, so a witness break fails here): every witness is a
    // chain of code-call sites ending at the root.
    for c in &w.answer.callers {
        check_witness(&w.g, &w.roots, c);
    }
    // Invariant 1: workspace-only code-call callers == the solve graph's.
    let ordinary_names: BTreeSet<String> = w
        .answer
        .ordinary
        .iter()
        .map(|id| w.g.display(w.g.nodes_with_stable_id(id)[0]))
        .collect();
    assert_eq!(
        ordinary_names,
        BTreeSet::from(
            [
                "Codeunit 52802::step",
                "Codeunit 52803::handleafterpostit",
                "Codeunit 52803::handleafterrun",
                "Codeunit 52805::calltarget",
                // Also reached through dependency code: the workspace path wins.
                "Codeunit 52809::direct",
            ]
            .map(String::from)
        ),
        "{form:?}: the ordinary code-call callers"
    );
    assert_eq!(
        w.answer.ordinary, w.solve_workspace,
        "{form:?}: R0's workspace-only code-call callers equal the solve graph's"
    );
    // ... and the listed callers are disjoint from them.
    for c in &w.answer.callers {
        assert!(
            !w.solve_workspace.contains(&c.stable_id),
            "{form:?}: {} is not also a solve caller",
            w.g.display(c.node)
        );
    }
    // Invariants 2 and 3: composition across workspace hops on both sides
    // of the dependency, and a record trigger as the first hop.
    let mut code = listed(&w, DepTier::Code);
    assert_eq!(
        code.first().map(String::as_str),
        Some("Codeunit 52804::start"),
        "{form:?}: nearest first"
    );
    code.sort();
    assert_eq!(
        code,
        vec![
            "Codeunit 52804::start",
            "Codeunit 52806::touch",
            "Codeunit 52808::outer",
            // Also reached through a configured site: the code path wins.
            "Codeunit 52809::mixed",
        ],
        "{form:?}: code-tier callers"
    );
    let hops: Vec<usize> = w.answer.callers.iter().map(|c| c.hops.len()).collect();
    let code_n = w.answer.count(DepTier::Code);
    assert!(
        hops[..code_n].is_sorted(),
        "{form:?}: sorted by hops within the tier: {hops:?}"
    );
    let start = caller(&w, "Codeunit 52804::start");
    let kinds: Vec<&str> = start.hops.iter().map(|h| h.kind.as_str()).collect();
    assert_eq!(
        kinds,
        vec!["call", "call", "event", "call", "call"],
        "{form:?}"
    );
    assert_eq!(
        w.g.display(start.hops[2].from),
        "Codeunit 52901::onafterrun",
        "{form:?}: the event hop names the publisher"
    );
    assert_eq!(
        start.enters_at.map(|n| w.g.display(n)).as_deref(),
        Some("Codeunit 52803::handleafterrun"),
        "{form:?}: it enters two workspace hops before the root"
    );
    assert!(start.crossed_event);
    assert_eq!(
        caller(&w, "Codeunit 52808::outer").hops.len(),
        6,
        "{form:?}"
    );
    let touch = caller(&w, "Codeunit 52806::touch");
    assert_eq!(touch.hops[0].kind, HopKind::RecordTrigger, "{form:?}");
    assert_eq!(
        w.g.display(touch.hops[0].to),
        "Table 52900::oninsert",
        "{form:?}"
    );
    // Invariant 4: the catalog labels, never hides.
    assert_eq!(
        listed(&w, DepTier::Configured),
        vec!["Codeunit 52807::approve"],
        "{form:?}: the configured caller is listed, after the code tier"
    );
    let approve = caller(&w, "Codeunit 52807::approve");
    let arms: Vec<&str> = approve
        .hops
        .iter()
        .filter_map(|h| h.configured.as_ref().map(|a| &*a.label))
        .collect();
    assert_eq!(arms, vec!["PostCode()"], "{form:?}: the configured hop");
    for c in w.answer.callers.iter().filter(|c| c.tier == DepTier::Code) {
        assert!(
            c.hops.iter().all(|h| h.configured.is_none()),
            "{form:?}: a code-tier witness has no configured hop"
        );
    }
}

/// Invariant 5 (design r2 §10.2): hop i leaves hop i-1's routine by a
/// code-call site of that routine; the first leaves the caller, the last
/// enters a root.
pub(super) fn check_witness(g: &SitedGraph, roots: &[NodeIx], c: &DepCaller) {
    let name = g.display(c.node);
    assert!(!c.hops.is_empty(), "{name}: a witness");
    assert_eq!(
        c.hops[0].from, c.node,
        "{name}: the witness starts at the caller"
    );
    for (i, h) in c.hops.iter().enumerate() {
        let st = &g.sites()[h.site as usize];
        assert_eq!(
            st.from, h.from,
            "{name} hop {i}: the site leaves the hop's routine"
        );
        assert!(
            super::super::reverse::is_code_call_site(g, h.site),
            "{name} hop {i}: a code-call site"
        );
        if i > 0 {
            assert_eq!(c.hops[i - 1].to, h.from, "{name} hop {i}: consecutive");
        }
    }
    assert!(
        roots.contains(&c.hops.last().unwrap().to),
        "{name}: the witness ends at the root"
    );
}

#[test]
fn source_folder_dependency() {
    check_form(Form::SourceFolder);
}

#[test]
fn package_dependency() {
    check_form(Form::Package);
}

/// Round-2 r2 §2.1 step 2: impact reads its tier from R0's `Tier` and
/// tracks no app identity of its own, which is sound because, in the
/// production universe (the dependency a package), R0's `flag::LOCAL` and
/// `is_dependency_node` agree on every app-owned node: `LOCAL ==
/// !is_dependency_node`. Hubs are outside the equality and are tested apart
/// (impact's `a_mixed_path_over_navigation_and_a_hub_…`): they carry no
/// `LOCAL` flag, and `is_dependency_node` is false for them. A callbacks hub
/// (`h:callbacks:<app>`) does name an app (`node_app` is `Some`), so the
/// equality skips every `HUB` node, not only the app-less ones. With the
/// dependency loaded as a SOURCE FOLDER (trust tier local source) R0 counts
/// it `LOCAL` while `is_dependency_node` does not: the equality is the
/// production universe's, and does not hold there (also checked).
///
/// DISCRIMINATION (2026-11-07): marking every app `LOCAL` in the sited
/// graph's builder (`local_apps` gaining every object's app) fails "LOCAL
/// == !is_dependency_node" on the first dependency node.
#[test]
fn local_flag_and_app_identity_agree_on_every_app_owned_node() {
    let w = world(Form::Package);
    let g = &w.g;
    let mut checked = (0usize, 0usize);
    for n in 0..g.node_count() as u32 {
        if g.node_app(n).is_none() || g.flags(n) & super::super::sited::flag::HUB != 0 {
            continue;
        }
        let local = g.flags(n) & super::super::sited::flag::LOCAL != 0;
        assert_eq!(
            local,
            !is_dependency_node(g, n),
            "LOCAL == !is_dependency_node: {}",
            g.display(n)
        );
        if local {
            checked.0 += 1;
        } else {
            checked.1 += 1;
        }
    }
    assert!(
        checked.0 > 3 && checked.1 > 3,
        "precondition: workspace and dependency nodes: {checked:?}"
    );
    let s = world(Form::SourceFolder);
    assert!(
        (0..s.g.node_count() as u32)
            .any(|n| is_dependency_node(&s.g, n)
                && s.g.flags(n) & super::super::sited::flag::LOCAL != 0),
        "a local-source dependency is LOCAL to R0 and a dependency by app identity"
    );
}
