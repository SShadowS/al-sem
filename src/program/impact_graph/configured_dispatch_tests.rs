//! The configured-dispatch catalog: the matcher's shapes, and its use by the
//! projection on `tests/fixtures/dependency-callers` (its README states the
//! fixture's hub).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::*;
use crate::program::impact_graph::{SitedGraph, sited_from_program};
use crate::program::inputs::WorkspaceInputs;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dependency-callers")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let p = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &p);
        } else {
            std::fs::copy(e.path(), p).unwrap();
        }
    }
}

/// The fixture in a temp folder, its hub's text passed through `edit`.
fn fixture_with(edit: impl Fn(&str) -> String) -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    copy_dir(&fixture(), t.path());
    let wf = t.path().join("dep/src/Workflow.al");
    let text = std::fs::read_to_string(&wf).unwrap();
    let edited = edit(&text);
    assert_ne!(edited, text, "the fixture edit applied");
    std::fs::write(&wf, edited).unwrap();
    t
}

fn sited(ws: &Path) -> SitedGraph {
    let inputs = Arc::new(WorkspaceInputs::read(ws, &Default::default()));
    let program = crate::program::resolved::ResolvedProgram::build(
        &inputs,
        crate::program::resolved::ProgramScope::CrossApp,
    )
    .expect("program");
    sited_from_program(&program, &|| false).expect("not cancelled")
}

/// `(target routine name, arm label)` of every site out of `ExecuteResponse`,
/// sorted; the label is `-` on a site without the condition.
fn execute_response_sites(g: &SitedGraph) -> Vec<(String, String)> {
    let er = g.routines_named("executeresponse");
    assert_eq!(er.len(), 1, "one ExecuteResponse node");
    let mut out: Vec<(String, String)> = (0..g.site_count() as u32)
        .filter(|&s| g.sites()[s as usize].from == er[0])
        .map(|s| {
            let to = g.display(g.sites()[s as usize].to);
            let name = to.rsplit("::").next().unwrap_or("").to_string();
            let label = match (g.has_condition(s, CONDITION), g.configured_arm(s)) {
                (true, Some(a)) => {
                    assert_eq!(a.entry_name(), CATALOG[0].name);
                    a.label.to_string()
                }
                (false, None) => "-".to_string(),
                other => panic!("condition and arm table disagree on site {s}: {other:?}"),
            };
            (name, label)
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

#[test]
fn the_catalog_labels_the_hubs_arm_sites_and_nothing_else() {
    let g = sited(&fixture().join("ws"));
    assert_eq!(
        g.catalog().entries,
        vec![Status::Matched {
            app_name: "Base Application".into(),
            app_version: "1.0.0.0".into(),
        }]
    );
    assert_eq!(
        execute_response_sites(&g),
        vec![
            ("donothing".into(), "DoNothingCode()".into()),
            ("donothingcode".into(), "DoNothingCode()".into()),
            ("onbeforeexecuteresponse".into(), "-".into()),
            ("onexecuteworkflowresponse".into(), "else".into()),
            ("postcode".into(), "PostCode()".into()),
            ("postit".into(), "PostCode()".into()),
        ],
        "the arm labels and bodies and the else body are labelled; the header's call is not"
    );
    // Only ExecuteResponse's sites carry it.
    let er = g.routines_named("executeresponse")[0];
    for s in 0..g.site_count() as u32 {
        if g.has_condition(s, CONDITION) {
            assert_eq!(g.sites()[s as usize].from, er, "site {s}");
        }
    }
}

#[test]
fn a_signature_mismatch_labels_nothing_and_says_why() {
    let t = fixture_with(|s| s.replacen("xVariant: Variant)", "xVariant: Integer)", 1));
    let g = sited(&t.path().join("ws"));
    match &g.catalog().entries[..] {
        [Status::Mismatch { reason }] => {
            assert!(reason.contains("the signature differs"), "{reason}")
        }
        other => panic!("expected a mismatch, got {other:?}"),
    }
    assert!(
        (0..g.site_count() as u32).all(|s| !g.has_condition(s, CONDITION)),
        "no whole-body fallback"
    );
}

#[test]
fn a_missing_routine_object_or_app_is_reported() {
    let t = fixture_with(|s| s.replacen("ExecuteResponse(", "ExecuteResponse2(", 1));
    let g = sited(&t.path().join("ws"));
    assert!(
        matches!(&g.catalog().entries[..], [Status::Mismatch { reason }] if reason.contains("no routine")),
        "{:?}",
        g.catalog()
    );
    let t = fixture_with(|s| {
        s.replacen(
            "\"Workflow Response Handling\"\n{",
            "\"Workflow Response Handler\"\n{",
            1,
        )
    });
    let g = sited(&t.path().join("ws"));
    assert!(
        matches!(&g.catalog().entries[..], [Status::Mismatch { reason }] if reason.contains("no Codeunit")),
        "{:?}",
        g.catalog()
    );
    // A workspace with no such app: not loaded, not a mismatch.
    let g =
        sited(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/caller-relations/ws"));
    assert_eq!(g.catalog().entries, vec![Status::NotLoaded]);
}

/// `region_in` over one parsed routine (the identity and signature checks
/// are `extract`'s, above).
fn region_of(body: &str) -> Result<Region, String> {
    let text = format!(
        "codeunit 1 \"Workflow Response Handling\"\n{{\n    procedure ExecuteResponse()\n    var\n        WorkflowResponse: Record \"Workflow Response\";\n    begin\n{body}\n    end;\n}}\n"
    );
    let file = al_syntax::parse(&text);
    let r = &file.objects[0].routines[0];
    region_in(0, &CATALOG[0], &file, &text, r.body)
}

#[test]
fn the_case_shape_must_be_exactly_one_case_on_its_own_lines() {
    let ok = region_of(
        "        case WorkflowResponse.\"Function Name\" of\n            A():\n                B();\n            else\n                C();\n        end;",
    )
    .expect("one case");
    assert_eq!(
        ok.arms.iter().map(|a| a.label.as_str()).collect::<Vec<_>>(),
        vec!["A()", "else"]
    );
    // Case-insensitive, quotes folded: `workflowresponse."FUNCTION NAME"`.
    assert!(
        region_of(
            "        case workflowresponse.\"FUNCTION NAME\" of\n            A():\n                B();\n        end;"
        )
        .is_ok()
    );
    let none = region_of(
        "        case WorkflowResponse.Other of\n            A():\n                B();\n        end;",
    );
    assert!(
        none.unwrap_err().contains("no `case"),
        "a case on another scrutinee"
    );
    let two = region_of(
        "        case WorkflowResponse.\"Function Name\" of\n            A():\n                B();\n        end;\n        case WorkflowResponse.\"Function Name\" of\n            C():\n                D();\n        end;",
    );
    assert!(two.unwrap_err().contains("2 `case"), "two cases");
    let shared = region_of(
        "        case WorkflowResponse.\"Function Name\" of\n            A():\n                B(); end; E();",
    );
    assert!(
        shared.unwrap_err().contains("shares source line"),
        "a shared line"
    );
}
