//! #160 step 9 D3: `alsem fingerprint --routine` reads a tree over the view's
//! cone graph, cross-app by default (`--single-app`: the workspace alone).
//! Spec `docs/superpowers/specs/2026-10-24-issue-160-step9-cross-app-prove-fingerprint-design.md`
//! §5, query arms: T3, T5, T6, T7, T8, T13, T14, T22, T25 (call, two
//! boundaries, `--witness 0`; the trigger and event arms are a lib test,
//! `fingerprint_witness_boundary_trigger_and_event`), T26.
//!
//! Every CLI case spawns `CARGO_BIN_EXE_alsem`, so the clap arm and its
//! default are pinned, not only the library pipeline.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::dep_dispatch_fixture as dd;

const VERSION_OVERRIDE: &str = "cli-fq-x-v1";

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
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

/// `fingerprint <ws> --routine <r> --format json [extra]` -> (doc, exit code).
fn query(ws: &Path, routine: &str, extra: &[&str]) -> (Value, i32) {
    let ws = ws.to_string_lossy();
    let mut a = vec!["fingerprint", &ws, "--routine", routine, "--format", "json"];
    a.extend_from_slice(extra);
    let (out, err, code) = alsem(&a);
    (
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}\nstderr: {err}")),
        code,
    )
}

/// `query`, asserting exit 0.
fn query_ok(ws: &Path, routine: &str, extra: &[&str]) -> Value {
    let (v, code) = query(ws, routine, extra);
    assert_eq!(code, 0, "fingerprint --routine {routine} {extra:?}: {v}");
    v
}

/// Every fact of every block, flattened.
fn facts(doc: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    for b in doc["payload"]["blocks"].as_array().into_iter().flatten() {
        for f in b["families"].as_array().into_iter().flatten() {
            for r in f["resources"].as_array().into_iter().flatten() {
                out.extend(r["facts"].as_array().into_iter().flatten().cloned());
            }
        }
    }
    out
}

/// The (0-based) line of the first occurrence of `needle` in `file`.
fn line_of(file: &Path, needle: &str) -> u64 {
    std::fs::read_to_string(file)
        .unwrap()
        .lines()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("{needle} in {file:?}")) as u64
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const CUSTOMER_AL: &str = "table 18 Customer\n{\n    fields\n    {\n        field(1; \"No.\"; Code[20])\n        {\n        }\n    }\n}\n";

/// The `XBase` source dependency of `dd`'s workspace: `Lib.al` = `lib`, plus
/// `extra` sources.
fn write_xbase(dir: &Path, lib: &str, extra: &[(&str, &str)]) {
    let symbols = format!(
        r#"{{"RuntimeVersion":"13.0","Tables":[{{"Id":18,"Name":"Customer","Fields":[{{"Id":1,"Name":"No.","TypeDefinition":{{"Name":"Code"}}}}]}}],"AppId":"{}","Name":"XBase","Publisher":"probe","Version":"1.0.0.0"}}"#,
        dd::DEP_GUID
    );
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
        "",
    );
}

/// T25 (two boundaries on one fact): `CallBoth` calls its own `Mid`, which
/// calls `Lib.Outer()` and `Lib.Outer2()`; both call `Inner`, which inserts
/// `Customer`. The boundaries sit past the first hop: a fact records ONE first
/// hop (`witness_callsite_id`), so two boundaries on the root's own calls
/// would seed one path only.
fn write_two_boundaries(dir: &Path) {
    dd::write_workspace(
        dir,
        "codeunit 50175 \"Ws Both\"\n{\n    procedure CallBoth()\n    begin\n        Mid();\n    end;\n\n    procedure Mid()\n    var\n        Lib: Codeunit \"Dep Lib\";\n    begin\n        Lib.Outer();\n        Lib.Outer2();\n    end;\n}\n",
    );
    let lib = "codeunit 10 \"Dep Lib\"\n{\n    procedure Outer()\n    begin\n        Inner();\n    end;\n\n    procedure Outer2()\n    begin\n        Inner();\n    end;\n\n    procedure Inner()\n    var\n        Cust: Record Customer;\n    begin\n        Cust.Insert();\n    end;\n\n    [IntegrationEvent(false, false)]\n    procedure OnDepEvent()\n    begin\n    end;\n}\n";
    write_xbase(dir, lib, &[]);
}

/// T5/T6: the workspace's `Ws Runner` has an `OnRun` that runs the
/// dependency's `Dep Runner`, whose own `OnRun` inserts `Customer`: the
/// cross-app identities hold two `OnRun` triggers, one per role.
fn write_two_on_runs(dir: &Path) {
    dd::write_workspace(
        dir,
        "codeunit 50176 \"Ws Runner\"\n{\n    trigger OnRun()\n    begin\n        Codeunit.Run(Codeunit::\"Dep Runner\");\n    end;\n}\n",
    );
    let lib = "codeunit 10 \"Dep Lib\"\n{\n    procedure Outer()\n    begin\n    end;\n\n    procedure Inner()\n    begin\n    end;\n\n    [IntegrationEvent(false, false)]\n    procedure OnDepEvent()\n    begin\n    end;\n}\n";
    let runner = "codeunit 14 \"Dep Runner\"\n{\n    trigger OnRun()\n    var\n        Cust: Record Customer;\n    begin\n        Cust.Insert();\n    end;\n}\n";
    write_xbase(dir, lib, &[("src/Runner.al", runner)]);
}

// ---------------------------------------------------------------------------
// T3, T26: inherited facts through dependency code, mayCommit
// ---------------------------------------------------------------------------

/// T3 (b), spec §5: `fingerprint --routine CallChain` on the fixture whose
/// dependency `Inner` modifies `Customer` (18) and commits. Precondition: the
/// `--single-app` output has no fact on the dependency's table 18.
/// Assertion: cross-app, `CallChain` (a primary routine, the one block)
/// inherits a `modify` on that table and a `commit`. DISCRIMINATION: the
/// `--routine` path built single-app regardless of the flag drops the facts.
#[test]
fn fingerprint_routine_inherits_through_dependency() {
    let tmp = tempfile::tempdir().unwrap();
    dd::write_own_modify(tmp.path(), "");
    let table = format!("{}/table/18", dd::DEP_GUID);
    let single = query_ok(tmp.path(), "CallChain", &["--single-app"]);
    assert!(
        facts(&single)
            .iter()
            .all(|f| f["resourceId"] != table.as_str()),
        "precondition: single-app has no fact on {table}: {single}"
    );
    let cross = query_ok(tmp.path(), "CallChain", &[]);
    let blocks = cross["payload"]["blocks"].as_array().unwrap();
    assert_eq!(blocks.len(), 1, "{cross}");
    assert!(
        blocks[0]["routine"]["stableId"]
            .as_str()
            .unwrap()
            .starts_with(dd::WS_GUID),
        "the block's routine is primary: {cross}"
    );
    let fs = facts(&cross);
    assert!(
        fs.iter().any(|f| f["resourceId"] == table.as_str()
            && f["op"] == "modify"
            && f["provenance"] == "inherited"),
        "an inherited modify on {table}: {cross}"
    );
    assert!(
        fs.iter()
            .any(|f| f["op"] == "commit" && f["provenance"] == "inherited"),
        "an inherited commit: {cross}"
    );
}

/// T26, spec §5 (S9-3): `CallChain` → source `Lib.Outer` → symbol-only
/// `Z Deep.Work`. Preconditions: `CallChain`'s direct coverage is
/// `complete`, its inherited coverage is not, and no commit fact is
/// rendered. Assertion: `mayCommit` is `unknown` with inherited facts, and
/// `no` with `--no-include-inherited`. DISCRIMINATION: reading
/// `direct_status` always answers `no` with inherited facts too.
#[test]
fn fingerprint_may_commit_uses_inherited_coverage() {
    let tmp = tempfile::tempdir().unwrap();
    crate::cli_prove_cross_app::write_two_hop(tmp.path());
    let v = query_ok(tmp.path(), "CallChain", &[]);
    let b = &v["payload"]["blocks"][0];
    assert_eq!(
        b["coverage"]["directStatus"], "complete",
        "precondition: {b}"
    );
    assert_ne!(
        b["coverage"]["inheritedStatus"], "complete",
        "precondition: {b}"
    );
    assert!(
        facts(&v).iter().all(|f| f["op"] != "commit"),
        "precondition: no commit fact: {v}"
    );
    assert_eq!(b["mayCommit"]["presence"], "unknown", "{b}");
    let direct = query_ok(tmp.path(), "CallChain", &["--no-include-inherited"]);
    assert_eq!(
        direct["payload"]["blocks"][0]["mayCommit"]["presence"], "no",
        "{direct}"
    );
}

// ---------------------------------------------------------------------------
// T5, T6: the role filters (Decision 28)
// ---------------------------------------------------------------------------

/// The cross-app model of `ws`.
fn cross_model(ws: &Path) -> al_sem::engine::gate::run::AnalysisTarget {
    let inputs = al_sem::program::inputs::InputSet::read(ws, &Default::default());
    al_sem::engine::gate::run::AnalysisTarget::build(&inputs, false, "t").expect("cross-app")
}

/// T5 (d), spec §5: the cross-app identities hold a dependency `OnRun` (run by
/// `Codeunit.Run`) and a primary one (precondition, read off the model).
/// Assertion: `--routine OnRun` renders the primary one, exit 0; the human
/// selector-error arm still prints #239's owner hint, naming only the primary
/// owner. DISCRIMINATION: the selector index built with `None` answers
/// `SelectorAmbiguous`, exit 2.
#[test]
fn fingerprint_selector_primary_only() {
    let tmp = tempfile::tempdir().unwrap();
    write_two_on_runs(tmp.path());
    let t = cross_model(tmp.path());
    let m = t.resolved();
    let roles: Vec<bool> = m
        .workspace
        .routines
        .iter()
        .filter(|r| r.name == "OnRun")
        .map(|r| m.routine_role(r) == al_sem::program::model::role::Role::Primary)
        .collect();
    assert!(
        roles.contains(&true) && roles.contains(&false),
        "precondition: a primary and a dependency OnRun: {roles:?}"
    );
    let v = query_ok(tmp.path(), "OnRun", &[]);
    let blocks = v["payload"]["blocks"].as_array().unwrap();
    assert_eq!(blocks.len(), 1, "{v}");
    assert_eq!(
        blocks[0]["routine"]["objectDisplay"], "Codeunit Ws Runner",
        "{v}"
    );
    // The human selector-error arm: an owner that declares no OnRun.
    let ws = tmp.path().to_string_lossy();
    let (out, err, code) = alsem(&[
        "fingerprint",
        &ws,
        "--routine",
        "Codeunit \"Ws Work\"::OnRun",
        "--format",
        "human",
    ]);
    assert_eq!(code, 2, "{out}{err}");
    assert!(
        err.contains("no routine of that name on the named owner; declared as:")
            && err.contains("Ws Runner")
            && !err.contains("Dep Runner"),
        "#239's hint names the primary owner only: {err}"
    );
}

/// T6 (d), spec §5, T5's fixture: the cross-app model classifies the
/// dependency `OnRun` (precondition, P5). Assertion: `totalClassifications`
/// equals the primary root classifications, counted before the selector.
/// DISCRIMINATION: counting every classification is one higher.
#[test]
fn fingerprint_total_counts_primary_roots() {
    let tmp = tempfile::tempdir().unwrap();
    write_two_on_runs(tmp.path());
    let t = cross_model(tmp.path());
    let m = t.resolved();
    let role_of = |rid: &str| {
        m.workspace
            .routines
            .iter()
            .find(|r| r.id == rid)
            .map(|r| m.routine_role(r))
    };
    let primary = m
        .root_classifications
        .iter()
        .filter(|rc| role_of(&rc.routine_id) == Some(al_sem::program::model::role::Role::Primary))
        .count();
    let dependency_on_run = m.root_classifications.iter().any(|rc| {
        m.workspace.routines.iter().any(|r| {
            r.id == rc.routine_id
                && r.name == "OnRun"
                && m.routine_role(r) == al_sem::program::model::role::Role::Dependency
        })
    });
    assert!(
        dependency_on_run,
        "precondition: the dependency OnRun is classified"
    );
    let v = query_ok(tmp.path(), "OnRun", &[]);
    assert_eq!(
        v["payload"]["summary"]["totalClassifications"].as_u64(),
        Some(primary as u64),
        "{v}"
    );
}

// ---------------------------------------------------------------------------
// T25: the witness stops at the boundary (call, two boundaries, --witness 0)
// ---------------------------------------------------------------------------

/// T25 (call), spec §5: the inherited `Customer` insert of `CallChain` is
/// carried by the dependency's `Inner`. Its witness ends at the boundary into
/// `Lib.Outer`, never at `Inner`: the path's `boundary` is a `call` at
/// `CallChain`'s call site (`cs:` + its id, anchored on the call's line), and
/// the witness's `evidence` is that boundary, with no `operationId` from
/// inside `Inner`. DISCRIMINATION: B1, the adapter's `boundary: None`, makes
/// the witness walk into `Inner`.
#[test]
fn fingerprint_witness_stops_at_boundary_call() {
    let tmp = tempfile::tempdir().unwrap();
    dd::write_own(tmp.path(), "");
    let v = query_ok(tmp.path(), "CallChain", &[]);
    let table = format!("{}/table/18", dd::DEP_GUID);
    let f = facts(&v)
        .into_iter()
        .find(|f| f["resourceId"] == table.as_str() && f["provenance"] == "inherited")
        .unwrap_or_else(|| panic!("precondition: the inherited insert: {v}"));
    let w = &f["witness"];
    let paths = w["paths"].as_array().expect("paths");
    assert_eq!(paths.len(), 1, "{w}");
    let hops = paths[0]["hops"].as_array().unwrap();
    assert!(
        hops.iter().all(|h| h["toDisplay"] != "Inner"),
        "the witness never reaches Inner: {w}"
    );
    let b = &paths[0]["boundary"];
    assert_eq!(b["kind"], "call", "{w}");
    let cs = b["callsiteId"].as_str().expect("callsiteId");
    assert_eq!(b["id"], format!("cs:{cs}"), "{w}");
    assert_eq!(b["anchor"]["file"], "ws:src/Work.al", "{w}");
    assert_eq!(
        b["anchor"]["line"].as_u64(),
        Some(line_of(&tmp.path().join("src/Work.al"), "Lib.Outer();")),
        "anchored on CallChain's call: {w}"
    );
    assert_eq!(&w["evidence"], b, "the evidence is the boundary: {w}");
    assert!(w["evidence"].get("operationId").is_none(), "{w}");
}

/// T25 (two boundaries), spec §5 (O9-24): `CallBoth` reaches `Inner`'s insert
/// through `Mid → Lib.Outer` and through `Mid → Lib.Outer2`: two paths, each
/// with its own `cs:` boundary; the witness's evidence is the first path's;
/// and two runs are byte-equal. The spec's shape of two calls by the root
/// itself gives one path (one first hop per fact), so the calls sit in `Mid`.
/// DISCRIMINATION: B2, one boundary per fact
/// copied to every path, fails the distinct-ids assertion; B4, the evidence
/// picked from an unordered set, fails the evidence or byte-equality
/// assertion (run 5 times).
#[test]
fn fingerprint_witness_two_boundaries_on_one_fact() {
    let tmp = tempfile::tempdir().unwrap();
    write_two_boundaries(tmp.path());
    let (out1, _, c1) = alsem(&[
        "fingerprint",
        &tmp.path().to_string_lossy(),
        "--routine",
        "CallBoth",
        "--format",
        "json",
    ]);
    let (out2, _, c2) = alsem(&[
        "fingerprint",
        &tmp.path().to_string_lossy(),
        "--routine",
        "CallBoth",
        "--format",
        "json",
    ]);
    assert_eq!((c1, c2), (0, 0), "{out1}");
    let v: Value = serde_json::from_str(&out1).unwrap();
    let table = format!("{}/table/18", dd::DEP_GUID);
    let f = facts(&v)
        .into_iter()
        .find(|f| f["resourceId"] == table.as_str() && f["provenance"] == "inherited")
        .unwrap_or_else(|| panic!("precondition: the inherited insert: {v}"));
    let paths = f["witness"]["paths"].as_array().expect("paths").clone();
    assert_eq!(paths.len(), 2, "two paths: {f}");
    let ids: Vec<&str> = paths
        .iter()
        .map(|p| p["boundary"]["id"].as_str().expect("a cs: boundary"))
        .collect();
    assert!(
        ids.iter().all(|i| i.starts_with("cs:")) && ids[0] != ids[1],
        "each path its own call boundary: {ids:?}"
    );
    assert_eq!(
        f["witness"]["evidence"], paths[0]["boundary"],
        "the evidence is the first path's boundary: {f}"
    );
    assert_eq!(out1, out2, "two runs are byte-equal");
}

/// T25 (`--witness 0`), spec §5: no path is reconstructed, so the evidence is
/// today's operation evidence (the fact's operation, here inside `Inner`).
/// DISCRIMINATION: B5, no evidence when `paths` is empty, fails it.
#[test]
fn fingerprint_witness_zero_keeps_operation_evidence() {
    let tmp = tempfile::tempdir().unwrap();
    dd::write_own(tmp.path(), "");
    let v = query_ok(tmp.path(), "CallChain", &["--witness", "0"]);
    let table = format!("{}/table/18", dd::DEP_GUID);
    let f = facts(&v)
        .into_iter()
        .find(|f| f["resourceId"] == table.as_str() && f["provenance"] == "inherited")
        .unwrap_or_else(|| panic!("precondition: the inherited insert: {v}"));
    let w = &f["witness"];
    assert_eq!(w["paths"].as_array().map(Vec::len), Some(0), "{w}");
    assert!(w["evidence"]["operationId"].is_string(), "{w}");
    assert_eq!(w["evidence"]["anchor"]["excerpt"], "Cust.Insert", "{w}");
}

// ---------------------------------------------------------------------------
// T7, T8, T13, T14, T22: parity, the flag, roots config, diagnostics, errors
// ---------------------------------------------------------------------------

/// T7 (query arm, E9), spec §5: over every r0 dir with an `app.json` and no
/// dependency, for every root, `fingerprint --routine <root>` (json and
/// human) is byte-equal cross-app and `--single-app`. In-process through
/// `run_fingerprint_pipeline`, the CLI's one call. DISCRIMINATION: B1, a
/// cross-app-only diagnostic in the envelope, fails it.
#[test]
fn no_dependency_parity_fingerprint_routine() {
    use al_sem::engine::l5::fingerprint_cli::{
        FingerprintFormat, FingerprintOptions, FingerprintOutput, run_fingerprint_pipeline,
    };
    let dirs = crate::cli_prove_cross_app::no_dependency_dirs();
    assert!(dirs.len() >= 180, "precondition: dirs ({})", dirs.len());
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
                    for root in crate::cli_prove_cross_app::root_ids(dir) {
                        for format in [FingerprintFormat::Json, FingerprintFormat::Human] {
                            let run = |single_app: bool| {
                                let opts = FingerprintOptions {
                                    inputs: &inputs,
                                    driver_version: "t",
                                    format: format.clone(),
                                    out: None,
                                    shard: None,
                                    witness_limit: None,
                                    roots: None,
                                    routine_selectors: vec![root.clone()],
                                    include_inherited: true,
                                    is_query_requested: true,
                                    deterministic: true,
                                    strict: false,
                                    verbosity: "compact",
                                    inventory_only: false,
                                    no_roots_config: false,
                                    single_app,
                                };
                                run_fingerprint_pipeline(&opts).map(|r| {
                                    let text = match r.output {
                                        FingerprintOutput::Text(t) => t,
                                        _ => String::from("<binary>"),
                                    };
                                    (text, r.exit_code, r.stderr_diagnostics)
                                })
                            };
                            if run(false) != run(true) {
                                failures
                                    .lock()
                                    .unwrap()
                                    .push(format!("{}: {root} {format:?}", dir.display()));
                            }
                            compared.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                }
            });
        }
    });
    let compared = compared.into_inner();
    assert!(compared > 1000, "precondition: cases compared ({compared})");
    let failures = failures.into_inner().unwrap();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// T8 (query arm), spec §5: on the dependency fixture the universes differ
/// (precondition), and `--single-app` equals the base's default output (a
/// golden minted from the base binary `1134bed1`), apart from the one
/// stated difference of every identity gate in `kind: fingerprint-query`
/// json, the `schemaVersion` `1.2.0` → `1.3.0` line (spec §1 item 2; the
/// fixture's `mayCommit` does not move). DISCRIMINATION: ignoring the flag
/// fails it.
#[test]
fn single_app_flag_reaches_fingerprint_routine() {
    let tmp = tempfile::tempdir().unwrap();
    dd::write_own(tmp.path(), "");
    let ws = tmp.path().to_string_lossy().into_owned();
    let run = |extra: &[&str], format: &str| {
        let mut a = vec![
            "fingerprint",
            ws.as_str(),
            "--routine",
            "CallChain",
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
    for (format, file, version_lines) in [
        ("json", "fingerprint.dd.CallChain.json", 1),
        ("human", "fingerprint.dd.CallChain.human.txt", 0),
    ] {
        let got = run(&["--single-app"], format);
        // Minted from the base binary, so never regenerated.
        let path = repo().join("tests/cli-b-goldens/single-app").join(file);
        let base = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
        let (old, new) = (
            "\"schemaVersion\": \"1.2.0\"",
            "\"schemaVersion\": \"1.3.0\"",
        );
        assert_eq!(
            base.matches(old).count(),
            version_lines,
            "{format}: the stated line"
        );
        let want = base.replace(old, new);
        assert_eq!(got, want, "{format}: --single-app != the base's default");
    }
}

/// T13 (query arm), spec §5: `ws-d51-jobqueue`'s `roots.config.json` gives a
/// classification a config entry (precondition: without the flag the block
/// has a `configEntryId` and the job-queue kind). Assertion: cross-app with
/// `--no-roots-config`, the query reports `rootsConfigIgnored: true` and the
/// AST kinds only. DISCRIMINATION: `build_with` dropping the parameter fails it.
#[test]
fn fingerprint_no_roots_config_cross_app_routine() {
    let ws = repo().join("tests/r0-corpus/ws-d51-jobqueue");
    let with = query_ok(&ws, "PostThenError", &[]);
    let b = &with["payload"]["blocks"][0];
    assert!(
        b["rootKinds"]
            .as_array()
            .is_some_and(|k| k.iter().any(|x| x == "job-queue-entrypoint")),
        "precondition: the configured kind: {b}"
    );
    let v = query_ok(&ws, "PostThenError", &["--no-roots-config"]);
    assert_eq!(v["payload"]["summary"]["rootsConfigIgnored"], true, "{v}");
    let kinds = v["payload"]["blocks"][0]["rootKinds"].clone();
    assert!(
        kinds
            .as_array()
            .is_some_and(|k| k.iter().all(|x| x != "job-queue-entrypoint")),
        "the AST kinds only: {kinds}"
    );
}

/// T14 (query arm), spec §5: `ws-codeunit-rec` with its `CodeunitRecLibA`
/// dependency declared at 2.0.0.0 against the packaged 1.0.0.0. Assertion:
/// the `--routine` json envelope's diagnostics carry the version warning
/// (the json query embeds them, `fingerprint_cli.rs`). Not a universe proof
/// (A9.3 (1)). DISCRIMINATION: dropping the run's diagnostics from the new
/// path's envelope fails it.
#[test]
fn diagnostics_channel_fingerprint_routine() {
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
    let v = query_ok(&ws, "Recalculate", &[]);
    let diags = v["diagnostics"].as_array().cloned().unwrap_or_default();
    assert!(
        diags.iter().any(|d| d["message"]
            .as_str()
            .is_some_and(|m| m.contains("is present only at version 1.0.0.0"))),
        "the version warning in the query envelope: {v}"
    );
}

/// T22 (query arm), spec §5: the r0 dirs with no `app.json` keep their error,
/// `fingerprint: could not compute modelInstanceId`, exit 1, in both
/// universes. DISCRIMINATION: a different failure text on the new path fails it.
#[test]
fn no_app_json_keeps_its_error_fingerprint_routine() {
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
            let mut a = vec!["fingerprint", &ws, "--routine", "Run", "--format", "json"];
            a.extend_from_slice(extra);
            let (_, err, code) = alsem(&a);
            assert_eq!(code, 1, "{a:?}");
            assert_eq!(
                err.trim_end(),
                "fingerprint: could not compute modelInstanceId",
                "{a:?}"
            );
        }
    }
}
