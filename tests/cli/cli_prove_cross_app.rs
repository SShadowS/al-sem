//! #160 step 9 D2: `alsem prove` is digest with one root, on the cross-app
//! view by default (`--single-app`: the workspace alone). Spec
//! `docs/superpowers/specs/2026-10-24-issue-160-step9-cross-app-prove-fingerprint-design.md`
//! §5, prove arms: T1, T7, T8, T12, T14, T15, T19, T22, T24 (T2 and T17 are
//! lib tests in `src/engine/l5/prove.rs`).
//!
//! Every CLI case spawns `CARGO_BIN_EXE_alsem`, so the clap arm and its
//! default are pinned, not only the library pipeline.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::dep_dispatch_fixture as dd;

const VERSION_OVERRIDE: &str = "cli-prove-x-v1";

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// `alsem <args> --deterministic` -> (stdout, stderr, exit code).
fn alsem(args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .arg("--deterministic")
        .env("ALCH_DRIVER_VERSION_OVERRIDE", VERSION_OVERRIDE)
        .output()
        .expect("spawn alsem");
    (
        String::from_utf8_lossy(&out.stdout).replace('\r', ""),
        String::from_utf8_lossy(&out.stderr).replace('\r', ""),
        out.status.code().unwrap_or(-1),
    )
}

fn json(args: &[&str]) -> (Value, i32) {
    let mut a = args.to_vec();
    a.extend(["--format", "json"]);
    let (out, err, code) = alsem(&a);
    (
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}\nstderr: {err}")),
        code,
    )
}

/// `prove <ws> <routine> <question>` as json, cross-app unless `single_app`.
fn prove(ws: &Path, routine: &str, question: &str, single_app: bool) -> Value {
    let ws = ws.to_string_lossy();
    let mut a = vec!["prove", &ws, routine, question];
    if single_app {
        a.push("--single-app");
    }
    let (v, code) = json(&a);
    assert_eq!(code, 0, "prove {routine} {question}: {v}");
    v
}

fn result(v: &Value) -> &Value {
    &v["payload"]["result"]
}

fn answer(v: &Value) -> &str {
    result(v)["answer"].as_str().unwrap_or("")
}

/// The blockers as (kind, detail), sorted.
fn blockers(v: &Value) -> Vec<(String, String)> {
    let mut b: Vec<(String, String)> = result(v)["blockedBy"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|b| {
                    (
                        b["kind"].as_str().unwrap_or("").to_string(),
                        b["detail"].as_str().unwrap_or("").to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    b.sort();
    b
}

fn has_kind(v: &Value, kind: &str) -> bool {
    blockers(v).iter().any(|(k, _)| k == kind)
}

/// `digest <ws> --routine <r>`'s one entry, cross-app.
fn digest_entry(ws: &Path, routine: &str) -> Value {
    let ws = ws.to_string_lossy();
    let (v, code) = json(&["digest", &ws, "--routine", routine]);
    assert_eq!(code, 0, "digest --routine {routine}: {v}");
    v["payload"]["entries"][0].clone()
}

/// The ledger rows (`unresolved`) of a digest entry: (calleeDisplay, status).
fn ledger(entry: &Value) -> Vec<(String, String)> {
    entry["unresolved"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|u| {
                    (
                        u["calleeDisplay"].as_str().unwrap_or("").to_string(),
                        u["status"].as_str().unwrap_or("").to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const CUSTOMER_AL: &str = "table 18 Customer\n{\n    fields\n    {\n        field(1; \"No.\"; Code[20])\n        {\n        }\n    }\n}\n";
const ZDEEP_GUID: &str = "cccc7070-0000-0000-0000-000000000070";
/// `Dep Lib`'s members `dd::WS_AL` calls, with no body content.
const LIB_STUBS: &str = "    procedure Inner()\n    begin\n    end;\n\n    [IntegrationEvent(false, false)]\n    procedure OnDepEvent()\n    begin\n    end;\n";

/// The `XBase` source dependency of `dd`'s workspace: `Lib.al` = `lib`, extra
/// source files `extra`, symbol tables `tables` (JSON objects), depending on
/// `ZDeep` when `deep`.
fn write_xbase(dir: &Path, lib: &str, extra: &[(&str, &str)], tables: &str, deep: bool) {
    let symbols = format!(
        r#"{{"RuntimeVersion":"13.0","Tables":[{{"Id":18,"Name":"Customer","Fields":[{{"Id":1,"Name":"No.","TypeDefinition":{{"Name":"Code"}}}}]}}{tables}],"AppId":"{}","Name":"XBase","Publisher":"probe","Version":"1.0.0.0"}}"#,
        dd::DEP_GUID
    );
    let manifest = if deep {
        format!(
            r#"<Dependencies><Dependency Id="{ZDEEP_GUID}" Name="ZDeep" Publisher="probe" MinVersion="1.0.0.0" /></Dependencies>"#
        )
    } else {
        String::new()
    };
    let mut sources: Vec<(&str, &str)> =
        vec![("src/Customer.al", CUSTOMER_AL), ("src/Lib.al", lib)];
    sources.extend_from_slice(extra);
    crate::symbol_app::write_source_app(
        &dir.join(".alpackages").join(dd::DEP_APP_FILE),
        dd::DEP_GUID,
        "XBase",
        "1.0.0.0",
        &symbols,
        &sources,
        &manifest,
    );
}

/// `ZDeep`: a symbol-only app with codeunit 20 "Z Deep", method `Work()`.
fn write_zdeep(dir: &Path) {
    let deep_symbols = format!(
        r#"{{"RuntimeVersion":"13.0","Codeunits":[{{"Id":20,"Name":"Z Deep","Methods":[{{"Name":"Work","Parameters":[]}}]}}],"AppId":"{ZDEEP_GUID}","Name":"ZDeep","Publisher":"probe","Version":"1.0.0.0"}}"#
    );
    crate::symbol_app::write_symbol_app(
        &dir.join(".alpackages/probe_ZDeep_1.0.0.0.app"),
        ZDEEP_GUID,
        "ZDeep",
        "1.0.0.0",
        &deep_symbols,
    );
}

/// T1 (a): `CallChain` → source `Lib.Outer` (a body, no COMMIT) →
/// symbol-only `Z Deep.Work`.
fn write_two_hop(dir: &Path) {
    dd::write_workspace(dir, "");
    let lib = format!(
        "codeunit 10 \"Dep Lib\"\n{{\n    procedure Outer()\n    var\n        Deep: Codeunit \"Z Deep\";\n    begin\n        Deep.Work();\n    end;\n\n{LIB_STUBS}}}\n"
    );
    write_xbase(dir, &lib, &[], "", true);
    write_zdeep(dir);
}

/// T12: `CallAbi` → `Z Deep.Missing()`, a member the symbol-only `ZDeep`
/// does not declare: an ABI-only target with no model row.
fn write_abi_only(dir: &Path) {
    dd::write_workspace(
        dir,
        "codeunit 50173 \"Ws Abi\"\n{\n    procedure CallAbi()\n    var\n        Deep: Codeunit \"Z Deep\";\n    begin\n        Deep.Missing();\n    end;\n}\n",
    );
    let lib = format!(
        "codeunit 10 \"Dep Lib\"\n{{\n    procedure Outer()\n    begin\n    end;\n\n{LIB_STUBS}}}\n"
    );
    write_xbase(dir, &lib, &[], "", true);
    write_zdeep(dir);
    // The workspace depends on ZDeep directly.
    write(
        &dir.join("app.json"),
        &format!(
            r#"{{"id":"{}","name":"Ws70","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50100,"to":50199}}],"dependencies":[{{"id":"{}","name":"XBase","publisher":"probe","version":"1.0.0.0"}},{{"id":"{ZDEEP_GUID}","name":"ZDeep","publisher":"probe","version":"1.0.0.0"}}]}}"#,
            dd::WS_GUID,
            dd::DEP_GUID
        ),
    );
}

/// T24: `CallW` → `Dep W.W`, which writes 11 dependency tables `T101` …
/// `T111` (resource ids sort in number order: three digits each).
fn write_eleven_tables(dir: &Path) {
    dd::write_workspace(
        dir,
        "codeunit 50174 \"Ws W\"\n{\n    procedure CallW()\n    var\n        W: Codeunit \"Dep W\";\n    begin\n        W.W();\n    end;\n}\n",
    );
    let lib = format!(
        "codeunit 10 \"Dep Lib\"\n{{\n    procedure Outer()\n    begin\n    end;\n\n{LIB_STUBS}}}\n"
    );
    let mut tables_al = String::new();
    let mut tables_sym = String::new();
    let mut vars = String::new();
    let mut writes = String::new();
    for n in 101..=111 {
        tables_al.push_str(&format!(
            "table {n} T{n}\n{{\n    fields\n    {{\n        field(1; \"No.\"; Code[20])\n        {{\n        }}\n    }}\n}}\n\n"
        ));
        tables_sym.push_str(&format!(
            r#",{{"Id":{n},"Name":"T{n}","Fields":[{{"Id":1,"Name":"No.","TypeDefinition":{{"Name":"Code"}}}}]}}"#
        ));
        vars.push_str(&format!("        R{n}: Record T{n};\n"));
        writes.push_str(&format!("        R{n}.Insert();\n"));
    }
    let w = format!(
        "codeunit 13 \"Dep W\"\n{{\n    procedure W()\n    var\n{vars}    begin\n{writes}    end;\n}}\n"
    );
    write_xbase(
        dir,
        &lib,
        &[("src/Tables.al", &tables_al), ("src/W.al", &w)],
        &tables_sym,
        false,
    );
}

/// T15: a workspace (no dependency) whose `Chain` codeunit holds a call chain
/// `P0 → P1 → … → P{n-1}`.
fn write_chain(dir: &Path, n: usize) {
    write(
        &dir.join("app.json"),
        r#"{"id":"aaaa9915-0000-0000-0000-000000000915","name":"Chain","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{"from":50100,"to":50199}]}"#,
    );
    let mut al = String::from("codeunit 50100 Chain\n{\n");
    for i in 0..n {
        let body = if i + 1 < n {
            format!("        P{}();\n", i + 1)
        } else {
            String::new()
        };
        al.push_str(&format!(
            "    procedure P{i}()\n    begin\n{body}    end;\n\n"
        ));
    }
    al.push_str("}\n");
    write(&dir.join("src/Chain.al"), &al);
}

// ---------------------------------------------------------------------------
// T1, T12, T19, T24: the cross-app answer
// ---------------------------------------------------------------------------

/// T1 (a), spec §5: `prove CallChain may-commit`, `CallChain` → source
/// `Lib.Outer` → symbol-only `Z Deep.Work`. Preconditions: `Outer` has a body
/// and no COMMIT in its cone (else `yes` comes before any obligation), and the
/// `Outer → Work` ledger row is `unfetched-dependency`. Assertion: `unknown`,
/// blocked by an analysis gap, incomplete coverage and the `Outer → Work`
/// callsite. DISCRIMINATION: B1, `&[]` as the tree's dependency calls in
/// `prepare`, drops the `Outer → Work` row; B2, `gaps_in_cone` forced 0 drops
/// the gap.
#[test]
fn prove_two_hop_not_proven() {
    let tmp = tempfile::tempdir().unwrap();
    write_two_hop(tmp.path());
    let entry = digest_entry(tmp.path(), "CallChain");
    assert!(
        entry["effects"]
            .as_array()
            .is_some_and(|e| e.iter().all(|f| f["type"] != "COMMIT")),
        "precondition: no COMMIT in the cone: {entry}"
    );
    assert_eq!(
        ledger(&entry),
        [("Deep.Work".to_string(), "unfetched-dependency".to_string())],
        "precondition: the Outer → Work row"
    );
    let v = prove(tmp.path(), "CallChain", "may-commit", false);
    assert_eq!(answer(&v), "unknown", "{v}");
    assert!(has_kind(&v, "analysis-gap"), "analysis-gap: {v}");
    assert!(
        has_kind(&v, "coverage-incomplete"),
        "coverage-incomplete: {v}"
    );
    assert!(
        blockers(&v).contains(&(
            "unresolved-callsite".to_string(),
            "Deep.Work in Outer".to_string()
        )),
        "the Outer → Work callsite: {v}"
    );
}

/// T12, spec §5: a call to a member the symbol-only app does not declare.
/// Preconditions: its ledger row is `external` and `CallAbi`'s coverage
/// reasons hold `external-target`. Assertion: `unknown`, blocked by the
/// callsite and incomplete coverage. DISCRIMINATION: skipping `external`
/// rows in `unresolved_cone`'s harvest drops the callsite blocker (the answer
/// stays `unknown` through coverage).
#[test]
fn abi_only_target_blocks_prove() {
    let tmp = tempfile::tempdir().unwrap();
    write_abi_only(tmp.path());
    let entry = digest_entry(tmp.path(), "CallAbi");
    assert_eq!(
        ledger(&entry),
        [("Deep.Missing".to_string(), "external".to_string())],
        "precondition: the row is external"
    );
    assert!(
        entry["coverage"]["reasons"]
            .as_array()
            .is_some_and(|r| r.iter().any(|x| x == "external-target")),
        "precondition: external-target coverage: {entry}"
    );
    let v = prove(tmp.path(), "CallAbi", "may-commit", false);
    assert_eq!(answer(&v), "unknown", "{v}");
    assert!(
        blockers(&v).contains(&(
            "unresolved-callsite".to_string(),
            "Deep.Missing in CallAbi".to_string()
        )),
        "the callsite blocker: {v}"
    );
    assert!(has_kind(&v, "coverage-incomplete"), "{v}");
}

/// T19, spec §5 (OD-3, rule B): `CallChain` calls `Lib.Outer` with no guard
/// and `Outer`'s `Inner` commits with no guard. Preconditions: no gap, full
/// coverage, and a COMMIT reached (as a boundary group). Assertion:
/// `commits-on-success-path` is `unknown` cross-app: a group's
/// conditionality is `unknown`, never a must-guarantee from inside the
/// dependency. DISCRIMINATION: giving boundary groups their computed path
/// conditionality (`digest.rs`) answers `yes`.
#[test]
fn prove_commits_on_success_path_through_boundary_is_unknown() {
    let tmp = tempfile::tempdir().unwrap();
    dd::write_own(tmp.path(), "");
    let v = prove(tmp.path(), "CallChain", "commits-on-success-path", false);
    let o = &result(&v)["obligations"];
    assert_eq!(o["analysisGaps"], 0, "precondition: no gap: {v}");
    assert_eq!(o["coverageComplete"], true, "precondition: coverage: {v}");
    assert_eq!(o["unresolvedCallsites"], 0, "precondition: {v}");
    let ev = result(&v)["evidence"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        ev.iter()
            .any(|e| e["type"] == "COMMIT" && e["detail"]["boundary"] == "XBase.Outer"),
        "precondition: the commit is reached through the boundary: {v}"
    );
    assert_eq!(answer(&v), "unknown", "{v}");
}

/// T24, spec §5 (S9-1): `CallW` reaches, only through `Dep W.W`, writes to
/// 11 dependency tables; the target `T111` sorts 11th by resource id, so the
/// group's `detail.resources` (10 listed) does not name it. Assertion:
/// `writes-table:T111` is `yes` with that group as the evidence.
/// DISCRIMINATION: matching a group on `detail` only answers `unknown`.
#[test]
fn prove_matches_a_group_resource_beyond_ten() {
    let tmp = tempfile::tempdir().unwrap();
    write_eleven_tables(tmp.path());
    let entry = digest_entry(tmp.path(), "CallW");
    let groups: Vec<&Value> = entry["effects"]
        .as_array()
        .expect("effects")
        .iter()
        .filter(|e| e["detail"]["boundary"].is_string())
        .collect();
    assert_eq!(groups.len(), 1, "precondition: one boundary group: {entry}");
    let g = groups[0];
    assert_eq!(g["detail"]["resourceCount"], "11", "precondition: {g}");
    let listed = g["detail"]["resources"].as_str().unwrap();
    assert_eq!(
        listed.split(", ").count(),
        10,
        "precondition: 10 listed: {g}"
    );
    assert!(
        listed.contains("T110") && !listed.contains("T111"),
        "precondition: T111 is the 11th: {listed}"
    );
    let v = prove(tmp.path(), "CallW", "writes-table:T111", false);
    assert_eq!(answer(&v), "yes", "{v}");
    let ev = result(&v)["evidence"].as_array().expect("evidence");
    assert_eq!(ev.len(), 1, "{v}");
    assert_eq!(ev[0]["detail"]["boundary"], "XBase.W", "the group: {v}");
}

// ---------------------------------------------------------------------------
// T15: truncation reaches the answer
// ---------------------------------------------------------------------------

/// T15, spec §5: a 66-routine chain is longer than the cone's depth cap, so
/// the cone is truncated and prove answers `unknown` with a `cone-truncated`
/// blocker (no COMMIT anywhere: an untruncated cone answers `no`).
/// Precondition: the chain is longer than the cap digest reports
/// (`unresolvedTraversal.maxDepth`, `DEFAULT_MAX_DEPTH`). DISCRIMINATION:
/// `cone_truncated: false` in place of the traversal's flag in prove's
/// obligations answers `no`.
#[test]
fn prove_cone_truncation_blocks() {
    let n = 66;
    let tmp = tempfile::tempdir().unwrap();
    write_chain(tmp.path(), n);
    let entry = digest_entry(tmp.path(), "P0");
    let cap = entry["unresolvedTraversal"]["maxDepth"]
        .as_u64()
        .expect("maxDepth");
    assert!(n as u64 > cap, "precondition: {n} routines > the cap {cap}");
    assert_eq!(
        entry["unresolvedTraversal"]["truncated"], true,
        "precondition: {entry}"
    );
    let v = prove(tmp.path(), "P0", "may-commit", false);
    assert_eq!(result(&v)["obligations"]["coneTruncated"], true, "{v}");
    assert_eq!(answer(&v), "unknown", "{v}");
    assert!(has_kind(&v, "cone-truncated"), "{v}");
}

// ---------------------------------------------------------------------------
// T14: the run's diagnostics reach the envelope
// ---------------------------------------------------------------------------

/// T14 (prove arm), spec §5: `ws-codeunit-rec` with its `CodeunitRecLibA`
/// dependency declared at 2.0.0.0 against the packaged 1.0.0.0 (the
/// `gate/diff/cli.rs` test's fixture). Precondition: the version warning
/// exists (in either universe's envelope). Assertion: the cross-app prove
/// envelope's `diagnostics` carry it. Not a universe proof (A9.3 (1)).
/// DISCRIMINATION: dropping the run's diagnostics from the envelope fails.
#[test]
fn diagnostics_channel_prove() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws-codeunit-rec");
    crate::copy_dir(&repo().join("tests/r0-corpus/ws-codeunit-rec"), &ws);
    let app_json = ws.join("app.json");
    let mut app: Value =
        serde_json::from_str(&std::fs::read_to_string(&app_json).unwrap()).unwrap();
    let dep = app["dependencies"]
        .as_array_mut()
        .expect("dependencies")
        .iter_mut()
        .find(|d| d["name"] == "CodeunitRecLibA")
        .expect("CodeunitRecLibA dependency");
    dep["version"] = Value::from("2.0.0.0");
    std::fs::write(&app_json, app.to_string()).unwrap();
    let want = "is present only at version 1.0.0.0";
    let v = prove(&ws, "Recalculate", "may-commit", false);
    let diags = v["diagnostics"].as_array().cloned().unwrap_or_default();
    assert!(
        diags
            .iter()
            .any(|d| d["message"].as_str().is_some_and(|m| m.contains(want))),
        "the version warning in prove's envelope: {v}"
    );
}

// ---------------------------------------------------------------------------
// T7, T8, T22: parity, the flag, the no-app.json error
// ---------------------------------------------------------------------------

/// Every r0 dir (recursive) with an `app.json`, no `.alpackages` and no
/// declared dependency.
fn no_dependency_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = walkdir::WalkDir::new(repo().join("tests/r0-corpus"))
        .into_iter()
        .map(|e| e.unwrap().into_path())
        .filter(|p| p.join("app.json").is_file() && !p.join(".alpackages").exists())
        .filter(|p| {
            let app: Value =
                serde_json::from_str(&std::fs::read_to_string(p.join("app.json")).unwrap())
                    .unwrap_or(Value::Null);
            app["dependencies"].as_array().is_none_or(|d| d.is_empty())
        })
        .collect();
    dirs.sort();
    dirs
}

/// The root classifications' routine ids of `ws`, from the single-app model.
fn root_ids(ws: &Path) -> Vec<String> {
    let inputs = al_sem::program::inputs::InputSet::read(ws, &Default::default());
    let Ok(t) = al_sem::engine::gate::run::AnalysisTarget::build(&inputs, true, "t") else {
        return vec![];
    };
    let m = t.resolved();
    let mut ids: Vec<String> = m
        .root_classifications
        .iter()
        .filter_map(|rc| m.workspace.routines.iter().find(|r| r.id == rc.routine_id))
        .map(|r| r.stable_routine_id.clone())
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// T7 (prove arm, E9), spec §5: over every r0 dir with an `app.json` and no
/// dependency, for every root (`may-commit`), prove's cross-app output
/// equals its `--single-app` output byte for byte (json and human, exit
/// code). In-process through `run_prove_pipeline`, the CLI's one call.
/// DISCRIMINATION: B1, a cross-app-only diagnostic line appended to the
/// envelope, fails it.
#[test]
fn no_dependency_parity_prove() {
    let dirs = no_dependency_dirs();
    assert!(
        dirs.len() >= 180,
        "precondition: no-dependency dirs ({})",
        dirs.len()
    );
    let failures = std::sync::Mutex::new(Vec::<String>::new());
    let compared = std::sync::atomic::AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
    let chunks: Vec<&[PathBuf]> = dirs.chunks(dirs.len().div_ceil(threads)).collect();
    std::thread::scope(|s| {
        for chunk in chunks {
            let (failures, compared) = (&failures, &compared);
            s.spawn(move || {
                for dir in chunk {
                    let inputs =
                        al_sem::program::inputs::InputSet::read(dir, &crate::fingerprint_needs());
                    for root in root_ids(dir) {
                        let run = |single: bool| {
                            al_sem::engine::l5::prove::run_prove_pipeline(
                                &inputs,
                                &root,
                                "may-commit",
                                "t",
                                true,
                                single,
                            )
                            .map(|r| (r.json_text, r.human_text, r.exit_code))
                        };
                        if run(false) != run(true) {
                            failures
                                .lock()
                                .unwrap()
                                .push(format!("{}: {root}", dir.display()));
                        }
                        compared.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            });
        }
    });
    let compared = compared.into_inner();
    assert!(compared > 500, "precondition: cases compared ({compared})");
    let failures = failures.into_inner().unwrap();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The T8 goldens: minted from the base binary (`1134bed1`, default flags =
/// the workspace alone) on the fixture `dd::write_own` writes.
fn t8_golden(name: &str) -> PathBuf {
    repo().join("tests/cli-b-goldens/single-app").join(name)
}

/// T8 (prove arm), spec §5: on the dependency fixture the cross-app output
/// differs from `--single-app` (precondition), and `--single-app` equals the
/// base's default output byte for byte (json and human). DISCRIMINATION:
/// ignoring the flag (always cross-app) fails the golden comparison.
#[test]
fn single_app_flag_reaches_prove() {
    let tmp = tempfile::tempdir().unwrap();
    dd::write_own(tmp.path(), "");
    let ws = tmp.path().to_string_lossy().into_owned();
    let run = |extra: &[&str], format: &str| {
        let mut a = vec![
            "prove",
            ws.as_str(),
            "CallChain",
            "may-commit",
            "--format",
            format,
        ];
        a.extend_from_slice(extra);
        let (out, err, code) = alsem(&a);
        assert_eq!(code, 0, "{a:?}: {err}");
        out
    };
    assert_ne!(
        run(&[], "json"),
        run(&["--single-app"], "json"),
        "precondition: the universes differ on the fixture"
    );
    for (format, file) in [
        ("json", "prove.dd.CallChain.may-commit.json"),
        ("human", "prove.dd.CallChain.may-commit.human.txt"),
    ] {
        let got = run(&["--single-app"], format);
        let path = t8_golden(file);
        if crate::regen::regen_mode() {
            panic!("T8 goldens are minted from the base binary, never regenerated: {path:?}");
        }
        let want = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
        assert_eq!(got, want, "{format}: --single-app != the base's default");
    }
}

/// T22 (prove arm), spec §5: the r0 dirs with no `app.json` keep their
/// error, `prove: could not compute modelInstanceId`, exit 1, in both
/// universes. DISCRIMINATION: building the target from a dummy model id
/// changes the line.
#[test]
fn no_app_json_keeps_its_error_prove() {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(repo().join("tests/r0-corpus"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir() && !p.join("app.json").exists())
        .collect();
    dirs.sort();
    assert_eq!(dirs.len(), 9, "precondition: the 9 dirs: {dirs:?}");
    for dir in &dirs {
        let ws = dir.to_string_lossy();
        for extra in [&[][..], &["--single-app"][..]] {
            let mut a = vec!["prove", &ws, "Run", "may-commit"];
            a.extend_from_slice(extra);
            let (_, err, code) = alsem(&a);
            assert_eq!(code, 1, "{a:?}");
            assert_eq!(
                err.trim_end(),
                "prove: could not compute modelInstanceId",
                "{a:?}"
            );
        }
    }
}
