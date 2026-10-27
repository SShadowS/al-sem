//! #159: `al-call-hierarchy --project <ws> --analyze --format json` gains
//! per-routine call-graph and identity fields, driven through the SHIPPED
//! binary on `tests/fixtures/analyze-json/`.
//!
//! Every expectation is hand-stated from the fixture text, or compared with an
//! independent producer of the same fact: the LSP's own handlers and
//! diagnostics (`al_sem::lsp`) for fan-in/fan-out/unused, and `alsem
//! fingerprint --inventory-only` for `stableRoutineId`.
//!
//! `tests/fixtures/analyze-json-expected/legacy.{json,txt,csv}` are FROZEN: they
//! were minted once from the base binary (84ba2d4b, before #159) with cwd =
//! repo root and `--project tests/fixtures/analyze-json`. They are the
//! "existing consumers are unaffected" contract and are deliberately not
//! regenerable.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::{Command, Output};

use al_sem::config::DiagnosticConfig;
use al_sem::lsp::diagnostics::compute_all;
use al_sem::lsp::encoding::PositionEncoding;
use al_sem::lsp::handlers::{ItemData, incoming, outgoing};
use al_sem::lsp::snapshot::LspSnapshot;
use serde_json::Value;

const FIXTURE: &str = "tests/fixtures/analyze-json";
const NOAPP_FIXTURE: &str = "tests/fixtures/analyze-json-noapp";
const EXPECTED: &str = "tests/fixtures/analyze-json-expected";

/// The base row keys, in base order.
const LEGACY_KEYS: [&str; 9] = [
    "object_type",
    "object_name",
    "procedure_name",
    "file",
    "line",
    "complexity",
    "line_count",
    "parameter_count",
    "quality_score",
];

/// The #159 row keys, appended after `quality_score`, in this order.
const NEW_KEYS: [&str; 8] = [
    "path",
    "object_id",
    "access",
    "is_event_subscriber",
    "is_trigger",
    "routine_id",
    "fan_in",
    "fan_out",
];

pub(crate) fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Run `cmd` with an empty home directory, so a developer's
/// `~/.al-sem/config.json` (diagnostic thresholds, `unusedProcedures`) cannot
/// change what the tests see.
fn output_with_empty_home(cmd: &mut Command) -> Output {
    let home = tempfile::tempdir().expect("empty home");
    cmd.env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("XDG_CONFIG_HOME", home.path())
        .output()
        .expect("spawn")
}

/// Run the binary with cwd = repo root and a RELATIVE project path, so the
/// text header (`Code Quality Analysis: <project>`) is checkout-independent.
pub(crate) fn run_analyze(project: &str, format: &str) -> Output {
    run_analyze_in(&repo_root(), project, format)
}

pub(crate) fn run_analyze_in(cwd: &std::path::Path, project: &str, format: &str) -> Output {
    let out = output_with_empty_home(
        Command::new(env!("CARGO_BIN_EXE_al-call-hierarchy"))
            .current_dir(cwd)
            .args(["--project", project, "--analyze", "--format", format])
            .arg("--no-telemetry"),
    );
    assert!(
        out.status.success(),
        "al-call-hierarchy --analyze --format {format} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

pub(crate) fn stdout_text(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).expect("utf8 stdout")
}

fn analyze_json(project: &str) -> Value {
    serde_json::from_str(&stdout_text(&run_analyze(project, "json"))).expect("valid JSON")
}

fn rows(v: &Value) -> &Vec<Value> {
    v["metrics"].as_array().expect("metrics[]")
}

fn findings(v: &Value) -> &Vec<Value> {
    v["findings"].as_array().expect("findings[]")
}

/// The one row for `object.procedure`; panics unless exactly one matches.
fn row<'a>(v: &'a Value, object: &str, procedure: &str) -> &'a Value {
    let hits: Vec<&Value> = rows(v)
        .iter()
        .filter(|r| r["object_name"] == object && r["procedure_name"] == procedure)
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "expected exactly one row for {object}.{procedure}, got {hits:#?}"
    );
    hits[0]
}

/// A row's field, asserting the key is PRESENT (a missing key and a JSON
/// `null` are different answers).
fn field<'a>(r: &'a Value, key: &str) -> &'a Value {
    r.get(key).unwrap_or_else(|| {
        panic!(
            "row {}.{} has no `{key}` field: {r}",
            r["object_name"], r["procedure_name"]
        )
    })
}

fn snapshot() -> LspSnapshot {
    LspSnapshot::build_full(&repo_root().join(FIXTURE)).expect("fixture snapshot builds")
}

/// The row for a snapshot declaration, by `(path, line)`.
fn row_at<'a>(v: &'a Value, path: &str, line: u32) -> &'a Value {
    rows(v)
        .iter()
        .find(|r| r.get("path").and_then(Value::as_str) == Some(path) && r["line"] == line)
        .unwrap_or_else(|| panic!("no row with path {path} line {line}"))
}

// ── Acceptance 1: fan_in / fan_out are DISTINCT callers / callees ─────────

#[test]
fn fan_in_and_fan_out_count_distinct() {
    let v = analyze_json(FIXTURE);

    // Target: called by CallerA (twice) and CallerB -> 2 callers; calls One
    // (twice), Two, Three -> 3 callees.
    let target = row(&v, "Probe Util", "Target");
    assert_eq!(field(target, "fan_in"), 2, "Target fan_in: {target}");
    assert_eq!(field(target, "fan_out"), 3, "Target fan_out: {target}");
    // NeverCalled: nobody calls it, it calls nothing (0, not null: the graph exists).
    let never = row(&v, "Probe Util", "NeverCalled");
    assert_eq!(field(never, "fan_in"), 0);
    assert_eq!(field(never, "fan_out"), 0);
    // Dispatch calls Greet through an interface parameter: BOTH
    // implementations are candidates, and each counts.
    let dispatch = row(&v, "Probe Util", "Dispatch");
    assert_eq!(
        field(dispatch, "fan_out"),
        2,
        "every candidate counts: {dispatch}"
    );
    // An interface member is a signature, never a call target: null, not 0.
    let sig = row(&v, "Probe Greeter", "Greet");
    assert_eq!(field(sig, "fan_in"), &Value::Null, "{sig}");
    assert_eq!(field(sig, "fan_out"), &Value::Null, "{sig}");

    // Contract with the call-hierarchy panel on every declaration: distinct
    // callers from `incoming` and distinct targets from `outgoing`. Equal here
    // because the fixture has no caller the panel cannot locate.
    let snap = snapshot();
    let mut checked = 0;
    for (virtual_path, decls) in &snap.decls_by_file {
        for decl in decls.iter() {
            let r = row_at(&v, virtual_path, decl.origin.start.row + 1);
            if r["object_type"] == "Interface" {
                continue; // null by design, asserted above
            }
            let data = ItemData {
                node: decl.id.clone(),
            };
            let callers = incoming(&snap, PositionEncoding::Utf16, &data).len();
            let callees: BTreeSet<String> = outgoing(&snap, PositionEncoding::Utf16, &data)
                .iter()
                .map(|c| serde_json::to_string(&c.to).expect("item json"))
                .collect();
            assert_eq!(field(r, "fan_in"), callers, "fan_in vs panel for {r}");
            assert_eq!(
                field(r, "fan_out"),
                callees.len(),
                "fan_out vs panel for {r}"
            );
            checked += 1;
        }
    }
    assert!(
        checked >= 20,
        "fixture precondition: {checked} declarations"
    );
}

// ── Acceptance 2: unused_procedure == the LSP's unused-procedure rule ─────

#[test]
fn unused_procedure_findings_equal_the_lsp_diagnostics() {
    let v = analyze_json(FIXTURE);
    let all = findings(&v);

    let unused: Vec<&Value> = all
        .iter()
        .filter(|f| f["category"] == "unused_procedure")
        .collect();
    // (workspace-relative path, 1-based line, procedure) of each finding; the
    // line is the one in `location` (`<base name>:<line>`).
    let cli: BTreeSet<(String, u32, String)> = unused
        .iter()
        .map(|f| {
            let location = f["location"].as_str().expect("location");
            let (base, line) = location.rsplit_once(':').expect("file:line");
            let path = f["path"].as_str().expect("unused finding carries `path`");
            assert!(path.ends_with(base), "path {path} vs location {location}");
            (
                path.to_string(),
                line.parse().expect("line"),
                f["procedure"].as_str().expect("procedure").to_string(),
            )
        })
        .collect();

    // Hand-stated: NeverCalled is the only procedure with no caller. Not
    // reported: the called procedures, both subscribers (R2), the trigger
    // (R1), the unraised IntegrationEvent (R4), the subscribed-only
    // InternalEvent (R5), the interface method (R6), [Test]/[ConfirmHandler] (R3).
    let expected: BTreeSet<(String, u32, String)> = [(
        "src/A/Util.Codeunit.al".to_string(),
        34,
        "Probe Util.NeverCalled".to_string(),
    )]
    .into();
    assert_eq!(cli, expected, "unused_procedure findings: {unused:#?}");
    for f in &unused {
        assert_eq!(f["severity"], "info", "{f}");
        assert_eq!(
            f["description"],
            "Procedure 'Probe Util.NeverCalled' is never called"
        );
    }
    // Existing findings keep their exact shape: no `path` key.
    for f in all.iter().filter(|f| f["category"] != "unused_procedure") {
        assert!(f.get("path").is_none(), "legacy finding gained `path`: {f}");
    }

    // The same set the LSP's diagnostics publish, computed by the library on
    // the same fixture: (workspace-relative path, 1-based line).
    let snap = snapshot();
    let root_uri = format!(
        "{}/",
        al_sem::protocol::path_to_uri(&snap.workspace_root).as_str()
    );
    let lsp: BTreeSet<(String, u32)> =
        compute_all(&snap, PositionEncoding::Utf16, &DiagnosticConfig::default())
            .into_iter()
            .flat_map(|(uri, diags)| {
                let rel = uri
                    .strip_prefix(&root_uri)
                    .unwrap_or_else(|| panic!("{uri} not under {root_uri}"))
                    .to_string();
                diags.into_iter().filter_map(move |d| {
                    (d.code == Some(lsp_types::NumberOrString::String("unused-procedure".into())))
                        .then(|| (rel.clone(), d.range.start.line + 1))
                })
            })
            .collect();
    let cli_locations: BTreeSet<(String, u32)> =
        cli.iter().map(|(p, l, _)| (p.clone(), *l)).collect();
    assert_eq!(cli_locations, lsp, "CLI unused set vs LSP diagnostics");

    // Appended after every existing finding.
    let first_unused = all
        .iter()
        .position(|f| f["category"] == "unused_procedure")
        .expect("an unused finding");
    assert!(
        all[first_unused..]
            .iter()
            .all(|f| f["category"] == "unused_procedure"),
        "unused_procedure findings must come after all legacy findings: {all:#?}"
    );
    assert!(
        first_unused > 0,
        "fixture precondition: a legacy finding exists"
    );
}

// ── Acceptance 3: workspace-relative POSIX path ───────────────────────────

#[test]
fn path_is_workspace_relative_posix() {
    let v = analyze_json(FIXTURE);
    let a = row(&v, "Probe Util", "Target");
    let b = row(&v, "Probe Subs", "HandleRaised");
    assert_eq!(a["file"], "Util.Codeunit.al");
    assert_eq!(b["file"], "Util.Codeunit.al");
    assert_eq!(field(a, "path"), "src/A/Util.Codeunit.al");
    assert_eq!(field(b, "path"), "src/B/Util.Codeunit.al");
}

// ── Acceptance 4: object_id, access, is_event_subscriber, is_trigger ──────

#[test]
fn object_id_access_and_kind_flags() {
    let v = analyze_json(FIXTURE);
    let cases: [(&str, &str, Value, &str, bool, bool); 9] = [
        ("Probe Util", "Target", 50100.into(), "public", false, false),
        ("Probe Util", "One", 50100.into(), "local", false, false),
        ("Probe Util", "Two", 50100.into(), "internal", false, false),
        (
            "Probe Util",
            "Three",
            50100.into(),
            "protected",
            false,
            false,
        ),
        ("Probe Util", "OnRun", 50100.into(), "public", false, true),
        (
            "Probe Subs",
            "HandleRaised",
            50101.into(),
            "local",
            true,
            false,
        ),
        (
            "Probe Base Ext",
            "ExtProc",
            50106.into(),
            "public",
            false,
            false,
        ),
        (
            "Probe Base Ext",
            "OnValidate",
            50106.into(),
            "public",
            false,
            true,
        ),
        (
            "Probe Greeter",
            "Greet",
            Value::Null,
            "public",
            false,
            false,
        ),
    ];
    for (object, procedure, object_id, access, subscriber, trigger) in cases {
        let r = row(&v, object, procedure);
        assert_eq!(field(r, "object_id"), &object_id, "object_id of {r}");
        assert_eq!(field(r, "access"), access, "access of {r}");
        assert_eq!(
            field(r, "is_event_subscriber"),
            subscriber,
            "is_event_subscriber of {r}"
        );
        assert_eq!(field(r, "is_trigger"), trigger, "is_trigger of {r}");
    }
}

// ── Acceptance 5: routine_id == alsem's stableRoutineId ───────────────────

#[test]
fn routine_id_equals_alsem_stable_routine_id() {
    let v = analyze_json(FIXTURE);

    // #160 step 9: `--inventory-only` is cross-app by default, as `analyze`
    // is, so both sides read the same universe's ids.
    let out = output_with_empty_home(
        Command::new(env!("CARGO_BIN_EXE_alsem"))
            .current_dir(repo_root())
            .args([
                "fingerprint",
                FIXTURE,
                "--inventory-only",
                "--format",
                "json",
            ])
            .arg("--deterministic"),
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let inventory: Value = serde_json::from_slice(&out.stdout).expect("inventory json");

    // (object number, routine name) -> sorted stableRoutineIds. A multiset per
    // key, so the two `Over` overloads and the two page `OnAction` triggers
    // must each map to two DIFFERENT ids on both sides.
    let mut alsem: BTreeMap<(i64, String), Vec<String>> = BTreeMap::new();
    let mut stack = vec![&inventory];
    while let Some(node) = stack.pop() {
        match node {
            Value::Object(m) => {
                if let Some(id) = m.get("stableRoutineId").and_then(Value::as_str) {
                    let key = (
                        m["objectNumber"].as_i64().expect("objectNumber"),
                        m["routineName"].as_str().expect("routineName").to_string(),
                    );
                    alsem.entry(key).or_default().push(id.to_string());
                }
                stack.extend(m.values());
            }
            Value::Array(a) => stack.extend(a.iter()),
            _ => {}
        }
    }
    let mut cli: BTreeMap<(i64, String), Vec<String>> = BTreeMap::new();
    for r in rows(&v) {
        match field(r, "routine_id") {
            Value::String(id) => cli
                .entry((
                    field(r, "object_id").as_i64().expect("object_id"),
                    r["procedure_name"].as_str().expect("name").to_string(),
                ))
                .or_default()
                .push(id.clone()),
            Value::Null => assert_eq!(
                r["object_type"], "Interface",
                "only interface members have no routine_id here: {r}"
            ),
            other => panic!("routine_id must be a string or null, got {other}"),
        }
    }
    for ids in alsem.values_mut().chain(cli.values_mut()) {
        ids.sort();
    }
    assert!(
        alsem.len() >= 20,
        "fixture precondition: {} alsem keys",
        alsem.len()
    );
    assert_eq!(cli, alsem, "routine_id vs alsem stableRoutineId");

    // Each overload and each member trigger has ITS id, pinned per row (by
    // path + line) against the id formula stated by hand, so swapping two
    // rows' ids fails even though the multisets above would still match.
    use al_sem::engine::ids::{
        ParamSpec, normalized_signature_hash, to_stable_routine_id_from_parts,
    };
    const APP: &str = "15900000-0000-4000-8000-000000000159";
    let util = format!("{APP}:Codeunit:50100");
    let card = format!("{APP}:Page:50107");
    let param = |t: &str| ParamSpec {
        type_text: t.to_string(),
        is_var: false,
    };
    let on_action = normalized_signature_hash("OnAction", &[], None);
    let pinned = [
        (
            "src/A/Util.Codeunit.al",
            38,
            to_stable_routine_id_from_parts(
                &util,
                &normalized_signature_hash("Over", &[param("Integer")], None),
                None,
            ),
        ),
        (
            "src/A/Util.Codeunit.al",
            42,
            to_stable_routine_id_from_parts(
                &util,
                &normalized_signature_hash("Over", &[param("Text")], None),
                None,
            ),
        ),
        (
            "src/D/Card.Page.al",
            22,
            to_stable_routine_id_from_parts(&card, &on_action, Some("First")),
        ),
        (
            "src/D/Card.Page.al",
            28,
            to_stable_routine_id_from_parts(&card, &on_action, Some("Second")),
        ),
    ];
    for (path, line, want) in &pinned {
        let r = row_at(&v, path, *line);
        assert_eq!(
            field(r, "routine_id"),
            want.as_str(),
            "routine_id of {path}:{line}: {r}"
        );
    }
    assert_ne!(pinned[0].2, pinned[1].2);
    assert_ne!(pinned[2].2, pinned[3].2);

    // Stable across runs on the same tree.
    let again = analyze_json(FIXTURE);
    let ids = |v: &Value| -> Vec<Value> {
        rows(v)
            .iter()
            .map(|r| field(r, "routine_id").clone())
            .collect()
    };
    assert_eq!(ids(&v), ids(&again));
}

// ── Acceptance 6: existing output unchanged ───────────────────────────────

#[test]
fn legacy_fields_text_and_csv_byte_identical_to_base() {
    let expected = |name: &str| {
        std::fs::read_to_string(repo_root().join(EXPECTED).join(name)).expect("frozen base output")
    };

    // JSON: drop the new keys and the new findings, compare as parsed values.
    // #253's `test_quality` block is a new top-level key after `summary`.
    let raw = stdout_text(&run_analyze(FIXTURE, "json"));
    let mut current: Value = serde_json::from_str(&raw).expect("json");
    current
        .as_object_mut()
        .expect("top-level object")
        .remove("test_quality");
    for r in current["metrics"].as_array_mut().expect("metrics") {
        let obj = r.as_object_mut().expect("row object");
        for k in NEW_KEYS {
            obj.remove(k);
        }
    }
    current["findings"]
        .as_array_mut()
        .expect("findings")
        .retain(|f| f["category"] != "unused_procedure");
    let legacy: Value = serde_json::from_str(&expected("legacy.json")).expect("legacy json");
    assert_eq!(current, legacy, "legacy fields/findings/summary changed");

    // Raw key order of every row: the legacy keys first, in base order, then
    // the new keys after `quality_score`.
    let want: Vec<&str> = LEGACY_KEYS.iter().chain(NEW_KEYS.iter()).copied().collect();
    let mut row_count = 0;
    let mut keys: Option<Vec<String>> = None;
    for line in raw.lines() {
        let t = line.trim();
        // The `metrics` array ends here; `test_quality` rows also carry
        // `object_type` and are not metrics rows.
        if t == "\"findings\": [" {
            break;
        }
        if t.starts_with("\"object_type\"") {
            keys = Some(Vec::new());
        }
        if let Some(k) = keys.as_mut() {
            if t.starts_with('}') {
                assert_eq!(
                    k.iter().map(String::as_str).collect::<Vec<_>>(),
                    want,
                    "row key order"
                );
                row_count += 1;
                keys = None;
            } else if let Some(rest) = t.strip_prefix('"') {
                k.push(rest.split('"').next().expect("key").to_string());
            }
        }
    }
    assert_eq!(
        row_count,
        rows(&current).len(),
        "every row's key order checked"
    );

    // Text and CSV: byte-for-byte.
    assert_eq!(
        stdout_text(&run_analyze(FIXTURE, "text")),
        expected("legacy.txt"),
        "text output changed"
    );
    assert_eq!(
        stdout_text(&run_analyze(FIXTURE, "csv")),
        expected("legacy.csv"),
        "CSV output changed"
    );
}

// ── Degraded workspace: no app.json -> no graph -> null, not 0 ────────────

#[test]
fn no_app_json_degrades_to_null() {
    let out = run_analyze(NOAPP_FIXTURE, "json");
    let v: Value = serde_json::from_str(&stdout_text(&out)).expect("valid JSON without app.json");
    assert!(!rows(&v).is_empty(), "fixture precondition: rows");
    for r in rows(&v) {
        for k in ["fan_in", "fan_out", "routine_id"] {
            assert_eq!(
                field(r, k),
                &Value::Null,
                "{k} must be null without a graph: {r}"
            );
        }
        assert_eq!(field(r, "path"), "Solo.Codeunit.al");
    }
    assert!(
        findings(&v)
            .iter()
            .all(|f| f["category"] != "unused_procedure"),
        "no unused findings without a graph"
    );
    // The warning names the reason and only the graph fields; it makes no
    // claim about routine_id, which does not depend on the graph.
    let stderr = String::from_utf8_lossy(&out.stderr);
    let warning = format!(
        "warning: call graph unavailable for {NOAPP_FIXTURE} (no app.json at the project root): \
         fan_in and fan_out are null and unused procedures are not reported"
    );
    assert!(
        stderr.lines().any(|l| l == warning),
        "stderr must carry exactly {warning:?}; got {stderr}"
    );
    assert!(
        !stderr.contains("routine_id"),
        "the warning must not claim routine_id is null: {stderr}"
    );
}

// ── An app.json that cannot be used: the warning says why (#G2) ───────────

#[test]
fn unusable_app_json_warning_names_the_builder_reason() {
    // The builder's reason used to be dropped: the warning said only "the
    // program snapshot could not be built", which hid a BOM or a typo.
    let ws = tempfile::tempdir().expect("workspace");
    std::fs::copy(
        repo_root().join(NOAPP_FIXTURE).join("Solo.Codeunit.al"),
        ws.path().join("Solo.Codeunit.al"),
    )
    .expect("copy fixture source");
    std::fs::write(ws.path().join("app.json"), "{ \"id\": ").expect("write app.json");
    let project = ws.path().to_str().expect("utf-8 temp path");
    let out = run_analyze(project, "json");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let warning = stderr
        .lines()
        .find(|l| l.starts_with("warning: call graph unavailable for "))
        .unwrap_or_else(|| panic!("no call-graph warning; stderr: {stderr}"));
    assert!(
        warning.contains("(snapshot build failed: parse ") && warning.contains("app.json: "),
        "the warning must carry the builder's parse reason: {warning}"
    );
}

// ── A nested app's files get no routine_id (the root guid is not theirs) ──

#[test]
fn nested_app_files_get_no_routine_id() {
    let ws = tempfile::tempdir().expect("workspace");
    let app = |guid: &str, name: &str| {
        format!(
            r#"{{"id":"{guid}","name":"{name}","publisher":"t","version":"1.0.0.0","idRanges":[{{"from":50100,"to":50199}}],"runtime":"14.0"}}"#
        )
    };
    let unit = |id: u32, name: &str| {
        format!("codeunit {id} \"{name}\"\n{{\n    trigger OnRun()\n    begin\n    end;\n}}\n")
    };
    std::fs::write(
        ws.path().join("app.json"),
        app("15900000-0000-4000-8000-0000000000aa", "Root"),
    )
    .unwrap();
    std::fs::write(
        ws.path().join("Root.Codeunit.al"),
        unit(50100, "Probe Root"),
    )
    .unwrap();
    std::fs::create_dir(ws.path().join("Sub")).unwrap();
    std::fs::write(
        ws.path().join("Sub/app.json"),
        app("15900000-0000-4000-8000-0000000000bb", "Sub"),
    )
    .unwrap();
    std::fs::write(
        ws.path().join("Sub/Nested.Codeunit.al"),
        unit(50101, "Probe Nested"),
    )
    .unwrap();

    let out = run_analyze_in(ws.path(), ".", "json");
    let v: Value = serde_json::from_str(&stdout_text(&out)).expect("json");
    let root = row(&v, "Probe Root", "OnRun");
    let nested = row(&v, "Probe Nested", "OnRun");
    assert!(
        field(root, "routine_id").as_str().is_some_and(
            |id| id.starts_with("15900000-0000-4000-8000-0000000000aa:Codeunit:50100#")
        ),
        "a root-app file keeps its routine_id: {root}"
    );
    assert_eq!(
        field(nested, "routine_id"),
        &Value::Null,
        "a nested-app file must not get an id minted with the root's guid: {nested}"
    );
}
