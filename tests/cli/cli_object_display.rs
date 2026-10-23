//! #78: every digest entry, fingerprint block and prove document names its
//! owning object as `"<ObjectType> <ObjectName>"`, the same value
//! `alsem query effects` prints (`RoutineJoin::of`), taken from the Model and
//! never from the snapshot's identity table.
//!
//! The fixture holds a Table and a TableExtension with the SAME number (50100),
//! the extension's file sorting after the table's, so the identity table keeps
//! the extension's name at `<guid>:Table:50100`. A test that took the owner from
//! the identity table would print `OD Item Ext` for the real table's routines.
//! The two Interfaces check the declaration-only rows' display. Since #83 they
//! have distinct ids (`<guid>:Interface:@ifoo`, `…:@ibar`), so they no longer
//! catch an identity-table owner; the Table/TableExtension pair does.

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

/// PRECONDITION shared by A1/A2/A4: the snapshot's identity table holds the
/// EXTENSION's name at the real table's id, so an identity-table owner would be
/// caught.
fn assert_identity_table_holds_extension_name(ws: &Path) {
    let model_id = compute_gate_model_instance_id(ws).unwrap();
    let inputs = al_sem::program::inputs::InputSet::read(ws, &Default::default());
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
        ids.display_names[at], "OD Item Ext",
        "precondition: the extension stub overwrote the table's identity name"
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
    assert_identity_table_holds_extension_name(ws);

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
    assert_identity_table_holds_extension_name(ws);
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
    assert_identity_table_holds_extension_name(ws);
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
/// fingerprint error says so.
///
/// Break: restore the fallback -> digest, fingerprint and prove each select
/// `Codeunit Sel B|<id>` at exit 0 (one assert shows all three).
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
