//! #71: `alsem digest --file` / `--diff` resolve changed roots by routine
//! DECLARATION SPAN (the attribute lines, the header and the whole body), not by
//! the start lines of the snapshot's operation / call-site evidence; hunk bodies
//! are read line by line and bounded by the header counts; Interface and
//! ControlAddIn procedures are declaration-only roots; every entry is anchored at
//! its declaration.
//!
//! Every case spawns `CARGO_BIN_EXE_alsem` (the clap arm included), except the
//! prove anchor (`run_prove_pipeline`) and the parser-level counts of the
//! bare-header rows (`parse_unified_diff`, the public parser `--diff` uses).
//! Diff line numbers are 1-based; anchor lines in the JSON are 0-based.
//! Ledger: `.agent/issue-71/ledger.md` (Design, Acceptance matrix).

use std::path::Path;
use std::process::Command;

use serde_json::Value;

use al_sem::engine::gate::model_instance_id::compute_gate_model_instance_id;
use al_sem::engine::ids::{
    normalized_signature_hash, to_stable_object_id, to_stable_routine_id_from_parts,
};
use al_sem::engine::l5::diff_parser::parse_unified_diff;
use al_sem::engine::l5::prove::run_prove_pipeline;
use al_sem::engine::l5::snapshot::compose_snapshot;
use al_sem::program::model::program_calls::assemble_and_resolve_workspace_program;

const VERSION_OVERRIDE: &str = "cli-spans-v1";

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn corpus(fixture: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus")
        .join(fixture)
}

/// The d8 fixture's `src/posting.al`, LF, as committed. `PostSalesDoc` is lines
/// 3-16, `[IntegrationEvent]` is line 18, `OnAfterPostSalesDoc` lines 19-21.
fn d8_posting() -> String {
    std::fs::read_to_string(corpus("ws-d8-commit-in-tx").join("src/posting.al"))
        .unwrap()
        .replace('\r', "")
}

/// Copy `ws-d8-commit-in-tx` into `dir`, with `posting.al` replaced by `posting`
/// (the post-change file text).
fn d8_workspace(dir: &Path, posting: &str) {
    let src = corpus("ws-d8-commit-in-tx");
    write(
        &dir.join("app.json"),
        &std::fs::read_to_string(src.join("app.json")).unwrap(),
    );
    write(
        &dir.join("src/subscriber.al"),
        &std::fs::read_to_string(src.join("src/subscriber.al"))
            .unwrap()
            .replace('\r', ""),
    );
    write(&dir.join("src/posting.al"), posting);
}

/// 1-based line `n` of `text`.
fn line(text: &str, n: usize) -> &str {
    text.split('\n').nth(n - 1).unwrap()
}

/// `text` without its 1-based lines `from..=to`.
fn without_lines(text: &str, from: usize, to: usize) -> String {
    text.split('\n')
        .enumerate()
        .filter(|(i, _)| *i + 1 < from || *i + 1 > to)
        .map(|(_, l)| l)
        .collect::<Vec<_>>()
        .join("\n")
}

/// `alsem digest <ws> <args> --deterministic --format json` -> (doc, exit code).
/// A `diff` is written next to the workspace and passed as `--diff <path>`.
fn digest(ws: &Path, args: &[&str], diff: Option<&str>) -> (Value, i32) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_alsem"));
    cmd.arg("digest").arg(ws).args(args);
    let diff_dir = tempfile::tempdir().unwrap();
    if let Some(d) = diff {
        let p = diff_dir.path().join("change.diff");
        std::fs::write(&p, d).unwrap();
        cmd.arg("--diff").arg(&p);
    }
    let out = cmd
        .args(["--deterministic", "--format", "json"])
        .env("ALCH_DRIVER_VERSION_OVERRIDE", VERSION_OVERRIDE)
        .output()
        .expect("spawn alsem");
    let stdout = String::from_utf8_lossy(&out.stdout).replace('\r', "");
    let doc: Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "{e}: stdout={stdout} stderr={}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (doc, out.status.code().unwrap_or(-1))
}

fn entries(doc: &Value) -> Vec<Value> {
    doc["payload"]["entries"].as_array().unwrap().clone()
}

/// The sorted display names of the digest entries (one per root).
fn roots(doc: &Value) -> Vec<String> {
    let mut r: Vec<String> = entries(doc)
        .iter()
        .map(|e| e["routine"]["display"].as_str().unwrap().to_string())
        .collect();
    r.sort();
    r
}

fn diags(doc: &Value) -> Vec<Value> {
    doc["payload"]["changedRootsDiagnostics"]
        .as_array()
        .unwrap()
        .clone()
}

fn diag_kinds(doc: &Value) -> Vec<String> {
    diags(doc)
        .iter()
        .map(|d| d["kind"].as_str().unwrap().to_string())
        .collect()
}

fn entry<'a>(doc: &'a Value, display: &str) -> &'a Value {
    let all = doc["payload"]["entries"].as_array().unwrap();
    let hits: Vec<&Value> = all
        .iter()
        .filter(|e| e["routine"]["display"] == display)
        .collect();
    assert_eq!(hits.len(), 1, "one `{display}` entry expected: {doc}");
    hits[0]
}

/// (file, line, column) of an entry's singular anchor.
fn anchor(e: &Value) -> (String, i64, i64) {
    let a = &e["routine"]["anchor"];
    (
        a["file"].as_str().unwrap_or("").to_string(),
        a["line"].as_i64().unwrap_or(-1),
        a["column"].as_i64().unwrap_or(-1),
    )
}

/// A one-line modification of `path`'s 1-based line `n` (the source on disk is
/// the post-change file; only the line numbers matter to root resolution).
fn one_line_diff(path: &str, n: usize, text: &str) -> String {
    format!("--- a/{path}\n+++ b/{path}\n@@ -{n},1 +{n},1 @@\n-{text} \n+{text}\n")
}

// ---------------------------------------------------------------------------
// A1a
// ---------------------------------------------------------------------------

/// A1a. A hunk on an op-less body line makes the routine a root.
///
/// Precondition: line 9 is `Header."Status Posted" := true;` and the capability
/// snapshot holds no operation and no call site on that line (0-based row 8), so
/// the pre-#71 evidence index could not see it.
/// Mutation: rebuild the index from evidence start lines (the old rule) -> exit 2.
#[test]
fn hunk_on_assignment_line_is_a_changed_root() {
    let dir = tempfile::tempdir().unwrap();
    let posting = d8_posting();
    assert_eq!(
        line(&posting, 9),
        "        Header.\"Status Posted\" := true;"
    );
    d8_workspace(dir.path(), &posting);

    let model_id = compute_gate_model_instance_id(dir.path()).unwrap();
    let model = assemble_and_resolve_workspace_program(dir.path(), &model_id, false).unwrap();
    let snap = compose_snapshot(&model);
    let on_row_8 =
        |file: &str, row: u32| file.to_lowercase().ends_with("src/posting.al") && row == 8;
    assert!(
        !snap
            .operation_index
            .iter()
            .any(|o| on_row_8(&o.source_file, o.start_line))
            && !snap
                .callsite_index
                .iter()
                .any(|c| on_row_8(&c.source_file, c.start_line)),
        "precondition: no evidence on row 8"
    );

    let diff = one_line_diff(
        "src/posting.al",
        9,
        "        Header.\"Status Posted\" := true;",
    );
    let (doc, code) = digest(dir.path(), &[], Some(&diff));
    assert_eq!(code, 0, "{doc}");
    assert_eq!(roots(&doc), vec!["PostSalesDoc"], "{doc}");
    assert!(diags(&doc).is_empty(), "{doc}");
}

// ---------------------------------------------------------------------------
// A1b
// ---------------------------------------------------------------------------

/// Every routine shape the model holds: object trigger, subscriber with its
/// attribute, empty publisher, an `#if` duplicate (two declarations, one id),
/// an `#if`-split header (one span, two ids), table field trigger, table
/// trigger, page field trigger, action trigger, report dataitem trigger.
const SHAPES: &str = "codeunit 50200 ShapesCu
{
    trigger OnRun()
    begin
        Message('run');
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::ShapesCu, 'OnPub', '', false, false)]
    local procedure Sub()
    begin
        Message('sub');
    end;

    [IntegrationEvent(false, false)]
    procedure OnPub()
    begin
    end;

#if CLEAN25
    procedure Dup(): Integer
    begin
        exit(1);
    end;
#else
    procedure Dup(): Integer
    begin
        exit(2);
    end;
#endif

#if CLEAN25
    procedure Split(A: Integer)
#else
    procedure Split(A: Integer; B: Integer)
#endif
    begin
        Message('split');
    end;
}

table 50200 ShapesTab
{
    fields
    {
        field(1; \"No.\"; Code[20])
        {
            trigger OnValidate()
            begin
                Message('validate');
            end;
        }
    }

    trigger OnInsert()
    begin
        Message('insert');
    end;
}

page 50200 ShapesPage
{
    SourceTable = ShapesTab;

    layout
    {
        area(Content)
        {
            field(No; Rec.\"No.\")
            {
                trigger OnValidate()
                begin
                    Message('pv');
                end;
            }
        }
    }

    actions
    {
        area(Processing)
        {
            action(Go)
            {
                trigger OnAction()
                begin
                    Message('go');
                end;
            }
        }
    }

    var
        GlobalCount: Integer;
}

report 50200 ShapesRep
{
    dataset
    {
        dataitem(D; ShapesTab)
        {
            trigger OnAfterGetRecord()
            begin
                Message('rec');
            end;
        }
    }
}
";

fn shapes_workspace(dir: &Path, shapes: &str) {
    write(
        &dir.join("app.json"),
        r#"{"id":"7a1e0000-0000-0000-0000-000000000071","name":"Issue71Shapes","publisher":"probe","version":"1.0.0.0","platform":"1.0.0.0","application":"1.0.0.0","idRanges":[{"from":50200,"to":50299}],"runtime":"14.0"}"#,
    );
    write(&dir.join("src/Shapes.al"), shapes);
}

/// A1b. Every modelled routine shape, every line of its span.
///
/// Mutations: (i) start the span at `source_anchor.start_line` -> the attribute
/// rows fail; (ii) `end` exclusive -> the `end;` rows fail; (iii) one span per id
/// -> the first-`Dup`-arm row fails; (iv) the first split arm only -> the `Split`
/// row fails.
#[test]
fn every_routine_shape_line_maps_to_its_routine() {
    // (file text, path, 1-based line, its exact text, expected sorted roots)
    let posting = d8_posting();
    let rows: &[(&str, &str, usize, &str, &[&str])] = &[
        (
            &posting,
            "src/posting.al",
            3,
            "    procedure PostSalesDoc()",
            &["PostSalesDoc"],
        ),
        (&posting, "src/posting.al", 4, "    var", &["PostSalesDoc"]),
        (
            &posting,
            "src/posting.al",
            7,
            "    begin",
            &["PostSalesDoc"],
        ),
        (
            &posting,
            "src/posting.al",
            16,
            "    end;",
            &["PostSalesDoc"],
        ),
        (
            &posting,
            "src/posting.al",
            18,
            "    [IntegrationEvent(false, false)]",
            &["OnAfterPostSalesDoc"],
        ),
        (
            &posting,
            "src/posting.al",
            20,
            "    begin",
            &["OnAfterPostSalesDoc"],
        ),
        (
            SHAPES,
            "src/Shapes.al",
            3,
            "    trigger OnRun()",
            &["OnRun"],
        ),
        (SHAPES, "src/Shapes.al", 6, "    end;", &["OnRun"]),
        (
            SHAPES,
            "src/Shapes.al",
            8,
            "    [EventSubscriber(ObjectType::Codeunit, Codeunit::ShapesCu, 'OnPub', '', false, false)]",
            &["Sub"],
        ),
        (
            SHAPES,
            "src/Shapes.al",
            14,
            "    [IntegrationEvent(false, false)]",
            &["OnPub"],
        ),
        (SHAPES, "src/Shapes.al", 16, "    begin", &["OnPub"]),
        (SHAPES, "src/Shapes.al", 21, "    begin", &["Dup"]),
        (SHAPES, "src/Shapes.al", 26, "    begin", &["Dup"]),
        (
            SHAPES,
            "src/Shapes.al",
            37,
            "        Message('split');",
            &["Split", "Split"],
        ),
        (
            SHAPES,
            "src/Shapes.al",
            47,
            "            trigger OnValidate()",
            &["OnValidate"],
        ),
        (SHAPES, "src/Shapes.al", 55, "    begin", &["OnInsert"]),
        (
            SHAPES,
            "src/Shapes.al",
            73,
            "                end;",
            &["OnValidate"],
        ),
        (
            SHAPES,
            "src/Shapes.al",
            85,
            "                begin",
            &["OnAction"],
        ),
        (
            SHAPES,
            "src/Shapes.al",
            105,
            "            end;",
            &["OnAfterGetRecord"],
        ),
    ];
    let d8 = tempfile::tempdir().unwrap();
    d8_workspace(d8.path(), &posting);
    let shapes = tempfile::tempdir().unwrap();
    shapes_workspace(shapes.path(), SHAPES);

    let mut failures = Vec::new();
    for (text, path, n, want_text, want) in rows {
        assert_eq!(line(text, *n), *want_text, "{path}:{n} fixture text");
        let ws = if *path == "src/posting.al" {
            d8.path()
        } else {
            shapes.path()
        };
        let (doc, code) = digest(ws, &[], Some(&one_line_diff(path, *n, want_text)));
        let got = roots(&doc);
        if got != *want || code != 0 {
            failures.push(format!(
                "{path}:{n} `{want_text}`: want {want:?}, got {got:?} exit {code} diags {:?}",
                diags(&doc)
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));

    // (ii) alone: an ADDED-only hunk on the routine's last line (`end;`, line 16).
    // No `-` line, so no `DeletedAfter` can stand in for an end-exclusive `Added`.
    let (doc, code) = digest(
        d8.path(),
        &[],
        Some("--- a/src/posting.al\n+++ b/src/posting.al\n@@ -15,0 +16,1 @@\n+    end;\n"),
    );
    assert_eq!(
        (code, roots(&doc)),
        (0, vec!["PostSalesDoc".to_string()]),
        "added-only `end;`: {doc}"
    );

    // The two `OnValidate`s (table field, page field) are different routines.
    let (t, _) = digest(
        shapes.path(),
        &[],
        Some(&one_line_diff(
            "src/Shapes.al",
            49,
            "                Message('validate');",
        )),
    );
    let (p, _) = digest(
        shapes.path(),
        &[],
        Some(&one_line_diff(
            "src/Shapes.al",
            72,
            "                    Message('pv');",
        )),
    );
    assert_ne!(
        entry(&t, "OnValidate")["routine"]["stableId"],
        entry(&p, "OnValidate")["routine"]["stableId"]
    );
}

// ---------------------------------------------------------------------------
// A1c-A1f, A1h, A1i
// ---------------------------------------------------------------------------

/// A1c. Context lines create no roots: only line 17 (the blank line between
/// `PostSalesDoc` and the attribute) changes; the header range 14-20 overlaps
/// both routines.
///
/// Mutation: match the header range -> `PostSalesDoc` (and `OnAfterPostSalesDoc`)
/// become roots.
#[test]
fn context_lines_do_not_create_roots() {
    let dir = tempfile::tempdir().unwrap();
    let posting = d8_posting();
    assert_eq!(line(&posting, 17), "");
    d8_workspace(dir.path(), &posting);
    let diff = "--- a/src/posting.al
+++ b/src/posting.al
@@ -14,7 +14,7 @@
         Header.\"Last Posting Date\" := Today;
         Header.Modify();
     end;
-
+
     [IntegrationEvent(false, false)]
     procedure OnAfterPostSalesDoc(Header: Record \"Sales Header\")
     begin
";
    let (doc, code) = digest(dir.path(), &[], Some(diff));
    assert_eq!(code, 2, "{doc}");
    assert!(roots(&doc).is_empty(), "{doc}");
    let d = diags(&doc);
    assert_eq!(d.len(), 1, "{doc}");
    assert_eq!(d[0]["kind"], "hunk-outside-routines");
    assert_eq!(d[0]["startLine"], 14);
    assert_eq!(d[0]["endLine"], 20);
}

/// A1d. Deletions resolve by the interior gap only. Each case's source on disk
/// is the POST-change file.
///
/// Mutations: (i) the cursor starts at `new_start` even when `new_count == 0` ->
/// (b) gives `PostSalesDoc`; (ii) a gap touches a span when L or L+1 is in it ->
/// (c) gives `PostSalesDoc`.
#[test]
fn deletions_resolve_by_interior_gap() {
    let posting = d8_posting();
    let run = |post: &str, diff: &str| {
        let dir = tempfile::tempdir().unwrap();
        d8_workspace(dir.path(), post);
        digest(dir.path(), &[], Some(diff))
    };

    // (a) delete body line 9: the gap between new lines 8 and 9 is interior.
    let (doc, code) = run(
        &without_lines(&posting, 9, 9),
        "--- a/src/posting.al\n+++ b/src/posting.al\n@@ -9,1 +8,0 @@\n-        Header.\"Status Posted\" := true;\n",
    );
    assert_eq!(
        (code, roots(&doc)),
        (0, vec!["PostSalesDoc".to_string()]),
        "(a) {doc}"
    );

    // (b) delete the blank line after `PostSalesDoc`'s `end;` (old line 17).
    let (doc, code) = run(
        &without_lines(&posting, 17, 17),
        "--- a/src/posting.al\n+++ b/src/posting.al\n@@ -17,1 +16,0 @@\n-\n",
    );
    assert_eq!(code, 2, "(b) {doc}");
    assert!(roots(&doc).is_empty(), "(b) {doc}");
    assert_eq!(diag_kinds(&doc), vec!["hunk-outside-routines"], "(b) {doc}");

    // (c) delete old lines 17-21: the blank line and the whole publisher.
    let post_c = without_lines(&posting, 17, 21);
    assert_eq!(line(&post_c, 16), "    end;");
    assert_eq!(line(&post_c, 17), "}");
    let (doc, code) = run(
        &post_c,
        "--- a/src/posting.al\n+++ b/src/posting.al\n@@ -17,5 +16,0 @@\n-\n-    [IntegrationEvent(false, false)]\n-    procedure OnAfterPostSalesDoc(Header: Record \"Sales Header\")\n-    begin\n-    end;\n",
    );
    assert_eq!(code, 2, "(c) {doc}");
    assert!(roots(&doc).is_empty(), "(c) {doc}");

    // (d) delete old line 1 (a comment above the codeunit); the post file is d8.
    let (doc, code) = run(
        &posting,
        "--- a/src/posting.al\n+++ b/src/posting.al\n@@ -1,1 +0,0 @@\n-// old header\n",
    );
    assert_eq!(code, 2, "(d) {doc}");
    assert!(roots(&doc).is_empty(), "(d) {doc}");

    // (e) delete the old last line (a trailing comment after the last table).
    let last = posting.trim_end_matches('\n').split('\n').count();
    assert_eq!(line(&posting, last), "}");
    let (doc, code) = run(
        &posting,
        &format!(
            "--- a/src/posting.al\n+++ b/src/posting.al\n@@ -{},1 +{last},0 @@\n-// trailer\n",
            last + 1
        ),
    );
    assert_eq!(code, 2, "(e) {doc}");
    assert!(roots(&doc).is_empty(), "(e) {doc}");
}

/// A1e. An object-level hunk (a table field line) stays outside every routine.
///
/// Mutation: map any hunk in a file to all its routines -> roots appear.
#[test]
fn object_level_hunk_is_outside_routines() {
    let dir = tempfile::tempdir().unwrap();
    let posting = d8_posting();
    let field = "        field(50; \"Status Posted\"; Boolean) { }";
    assert_eq!(line(&posting, 29), field);
    d8_workspace(dir.path(), &posting);
    let (doc, code) = digest(
        dir.path(),
        &[],
        Some(&one_line_diff("src/posting.al", 29, field)),
    );
    assert_eq!(code, 2, "{doc}");
    assert!(roots(&doc).is_empty(), "{doc}");
    assert_eq!(diag_kinds(&doc), vec!["hunk-outside-routines"], "{doc}");
}

/// A1f. One hunk inside a routine and one outside, in one file: a root AND the
/// diagnostic for the outside hunk, exit 0.
///
/// Mutations: drop the per-hunk diagnostic when the file has any root -> the
/// diagnostic is missing; reset the cursor per file instead of per hunk -> the
/// second hunk mis-resolves.
#[test]
fn one_inside_one_outside_hunk() {
    let dir = tempfile::tempdir().unwrap();
    let posting = d8_posting();
    d8_workspace(dir.path(), &posting);
    let diff = "--- a/src/posting.al
+++ b/src/posting.al
@@ -9,1 +9,1 @@
-        Header.\"Status Posted\" := false;
+        Header.\"Status Posted\" := true;
@@ -29,1 +29,1 @@
-        field(50; \"Status Posted\"; Integer) { }
+        field(50; \"Status Posted\"; Boolean) { }
";
    let (doc, code) = digest(dir.path(), &[], Some(diff));
    assert_eq!(code, 0, "{doc}");
    assert_eq!(roots(&doc), vec!["PostSalesDoc"], "{doc}");
    let d = diags(&doc);
    assert_eq!(d.len(), 1, "{doc}");
    assert_eq!(d[0]["kind"], "hunk-outside-routines");
    assert_eq!(d[0]["startLine"], 29);
    assert_eq!(d[0]["endLine"], 29);
}

/// A1h. Renames: with a hunk, only the touched routine; without one, every
/// routine of the file.
///
/// Mutation: treat a rename-with-hunks as whole-file -> `OnAfterPostSalesDoc`
/// joins the first case.
#[test]
fn rename_with_and_without_hunks() {
    let dir = tempfile::tempdir().unwrap();
    d8_workspace(dir.path(), &d8_posting());
    let head = "diff --git a/src/old.al b/src/posting.al
similarity index 90%
rename from src/old.al
rename to src/posting.al
";
    let with_hunk = format!(
        "{head}--- a/src/old.al\n+++ b/src/posting.al\n@@ -9,1 +9,1 @@\n-        Header.\"Status Posted\" := false;\n+        Header.\"Status Posted\" := true;\n"
    );
    let (doc, code) = digest(dir.path(), &[], Some(&with_hunk));
    assert_eq!(code, 0, "{doc}");
    assert_eq!(roots(&doc), vec!["PostSalesDoc"], "{doc}");

    let pure = "diff --git a/src/old.al b/src/posting.al
similarity index 100%
rename from src/old.al
rename to src/posting.al
";
    let (doc, code) = digest(dir.path(), &[], Some(pure));
    assert_eq!(code, 0, "{doc}");
    assert_eq!(
        roots(&doc),
        vec!["OnAfterPostSalesDoc", "PostSalesDoc"],
        "{doc}"
    );
}

/// Final panel (code-review): deleting a routine's FIRST (or only) attribute line
/// makes the routine a root. The gap sits right above the span, and the deleted
/// text, all attribute lines, belongs to the routine below it. (a) delete the
/// only attribute `[IntegrationEvent(false, false)]` (line 18) of
/// `OnAfterPostSalesDoc` -> root. (b) the rule is narrow: the same gap with a
/// deleted blank line in the run (lines 17-18) is a plain deletion -> no root.
///
/// Mutations: (a) treat `DeletedAttributesAfter` like `DeletedAfter` (interior
/// only) -> no root, `hunk-outside-routines`; (b) classify the run by its LAST
/// deleted line -> `OnAfterPostSalesDoc` becomes a root. The parser's run rule
/// itself is pinned by `diff_parser::tests::attribute_only_deletion_runs`.
#[test]
fn deleting_a_routines_first_attribute_line_makes_it_a_root() {
    let posting = d8_posting();
    assert_eq!(line(&posting, 17), "");
    assert_eq!(line(&posting, 18), "    [IntegrationEvent(false, false)]");

    let dir = tempfile::tempdir().unwrap();
    d8_workspace(dir.path(), &without_lines(&posting, 18, 18));
    let (doc, code) = digest(
        dir.path(),
        &[],
        Some(
            "--- a/src/posting.al\n+++ b/src/posting.al\n@@ -18,1 +17,0 @@\n-    [IntegrationEvent(false, false)]\n",
        ),
    );
    assert_eq!(code, 0, "(a) {doc}");
    assert_eq!(roots(&doc), vec!["OnAfterPostSalesDoc"], "(a) {doc}");
    assert!(diags(&doc).is_empty(), "(a) {doc}");

    let dir = tempfile::tempdir().unwrap();
    d8_workspace(dir.path(), &without_lines(&posting, 17, 18));
    let (doc, code) = digest(
        dir.path(),
        &[],
        Some(
            "--- a/src/posting.al\n+++ b/src/posting.al\n@@ -17,2 +16,0 @@\n-\n-    [IntegrationEvent(false, false)]\n",
        ),
    );
    assert_eq!(code, 2, "(b) {doc}");
    assert!(roots(&doc).is_empty(), "(b) {doc}");
    assert_eq!(diag_kinds(&doc), vec!["hunk-outside-routines"], "(b) {doc}");
}

/// A1i. The stated limit stays pinned; a change of it is deliberate: a `#if`
/// directive line around duplicated procedures is outside every span.
/// Mutation: extend spans to the enclosing `#if` -> `Dup` becomes a root.
#[test]
fn limits_are_pinned() {
    let shapes = SHAPES.replacen(
        "#if CLEAN25\n    procedure Dup",
        "#if CLEAN26\n    procedure Dup",
        1,
    );
    assert_eq!(line(&shapes, 19), "#if CLEAN26");
    assert_eq!(line(&shapes, 31), "#if CLEAN25");
    let dir = tempfile::tempdir().unwrap();
    shapes_workspace(dir.path(), &shapes);
    let (doc, code) = digest(
        dir.path(),
        &[],
        Some(
            "--- a/src/Shapes.al\n+++ b/src/Shapes.al\n@@ -19,1 +19,1 @@\n-#if CLEAN25\n+#if CLEAN26\n",
        ),
    );
    assert_eq!(code, 2, "(b) {doc}");
    assert!(roots(&doc).is_empty(), "(b) {doc}");
    assert_eq!(diag_kinds(&doc), vec!["hunk-outside-routines"], "(b) {doc}");
}

// ---------------------------------------------------------------------------
// A1g: count-bounded hunk bodies
// ---------------------------------------------------------------------------

/// A1g. Hunk bodies are read line by line, bounded by the header counts.
///
/// Mutations: (a) header tests before the body state -> a phantom file `y`;
/// (b) skip empty lines -> the `+` lands on the blank line 17, no root;
/// (c) read the final split element as a blank context line -> the
/// with-newline case reports no error; (c1) drop the terminating line ->
/// `HandlePosted` missing; header-range fallback -> a context-only root;
/// (e) count `\ No newline` as context -> a parse error.
#[test]
fn hunk_body_is_count_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let posting = d8_posting();
    d8_workspace(dir.path(), &posting);
    let ws = dir.path();
    let sub = std::fs::read_to_string(ws.join("src/subscriber.al")).unwrap();
    assert_eq!(
        line(&sub, 9),
        "        Audit.\"Doc No.\" := Header.\"No.\";"
    );

    // (a) a deleted line `-- x` and an added line `++ y` are body lines.
    let a = "--- a/src/posting.al\n+++ b/src/posting.al\n@@ -9,1 +9,1 @@\n--- x\n+++ y\n";
    let parsed = parse_unified_diff(a);
    assert_eq!(parsed.files.len(), 1, "(a) no phantom file");
    let (doc, code) = digest(ws, &[], Some(a));
    assert_eq!(code, 0, "(a) {doc}");
    assert_eq!(roots(&doc), vec!["PostSalesDoc"], "(a) {doc}");
    assert!(diags(&doc).is_empty(), "(a) {doc}");

    // (b) an EMPTY physical line is context: the `+` is new line 18, the
    // attribute of `OnAfterPostSalesDoc` (line 17 is blank).
    assert_eq!(line(&posting, 17), "");
    let b = "--- a/src/posting.al\n+++ b/src/posting.al\n@@ -17,2 +17,2 @@\n\n-    [IntegrationEvent(true, false)]\n+    [IntegrationEvent(false, false)]\n";
    let (doc, code) = digest(ws, &[], Some(b));
    assert_eq!(code, 0, "(b) {doc}");
    assert_eq!(roots(&doc), vec!["OnAfterPostSalesDoc"], "(b) {doc}");

    // (c) a truncated hunk, with and without the final newline: one error each,
    // roots only from the lines read (`DeletedAfter(2)` is not interior).
    for c in [
        "--- a/src/posting.al\n+++ b/src/posting.al\n@@ -3,2 +3,1 @@\n-old\n",
        "--- a/src/posting.al\n+++ b/src/posting.al\n@@ -3,2 +3,1 @@\n-old",
    ] {
        let (doc, code) = digest(ws, &[], Some(c));
        assert_eq!(
            diag_kinds(&doc)
                .iter()
                .filter(|k| *k == "diff-parse-error")
                .count(),
            1,
            "(c) {c:?}: {doc}"
        );
        assert!(roots(&doc).is_empty(), "(c) {c:?}: {doc}");
        assert_eq!(code, 2, "(c) {c:?}: {doc}");
    }

    // (c1) the line that ends a truncated hunk is re-dispatched as a header. The
    // terminator is an `@@` line, so swallowing it loses the second hunk (line 20,
    // inside `OnAfterPostSalesDoc`); a `diff --git` terminator could not tell,
    // because the following `---`/`+++` open the next file anyway.
    let c1 = "--- a/src/posting.al
+++ b/src/posting.al
@@ -3,2 +3,1 @@
-old
@@ -20,1 +20,1 @@
-x
+y
";
    let (doc, code) = digest(ws, &[], Some(c1));
    assert_eq!(code, 0, "(c1) {doc}");
    assert_eq!(roots(&doc), vec!["OnAfterPostSalesDoc"], "(c1) {doc}");
    assert_eq!(
        diag_kinds(&doc)
            .iter()
            .filter(|k| *k == "diff-parse-error")
            .count(),
        1,
        "(c1) {doc}"
    );

    // (d) a CRLF diff over a CRLF source gives the same roots as LF.
    let crlf = tempfile::tempdir().unwrap();
    d8_workspace(crlf.path(), &posting.replace('\n', "\r\n"));
    let d = one_line_diff(
        "src/posting.al",
        9,
        "        Header.\"Status Posted\" := true;",
    );
    let (lf_doc, _) = digest(ws, &[], Some(&d));
    let (crlf_doc, crlf_code) = digest(crlf.path(), &[], Some(&d.replace('\n', "\r\n")));
    assert_eq!(crlf_code, 0, "(d) {crlf_doc}");
    assert_eq!(roots(&crlf_doc), vec!["PostSalesDoc"], "(d) {crlf_doc}");
    assert_eq!(roots(&crlf_doc), roots(&lf_doc), "(d)");
    assert_eq!(diags(&crlf_doc), diags(&lf_doc), "(d)");

    // (e) `\ No newline at end of file` is not counted.
    let e = "--- a/src/posting.al\n+++ b/src/posting.al\n@@ -9,1 +9,1 @@\n-x\n\\ No newline at end of file\n+y\n\\ No newline at end of file\n";
    let (doc, code) = digest(ws, &[], Some(e));
    assert_eq!(code, 0, "(e) {doc}");
    assert_eq!(roots(&doc), vec!["PostSalesDoc"], "(e) {doc}");
    assert!(diags(&doc).is_empty(), "(e) {doc}");
}

/// A1g(c2). A bare `---`/`+++` pair inside a short hunk is body while its side
/// has count left; the next `@@` ends the hunk as INCOMPLETE (old count 1 left)
/// and attaches to the current file.
///
/// Mutations: apply header tests inside a body -> `src/subscriber.al` becomes a
/// second file and `HandlePosted` a root; drop the re-dispatched line -> hunk 2
/// is lost (caught by the parser-level two-hunk assertion).
#[test]
fn bare_header_pair_inside_short_hunk_is_body() {
    let dir = tempfile::tempdir().unwrap();
    d8_workspace(dir.path(), &d8_posting());
    let diff = "diff --git a/src/posting.al b/src/posting.al
--- a/src/posting.al
+++ b/src/posting.al
@@ -3,3 +3,1 @@
-old
--- a/src/subscriber.al
+++ b/src/subscriber.al
@@ -9,1 +9,1 @@
-x
+y
";
    let parsed = parse_unified_diff(diff);
    assert_eq!(parsed.files.len(), 1, "{:?}", parsed.files);
    assert_eq!(parsed.files[0].path, "src/posting.al");
    assert_eq!(parsed.files[0].hunks.len(), 2);
    assert_eq!(parsed.errors.len(), 1, "{:?}", parsed.errors);
    assert!(parsed.errors[0].contains("+3"), "{:?}", parsed.errors);

    let (doc, code) = digest(dir.path(), &[], Some(diff));
    assert_eq!(code, 0, "{doc}");
    assert_eq!(roots(&doc), vec!["PostSalesDoc"], "{doc}");
    assert_eq!(diag_kinds(&doc), vec!["diff-parse-error"], "{doc}");
}

/// A1g(c2-overrun). A `-` line after the old-side count is spent ends the hunk;
/// the `---` line is re-dispatched as a top-level header and opens file 2.
///
/// Mutation: accept a `-` line while only SOME count remains -> one parsed file,
/// zero errors, roots [`PostSalesDoc`] only.
#[test]
fn minus_line_past_old_count_ends_the_hunk() {
    let dir = tempfile::tempdir().unwrap();
    d8_workspace(dir.path(), &d8_posting());
    let diff = "diff --git a/src/posting.al b/src/posting.al
--- a/src/posting.al
+++ b/src/posting.al
@@ -3,1 +3,2 @@
-old
+a
--- a/src/subscriber.al
+++ b/src/subscriber.al
@@ -9,1 +9,1 @@
-x
+y
";
    let parsed = parse_unified_diff(diff);
    let paths: Vec<&str> = parsed.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["src/posting.al", "src/subscriber.al"]);
    assert_eq!(parsed.files[0].hunks.len(), 1);
    assert_eq!(parsed.files[1].hunks.len(), 1);
    assert_eq!(parsed.errors.len(), 1, "{:?}", parsed.errors);

    let (doc, code) = digest(dir.path(), &[], Some(diff));
    assert_eq!(code, 0, "{doc}");
    assert_eq!(roots(&doc), vec!["HandlePosted", "PostSalesDoc"], "{doc}");
    assert_eq!(diag_kinds(&doc), vec!["diff-parse-error"], "{doc}");
}

/// A1g(c2-exact). Counts filled exactly: the hunk is COMPLETE, no error; the
/// next `@@` attaches to the current file (`src/posting.al`).
///
/// Mutations: treat count exhaustion as truncation -> one `diff-parse-error`;
/// apply header tests inside a body -> file 2 and `HandlePosted` appear.
#[test]
fn bare_header_pair_exact_fill_is_complete() {
    let dir = tempfile::tempdir().unwrap();
    d8_workspace(dir.path(), &d8_posting());
    let diff = "diff --git a/src/posting.al b/src/posting.al
--- a/src/posting.al
+++ b/src/posting.al
@@ -3,2 +3,1 @@
-old
--- a/src/subscriber.al
+++ b/src/subscriber.al
@@ -9,1 +9,1 @@
-x
+y
";
    let parsed = parse_unified_diff(diff);
    assert_eq!(parsed.files.len(), 1, "{:?}", parsed.files);
    assert_eq!(parsed.files[0].path, "src/posting.al");
    assert_eq!(parsed.files[0].hunks.len(), 2);
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);

    let (doc, code) = digest(dir.path(), &[], Some(diff));
    assert_eq!(code, 0, "{doc}");
    assert_eq!(roots(&doc), vec!["PostSalesDoc"], "{doc}");
    assert!(diags(&doc).is_empty(), "{doc}");
}

/// Final panel (astra): a `+++` line that overruns a hunk's new count is re-read
/// at top level. With no `---` header before it, it must not open a file: the
/// posting hunk's `Added(3)` (inside `PostSalesDoc`, lines 3-16) stays, and the
/// next `@@` attaches to `src/posting.al` (line 9, also inside `PostSalesDoc`).
///
/// Mutation: let any `+++ ` line open a file once a `---` was ever seen -> the
/// posting file is replaced by `src/subscriber.al`; `HandlePosted` (its line 9)
/// becomes the only root.
#[test]
fn orphan_plus_header_keeps_the_current_file() {
    let dir = tempfile::tempdir().unwrap();
    d8_workspace(dir.path(), &d8_posting());
    let diff = "diff --git a/src/posting.al b/src/posting.al
--- a/src/posting.al
+++ b/src/posting.al
@@ -3,2 +3,1 @@
-old
+new
+++ b/src/subscriber.al
@@ -9 +9 @@
-x
+y
";
    let (doc, code) = digest(dir.path(), &[], Some(diff));
    assert_eq!(code, 0, "{doc}");
    assert_eq!(roots(&doc), vec!["PostSalesDoc"], "{doc}");
    assert_eq!(diag_kinds(&doc), vec!["diff-parse-error"], "{doc}");
}

/// Final panel (astra): a hunk header whose range does not fit in i32 is a
/// `diff-parse-error`, not a panic, and opens no hunk. The file then has no
/// hunk, so (like a hunkless rename) every declaration of it is a root.
///
/// Mutation: drop the range check -> `attempt to add with overflow` in a debug
/// build (the process exits 101, no JSON).
#[test]
fn out_of_range_hunk_header_is_a_parse_error() {
    let dir = tempfile::tempdir().unwrap();
    d8_workspace(dir.path(), &d8_posting());
    let diff = "--- a/src/posting.al\n+++ b/src/posting.al\n@@ -1,1 +2147483647,0 @@\n-x\n";
    let (doc, code) = digest(dir.path(), &[], Some(diff));
    assert_eq!(code, 0, "{doc}");
    assert_eq!(diag_kinds(&doc), vec!["diff-parse-error"], "{doc}");
    assert_eq!(
        roots(&doc),
        vec!["OnAfterPostSalesDoc", "PostSalesDoc"],
        "{doc}"
    );
}

// ---------------------------------------------------------------------------
// A2a, A2b
// ---------------------------------------------------------------------------

/// A2a. A file whose routines have no indexed operation or call site is still
/// matched, by `--file` and by `--diff`.
///
/// Precondition: the snapshot holds no operation and no call site for `Add`.
/// Mutation: the old evidence index -> `file-unmatched`, exit 2.
#[test]
fn file_with_only_opless_routines_is_matched() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("app.json"),
        r#"{"id":"7a1e0000-0000-0000-0000-000000000073","name":"Issue71Pure","publisher":"probe","version":"1.0.0.0","platform":"1.0.0.0","application":"1.0.0.0","idRanges":[{"from":50300,"to":50399}],"runtime":"14.0"}"#,
    );
    let pure = "codeunit 50300 PureCu\n{\n    procedure Add(A: Integer; B: Integer): Integer\n    begin\n        exit(A + B);\n    end;\n}\n";
    write(&dir.path().join("src/Pure.al"), pure);

    let model_id = compute_gate_model_instance_id(dir.path()).unwrap();
    let model = assemble_and_resolve_workspace_program(dir.path(), &model_id, false).unwrap();
    let snap = compose_snapshot(&model);
    assert!(
        snap.operation_index.is_empty() && snap.callsite_index.is_empty(),
        "precondition: no evidence at all"
    );

    let (doc, code) = digest(dir.path(), &["--file", "src/Pure.al"], None);
    assert_eq!(code, 0, "{doc}");
    assert_eq!(roots(&doc), vec!["Add"], "{doc}");
    assert!(diags(&doc).is_empty(), "{doc}");

    let (doc, code) = digest(
        dir.path(),
        &[],
        Some(&one_line_diff("src/Pure.al", 5, "        exit(A + B);")),
    );
    assert_eq!(code, 0, "{doc}");
    assert_eq!(roots(&doc), vec!["Add"], "{doc}");
    assert!(
        !diag_kinds(&doc).contains(&"diff-file-unmatched".to_string()),
        "{doc}"
    );
}

/// A2b. `--file` returns every modelled routine of the file: the Shapes file
/// declares 12 routines with 11 distinct ids (the two `Dup` arms share one).
///
/// Mutation: the old evidence index -> `OnPub` and `Dup` are missing.
#[test]
fn file_digest_returns_every_routine_of_the_file() {
    let decls = SHAPES
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            t.starts_with("procedure ")
                || t.starts_with("local procedure ")
                || t.starts_with("trigger ")
        })
        .count();
    assert_eq!(decls, 12, "precondition: 12 declarations");

    let dir = tempfile::tempdir().unwrap();
    shapes_workspace(dir.path(), SHAPES);
    let (doc, code) = digest(dir.path(), &["--file", "src/Shapes.al"], None);
    assert_eq!(code, 0, "{doc}");
    assert_eq!(
        roots(&doc),
        vec![
            "Dup",
            "OnAction",
            "OnAfterGetRecord",
            "OnInsert",
            "OnPub",
            "OnRun",
            "OnValidate",
            "OnValidate",
            "Split",
            "Split",
            "Sub"
        ],
        "{doc}"
    );
    let ids: std::collections::BTreeSet<String> = entries(&doc)
        .iter()
        .map(|e| e["routine"]["stableId"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids.len(), 11, "{doc}");
    assert!(diags(&doc).is_empty(), "{doc}");
}

// ---------------------------------------------------------------------------
// A2d: declaration-only procedures
// ---------------------------------------------------------------------------

const DECL_GUID: &str = "7a1e0000-0000-0000-0000-000000000074";

fn decl_only_workspace(dir: &Path) {
    write(
        &dir.join("app.json"),
        &format!(
            r#"{{"id":"{DECL_GUID}","name":"Issue71Decl","publisher":"probe","version":"1.0.0.0","platform":"1.0.0.0","application":"1.0.0.0","idRanges":[{{"from":50500,"to":50599}}],"runtime":"14.0"}}"#
        ),
    );
    write(
        &dir.join("src/IFoo.Interface.al"),
        "interface IFoo\n{\n    procedure Bar();\n    procedure Baz(X: Integer);\n}\n",
    );
    write(
        &dir.join("src/IQux.Interface.al"),
        "interface IQux\n{\n    procedure Bar();\n}\n",
    );
    write(
        &dir.join("src/Add.ControlAddIn.al"),
        "controladdin MyAddIn\n{\n    procedure Init();\n}\n",
    );
    write(
        &dir.join("src/IEmpty.Interface.al"),
        "interface IEmpty\n{\n}\n",
    );
    write(
        &dir.join("src/Cu.Codeunit.al"),
        "codeunit 50500 DeclCu\n{\n    procedure Run()\n    begin\n        Message('x');\n    end;\n}\n",
    );
}

/// The id the design gives a declaration-only procedure without parameters or
/// return value: the object segment `<guid>:<Kind>:0` and the signature hash
/// folded with the declaring object's name.
fn decl_only_id(kind: &str, object: &str, procedure: &str) -> String {
    to_stable_routine_id_from_parts(
        &to_stable_object_id(&format!("{DECL_GUID}/{kind}/0")),
        &normalized_signature_hash(procedure, &[], None),
        Some(object),
    )
}

/// A2d. A hunk inside an Interface/ControlAddIn procedure makes it a changed
/// root: a declaration-only entry with no effects, counted once, with no
/// `DIAG-digest-*` diagnostic.
///
/// Mutations: (i) leave `declaration_only_routines` out of the span index ->
/// (a)-(d) fail; (ii) pass `None` instead of the object name -> (c) yields one
/// id; (iii) derive the id from the procedure name alone -> (a)'s stableId
/// fails; (iv) route declaration-only roots through the identity/coverage
/// lookups -> (a) loses its entry or counts it twice with a `DIAG-digest-*`.
#[test]
fn declaration_only_procedures_are_roots() {
    let dir = tempfile::tempdir().unwrap();
    decl_only_workspace(dir.path());
    let ws = dir.path();

    // (a) the `Bar` line of IFoo.
    let (doc, code) = digest(
        ws,
        &[],
        Some(&one_line_diff(
            "src/IFoo.Interface.al",
            3,
            "    procedure Bar();",
        )),
    );
    assert_eq!(code, 0, "(a) {doc}");
    let es = entries(&doc);
    assert_eq!(es.len(), 1, "(a) {doc}");
    let e = &es[0];
    assert_eq!(e["routine"]["display"], "Bar");
    assert_eq!(e["routine"]["declarationOnly"], true);
    assert_eq!(
        anchor(e),
        ("ws:src/IFoo.Interface.al".to_string(), 2, 4),
        "(a) {doc}"
    );
    let id = e["routine"]["stableId"].as_str().unwrap();
    assert!(
        id.starts_with(&format!("{DECL_GUID}:Interface:0#"))
            && id.len() == format!("{DECL_GUID}:Interface:0#").len() + 64,
        "(a) {id}"
    );
    assert_eq!(id, decl_only_id("Interface", "IFoo", "Bar"), "(a)");
    assert_eq!(e["effects"], Value::Array(Vec::new()));
    assert_eq!(e["coverage"]["status"], "unknown");
    assert_eq!(
        e["coverage"]["reasons"],
        serde_json::json!(["declaration-only"])
    );
    assert_eq!(doc["payload"]["summary"]["rootsRequested"], 1, "(a) {doc}");
    assert_eq!(doc["payload"]["summary"]["rootsResolved"], 1, "(a) {doc}");
    assert!(
        !doc["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"].as_str().unwrap_or("").starts_with("DIAG-digest-")),
        "(a) {doc}"
    );
    assert!(diags(&doc).is_empty(), "(a) {doc}");
    // (f) the additive keys ship under digest 1.5.0.
    assert_eq!(doc["schemaVersion"], "1.5.0");

    // (b) `--file` on IFoo: both methods.
    let (doc, code) = digest(ws, &["--file", "src/IFoo.Interface.al"], None);
    assert_eq!(code, 0, "(b) {doc}");
    assert_eq!(roots(&doc), vec!["Bar", "Baz"], "(b) {doc}");
    assert_eq!(doc["payload"]["summary"]["rootsRequested"], 2, "(b) {doc}");
    assert_eq!(doc["payload"]["summary"]["rootsResolved"], 2, "(b) {doc}");
    assert!(diags(&doc).is_empty(), "(b) {doc}");

    // (c) the same `Bar` in IFoo and IQux: two roots with distinct ids.
    let both = format!(
        "{}{}",
        one_line_diff("src/IFoo.Interface.al", 3, "    procedure Bar();"),
        one_line_diff("src/IQux.Interface.al", 3, "    procedure Bar();")
    );
    let (doc, code) = digest(ws, &[], Some(&both));
    assert_eq!(code, 0, "(c) {doc}");
    let mut got: Vec<(String, String)> = entries(&doc)
        .iter()
        .map(|e| {
            (
                anchor(e).0,
                e["routine"]["stableId"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    got.sort();
    assert_eq!(
        got,
        vec![
            (
                "ws:src/IFoo.Interface.al".to_string(),
                decl_only_id("Interface", "IFoo", "Bar")
            ),
            (
                "ws:src/IQux.Interface.al".to_string(),
                decl_only_id("Interface", "IQux", "Bar")
            ),
        ],
        "(c) {doc}"
    );
    assert_ne!(got[0].1, got[1].1);

    // (d) the controladdin method.
    let (doc, code) = digest(
        ws,
        &[],
        Some(&one_line_diff(
            "src/Add.ControlAddIn.al",
            3,
            "    procedure Init();",
        )),
    );
    assert_eq!(code, 0, "(d) {doc}");
    let e = entry(&doc, "Init");
    assert_eq!(e["routine"]["declarationOnly"], true, "(d) {doc}");
    assert_eq!(
        e["routine"]["stableId"],
        decl_only_id("ControlAddIn", "MyAddIn", "Init"),
        "(d)"
    );

    // (e) an interface with no procedures declares no routine.
    let (doc, code) = digest(ws, &["--file", "src/IEmpty.Interface.al"], None);
    assert_eq!(code, 2, "(e) {doc}");
    assert_eq!(diag_kinds(&doc), vec!["file-unmatched"], "(e) {doc}");
}

// ---------------------------------------------------------------------------
// A4: entry anchors
// ---------------------------------------------------------------------------

/// A4. Every entry is anchored at its declaration (digest and prove), with the
/// ORIGINAL unit spelling and UTF-8 byte columns.
///
/// Mutations: (a)/(e) restore the root-slot / op / call-site fallback -> no
/// anchor for `OnAfterPostSalesDoc`, line 47 for `Persist`; (b) build the anchor
/// from the normalized lookup key -> a lowercased / `ws:`-less file; (d) convert
/// the column to UTF-16 -> 10.
#[test]
fn entry_anchor_is_the_declaration() {
    // (a) a publisher that today gets no anchor, and an op-fallback anchor.
    let (doc, code) = digest(
        &corpus("ws-d8-commit-in-tx"),
        &["--routine", "OnAfterPostSalesDoc"],
        None,
    );
    assert_eq!(code, 0, "{doc}");
    assert_eq!(
        anchor(entry(&doc, "OnAfterPostSalesDoc")),
        ("ws:src/posting.al".to_string(), 18, 4)
    );
    let d34 = std::fs::read_to_string(corpus("ws-d34").join("src/Codeunit.al"))
        .unwrap()
        .replace('\r', "");
    assert_eq!(line(&d34, 46), "    local procedure Persist(_n: Integer)");
    let (doc, code) = digest(&corpus("ws-d34"), &["--routine", "Persist"], None);
    assert_eq!(code, 0, "{doc}");
    assert_eq!(
        anchor(entry(&doc, "Persist")),
        ("ws:src/Codeunit.al".to_string(), 45, 4)
    );

    // (b) the original unit spelling; (d) a UTF-8 byte column.
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("app.json"),
        r#"{"id":"7a1e0000-0000-0000-0000-000000000075","name":"Issue71Anchor","publisher":"probe","version":"1.0.0.0","platform":"1.0.0.0","application":"1.0.0.0","idRanges":[{"from":50600,"to":50699}],"runtime":"14.0"}"#,
    );
    let mixed = "codeunit 50600 MixedCu\n{\n    procedure X()\n    begin\n    end;\n\n    /*\u{e9}*/ procedure Y()\n    begin\n    end;\n}\n";
    assert_eq!(line(mixed, 7).find("procedure"), Some(11), "byte column");
    assert_eq!(
        line(mixed, 7)[..11].encode_utf16().count(),
        10,
        "the UTF-16 column differs"
    );
    write(&dir.path().join("src/MixedCase.Codeunit.al"), mixed);
    let (doc, code) = digest(dir.path(), &["--file", "src/MixedCase.Codeunit.al"], None);
    assert_eq!(code, 0, "{doc}");
    assert_eq!(
        anchor(entry(&doc, "X")),
        ("ws:src/MixedCase.Codeunit.al".to_string(), 2, 4),
        "(b)"
    );
    assert_eq!(
        anchor(entry(&doc, "Y")),
        ("ws:src/MixedCase.Codeunit.al".to_string(), 6, 11),
        "(d)"
    );

    // (e) prove reads the same anchor; its schema stays 1.3.0.
    let prove = run_prove_pipeline(
        &corpus("ws-d34"),
        "Persist",
        "may-commit",
        VERSION_OVERRIDE,
        true,
    )
    .expect("prove runs");
    let p: Value = serde_json::from_str(&prove.json_text).unwrap();
    assert_eq!(p["schemaVersion"], "1.3.0");
    let a = &p["payload"]["routine"]["anchor"];
    assert_eq!(
        (
            a["file"].as_str().unwrap(),
            a["line"].as_i64(),
            a["column"].as_i64()
        ),
        ("ws:src/Codeunit.al", Some(45), Some(4)),
        "{p}"
    );
}

/// bc-dev-mcp's join, transcribed literally (`src/core/risk.ts:99-110` and
/// `:158-169` at `715e0b9`): an entry joins a subject when the display matches,
/// the anchor file minus `ws:` equals the subject file (case-insensitive), and
/// `anchor.line + 1` lies in the subject's 1-based `[start, end]`.
fn joins(e: &Value, name: &str, file: &str, start: i64, end: i64) -> bool {
    let (afile, aline, _) = anchor(e);
    e["routine"]["display"] == name
        && afile.trim_start_matches("ws:").to_lowercase() == file.to_lowercase()
        && (start..=end).contains(&(aline + 1))
}

/// A4(c). A multi-declaration id is anchored at its LAST TOUCHED declaration and
/// lists every declaration in `routine.anchors`.
///
/// `Dup`'s arms are 1-based lines 20-23 and 25-28 (anchor lines 19 and 24).
/// Mutations: (i) first declaration regardless of touch -> (c1), (c3), (c4)
/// fail; (ii) last declaration regardless of touch -> (c2) fails; (iii) first
/// TOUCHED declaration -> (c3), (c4) fail; (iv) drop `routine.anchors` -> the
/// `anchors` assertions fail.
#[test]
fn duplicate_id_anchor_follows_the_changed_arm() {
    let dir = tempfile::tempdir().unwrap();
    shapes_workspace(dir.path(), SHAPES);
    let ws = dir.path();
    assert_eq!(line(SHAPES, 20), "    procedure Dup(): Integer");
    assert_eq!(line(SHAPES, 25), "    procedure Dup(): Integer");

    // Precondition: the two arms are ONE id (one selector candidate, one entry).
    let (doc, code) = digest(ws, &["--routine", "Dup"], None);
    assert_eq!(code, 0, "{doc}");
    assert!(diags(&doc).is_empty(), "{doc}");
    let routine_doc = doc;

    let dup_lines = |doc: &Value| -> (i64, Vec<i64>) {
        let e = entry(doc, "Dup");
        let anchors = e["routine"]["anchors"]
            .as_array()
            .map(|a| a.iter().map(|x| x["line"].as_i64().unwrap()).collect())
            .unwrap_or_default();
        (anchor(e).1, anchors)
    };
    let arm_a = ("src/Shapes.al", 20, 23);
    let arm_b = ("src/Shapes.al", 25, 28);
    let join = |doc: &Value| {
        let e = entry(doc, "Dup");
        (
            joins(e, "Dup", arm_a.0, arm_a.1, arm_a.2),
            joins(e, "Dup", arm_b.0, arm_b.1, arm_b.2),
        )
    };

    // (c1) the second arm only.
    let (c1, _) = digest(
        ws,
        &[],
        Some(&one_line_diff("src/Shapes.al", 26, "    begin")),
    );
    assert_eq!(dup_lines(&c1), (24, vec![19, 24]), "(c1) {c1}");
    assert_eq!(join(&c1), (false, true), "(c1) joins arm B only");

    // (c2) the first arm only.
    let (c2, _) = digest(
        ws,
        &[],
        Some(&one_line_diff("src/Shapes.al", 21, "    begin")),
    );
    assert_eq!(dup_lines(&c2), (19, vec![19, 24]), "(c2) {c2}");
    assert_eq!(join(&c2), (true, false), "(c2) joins arm A only");

    // (c3) both arms in one diff.
    let both = "--- a/src/Shapes.al
+++ b/src/Shapes.al
@@ -21,1 +21,1 @@
-    begin
+    begin
@@ -26,1 +26,1 @@
-    begin
+    begin
";
    let (c3, _) = digest(ws, &[], Some(both));
    assert_eq!(dup_lines(&c3), (24, vec![19, 24]), "(c3) {c3}");

    // (c4) `--file` and `--routine`: every declaration is touched -> the last.
    let (c4, _) = digest(ws, &["--file", "src/Shapes.al"], None);
    assert_eq!(dup_lines(&c4), (24, vec![19, 24]), "(c4) {c4}");
    assert_eq!(
        join(&c4),
        (false, true),
        "(c4) the stated limit: a reader that ignores `anchors` joins arm B only"
    );
    assert_eq!(dup_lines(&routine_doc).0, 24, "(c4) --routine Dup");
}

/// Plan Task 3: a routine's declaration starts at its first attached attribute;
/// `source_anchor` still starts at the keyword.
#[test]
fn model_routine_declaration_start_includes_attributes() {
    let dir = tempfile::tempdir().unwrap();
    d8_workspace(dir.path(), &d8_posting());
    let model_id = compute_gate_model_instance_id(dir.path()).unwrap();
    let model = assemble_and_resolve_workspace_program(dir.path(), &model_id, false).unwrap();
    let r = |name: &str| {
        model
            .workspace
            .routines
            .iter()
            .find(|r| r.name == name)
            .unwrap()
    };
    // 0-based: `[IntegrationEvent]` row 17, `procedure OnAfterPostSalesDoc` row 18.
    let p = r("OnAfterPostSalesDoc");
    assert_eq!(
        (p.declaration_start_line, p.source_anchor.start_line),
        (17, 18)
    );
    let q = r("PostSalesDoc");
    assert_eq!(
        (q.declaration_start_line, q.source_anchor.start_line),
        (2, 2)
    );
}

/// #71 attribute-start limits: (a) an attribute separated from its routine by a
/// COMMENT is not claimed by `declaration_start_line` today (issue #80's lowerer
/// bug drops it; a #80 fix moves this assertion on purpose); (b) a multi-line
/// attribute's FIRST line is the declaration start.
#[test]
fn declaration_start_line_attribute_limits() {
    let dir = tempfile::tempdir().unwrap();
    d8_workspace(dir.path(), &d8_posting());
    write(
        &dir.path().join("src/Attrs.Codeunit.al"),
        "codeunit 50120 AttrsCu\n{\n    [IntegrationEvent(false, false)]\n    // note\n    procedure Commented()\n    begin\n    end;\n\n    [EventSubscriber(ObjectType::Codeunit, Codeunit::AttrsCu,\n        'Commented', '', false, false)]\n    local procedure MultiLine()\n    begin\n    end;\n}\n",
    );
    let model_id = compute_gate_model_instance_id(dir.path()).unwrap();
    let model = assemble_and_resolve_workspace_program(dir.path(), &model_id, false).unwrap();
    let r = |name: &str| {
        model
            .workspace
            .routines
            .iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("{name}: no routine"))
    };
    // 0-based: `[IntegrationEvent]` row 2, `// note` row 3, `procedure` row 4.
    let c = r("Commented");
    assert_eq!(
        (c.declaration_start_line, c.source_anchor.start_line),
        (4, 4),
        "#80: the comment-separated attribute is not claimed"
    );
    // 0-based: `[EventSubscriber(` row 8, its last line row 9, `procedure` row 10.
    let m = r("MultiLine");
    assert_eq!(
        (m.declaration_start_line, m.source_anchor.start_line),
        (8, 10)
    );
}

/// Review focus 3: touched sets from several inputs are unioned per id.
#[test]
fn mixed_inputs_union_touched_declarations() {
    let dir = tempfile::tempdir().unwrap();
    shapes_workspace(dir.path(), SHAPES);
    let (doc, code) = digest(
        dir.path(),
        &["--routine", "Dup"],
        Some(&one_line_diff("src/Shapes.al", 21, "    begin")),
    );
    assert_eq!(code, 0, "{doc}");
    assert_eq!(anchor(entry(&doc, "Dup")).1, 24, "{doc}");
}

/// Review focus 2: a modified file with headers and no hunk takes every
/// declaration of the file, like a pure rename (unchanged behaviour).
#[test]
fn hunkless_modified_file_takes_every_declaration() {
    let dir = tempfile::tempdir().unwrap();
    d8_workspace(dir.path(), &d8_posting());
    let (doc, code) = digest(
        dir.path(),
        &[],
        Some(
            "diff --git a/src/posting.al b/src/posting.al\nold mode 100644\nnew mode 100755\n--- a/src/posting.al\n+++ b/src/posting.al\n",
        ),
    );
    assert_eq!(code, 0, "{doc}");
    assert_eq!(
        roots(&doc),
        vec!["OnAfterPostSalesDoc", "PostSalesDoc"],
        "{doc}"
    );
}

/// Final panel (astra, R2-07): the object name folded into a declaration-only id
/// is the LOGICAL name. `interface "I""Foo"` is the interface `I"Foo`; the IR keeps
/// the escaped inner text `I""Foo`, which must be unescaped once.
///
/// Mutation: fold the raw IR name (drop the unescape) -> the id is the hash of
/// `I""Foo`.
#[test]
fn quoted_interface_name_is_unescaped_in_the_id() {
    let dir = tempfile::tempdir().unwrap();
    decl_only_workspace(dir.path());
    write(
        &dir.path().join("src/IQuoted.Interface.al"),
        "interface \"I\"\"Foo\"\n{\n    procedure Bar();\n}\n",
    );
    let model_id = compute_gate_model_instance_id(dir.path()).unwrap();
    let model = assemble_and_resolve_workspace_program(dir.path(), &model_id, false).unwrap();
    let quoted = model
        .workspace
        .objects
        .iter()
        .find(|o| o.name == "I\"\"Foo")
        .expect("the IR keeps the escaped inner text");
    let ids: Vec<&str> = quoted
        .declaration_only_routines
        .iter()
        .map(|d| d.stable_routine_id.as_str())
        .collect();
    let logical = decl_only_id("Interface", "I\"Foo", "Bar");
    assert_ne!(logical, decl_only_id("Interface", "I\"\"Foo", "Bar"));
    assert_eq!(ids, vec![logical.as_str()]);
}

/// Plan Task 4: Interface/ControlAddIn procedures ride on their object row with a
/// name-folded stable id; they are still not `ModelRoutine`s.
#[test]
fn interface_object_rows_carry_declaration_only_routines() {
    let dir = tempfile::tempdir().unwrap();
    decl_only_workspace(dir.path());
    let model_id = compute_gate_model_instance_id(dir.path()).unwrap();
    let model = assemble_and_resolve_workspace_program(dir.path(), &model_id, false).unwrap();
    let obj = |name: &str| {
        model
            .workspace
            .objects
            .iter()
            .find(|o| o.name == name)
            .unwrap_or_else(|| panic!("{name}: no object row"))
    };
    let rows = |name: &str| -> Vec<(String, String, u32, u32)> {
        obj(name)
            .declaration_only_routines
            .iter()
            .map(|d| {
                (
                    d.name.clone(),
                    d.stable_routine_id.clone(),
                    d.source_anchor.start_line,
                    d.declaration_start_line,
                )
            })
            .collect()
    };
    let ifoo = rows("IFoo");
    assert_eq!(ifoo.len(), 2, "{ifoo:?}");
    assert_eq!(
        ifoo[0],
        (
            "Bar".to_string(),
            decl_only_id("Interface", "IFoo", "Bar"),
            2,
            2
        )
    );
    assert_eq!((ifoo[1].0.as_str(), ifoo[1].2), ("Baz", 3));
    assert_ne!(ifoo[1].1, ifoo[0].1);
    // The key property: two interfaces declaring the same `Bar()` get two ids.
    assert_ne!(rows("IFoo")[0].1, rows("IQux")[0].1);
    assert_eq!(
        rows("IQux"),
        vec![(
            "Bar".to_string(),
            decl_only_id("Interface", "IQux", "Bar"),
            2,
            2
        )]
    );
    assert_eq!(
        rows("MyAddIn")[0].1,
        decl_only_id("ControlAddIn", "MyAddIn", "Init")
    );
    assert!(rows("IEmpty").is_empty());
    assert!(rows("DeclCu").is_empty());
    assert!(
        !model.workspace.routines.iter().any(|r| r.name == "Bar"),
        "the Interface skip stays"
    );
}
