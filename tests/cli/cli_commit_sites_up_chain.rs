//! Commit sites are not dropped going up the call chain (fact 1 of the
//! 2026-11-05 MCP output review).
//!
//! On Continia DO, `CDO File::CalcSalesDisc` (a local table procedure) runs
//! `Sales-Calc. Discount` and then commits, inside an `if`. From
//! `CalcSalesDisc`, `prove may-commit` listed both commits; from
//! `RunReportID`, seven hops up, it listed neither: a nearer `Commit()` reached
//! through another callee was the only row, and `explain` printed
//! "1 site in workspace code; 0 commit-bearing entry points" as an exact count.
//! The cone keeps ONE inherited fact per key, and for a COMMIT that is one per
//! root (the nearest); its witness reached only the nearest carrier. The digest
//! now lists every commit site the root reaches, and every crossing into
//! dependency code behind which a commit lies (`digest::SITE_ENUMERATED`).
//!
//! The minimized shape: `Root` calls `Near` (commits, one hop) and `Middle`,
//! which calls the table's `CreateDocument`, which calls the local
//! `CalcDisc`, which runs the dependency codeunit `Dep Calc` (its `OnRun`
//! commits) and then commits under an `if`.
//!
//! With one row per site, a site is also judged by itself under literal
//! arguments (`literal_reach::unreachable_effects`'s `per_site`), and
//! `commits-on-success-path` ignores a COMMIT the root cannot reach, as the
//! MAY questions do (G23): the `Lit Mgt` workspace below.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

const DEP_GUID: &str = "c0c00001-0000-4000-8000-0000000000c1";

const DEP_SYMBOLS: &str = r#"{"RuntimeVersion":"13.0","Codeunits":[{"Id":50400,"Name":"Dep Calc","Methods":[]}],"AppId":"c0c00001-0000-4000-8000-0000000000c1","Name":"DepCalc","Publisher":"probe","Version":"1.0.0.0"}"#;

const DEP_SOURCE: &str = r#"codeunit 50400 "Dep Calc"
{
    trigger OnRun()
    begin
        Commit();
    end;
}
"#;

/// Line 20 is `CalcDisc`'s `Codeunit.Run`, line 21 its `Commit()`.
const WS_TABLE: &str = r#"table 50410 "Chain Doc"
{
    fields
    {
        field(1; "No."; Code[20]) { }
    }
    keys
    {
        key(PK; "No.") { Clustered = true; }
    }

    procedure CreateDocument()
    begin
        CalcDisc();
    end;

    local procedure CalcDisc()
    begin
        if Rec."No." <> '' then begin
            Codeunit.Run(Codeunit::"Dep Calc");
            Commit();
        end;
    end;
}
"#;

/// Line 11 is `Near`'s `Commit()`.
const WS_CODEUNIT: &str = r#"codeunit 50411 "Chain Mgt"
{
    procedure Root()
    begin
        Near();
        Middle();
    end;

    local procedure Near()
    begin
        Commit();
    end;

    local procedure Middle()
    var
        Doc: Record "Chain Doc";
    begin
        Doc.CreateDocument();
    end;
}
"#;

fn workspace(root: &Path) -> std::path::PathBuf {
    let ws = root.join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        format!(
            r#"{{"id":"c0c00002-0000-4000-8000-0000000000c2","name":"Chain","publisher":"t","version":"1.0.0.0",
            "dependencies":[{{"id":"{DEP_GUID}","name":"DepCalc","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    )
    .unwrap();
    std::fs::write(ws.join("src/ChainDoc.Table.al"), WS_TABLE).unwrap();
    std::fs::write(ws.join("src/ChainMgt.Codeunit.al"), WS_CODEUNIT).unwrap();
    crate::symbol_app::write_source_app(
        &ws.join(".alpackages/probe_DepCalc_1.0.0.0.app"),
        DEP_GUID,
        "DepCalc",
        "1.0.0.0",
        DEP_SYMBOLS,
        &[("src/DepCalc.Codeunit.al", DEP_SOURCE)],
        "",
    );
    ws
}

fn alsem(args: &[&str]) -> (Option<i32>, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

/// A COMMIT row's place: `file:line` of a site, or `beyond <boundary>`.
fn place(e: &Value) -> String {
    match e["detail"]["boundary"].as_str() {
        Some(b) => format!("beyond {b}"),
        None => format!(
            "{}:{}",
            e["evidence"]["file"].as_str().unwrap_or("?"),
            e["evidence"]["line"]
        ),
    }
}

fn commit_places(ws: &Path, routine: &str) -> Vec<String> {
    let (code, stdout) = alsem(&["prove", ws.to_str().unwrap(), routine, "may-commit"]);
    assert_eq!(code, Some(0), "prove {routine}: {stdout}");
    let v: Value = serde_json::from_str(&stdout).expect("prove JSON");
    assert_eq!(v["payload"]["result"]["answer"], "yes", "{v}");
    let mut places: Vec<String> = v["payload"]["result"]["evidence"]
        .as_array()
        .expect("evidence")
        .iter()
        .filter(|e| e["type"] == "COMMIT")
        .map(place)
        .collect();
    places.sort();
    places
}

const CALC_DISC_COMMIT: &str = "ws:src/ChainDoc.Table.al:21";
const NEAR_COMMIT: &str = "ws:src/ChainMgt.Codeunit.al:11";
const BEYOND_DEP: &str = "beyond Codeunit Dep Calc::OnRun";

/// The precondition: from the table procedure itself, both of its commits.
#[test]
fn the_table_procedure_lists_its_own_commit_and_the_dependency_one() {
    let t = tempfile::tempdir().unwrap();
    let ws = workspace(t.path());
    assert_eq!(
        commit_places(&ws, "Chain Doc::CalcDisc"),
        vec![BEYOND_DEP.to_string(), CALC_DISC_COMMIT.to_string()]
    );
}

/// The bug: two hops above `CalcDisc` (three from `Root`), with a nearer
/// commit through `Near`, every commit is still a row.
#[test]
fn a_root_above_a_nearer_commit_lists_every_commit_site() {
    let t = tempfile::tempdir().unwrap();
    let ws = workspace(t.path());
    let all = vec![
        BEYOND_DEP.to_string(),
        CALC_DISC_COMMIT.to_string(),
        NEAR_COMMIT.to_string(),
    ];
    assert_eq!(commit_places(&ws, "Chain Mgt::Root"), all);
    // Two hops above the table procedure, with no nearer commit.
    assert_eq!(
        commit_places(&ws, "Chain Mgt::Middle"),
        vec![BEYOND_DEP.to_string(), CALC_DISC_COMMIT.to_string()]
    );
}

/// `explain`'s counts agree with the rows, and claim no more than they know.
#[test]
fn explain_counts_every_commit_site_and_entry_point() {
    let t = tempfile::tempdir().unwrap();
    let ws = workspace(t.path());
    let (code, stdout) = alsem(&[
        "explain",
        ws.to_str().unwrap(),
        "--routine",
        "Chain Mgt::Root",
    ]);
    assert_eq!(code, Some(0), "{stdout}");
    // `yes`: `Near` commits unconditionally.
    assert!(
        stdout.contains("commits: yes (2 sites, 1 commit-bearing entry point)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("commits: yes — 2 sites in workspace code; 1 commit-bearing entry point:"),
        "{stdout}"
    );
    assert!(
        stdout.contains("COMMIT at ws:src/ChainDoc.Table.al:21 (conditional)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("COMMIT at ws:src/ChainMgt.Codeunit.al:11"),
        "{stdout}"
    );
    assert!(
        stdout.contains("Codeunit Dep Calc::OnRun, entered at ws:src/ChainDoc.Table.al:20"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("at least"),
        "a finished search is exact: {stdout}"
    );
}

/// `Loop` is self-recursive, so the cone keeps its guarded `Commit()` (a
/// recursive SCC has nowhere to keep the requirement); only the literal pass
/// sees that both roots pass `false`. Line 16 is `Near`'s `Commit()`, line 22
/// `Loop`'s.
const LIT_CODEUNIT: &str = r#"codeunit 50420 "Lit Mgt"
{
    procedure RootDead()
    begin
        Loop(false, 0);
    end;

    procedure RootMixed()
    begin
        Near();
        Loop(false, 0);
    end;

    local procedure Near()
    begin
        Commit();
    end;

    local procedure Loop(DoCommit: Boolean; Depth: Integer)
    begin
        if DoCommit then
            Commit();
        if Depth < 3 then
            Loop(DoCommit, Depth + 1);
    end;
}
"#;

fn lit_workspace(root: &Path) -> std::path::PathBuf {
    let ws = root.join("lit");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"c0c00003-0000-4000-8000-0000000000c3","name":"Lit","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/LitMgt.Codeunit.al"), LIT_CODEUNIT).unwrap();
    ws
}

/// A site is judged by its own operation: `RootMixed` reaches two commits,
/// and the one in `Loop` cannot run (`false` reaches `if DoCommit`). Judged
/// by its kind, it was live because `Near`'s is.
#[test]
fn a_literal_dead_site_is_not_counted_beside_a_live_one() {
    let t = tempfile::tempdir().unwrap();
    let ws = lit_workspace(t.path());
    let (code, stdout) = alsem(&[
        "explain",
        ws.to_str().unwrap(),
        "--routine",
        "Lit Mgt::RootMixed",
    ]);
    assert_eq!(code, Some(0), "{stdout}");
    assert!(
        stdout.contains("commits: yes — 1 site in workspace code; 0 commit-bearing entry points:"),
        "{stdout}"
    );
    assert!(
        stdout.contains("COMMIT at ws:src/LitMgt.Codeunit.al:16"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("COMMIT at ws:src/LitMgt.Codeunit.al:22"),
        "{stdout}"
    );
}

/// `commits-on-success-path` counts no COMMIT the root cannot reach under
/// literal arguments, as `may-commit` does not: both answer `no`.
#[test]
fn commits_on_success_path_ignores_a_literal_dead_commit() {
    let t = tempfile::tempdir().unwrap();
    let ws = lit_workspace(t.path());
    for question in ["may-commit", "commits-on-success-path"] {
        let (code, stdout) = alsem(&["prove", ws.to_str().unwrap(), "Lit Mgt::RootDead", question]);
        let v: Value = serde_json::from_str(&stdout).expect("prove JSON");
        assert_eq!(v["payload"]["result"]["answer"], "no", "{question}: {v}");
        assert_eq!(code, Some(0), "{question}: {stdout}");
    }
}

/// A commit deeper than the site search's depth bound (64 calls) is not
/// listed, so the counts say "at least": `Root` calls `Near` (commits) and a
/// chain of 70 procedures whose last one commits.
#[test]
fn a_site_beyond_the_depth_bound_makes_the_counts_a_lower_bound() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("deep");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"c0c00004-0000-4000-8000-0000000000c4","name":"Deep","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    let mut src = String::from(
        "codeunit 50430 \"Deep Mgt\"\n{\n    procedure Root()\n    begin\n        Near();\n        P0();\n    end;\n\n    local procedure Near()\n    begin\n        Commit();\n    end;\n",
    );
    for i in 0..70 {
        let body = if i == 69 {
            "Commit();".to_string()
        } else {
            format!("P{}();", i + 1)
        };
        src.push_str(&format!(
            "\n    local procedure P{i}()\n    begin\n        {body}\n    end;\n"
        ));
    }
    src.push_str("}\n");
    std::fs::write(ws.join("src/DeepMgt.Codeunit.al"), src).unwrap();
    let (code, stdout) = alsem(&[
        "explain",
        ws.to_str().unwrap(),
        "--routine",
        "Deep Mgt::Root",
    ]);
    assert_eq!(code, Some(0), "{stdout}");
    assert!(
        stdout.contains("commits: yes (at least 1 site)"),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "commits: yes — at least 1 site in workspace code; at least 0 commit-bearing entry points:"
        ),
        "{stdout}"
    );
}

/// The reported case on the CDO baseline (skipped unless `CDO_WS` names it;
/// run by `scripts/cdo-gate`): from `RunReportID`, `CalcSalesDisc`'s own
/// `Commit()` (`CDOFile.Table.al:128`) and the COMMIT beyond `Sales-Calc.
/// Discount::OnRun` (entered at `:125`) are rows beside the nearer one at
/// `CDOEMailTemplateManagement.Codeunit.al:1241`. Before the fix only `:1241`
/// was.
#[test]
fn cdo_run_report_id_lists_calc_sales_disc_commits() {
    let Some(ws) = crate::cdo::cdo_ws_or_enforce() else {
        return;
    };
    let places = commit_places(&ws, "CDO E-Mail Handler::RunReportID");
    for want in [
        "ws:src/Foundation/CDOFile.Table.al:128",
        "ws:src/Features/Email/Templates/CDOEMailTemplateManagement.Codeunit.al:1241",
        "beyond Codeunit Sales-Calc. Discount::OnRun",
    ] {
        assert!(
            places.iter().any(|p| p == want),
            "{want} missing: {places:?}"
        );
    }
}
