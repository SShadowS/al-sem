//! #66: `alsem digest` and `alsem query` load the workspace's dependencies by
//! default (the cross-app base `alsem analyze` builds), `--single-app` analyses
//! the workspace alone, and every document states which apps were loaded and how.
//!
//! Every case spawns `CARGO_BIN_EXE_alsem`, so the clap arm and its default are
//! pinned, not only the library pipelines.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

const VERSION_OVERRIDE: &str = "cli-deps-v1";
const WS_GUID: &str = "aaaa6666-0000-0000-0000-000000000066";
const DEP_GUID: &str = "bbbb6666-0000-0000-0000-000000000066";

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// `alsem <args> --deterministic` -> (stdout, exit code).
fn alsem(args: &[&str]) -> (String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .arg("--deterministic")
        .env("ALCH_DRIVER_VERSION_OVERRIDE", VERSION_OVERRIDE)
        .output()
        .expect("spawn alsem");
    (
        String::from_utf8_lossy(&out.stdout).replace('\r', ""),
        out.status.code().unwrap_or(-1),
    )
}

fn json(args: &[&str]) -> (Value, i32) {
    let mut a = args.to_vec();
    a.extend(["--format", "json"]);
    let (out, code) = alsem(&a);
    (
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}")),
        code,
    )
}

const DEP_TABLE_SYMBOL: &str = r#"{"Id":18,"Name":"Customer","Fields":[{"Id":1,"Name":"No.","TypeDefinition":{"Name":"Code"}}]}"#;

/// The workspace: one codeunit writing the dependency's table 18 "Customer" and
/// its own table 50160, plus (when `call_lib`) calls into the dependency's
/// `Dep Lib`. `DoWork` shares its name with a dependency routine in the embedded
/// case, so a bare `DoWork` selector must stay on the workspace's. `CallLib`
/// calls the dependency's `DoWork` too: the cross-app model holds only the
/// dependency code the workspace reaches, so an uncalled one would never collide.
pub(crate) fn workspace(dir: &Path, call_lib: bool) {
    write(
        &dir.join("app.json"),
        &format!(
            r#"{{"id":"{WS_GUID}","name":"Ws66","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50100,"to":50199}}],"dependencies":[{{"id":"{DEP_GUID}","name":"XBase","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    );
    let lib_call = if call_lib {
        "\n    procedure CallLib()\n    var\n        Lib: Codeunit \"Dep Lib\";\n    begin\n        Lib.InsertCust();\n        Lib.DoWork();\n    end;\n"
    } else {
        ""
    };
    write(
        &dir.join("src/Work.al"),
        &format!(
            "table 50160 \"Order Pollution Flag\"\n{{\n    fields\n    {{\n        field(1; \"Entry No.\"; Integer)\n        {{\n        }}\n    }}\n}}\n\ncodeunit 50100 \"Ws Work\"\n{{\n    procedure DoWork()\n    var\n        Customer: Record Customer;\n        Flag: Record \"Order Pollution Flag\";\n    begin\n        Customer.Insert();\n        Flag.Insert();\n    end;\n{lib_call}}}\n"
        ),
    );
}

/// A symbol-only dependency declaring table 18 "Customer".
pub(crate) fn symbol_only_dependency(dir: &Path) {
    crate::symbol_app::write_symbol_app(
        &dir.join(".alpackages/probe_XBase_1.0.0.0.app"),
        DEP_GUID,
        "XBase",
        "1.0.0.0",
        &format!(
            r#"{{"RuntimeVersion":"13.0","Tables":[{DEP_TABLE_SYMBOL}],"AppId":"{DEP_GUID}","Name":"XBase","Publisher":"probe","Version":"1.0.0.0"}}"#
        ),
    );
}

/// The same dependency shipping source: table 18 and a codeunit whose
/// `InsertCust` writes it, plus a `DoWork` of its own.
pub(crate) fn embedded_dependency(dir: &Path) {
    crate::symbol_app::write_source_app(
        &dir.join(".alpackages/probe_XBase_1.0.0.0.app"),
        DEP_GUID,
        "XBase",
        "1.0.0.0",
        &format!(
            r#"{{"RuntimeVersion":"13.0","Tables":[{DEP_TABLE_SYMBOL}],"Codeunits":[{{"Id":10,"Name":"Dep Lib","Methods":[{{"Name":"InsertCust","Parameters":[]}},{{"Name":"DoWork","Parameters":[]}}]}}],"AppId":"{DEP_GUID}","Name":"XBase","Publisher":"probe","Version":"1.0.0.0"}}"#
        ),
        &[
            (
                "src/Customer.al",
                "table 18 Customer\n{\n    fields\n    {\n        field(1; \"No.\"; Code[20])\n        {\n        }\n    }\n}\n",
            ),
            (
                "src/Lib.al",
                "codeunit 10 \"Dep Lib\"\n{\n    procedure InsertCust()\n    var\n        Cust: Record Customer;\n    begin\n        Cust.Insert();\n    end;\n\n    procedure DoWork()\n    begin\n    end;\n}\n",
            ),
        ],
        "",
    );
}

/// The workspace + source-bearing dependency `.app` (spec §4.1.2 "the dependency workspace").
pub(crate) fn write_dependency_workspace(dir: &Path) {
    workspace(dir, true);
    embedded_dependency(dir);
}

fn dep_table() -> String {
    format!("{DEP_GUID}/table/18")
}

/// Every digest effect's `detail.resourceId`, paired with its `resourceDisplay`.
fn digest_resources(doc: &Value) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for e in doc["payload"]["entries"].as_array().unwrap() {
        for f in e["effects"].as_array().unwrap() {
            out.push((
                f["type"].as_str().unwrap_or("").to_string(),
                f["detail"]["resourceId"].as_str().unwrap_or("").to_string(),
                f["detail"]["resourceDisplay"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
            ));
        }
    }
    out
}

fn app_sources(doc: &Value) -> Vec<(String, String)> {
    doc["payload"]["dependencies"]["apps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                a["appId"].as_str().unwrap().to_string(),
                a["source"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// The issue's acceptance: a write to a dependency table resolves to
/// `<appId>/table/18` with its name, `--table 18` and `--table Customer` select
/// it, and the documents say the dependency was loaded and how. `--single-app`
/// states the precondition the other way round: the same effect exists but has
/// no table, and table 18 is not selectable.
///
/// Discrimination (2026-10-13): forcing the single-app branch in
/// `run_digest_pipeline` (`if true || single_app`) fails this test at the digest
/// `dependencies.mode` assertion (`"singleApp"`); restored, it passes. Dropping the
/// number tier from `resolve_table_selector` (`false && number…`) fails it at
/// `--table 18` (exit 2, `unmatched`); restored, it passes.
#[test]
fn symbol_only_dependency_table_resolves_and_is_selectable() {
    let dir = tempfile::tempdir().unwrap();
    workspace(dir.path(), false);
    symbol_only_dependency(dir.path());
    let ws = dir.path().to_string_lossy().to_string();

    // digest, default (cross-app).
    let (doc, code) = json(&["digest", &ws, "--file", "src/Work.al"]);
    assert_eq!(code, 0, "{doc}");
    assert_eq!(doc["schemaVersion"], "1.6.0");
    assert_eq!(doc["payload"]["dependencies"]["mode"], "crossApp");
    assert_eq!(
        app_sources(&doc),
        vec![(DEP_GUID.to_string(), "symbolOnly".to_string())]
    );
    let res = digest_resources(&doc);
    assert!(
        res.contains(&("DB_INSERT".to_string(), dep_table(), "Customer".to_string())),
        "{res:?}"
    );
    assert!(
        res.iter().any(|(t, id, n)| t == "DB_INSERT"
            && id.ends_with("/table/50160")
            && n == "Order Pollution Flag"),
        "{res:?}"
    );
    let (human, _) = alsem(&["digest", &ws, "--file", "src/Work.al", "--format", "human"]);
    assert!(
        human.starts_with("dependencies: XBase (symbolOnly)\n"),
        "{human}"
    );

    // digest --single-app: the Customer insert is still there, with no table.
    let (single, code) = json(&["digest", &ws, "--file", "src/Work.al", "--single-app"]);
    assert_eq!(code, 0);
    assert_eq!(single["payload"]["dependencies"]["mode"], "singleApp");
    assert!(app_sources(&single).is_empty());
    let res = digest_resources(&single);
    assert!(!res.iter().any(|(_, id, _)| id == &dep_table()), "{res:?}");
    assert!(
        res.iter()
            .any(|(t, id, _)| t == "DB_INSERT" && id.is_empty()),
        "the precondition: the unresolved Customer insert exists: {res:?}"
    );

    // query touches, by number and by name.
    for sel in ["18", "Customer"] {
        let (q, code) = json(&["query", "touches", &ws, "--table", sel]);
        assert_eq!(code, 0, "--table {sel}: {q}");
        assert_eq!(q["schemaVersion"], "1.1.0");
        assert_eq!(q["payload"]["table"]["id"], dep_table(), "--table {sel}");
        assert_eq!(q["payload"]["table"]["name"], "Customer");
        assert_eq!(q["payload"]["dependencies"]["mode"], "crossApp");
        let names: Vec<&str> = q["payload"]["up"]["routines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["display"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["DoWork"], "--table {sel}");
    }
    // A workspace table by number (it never matched before #66 either).
    let (q, code) = json(&["query", "touches", &ws, "--table", "50160"]);
    assert_eq!(code, 0, "{q}");
    assert_eq!(q["payload"]["table"]["name"], "Order Pollution Flag");

    let (q, code) = json(&["query", "touches", &ws, "--table", "18", "--single-app"]);
    assert_eq!(code, 2, "{q}");
    assert_eq!(q["payload"]["table"]["resolution"], "unmatched");
    assert_eq!(q["payload"]["dependencies"]["mode"], "singleApp");
}

/// A dependency that ships source: its routine is a fixed leaf, so a workspace
/// caller's effects include the write the dependency performs, anchored in the
/// dependency's file; and a bare routine name the dependency shares stays on the
/// workspace's routine (digest and query alike).
///
/// Discrimination (2026-10-13): removing the primary-app test from
/// `build_selector_indexes` fails the digest `--routine DoWork` assertion
/// (`selector-ambiguous`, candidates `DoWork`, `DoWork`); making
/// `is_primary_routine` always true fails `query effects --routine DoWork`
/// (`ambiguous`: `Dep Lib.DoWork`, `Ws Work.DoWork`); dropping only the
/// workspace-global list's filter fails the last assertion (`InsertCust` listed).
/// Each restored, it passes.
///
/// Limit (recorded 2026-10-13): replacing `&base.leaf_summaries` with an empty map
/// in `QuerySubstrate::from_cross_app_base` leaves this test GREEN. The dependency
/// routine's calls are cut from the combined graph, so its solved summary is its
/// own direct effects, the same as its leaf; a leaf differs only where an event
/// edge would feed the dependency routine more, which this fixture does not have.
/// (2026-10-19: `from_cross_app_base` is gone; query's leaves now reach its store
/// through `build_l4`'s cross-app inputs; the limit was not re-measured there.)
#[test]
fn embedded_dependency_leaf_effects_and_workspace_scoped_selectors() {
    let dir = tempfile::tempdir().unwrap();
    workspace(dir.path(), true);
    embedded_dependency(dir.path());
    let ws = dir.path().to_string_lossy().to_string();

    let (q, code) = json(&["query", "effects", &ws, "--routine", "CallLib"]);
    assert_eq!(code, 0, "{q}");
    assert_eq!(
        app_sources(&q),
        vec![(DEP_GUID.to_string(), "embedded".to_string())]
    );
    let effects = q["payload"]["effects"].as_array().unwrap();
    assert!(
        effects.iter().any(|e| e["tableId"] == dep_table()
            && e["tableName"] == "Customer"
            && e["origin"]["anchor"]["file"]
                .as_str()
                .is_some_and(|f| f.starts_with(&format!("dep:{DEP_GUID}")))),
        "{q}"
    );

    // `DoWork` exists in both apps; the selector resolves to the workspace's.
    let (q, code) = json(&["query", "effects", &ws, "--routine", "DoWork"]);
    assert_eq!(code, 0, "{q}");
    assert_eq!(
        q["payload"]["routine"]["routine"]["objectDisplay"],
        "Codeunit Ws Work"
    );
    let (d, code) = json(&["digest", &ws, "--routine", "DoWork"]);
    assert_eq!(code, 0, "{d}");
    assert_eq!(
        d["payload"]["changedRootsDiagnostics"],
        Value::Array(Vec::new()),
        "{d}"
    );
    assert_eq!(d["payload"]["entries"].as_array().unwrap().len(), 1, "{d}");

    // The workspace-global list holds workspace routines only.
    let (q, code) = json(&["query", "touches", &ws, "--table", "18"]);
    assert_eq!(code, 0, "{q}");
    let mut names: Vec<&str> = q["payload"]["up"]["routines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["display"].as_str().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, vec!["CallLib", "DoWork"], "{q}");
}

/// #71 A2c: changed-root resolution by declaration span indexes workspace
/// routines only (`ws:` units). A dependency routine's unit is
/// `dep:<guid>:<path>`; naming it as a `--file` matches nothing, in both modes.
///
/// Precondition (default mode, asserted first): the dependency is loaded with
/// source and `CallLib`'s effects are anchored in a `dep:{DEP_GUID}` file, so
/// dependency routines with `dep:` units ARE in the model.
/// Mutation: index every non-empty `source_unit_id` (drop the `ws:` filter) ->
/// `dep:{DEP_GUID}:src/lib.al` matches the dependency routines, roots appear.
#[test]
fn dependency_routines_never_become_changed_roots() {
    let dir = tempfile::tempdir().unwrap();
    workspace(dir.path(), true);
    embedded_dependency(dir.path());
    let ws = dir.path().to_string_lossy().to_string();

    let (q, code) = json(&["query", "effects", &ws, "--routine", "CallLib"]);
    assert_eq!(code, 0, "{q}");
    assert_eq!(
        app_sources(&q),
        vec![(DEP_GUID.to_string(), "embedded".to_string())]
    );
    assert!(
        q["payload"]["effects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["origin"]["anchor"]["file"]
                .as_str()
                .is_some_and(|f| f.starts_with(&format!("dep:{DEP_GUID}")))),
        "precondition: dependency routines are in the model: {q}"
    );

    let dep_file = format!("dep:{DEP_GUID}:src/Lib.al");
    for mode in [&[][..], &["--single-app"][..]] {
        let mut a = vec!["digest", &ws, "--file", &dep_file];
        a.extend_from_slice(mode);
        let (d, code) = json(&a);
        assert_eq!(code, 2, "{mode:?}: {d}");
        assert!(
            d["payload"]["entries"].as_array().unwrap().is_empty(),
            "{mode:?}: {d}"
        );
        let kinds: Vec<&str> = d["payload"]["changedRootsDiagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["kind"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, vec!["file-unmatched"], "{mode:?}: {d}");

        let mut a = vec!["digest", &ws, "--file", "src/Work.al"];
        a.extend_from_slice(mode);
        let (d, code) = json(&a);
        assert_eq!(code, 0, "{mode:?}: {d}");
        let mut names: Vec<&str> = d["payload"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["routine"]["display"].as_str().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, vec!["CallLib", "DoWork"], "{mode:?}: {d}");
        assert!(
            d["payload"]["entries"]
                .as_array()
                .unwrap()
                .iter()
                .all(|e| e["routine"]["anchor"]["file"] == "ws:src/Work.al"),
            "{mode:?}: {d}"
        );
    }
}

/// #70 row 3a: a dependency PUBLISHER the workspace raises is solved, so the L4
/// store follows its dispatch exactly as digest's cone does. On the
/// `tests/common/dep_dispatch_fixture.rs` workspace, cross-app
/// `query effects --routine RaiseDep` lists exactly two rows, each
/// `tempState: known(false)`:
///   - `Insert` into the dependency's `Customer`, origin `HandleDep`, anchored in
///     a `dep:` file;
///   - `Insert` into `Ws Flag`, origin `HandleInWs`, anchored in `ws:src/Work.al`.
///
/// Hand-stated precondition: the dependency is loaded `embedded`.
///
/// Mutation: make dependency publishers fixed leaves again (today's predicate in
/// `build_cross_app_base`) -> `effects: []` (probe E6).
#[test]
fn dependency_publisher_dispatch_reaches_the_store() {
    use crate::dep_dispatch_fixture::{DEP_GUID as XDEP, write_own};
    let dir = tempfile::tempdir().unwrap();
    write_own(dir.path(), "");
    let ws = dir.path().to_string_lossy().to_string();

    let (q, code) = json(&["query", "effects", &ws, "--routine", "RaiseDep"]);
    assert_eq!(code, 0, "{q}");
    assert_eq!(
        app_sources(&q),
        vec![(XDEP.to_string(), "embedded".to_string())],
        "precondition: {q}"
    );
    let mut rows: Vec<(String, String, String, String, String)> = q["payload"]["effects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            let file = e["origin"]["anchor"]["file"].as_str().unwrap_or("");
            let unit = if file.starts_with(&format!("dep:{XDEP}:")) {
                "dep"
            } else {
                file
            };
            (
                e["op"].as_str().unwrap_or("").to_string(),
                e["tableName"].as_str().unwrap_or("").to_string(),
                e["origin"]["routine"]["display"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
                unit.to_string(),
                e["tempState"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    rows.sort();
    let row = |t: &str, r: &str, u: &str| {
        (
            "Insert".to_string(),
            t.to_string(),
            r.to_string(),
            u.to_string(),
            "known(false)".to_string(),
        )
    };
    assert_eq!(
        rows,
        vec![
            row("Customer", "HandleDep", "dep"),
            row("Ws Flag", "HandleInWs", "ws:src/Work.al"),
        ],
        "{q}"
    );
}

/// #70: cross-app digest's transaction spans see the dependency's writes. A
/// workspace routine calls the dependency's `Outer()` (whose `Inner()` inserts
/// `Customer`) and then runs its OWN `Commit()`; that COMMIT's
/// `transactionContext` is `span-has-known-writes`.
///
/// Hand-stated precondition: the dependency is loaded `embedded`.
///
/// Discrimination (2026-10-15): `run_digest_pipeline` passing `None` instead of
/// `target.cross_app_inputs()` to `compute_tx_context_map` fails this test with
/// `["span-has-no-known-writes"]`; restored, it passes.
///
/// Discrimination (2026-10-19): `compute_tx_context_map` is gone. Passing
/// `None` instead of `target.cross_app_inputs()` into
/// `build_detector_artifacts` in `View::prepare` now stays GREEN: those inputs
/// only pick which routines seed spans and feed the l4 solve. The dependency
/// writes reach the span through the graph, so the live seam was `View::prepare`'s
/// cross-app graph fill (`base.graph`, which carried the dependency-internal
/// typed edges).
///
/// Discrimination (2026-10-20, #160 step 4): the seam is now
/// `ExecRelation::new`'s dependency typed-edge extend (the Cone payload the
/// base's relation carries). Skipping that extend fails this test with
/// `["span-has-no-known-writes"]`; restored, it passes.
#[test]
fn cross_app_commit_span_sees_dependency_writes() {
    use crate::dep_dispatch_fixture::{DEP_GUID as XDEP, write_own};
    let dir = tempfile::tempdir().unwrap();
    write_own(
        dir.path(),
        "codeunit 50172 \"Ws Commit\"\n{\n    procedure CommitAfterDep()\n    var\n        Lib: Codeunit \"Dep Lib\";\n    begin\n        Lib.Outer();\n        Commit();\n    end;\n}\n",
    );
    let ws = dir.path().to_string_lossy().to_string();

    let (doc, code) = json(&["digest", &ws, "--file", "src/Extra.al"]);
    assert_eq!(code, 0, "{doc}");
    assert_eq!(
        app_sources(&doc),
        vec![(XDEP.to_string(), "embedded".to_string())],
        "precondition: {doc}"
    );
    let entry = doc["payload"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["routine"]["display"] == "CommitAfterDep")
        .unwrap_or_else(|| panic!("no CommitAfterDep entry: {doc}"));
    let own: Vec<&str> = entry["effects"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["type"] == "COMMIT" && f["provenance"] == "direct")
        .map(|f| f["transactionContext"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(own, vec!["span-has-known-writes"], "{entry}");
}

/// The no-dependency contract: on every digest and query golden workspace (none
/// has a `.alpackages`), cross-app output equals `--single-app` output apart
/// from `payload.dependencies.mode`; human output is byte-equal.
///
/// Discrimination (2026-10-13): printing the human `dependencies:` line in
/// cross-app mode even with no app loaded fails this test on the first digest
/// fixture's human output; restored, it passes.
///
/// Limit: with no dependency the cross-app base has no dependency calls to cut,
/// so a broken cut cannot show here. The cut is `build_cross_app_base`'s, shared
/// with `alsem analyze`, and is not pinned by this file.
#[test]
fn without_dependencies_cross_app_equals_single_app() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus");
    let mut runs: Vec<Vec<String>> = Vec::new();
    for fixture in crate::cli_b_digest_differential::DIGEST_CORPUS {
        let ws = corpus.join(fixture);
        let mut argv = vec!["digest".to_string(), ws.to_string_lossy().to_string()];
        for f in crate::cli_b_digest_differential::collect_al_files(&ws) {
            argv.push("--file".to_string());
            argv.push(f);
        }
        runs.push(argv);
    }
    for c in crate::cli_query_differential::CORPUS {
        let mut argv = vec![
            "query".to_string(),
            c.sub.to_string(),
            corpus.join(c.workspace).to_string_lossy().to_string(),
        ];
        argv.extend(c.args.iter().map(|a| a.to_string()));
        runs.push(argv);
    }
    let mut compared = 0;
    for argv in &runs {
        for format in ["json", "human"] {
            let mut a: Vec<&str> = argv.iter().map(String::as_str).collect();
            a.extend(["--format", format]);
            let (cross, cross_code) = alsem(&a);
            a.push("--single-app");
            let (single, single_code) = alsem(&a);
            assert_eq!(cross_code, single_code, "{argv:?} {format}");
            let single = if format == "json" {
                assert!(single.contains("\"mode\": \"singleApp\""), "{single}");
                single.replace("\"mode\": \"singleApp\"", "\"mode\": \"crossApp\"")
            } else {
                single
            };
            assert_eq!(cross, single, "{argv:?} --format {format}");
            compared += 1;
        }
    }
    assert!(compared >= 58, "compared only {compared} outputs");
}
