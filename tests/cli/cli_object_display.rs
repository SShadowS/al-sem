//! #78: every digest entry, fingerprint block and prove document names its
//! owning object as `"<ObjectType> <ObjectName>"`, the same value
//! `alsem query effects` prints (`RoutineJoin::of`), taken from the Model and
//! never from the snapshot's identity table.
//!
//! The fixture holds a Table and a TableExtension with the SAME number (50100),
//! the extension's file sorting after the table's. Until G40 the identity table
//! kept the extension's name at `<guid>:Table:50100`, so an owner taken from the
//! identity table printed `OD Item Ext` for the real table's routines; the
//! "Mutations (ii)" notes below date from then. Since G40 the identity table
//! holds the real table's name there (`assert_identity_table_holds_table_name`
//! pins that), so this pair no longer tells an identity-table owner from the
//! Model's by the name alone. The two Interfaces check the declaration-only
//! rows' display. Since #83 they have distinct ids (`<guid>:Interface:@ifoo`,
//! `…:@ibar`), so they do not catch an identity-table owner either.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

use al_sem::engine::gate::model_instance_id::compute_gate_model_instance_id;
use al_sem::engine::l5::snapshot::compose_snapshot;
use al_sem::program::model::program_calls::assemble_and_resolve_workspace_program;

const VERSION_OVERRIDE: &str = "cli-object-display-v1";
const GUID: &str = "7a1e0000-0000-0000-0000-000000000078";

const TABLE_OWNER: &str = "Table OD Item";
const TABLE_EXT_OWNER: &str = "TableExtension OD Item Ext";
const CODEUNIT_OWNER: &str = "Codeunit OD Cu";

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// (file, expected digest roots by display, the literal owner of each).
fn files() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    vec![
        (
            "src/a_Table.al",
            vec![
                ("OnInsert", TABLE_OWNER),
                ("OnValidate", TABLE_OWNER),
                ("TableProc", TABLE_OWNER),
            ],
        ),
        ("src/b_TableExt.al", vec![("ExtProc", TABLE_EXT_OWNER)]),
        ("src/c_Codeunit.al", vec![("CuProc", CODEUNIT_OWNER)]),
        ("src/d_IFoo.al", vec![("FooMethod", "Interface IFoo")]),
        ("src/e_IBar.al", vec![("BarMethod", "Interface IBar")]),
    ]
}

fn owner_workspace(dir: &Path) {
    write(
        &dir.join("app.json"),
        &format!(
            r#"{{"id":"{GUID}","name":"Issue78Owner","publisher":"probe","version":"1.0.0.0","platform":"1.0.0.0","application":"1.0.0.0","idRanges":[{{"from":50100,"to":50199}}],"runtime":"14.0"}}"#
        ),
    );
    write(
        &dir.join("src/a_Table.al"),
        "table 50100 \"OD Item\"\n{\n    fields\n    {\n        field(1; \"No.\"; Code[20])\n        {\n            trigger OnValidate()\n            begin\n                Message('v');\n            end;\n        }\n    }\n\n    trigger OnInsert()\n    begin\n        Message('i');\n    end;\n\n    procedure TableProc()\n    begin\n        Message('t');\n    end;\n}\n",
    );
    write(
        &dir.join("src/b_TableExt.al"),
        "tableextension 50100 \"OD Item Ext\" extends \"OD Item\"\n{\n    procedure ExtProc()\n    begin\n        Message('e');\n    end;\n}\n",
    );
    write(
        &dir.join("src/c_Codeunit.al"),
        "codeunit 50101 \"OD Cu\"\n{\n    procedure CuProc()\n    begin\n        Message('c');\n    end;\n}\n",
    );
    write(
        &dir.join("src/d_IFoo.al"),
        "interface IFoo\n{\n    procedure FooMethod();\n}\n",
    );
    write(
        &dir.join("src/e_IBar.al"),
        "interface IBar\n{\n    procedure BarMethod();\n}\n",
    );
}

/// Spawn `alsem <args>` -> (stdout parsed as JSON, exit code).
fn alsem(args: &[&str]) -> (Value, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .env("ALCH_DRIVER_VERSION_OVERRIDE", VERSION_OVERRIDE)
        .output()
        .expect("spawn alsem");
    let stdout = String::from_utf8_lossy(&out.stdout).replace('\r', "");
    let doc: Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "{e}: args={args:?} stdout={stdout} stderr={}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (doc, out.status.code().unwrap_or(-1))
}

/// `alsem digest <ws> --file <file> --deterministic --format json`.
fn digest_file(ws: &Path, file: &str) -> (Value, i32) {
    alsem(&[
        "digest",
        ws.to_str().unwrap(),
        "--file",
        file,
        "--deterministic",
        "--format",
        "json",
    ])
}

fn entries(doc: &Value) -> Vec<Value> {
    doc["payload"]["entries"].as_array().unwrap().clone()
}

/// The stable id of the one digest entry of `file` displayed as `display`.
fn stable_id_of(ws: &Path, file: &str, display: &str) -> String {
    let (doc, code) = digest_file(ws, file);
    assert_eq!(code, 0, "{doc}");
    let hits: Vec<Value> = entries(&doc)
        .into_iter()
        .filter(|e| e["routine"]["display"] == display)
        .collect();
    assert_eq!(hits.len(), 1, "one `{display}` entry expected: {doc}");
    hits[0]["routine"]["stableId"].as_str().unwrap().to_string()
}

/// Shared by A1/A2/A4: the snapshot's identity table holds the REAL table's
/// name at its id, though tableextension 50100 shares the number (G40: the
/// extension stub used to overwrite it, and `prove … writes-table:<name>`
/// then matched none of the table's writes). Before G40 this asserted the
/// overwrite, as the precondition for catching an identity-table owner.
fn assert_identity_table_holds_table_name(ws: &Path) {
    let model_id = compute_gate_model_instance_id(ws).unwrap();
    let inputs = al_sem::program::inputs::WorkspaceInputs::read(ws, &Default::default());
    let model = assemble_and_resolve_workspace_program(&inputs, &model_id, false).unwrap();
    let snap = compose_snapshot(&model);
    let ids = &snap.identities;
    let key = format!("{GUID}:Table:50100");
    let at = ids
        .stable_ids
        .iter()
        .position(|s| *s == key)
        .unwrap_or_else(|| panic!("precondition: `{key}` in identities: {:?}", ids.stable_ids));
    assert_eq!(
        ids.display_names[at], "OD Item",
        "the extension stub must not overwrite the table's identity name"
    );
}

/// A1. Every digest root's `routine.objectDisplay` is the literal owner, and for
/// every non-declaration-only root it equals `query effects`' `objectDisplay`.
///
/// Mutations: (i) keep the old `split_object_routine` -> every value is `""`;
/// (ii) take the owner from the identity table -> the Table's three routines
/// read `OD Item Ext`.
#[test]
fn digest_entries_name_their_owning_object() {
    let dir = tempfile::tempdir().unwrap();
    owner_workspace(dir.path());
    let ws = dir.path();
    assert_identity_table_holds_table_name(ws);

    let mut got: Vec<(String, String, String)> = Vec::new();
    let mut want: Vec<(String, String, String)> = Vec::new();
    let mut docs = String::new();
    for (file, expected) in files() {
        let (doc, code) = digest_file(ws, file);
        assert_eq!(code, 0, "{file}: {doc}");
        let es = entries(&doc);
        let mut displays: Vec<&str> = es
            .iter()
            .map(|e| e["routine"]["display"].as_str().unwrap())
            .collect();
        displays.sort();
        let mut exp_displays: Vec<&str> = expected.iter().map(|(d, _)| *d).collect();
        exp_displays.sort();
        assert_eq!(
            displays, exp_displays,
            "precondition: roots of {file}: {doc}"
        );

        for e in &es {
            let r = &e["routine"];
            let display = r["display"].as_str().unwrap();
            let owner = expected.iter().find(|(d, _)| *d == display).unwrap().1;
            let decl_only = r["declarationOnly"] == true;
            assert_eq!(
                decl_only,
                owner.starts_with("Interface "),
                "precondition: declarationOnly of {display}: {doc}"
            );
            if !decl_only {
                // PRECONDITION: the literal is what `query effects` prints.
                let id = r["stableId"].as_str().unwrap();
                let (q, qcode) = alsem(&[
                    "query",
                    "effects",
                    ws.to_str().unwrap(),
                    "--routine",
                    id,
                    "--format",
                    "json",
                ]);
                assert_eq!(qcode, 0, "{q}");
                assert_eq!(
                    q["payload"]["routine"]["routine"]["objectDisplay"], owner,
                    "precondition: query effects owner of {display}: {q}"
                );
            }
            got.push((
                file.to_string(),
                display.to_string(),
                r["objectDisplay"]
                    .as_str()
                    .unwrap_or("<missing>")
                    .to_string(),
            ));
            want.push((file.to_string(), display.to_string(), owner.to_string()));
        }
        docs.push_str(&format!("\n{file}: {doc}"));
    }
    got.sort();
    want.sort();
    assert_eq!(got, want, "digest objectDisplay per root:{docs}");
    for (file, display, od) in &got {
        assert!(
            !od.is_empty(),
            "{file} {display}: empty objectDisplay:{docs}"
        );
    }
}

/// A2. The fingerprint query block of the REAL table's `OnInsert` trigger names
/// `Table OD Item` (the identity table would say `OD Item Ext`).
///
/// Mutations: as A1 (i) -> `""`; (ii) -> `OD Item Ext`.
#[test]
fn fingerprint_block_names_its_owning_object() {
    let dir = tempfile::tempdir().unwrap();
    owner_workspace(dir.path());
    let ws = dir.path();
    assert_identity_table_holds_table_name(ws);
    let id = stable_id_of(ws, "src/a_Table.al", "OnInsert");
    assert!(id.starts_with(&format!("{GUID}:Table:50100#")), "{id}");

    // `--routine` makes it a query, so `--format json` yields blocks (not the
    // full-snapshot envelope).
    let (doc, code) = alsem(&[
        "fingerprint",
        ws.to_str().unwrap(),
        "--format",
        "json",
        "--routine",
        &id,
        "--deterministic",
    ]);
    assert_eq!(code, 0, "{doc}");
    let blocks: Vec<&Value> = doc["payload"]["blocks"]
        .as_array()
        .unwrap_or_else(|| panic!("blocks: {doc}"))
        .iter()
        .filter(|b| b["routine"]["stableId"] == id.as_str())
        .collect();
    assert_eq!(blocks.len(), 1, "precondition: one OnInsert block: {doc}");
    let b = blocks[0];
    assert_eq!(b["routine"]["display"], "OnInsert", "{doc}");
    assert_eq!(
        b["rootKinds"],
        serde_json::json!(["trigger-table"]),
        "precondition: OnInsert is root-classified: {doc}"
    );
    assert_eq!(b["routine"]["objectDisplay"], TABLE_OWNER, "{doc}");
}

/// A4. `alsem prove` on the real table's `OnInsert`, selected by its stable id,
/// names `Table OD Item`.
///
/// Mutations: as A1 (i) -> `""`; (ii) -> `OD Item Ext`.
#[test]
fn prove_names_its_owning_object() {
    let dir = tempfile::tempdir().unwrap();
    owner_workspace(dir.path());
    let ws = dir.path();
    assert_identity_table_holds_table_name(ws);
    let id = stable_id_of(ws, "src/a_Table.al", "OnInsert");
    assert!(id.starts_with(&format!("{GUID}:Table:50100#")), "{id}");

    let (doc, code) = alsem(&[
        "prove",
        ws.to_str().unwrap(),
        &id,
        "may-commit",
        "--deterministic",
    ]);
    // PRECONDITION: answered (exit 2 prints `objectDisplay: ""` for another reason).
    assert_eq!(code, 0, "{doc}");
    let r = &doc["payload"]["routine"];
    assert_eq!(r["stableId"], id.as_str(), "{doc}");
    assert_eq!(r["display"], "OnInsert", "{doc}");
    assert_eq!(r["objectDisplay"], TABLE_OWNER, "{doc}");
}

/// Spawn `alsem <args>` -> (stdout text, exit code).
fn alsem_text(args: &[&str]) -> (String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .env("ALCH_DRIVER_VERSION_OVERRIDE", VERSION_OVERRIDE)
        .output()
        .expect("spawn alsem");
    let stdout = String::from_utf8_lossy(&out.stdout).replace('\r', "");
    (stdout, out.status.code().unwrap_or(-1))
}

/// A5. The human headers of digest, fingerprint and prove print
/// `<owner>::<routine>` for the real table's `OnInsert` (digest and prove as
/// `--- … ---`, fingerprint as the block's first line).
///
/// Mutations: a renderer that prints only the routine, or the owner from the
/// identity table (`OD Item Ext`), fails its assert.
#[test]
fn human_headers_name_the_owning_object() {
    let dir = tempfile::tempdir().unwrap();
    owner_workspace(dir.path());
    let ws = dir.path();
    let w = ws.to_str().unwrap();
    let id = stable_id_of(ws, "src/a_Table.al", "OnInsert");
    let header = format!("{TABLE_OWNER}::OnInsert");

    let (digest, code) = alsem_text(&[
        "digest",
        w,
        "--file",
        "src/a_Table.al",
        "--deterministic",
        "--format",
        "human",
    ]);
    assert_eq!(code, 0, "{digest}");
    assert!(
        digest.lines().any(|l| l == format!("--- {header} ---")),
        "{digest}"
    );

    let (fp, code) = alsem_text(&[
        "fingerprint",
        w,
        "--routine",
        &id,
        "--deterministic",
        "--format",
        "human",
    ]);
    assert_eq!(code, 0, "{fp}");
    assert!(
        fp.lines().any(|l| l.starts_with(&format!("{header}  ["))),
        "{fp}"
    );

    let (prove, code) = alsem_text(&[
        "prove",
        w,
        &id,
        "may-commit",
        "--deterministic",
        "--format",
        "human",
    ]);
    assert_eq!(code, 0, "{prove}");
    assert!(
        prove.lines().any(|l| l == format!("--- {header} ---")),
        "{prove}"
    );
}

const SEL_A: &str = "Codeunit Sel A";
const SEL_B: &str = "Codeunit Sel B";

/// Two codeunits that both declare `Persist`, so the bare name is ambiguous.
fn same_name_workspace(dir: &Path) {
    write(
        &dir.join("app.json"),
        &format!(
            r#"{{"id":"{GUID}","name":"Issue78Sel","publisher":"probe","version":"1.0.0.0","platform":"1.0.0.0","application":"1.0.0.0","idRanges":[{{"from":50100,"to":50199}}],"runtime":"14.0"}}"#
        ),
    );
    for (file, num, name) in [("src/a.al", 50101, "Sel A"), ("src/b.al", 50102, "Sel B")] {
        write(
            &dir.join(file),
            &format!(
                "codeunit {num} \"{name}\"\n{{\n    procedure Persist()\n    begin\n        Message('p');\n    end;\n}}\n"
            ),
        );
    }
}

/// Spawn `alsem <args>` -> (stdout, stderr, exit code).
fn alsem_out(args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .env("ALCH_DRIVER_VERSION_OVERRIDE", VERSION_OVERRIDE)
        .output()
        .expect("spawn alsem");
    (
        String::from_utf8_lossy(&out.stdout).replace('\r', ""),
        String::from_utf8_lossy(&out.stderr).replace('\r', ""),
        out.status.code().unwrap_or(-1),
    )
}

/// A6 (CR1). A header copied as a selector, `<owner>::<routine>`, selects that
/// owner's routine in digest, fingerprint and prove, where the bare routine
/// name is ambiguous.
///
/// Mutation: drop the owner narrowing -> each qualified selector is
/// `selector-ambiguous` again (digest: no entry; fingerprint and prove: exit 2).
#[test]
fn owner_qualified_selector_narrows_to_its_owner() {
    let dir = tempfile::tempdir().unwrap();
    same_name_workspace(dir.path());
    let w = dir.path().to_str().unwrap();

    // PRECONDITION: the bare name matches both codeunits (no root: exit 2).
    let (d, code) = alsem(&["digest", w, "--routine", "Persist", "--deterministic"]);
    assert_eq!(code, 2, "{d}");
    assert_eq!(
        d["payload"]["changedRootsDiagnostics"][0]["kind"], "selector-ambiguous",
        "precondition: {d}"
    );

    let sel = format!("{SEL_A}::Persist");
    let (d, code) = alsem(&["digest", w, "--routine", &sel, "--deterministic"]);
    assert_eq!(code, 0, "{d}");
    assert_eq!(
        d["payload"]["changedRootsDiagnostics"],
        Value::Array(Vec::new()),
        "{d}"
    );
    let es = entries(&d);
    assert_eq!(es.len(), 1, "{d}");
    assert_eq!(es[0]["routine"]["objectDisplay"], SEL_A, "{d}");

    let (fp, code) = alsem(&[
        "fingerprint",
        w,
        "--routine",
        &sel,
        "--format",
        "json",
        "--deterministic",
    ]);
    assert_eq!(code, 0, "{fp}");
    let blocks = fp["payload"]["blocks"].as_array().unwrap();
    assert_eq!(blocks.len(), 1, "{fp}");
    assert_eq!(blocks[0]["routine"]["objectDisplay"], SEL_A, "{fp}");

    let (p, code) = alsem(&["prove", w, &sel, "may-commit", "--deterministic"]);
    assert_eq!(code, 0, "{p}");
    assert_eq!(p["payload"]["routine"]["objectDisplay"], SEL_A, "{p}");
}

/// A7 (CR2). An ambiguous selector's candidates name their owners, in the
/// digest diagnostic and in fingerprint's error lines.
///
/// Mutation: candidates print the bare routine -> `["Persist", "Persist"]` and
/// `  - Persist  (`.
#[test]
fn ambiguous_selector_candidates_name_their_owner() {
    let dir = tempfile::tempdir().unwrap();
    same_name_workspace(dir.path());
    let w = dir.path().to_str().unwrap();

    let (d, code) = alsem(&["digest", w, "--routine", "Persist", "--deterministic"]);
    assert_eq!(code, 2, "{d}");
    let diag = &d["payload"]["changedRootsDiagnostics"][0];
    assert_eq!(diag["kind"], "selector-ambiguous", "{d}");
    assert_eq!(
        diag["candidates"],
        serde_json::json!([format!("{SEL_A}::Persist"), format!("{SEL_B}::Persist")]),
        "{d}"
    );

    let (_out, err, code) = alsem_out(&[
        "fingerprint",
        w,
        "--routine",
        "Persist",
        "--format",
        "human",
        "--deterministic",
    ]);
    assert_eq!(code, 2, "{err}");
    for owner in [SEL_A, SEL_B] {
        assert!(
            err.lines()
                .any(|l| l.starts_with(&format!("  - {owner}::Persist  ("))),
            "{err}"
        );
    }
}

// ---------------------------------------------------------------------------
// #239: an `<owner>::<routine>` selector selects the named owner's routine or
// nothing; it never falls back to another object's routine.
// ---------------------------------------------------------------------------

/// `<header>` + one `procedure <routine>()`.
fn object_with(header: &str, body: &str, routine: &str) -> String {
    format!(
        "{header}\n{{\n{body}    procedure {routine}()\n    begin\n        Message('p');\n    end;\n}}\n"
    )
}

const TABLE_FIELDS: &str = "    fields\n    {\n        field(1; \"No.\"; Code[20]) { }\n    }\n\n";
const TABLE_SEL_B: &str = "Table Sel B";

/// An app whose `src/<file>` holds each `(file, header, body, routine)` object.
fn objects_workspace(dir: &Path, objects: &[(&str, &str, &str, &str)]) {
    write(
        &dir.join("app.json"),
        &format!(
            r#"{{"id":"{GUID}","name":"Issue239","publisher":"probe","version":"1.0.0.0","platform":"1.0.0.0","application":"1.0.0.0","idRanges":[{{"from":50100,"to":50199}}],"runtime":"14.0"}}"#
        ),
    );
    for (file, header, body, routine) in objects {
        write(
            &dir.join("src").join(file),
            &object_with(header, body, routine),
        );
    }
}

/// The issue's exact shape: only `Codeunit "Sel B"` declares `Persist`.
fn one_owner_workspace(dir: &Path) {
    objects_workspace(
        dir,
        &[
            ("a.al", "codeunit 50101 \"Sel A\"", "", "Other"),
            ("b.al", "codeunit 50102 \"Sel B\"", "", "Persist"),
        ],
    );
}

/// `Codeunit "Sel B"` and `Table "Sel B"` share the bare name `Sel B`;
/// `Table "Codeunit Sel B"`'s bare name spells the codeunit's full owner.
fn shared_name_workspace(dir: &Path) {
    objects_workspace(
        dir,
        &[
            ("a.al", "codeunit 50101 \"Sel B\"", "", "Persist"),
            ("b.al", "table 50111 \"Sel B\"", TABLE_FIELDS, "Persist"),
            (
                "c.al",
                "table 50112 \"Codeunit Sel B\"",
                TABLE_FIELDS,
                "Persist",
            ),
        ],
    );
}

/// CR1 / sol F2: `Codeunit "Sel B"` declares only `Other`; `Persist` belongs to
/// `Table "Codeunit Sel B"`, whose bare name spells the codeunit's full owner.
fn typed_owner_workspace(dir: &Path) {
    objects_workspace(
        dir,
        &[
            ("a.al", "codeunit 50101 \"Sel B\"", "", "Other"),
            (
                "c.al",
                "table 50112 \"Codeunit Sel B\"",
                TABLE_FIELDS,
                "Persist",
            ),
        ],
    );
}

/// What digest, fingerprint (JSON) and prove each did with `sel`: the exit
/// code, every selected routine as `<owner>|<stableId>`, and the diagnostics
/// (digest: kind and candidates; fingerprint: kind; prove: the answer). One
/// assert over all three shows every command's outcome, not only the first.
fn per_command(w: &str, sel: &str) -> [String; 3] {
    let s = |v: &Value| v.as_str().unwrap_or("").to_string();
    let routine = |r: &Value| format!("{}|{}", s(&r["objectDisplay"]), s(&r["stableId"]));
    let list = |v: &Value, f: &dyn Fn(&Value) -> String| {
        v.as_array()
            .map_or(Vec::new(), |a| a.iter().map(f).collect::<Vec<_>>())
            .join(", ")
    };
    let (d, dc) = alsem(&["digest", w, "--routine", sel, "--deterministic"]);
    let d_roots = list(&d["payload"]["entries"], &|e| routine(&e["routine"]));
    let d_diags = list(&d["payload"]["changedRootsDiagnostics"], &|g| {
        format!("{}({})", s(&g["kind"]), list(&g["candidates"], &s))
    });
    let (f, fc) = alsem(&[
        "fingerprint",
        w,
        "--routine",
        sel,
        "--format",
        "json",
        "--deterministic",
    ]);
    let f_roots = list(&f["payload"]["blocks"], &|b| routine(&b["routine"]));
    let f_diags = list(&f["payload"]["diagnostics"], &|g| s(&g["kind"]));
    let (p, pc) = alsem(&["prove", w, sel, "may-commit", "--deterministic"]);
    [
        format!("digest {dc} [{d_roots}] [{d_diags}]"),
        format!("fingerprint {fc} [{f_roots}] [{f_diags}]"),
        format!(
            "prove {pc} [{}] {}",
            routine(&p["payload"]["routine"]),
            s(&p["payload"]["result"]["answer"])
        ),
    ]
}

/// `per_command` when `sel` selects exactly `owner`'s routine `id`.
fn selects(owner: &str, id: &str) -> [String; 3] {
    [
        format!("digest 0 [{owner}|{id}] []"),
        format!("fingerprint 0 [{owner}|{id}] []"),
        format!("prove 0 [{owner}|{id}] no"),
    ]
}

/// `per_command` when `sel` matches no routine (prove's dummy routine echoes it).
fn unmatched(sel: &str) -> [String; 3] {
    [
        "digest 2 [] [selector-unmatched()]".to_string(),
        "fingerprint 2 [] [selector-unresolved]".to_string(),
        format!("prove 2 [|{sel}] unknown"),
    ]
}

/// A1 (#239, M1). An owner that declares no such routine matches nothing, in
/// all three commands: never `Codeunit Sel B`'s `Persist`. The human
/// fingerprint error says so and names the owner that does declare it (B5).
///
/// Breaks: restore the fallback -> digest, fingerprint and prove each select
/// `Codeunit Sel B|<id>` at exit 0 (one assert shows all three); drop the hint
/// -> the `declared as` assert fails.
#[test]
fn owner_qualified_selector_with_no_such_routine_is_unmatched() {
    let dir = tempfile::tempdir().unwrap();
    one_owner_workspace(dir.path());
    let w = dir.path().to_str().unwrap();
    let sel = format!("{SEL_A}::Persist");

    assert_eq!(per_command(w, &sel), unmatched(&sel));
    let (_out, err, code) = alsem_out(&[
        "fingerprint",
        w,
        "--routine",
        &sel,
        "--format",
        "human",
        "--deterministic",
    ]);
    assert_eq!(code, 2, "{err}");
    assert!(
        err.contains(&format!("error: --routine '{sel}' did not match")),
        "{err}"
    );
    assert!(
        err.contains(&format!(
            "\n  no routine of that name on the named owner; declared as: {SEL_B}::Persist"
        )),
        "{err}"
    );
}

/// A2 (#239, M2, M3, CR2). The bare object name, the AL-quoted owner and an
/// AL-quoted routine select Sel A's routine in all three commands.
///
/// Breaks: drop the bare-name match -> M2 unmatched; keep the quotes of the
/// owner (M3) or of the routine (CR2) -> unmatched.
#[test]
fn bare_and_quoted_owner_selectors_select_that_owner() {
    let dir = tempfile::tempdir().unwrap();
    same_name_workspace(dir.path());
    let ws = dir.path();
    let w = ws.to_str().unwrap();
    let id = stable_id_of(ws, "src/a.al", "Persist");

    for sel in [
        "Sel A::Persist",
        "Codeunit \"Sel A\"::Persist",
        "Codeunit \"Sel A\"::\"Persist\"",
    ] {
        assert_eq!(per_command(w, sel), selects(SEL_A, &id), "{sel}");
    }
}

/// A3 (#239, M4, M6). A bare name two owners share is ambiguous with both
/// owners as candidates; a typed owner selects only itself, never
/// `Table "Codeunit Sel B"` (whose bare name spells it).
///
/// Breaks: a typed object part also matches bare names -> M6 ambiguous; keep
/// only the first bare-name match -> M4 selects one routine.
#[test]
fn shared_bare_name_is_ambiguous_and_typed_owner_is_exact() {
    let dir = tempfile::tempdir().unwrap();
    shared_name_workspace(dir.path());
    let ws = dir.path();
    let w = ws.to_str().unwrap();

    assert_eq!(
        per_command(w, "Sel B::Persist"),
        [
            format!("digest 2 [] [selector-ambiguous({SEL_B}::Persist, {TABLE_SEL_B}::Persist)]"),
            "fingerprint 2 [] [selector-ambiguous]".to_string(),
            "prove 2 [|Sel B::Persist] unknown".to_string(),
        ]
    );
    let (_out, err, code) = alsem_out(&[
        "fingerprint",
        w,
        "--routine",
        "Sel B::Persist",
        "--format",
        "human",
        "--deterministic",
    ]);
    assert_eq!(code, 2, "{err}");
    let listed: Vec<&str> = err.lines().filter(|l| l.starts_with("  - ")).collect();
    assert_eq!(listed.len(), 2, "{err}");
    for owner in [SEL_B, TABLE_SEL_B] {
        assert!(
            listed
                .iter()
                .any(|l| l.starts_with(&format!("  - {owner}::Persist  ("))),
            "{err}"
        );
    }

    let id = stable_id_of(ws, "src/a.al", "Persist");
    assert_eq!(
        per_command(w, &format!("{SEL_B}::Persist")),
        selects(SEL_B, &id)
    );
}

/// A4 (#239, CR1, sol F2). A typed object part never matches by bare name:
/// with `Codeunit "Sel B"` declaring no `Persist`, `Codeunit "Sel B"::Persist`
/// is unmatched rather than `Table "Codeunit Sel B"`'s routine, which the
/// quoted bare name `"Codeunit Sel B"::Persist` does select.
///
/// Break: a typed object part also matches bare names -> the first assert
/// selects the table's routine in all three commands.
#[test]
fn typed_owner_never_matches_a_bare_name() {
    let dir = tempfile::tempdir().unwrap();
    typed_owner_workspace(dir.path());
    let ws = dir.path();
    let w = ws.to_str().unwrap();

    let sel = "Codeunit \"Sel B\"::Persist";
    assert_eq!(per_command(w, sel), unmatched(sel));
    let id = stable_id_of(ws, "src/c.al", "Persist");
    assert_eq!(
        per_command(w, "\"Codeunit Sel B\"::Persist"),
        selects("Table Codeunit Sel B", &id)
    );
}

/// A5 (#239, CR4). An empty object part (`::Persist`, the pre-#78 header) is the
/// bare routine name: it selects the one `Persist`.
///
/// Break: narrow an empty object part (compare `""` with the owners) -> unmatched.
#[test]
fn empty_owner_part_is_a_bare_routine_name() {
    let dir = tempfile::tempdir().unwrap();
    one_owner_workspace(dir.path());
    let ws = dir.path();
    let w = ws.to_str().unwrap();
    let id = stable_id_of(ws, "src/b.al", "Persist");

    assert_eq!(per_command(w, "::Persist"), selects(SEL_B, &id));
}

// ---------------------------------------------------------------------------
// #240 / #249: a quoted object name that holds an escaped quote (`"A ""B"" C"`)
// is shown by its LOGICAL name (`A "B" C`) everywhere, the name the ids use (#83).
// ---------------------------------------------------------------------------

const ESC_GUID: &str = "24024024-0000-0000-0000-000000000240";

/// The issue's workspace: an interface, a table and a codeunit whose names each
/// hold `""`; the codeunit implements the interface and its `P` inserts into the
/// table through a record variable written with the escaped spelling.
fn escaped_workspace(dir: &Path) {
    write(
        &dir.join("app.json"),
        &format!(
            r#"{{"id":"{ESC_GUID}","name":"Ws240","publisher":"probe","version":"1.0.0.0","platform":"1.0.0.0","application":"1.0.0.0","idRanges":[{{"from":50100,"to":50199}}],"runtime":"14.0"}}"#
        ),
    );
    write(
        &dir.join("src/Iface.Interface.al"),
        "interface \"A \"\"B\"\" C\"\n{\n    procedure P();\n}\n",
    );
    write(
        &dir.join("src/T.Table.al"),
        "table 50100 \"T \"\"Q\"\" X\"\n{\n    fields\n    {\n        field(1; \"No.\"; Code[20])\n        {\n        }\n    }\n}\n",
    );
    write(
        &dir.join("src/Cu.Codeunit.al"),
        "codeunit 50101 \"C \"\"Q\"\" U\" implements \"A \"\"B\"\" C\"\n{\n    procedure P()\n    var\n        R: Record \"T \"\"Q\"\" X\";\n    begin\n        R.Insert();\n    end;\n}\n",
    );
}

fn esc_table_id() -> String {
    format!("{ESC_GUID}/table/50100")
}

/// T1 (#240). `query effects` names the owner and the table by their logical
/// names, and the write still binds the real table.
///
/// Discrimination: revert the model unescape (`workspace.rs`, `name = o.name.clone()`)
/// -> `Codeunit C ""Q"" U`; fails. Restored, it passes.
#[test]
fn escaped_names_are_logical_in_query_effects() {
    let dir = tempfile::tempdir().unwrap();
    escaped_workspace(dir.path());
    let w = dir.path().to_str().unwrap();
    let (q, code) = alsem(&[
        "query",
        "effects",
        w,
        "--routine",
        "P",
        "--format",
        "json",
        "--deterministic",
    ]);
    assert_eq!(code, 0, "{q}");
    let p = &q["payload"];
    assert_eq!(
        p["routine"]["routine"]["objectDisplay"], "Codeunit C \"Q\" U",
        "{q}"
    );
    let effects = p["effects"].as_array().unwrap();
    assert_eq!(effects.len(), 1, "{q}");
    assert_eq!(effects[0]["tableName"], "T \"Q\" X", "{q}");
    assert_eq!(effects[0]["tableId"], esc_table_id(), "{q}");
    assert_eq!(
        effects[0]["origin"]["routine"]["objectDisplay"], "Codeunit C \"Q\" U",
        "{q}"
    );
}

/// T2 (#240). The digest names both owners and the written table by their
/// logical names; the write binds the real table.
///
/// Discrimination: the same revert -> `Interface A ""B"" C`; fails.
#[test]
fn escaped_names_are_logical_in_digest() {
    let dir = tempfile::tempdir().unwrap();
    escaped_workspace(dir.path());
    let ws = dir.path();
    let mut owners = Vec::new();
    let mut resources = Vec::new();
    for file in ["src/Cu.Codeunit.al", "src/Iface.Interface.al"] {
        let (doc, code) = digest_file(ws, file);
        assert_eq!(code, 0, "{doc}");
        for e in entries(&doc) {
            owners.push(e["routine"]["objectDisplay"].as_str().unwrap().to_string());
            for f in e["effects"].as_array().unwrap() {
                resources.push((
                    f["detail"]["resourceDisplay"]
                        .as_str()
                        .unwrap_or("")
                        .to_string(),
                    f["detail"]["resourceId"].as_str().unwrap_or("").to_string(),
                ));
            }
        }
    }
    owners.sort();
    assert_eq!(owners, vec!["Codeunit C \"Q\" U", "Interface A \"B\" C"]);
    assert_eq!(
        resources,
        vec![("T \"Q\" X".to_string(), esc_table_id())],
        "{resources:?}"
    );
}

/// The full fingerprint document of the escaped workspace.
fn escaped_fingerprint(ws: &Path) -> Value {
    let (doc, code) = alsem(&[
        "fingerprint",
        ws.to_str().unwrap(),
        "--format",
        "json",
        "--deterministic",
    ]);
    assert_eq!(code, 0, "{doc}");
    doc
}

/// T3 (#249). The identity table holds each object's logical name at its id.
///
/// Discrimination: the same revert -> `C ""Q"" U`; fails.
#[test]
fn escaped_names_are_logical_in_fingerprint_identities() {
    let dir = tempfile::tempdir().unwrap();
    escaped_workspace(dir.path());
    let doc = escaped_fingerprint(dir.path());
    let ids = doc["payload"]["identities"]["stableIds"]
        .as_array()
        .unwrap();
    let names = doc["payload"]["identities"]["displayNames"]
        .as_array()
        .unwrap();
    let name_of = |id: String| {
        let at = ids
            .iter()
            .position(|i| *i == id.as_str())
            .unwrap_or_else(|| panic!("`{id}` in identities: {doc}"));
        names[at].as_str().unwrap().to_string()
    };
    assert_eq!(name_of(format!("{ESC_GUID}:Codeunit:50101")), "C \"Q\" U");
    assert_eq!(
        name_of(format!("{ESC_GUID}:Interface:@a%20%22b%22%20c")),
        "A \"B\" C"
    );
    assert_eq!(name_of(format!("{ESC_GUID}:Table:50100")), "T \"Q\" X");
}

/// T4 (#249). The contracts and the table shape hash the logical name. The
/// literals are `sha256("Interface|0|A \"B\" C")`, `sha256("Codeunit|50101|C \"Q\" U")`,
/// `sha256("Table|50100|T \"Q\" X")` and `sha256({"name":"T \"Q\" X","number":50100})`,
/// computed outside the engine.
///
/// Discrimination: the same revert -> the raw hashes (`32465fdc…`, `936e2be9…`,
/// `b594178c…`, `c5147399…`); fails.
#[test]
fn escaped_names_are_logical_in_contract_and_shape_hashes() {
    let dir = tempfile::tempdir().unwrap();
    escaped_workspace(dir.path());
    let doc = escaped_fingerprint(dir.path());
    let contract = |id: String| {
        doc["payload"]["contractFacts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["kind"] == "object" && c["stableId"] == id.as_str())
            .unwrap_or_else(|| panic!("contract `{id}`: {doc}"))["signatureFingerprint"]
            .clone()
    };
    assert_eq!(
        contract(format!("{ESC_GUID}:Interface:@a%20%22b%22%20c")),
        "e9c79d08c44fa746cf5789db704e01146d67291e7228b0d33f050e449979c616"
    );
    assert_eq!(
        contract(format!("{ESC_GUID}:Codeunit:50101")),
        "f84fcc77443651f307b410f7ccea4d9d323f27c59f5598871eab95197f9a391b"
    );
    assert_eq!(
        contract(format!("{ESC_GUID}:Table:50100")),
        "c9718004312f70984d402cfee4699dd5d4e9773175b3ca51e137e262dfb98cc4"
    );
    let shape = doc["payload"]["schemaFacts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["kind"] == "table" && s["stableId"] == format!("{ESC_GUID}:Table:50100"))
        .unwrap_or_else(|| panic!("table schema fact: {doc}"))["shapeFingerprint"]
        .clone();
    assert_eq!(
        shape,
        "2a4b0802040e2ccd6d6779e6bd3fa4e7c1f158e6992f194ce03991b395cbea2c"
    );
}

/// T9. A record variable of a table that is not loaded is described by its
/// declared type text, unescaped once: d1's "type not loaded" text and d52's
/// fallback both read `Missing "Z" T`. Both detectors are default, so this runs
/// the real `alsem analyze`.
///
/// Discrimination: revert the unescape in `table_display::describe_table` tier 2
/// -> `Missing ""Z"" T (type not loaded)`; fails. Revert it in d52 -> d52's text
/// reads `(Missing ""Z"" T)`; fails. Each restored, it passes.
#[test]
fn unloaded_escaped_table_is_described_by_its_logical_name() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    escaped_workspace(ws);
    write(
        &ws.join("src/Del.Codeunit.al"),
        "codeunit 50102 \"Del Cu\"\n{\n    procedure Wipe(var R: Record \"Missing \"\"Z\"\" T\")\n    begin\n        R.DeleteAll();\n    end;\n\n    procedure Loop()\n    var\n        L: Record \"Missing \"\"Z\"\" T\";\n        i: Integer;\n    begin\n        for i := 1 to 10 do\n            L.Insert();\n    end;\n}\n",
    );
    let (doc, code) = alsem(&[
        "analyze",
        ws.to_str().unwrap(),
        "--format",
        "json",
        "--deterministic",
    ]);
    assert_eq!(code, 0, "{doc}");
    let causes: Vec<&str> = doc["payload"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["rootCause"].as_str())
        .collect();
    assert!(
        causes
            .iter()
            .any(|c| c.contains("Insert on Missing \"Z\" T (type not loaded).")),
        "d1: {causes:#?}"
    );
    assert!(
        causes
            .iter()
            .any(|c| c.contains("var record parameter R (Missing \"Z\" T) without")),
        "d52: {causes:#?}"
    );
}

// ---------------------------------------------------------------------------
// #241: an object split across `#if` arms with different names under one id
// (`codeunit 50100 "Foo"` / `codeunit 50100 "Bar"`): each routine's owner is the
// declaration that contains it, never the last arm's name for the id.
// ---------------------------------------------------------------------------

const SPLIT_GUID: &str = "24124124-0000-0000-0000-000000000241";
const SPLIT_FILE: &str = "src/Split.Codeunit.al";
const FOO: &str = "Codeunit Foo";
const BAR: &str = "Codeunit Bar";

/// One `#if` arm: `codeunit 50100 "<name>"` whose `<routine>` runs `<stmt>` on
/// the table.
fn split_arm(name: &str, routine: &str, stmt: &str) -> String {
    format!(
        "codeunit 50100 \"{name}\"\n{{\n    procedure {routine}()\n    var\n        R: Record \"Probe Rec\";\n    begin\n        {stmt};\n    end;\n}}\n"
    )
}

/// `Foo` (P: DeleteAll) and `Bar` (Q: ModifyAll) as the two arms of one `#if`,
/// `Foo` first unless `bar_first`.
fn split_workspace(dir: &Path, bar_first: bool) {
    write(
        &dir.join("app.json"),
        &format!(
            r#"{{"id":"{SPLIT_GUID}","name":"Probe241","publisher":"probe","version":"1.0.0.0","platform":"1.0.0.0","application":"1.0.0.0","idRanges":[{{"from":50100,"to":50199}}],"runtime":"14.0"}}"#
        ),
    );
    write(
        &dir.join("src/T.Table.al"),
        "table 50190 \"Probe Rec\"\n{\n    fields\n    {\n        field(1; \"No.\"; Code[20]) { }\n    }\n}\n",
    );
    let arm_p = split_arm("Foo", "P", "R.DeleteAll()");
    let arm_q = split_arm("Bar", "Q", "R.ModifyAll(\"No.\", 'X')");
    let (first, second) = if bar_first {
        (arm_q, arm_p)
    } else {
        (arm_p, arm_q)
    };
    write(
        &dir.join(SPLIT_FILE),
        &format!("#if A\n{first}#else\n{second}#endif\n"),
    );
}

/// `query effects --routine <routine>`: the header's owner and every effect
/// origin's owner name `owner`, and the human header reads `<owner>::<routine>`.
fn assert_query_effects_owner(w: &str, routine: &str, owner: &str) {
    let (q, code) = alsem(&[
        "query",
        "effects",
        w,
        "--routine",
        routine,
        "--format",
        "json",
        "--deterministic",
    ]);
    assert_eq!(code, 0, "{q}");
    let p = &q["payload"];
    assert_eq!(p["routine"]["routine"]["objectDisplay"], owner, "{q}");
    let effects = p["effects"].as_array().unwrap();
    assert_eq!(effects.len(), 1, "precondition: one effect line: {q}");
    assert_eq!(
        effects[0]["origin"]["routine"]["objectDisplay"], owner,
        "{q}"
    );

    let (text, code) = alsem_text(&[
        "query",
        "effects",
        w,
        "--routine",
        routine,
        "--deterministic",
    ]);
    assert_eq!(code, 0, "{text}");
    assert!(
        text.lines()
            .any(|l| l == format!("--- effects of {owner}::{routine} ---")),
        "{text}"
    );
}

/// The split file's digest: P's entry names `Foo`, Q's names `Bar`, and P's
/// human header reads `--- Codeunit Foo::P ---`.
fn assert_digest_owners(ws: &Path) {
    let (doc, code) = digest_file(ws, SPLIT_FILE);
    assert_eq!(code, 0, "{doc}");
    let mut got: Vec<(String, String)> = entries(&doc)
        .iter()
        .map(|e| {
            (
                e["routine"]["display"].as_str().unwrap().to_string(),
                e["routine"]["objectDisplay"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    got.sort();
    assert_eq!(
        got,
        vec![
            ("P".to_string(), FOO.to_string()),
            ("Q".to_string(), BAR.to_string())
        ],
        "{doc}"
    );
    let (text, code) = alsem_text(&[
        "digest",
        ws.to_str().unwrap(),
        "--file",
        SPLIT_FILE,
        "--deterministic",
        "--format",
        "human",
    ]);
    assert_eq!(code, 0, "{text}");
    assert!(
        text.lines().any(|l| l == format!("--- {FOO}::P ---")),
        "{text}"
    );
}

/// A1 (#241). `query effects` names each routine's own arm: P is `Foo`'s, Q is
/// `Bar`'s, in the header and on the effect line.
///
/// Discrimination: revert `RoutineJoin::of` to the id -> name map -> P reads
/// `Codeunit Bar`; fails.
#[test]
fn split_object_query_effects_names_each_routines_own_object() {
    let dir = tempfile::tempdir().unwrap();
    split_workspace(dir.path(), false);
    let w = dir.path().to_str().unwrap();
    assert_query_effects_owner(w, "P", FOO);
    assert_query_effects_owner(w, "Q", BAR);
}

/// A2 (#241). The digest names each routine's own arm, in JSON and in the human
/// header.
///
/// Discrimination: revert `OwnerIndex::build` to the id -> name map -> P's
/// entry reads `Codeunit Bar`; fails.
#[test]
fn split_object_digest_names_each_routines_own_object() {
    let dir = tempfile::tempdir().unwrap();
    split_workspace(dir.path(), false);
    assert_digest_owners(dir.path());
}

/// A3 (#241). The fingerprint block of P and prove on P name `Codeunit Foo`.
///
/// Discrimination: as A2 -> `Codeunit Bar`; fails.
#[test]
fn split_object_fingerprint_and_prove_name_the_declaring_object() {
    let dir = tempfile::tempdir().unwrap();
    split_workspace(dir.path(), false);
    let w = dir.path().to_str().unwrap();

    let (fp, code) = alsem(&[
        "fingerprint",
        w,
        "--routine",
        "P",
        "--format",
        "json",
        "--deterministic",
    ]);
    assert_eq!(code, 0, "{fp}");
    let blocks = fp["payload"]["blocks"].as_array().unwrap();
    assert_eq!(blocks.len(), 1, "{fp}");
    assert_eq!(blocks[0]["routine"]["objectDisplay"], FOO, "{fp}");
    let (text, code) = alsem_text(&[
        "fingerprint",
        w,
        "--routine",
        "P",
        "--deterministic",
        "--format",
        "human",
    ]);
    assert_eq!(code, 0, "{text}");
    assert!(
        text.lines().any(|l| l.starts_with(&format!("{FOO}::P  ["))),
        "{text}"
    );

    let (p, code) = alsem(&["prove", w, "P", "may-commit", "--deterministic"]);
    assert_eq!(code, 0, "{p}");
    assert_eq!(p["payload"]["routine"]["objectDisplay"], FOO, "{p}");
}

/// A4 (#241, closes #239 L1 for arms with different routines).
/// `Codeunit Foo::P` selects P in digest, fingerprint and prove (prove answers
/// `no`: P only deletes); `Codeunit Bar::P` matches nothing in all three.
///
/// Discrimination: as A2 -> `Codeunit Foo::P` is unmatched and
/// `Codeunit Bar::P` selects P; fails.
#[test]
fn split_object_owner_selector_names_the_declaring_object() {
    let dir = tempfile::tempdir().unwrap();
    split_workspace(dir.path(), false);
    let ws = dir.path();
    let w = ws.to_str().unwrap();
    let id = stable_id_of(ws, SPLIT_FILE, "P");

    assert_eq!(per_command(w, &format!("{FOO}::P")), selects(FOO, &id));
    let bar_p = format!("{BAR}::P");
    assert_eq!(per_command(w, &bar_p), unmatched(&bar_p));
}

/// A5 (#241). `query effects`' `Object.Routine` selector reads the declaring
/// object: `Foo.P` selects P, `Bar.P` matches nothing.
///
/// Discrimination: revert `resolve_routine_selector`'s map to the id -> name
/// map -> `Foo.P` is unmatched; fails.
#[test]
fn split_object_dotted_selector_names_the_declaring_object() {
    let dir = tempfile::tempdir().unwrap();
    split_workspace(dir.path(), false);
    let w = dir.path().to_str().unwrap();
    let query = |sel: &str| {
        alsem(&[
            "query",
            "effects",
            w,
            "--routine",
            sel,
            "--format",
            "json",
            "--deterministic",
        ])
    };

    let (q, code) = query("Foo.P");
    assert_eq!(code, 0, "{q}");
    let r = &q["payload"]["routine"];
    assert_eq!(r["resolution"], "resolved", "{q}");
    assert_eq!(r["routine"]["display"], "P", "{q}");
    assert_eq!(r["routine"]["objectDisplay"], FOO, "{q}");

    let (q, code) = query("Bar.P");
    assert_eq!(code, 2, "{q}");
    assert_eq!(q["payload"]["routine"]["resolution"], "unmatched", "{q}");
}

/// A6 (#241). With the arms swapped (`Bar` first), each routine still names its
/// own arm: the owner is found by declaration, not by arm order.
///
/// Discrimination: make the several-rows case pick the FIRST row -> P reads
/// `Codeunit Bar`; fails (A1 alone catches "last row wins").
#[test]
fn split_object_owner_does_not_follow_arm_order() {
    let dir = tempfile::tempdir().unwrap();
    split_workspace(dir.path(), true);
    let ws = dir.path();
    let w = ws.to_str().unwrap();
    assert_query_effects_owner(w, "P", FOO);
    assert_query_effects_owner(w, "Q", BAR);
    assert_digest_owners(ws);
}

/// A7 (#241, CR1). `alsem analyze`'s finding locations name each routine's own
/// arm: d33's finding on P's `DeleteAll` reads `objectName: "Foo"`, on Q's
/// `ModifyAll` `"Bar"`; both keep the one object id.
///
/// Discrimination: take `to_location`'s object from the id -> object map again
/// -> P's finding reads `Bar`; fails.
#[test]
fn split_object_analyze_locations_name_the_declaring_object() {
    let dir = tempfile::tempdir().unwrap();
    split_workspace(dir.path(), false);
    let (doc, code) = alsem(&[
        "analyze",
        dir.path().to_str().unwrap(),
        "--format",
        "json",
        "--deterministic",
    ]);
    assert_eq!(code, 0, "{doc}");
    let mut got: Vec<(String, String, String)> = doc["payload"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["detector"] == "d33-unfiltered-bulk-write")
        .map(|f| {
            let p = &f["primaryLocation"];
            let s = |k: &str| p[k].as_str().unwrap_or("<missing>").to_string();
            (s("routineName"), s("objectName"), s("objectId"))
        })
        .collect();
    got.sort();
    let id = format!("{SPLIT_GUID}/Codeunit/50100");
    assert_eq!(
        got,
        vec![
            ("P".to_string(), "Foo".to_string(), id.clone()),
            ("Q".to_string(), "Bar".to_string(), id),
        ],
        "{doc}"
    );
}
