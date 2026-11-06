//! Every-changed-line accounting (agent-tasks B-1b) over hand-stated files,
//! spans and a hand-written diff: each bucket count is stated from the text.

use std::sync::Arc;

use super::*;
use crate::engine::l5::diff_parser::parse_unified_diff;
use crate::engine::l5::digest_cli::RoutineSpan;

const ORDER: &str = "table 50100 \"Acme Order\"
{
    fields
    {
        field(1; \"No.\"; Code[20]) { }
        field(2; Amount; Decimal) { }
    }

    var
        Setup: Record \"Acme Setup\";
        Limit: Decimal;

    [Scope('OnPrem')]
    procedure Check(NewLimit: Decimal)
    begin
        Limit := NewLimit;
    end;
}
";

const CALC: &str = "namespace Acme;

codeunit 50101 Calc
{
}
";

const DIFF: &str = "diff --git a/src/Order.Table.al b/src/Order.Table.al
index 1111111..2222222 100644
--- a/src/Order.Table.al
+++ b/src/Order.Table.al
@@ -5,4 +5,3 @@
         field(1; \"No.\"; Code[20]) { }
         field(2; Amount; Decimal) { }
-        field(3; Old; Text[10]) { }
     }
@@ -10,9 +9,9 @@
     var
         Setup: Record \"Acme Setup\";
-        Discount: Decimal;
+        Limit: Decimal;

     [Scope('OnPrem')]
-    procedure Check()
+    procedure Check(NewLimit: Decimal)
     begin
-        Limit := 0;
+        Limit := NewLimit;
     end;
@@ -19,5 +18,1 @@
-
-    procedure Gone()
-    begin
-    end;
 }
diff --git a/src/Calc.Codeunit.al b/src/Calc.Codeunit.al
--- a/src/Calc.Codeunit.al
+++ b/src/Calc.Codeunit.al
@@ -1,3 +1,3 @@
-namespace Old;
+namespace Acme;

-codeunit 50101 Calc
+codeunit 50101 \"Calc X\"
diff --git a/src/notes.txt b/src/notes.txt
--- a/src/notes.txt
+++ b/src/notes.txt
@@ -1 +1,2 @@
 a
+b
diff --git a/other/Ghost.Codeunit.al b/other/Ghost.Codeunit.al
new file mode 100644
--- /dev/null
+++ b/other/Ghost.Codeunit.al
@@ -0,0 +1 @@
+codeunit 50103 Ghost { }
diff --git a/src/Old.Codeunit.al b/src/Old.Codeunit.al
deleted file mode 100644
--- a/src/Old.Codeunit.al
+++ /dev/null
@@ -1,3 +0,0 @@
-codeunit 50102 Old
-{
-}
";

const CHECK: &str = "app:Table:50100#check";

fn ws(rel: &str, text: &str) -> WsFile {
    WsFile {
        rel_posix: rel.into(),
        abs_path: std::path::PathBuf::from("/ws").join(rel),
        nested_app: false,
        text: Ok(Arc::from(text)),
    }
}

fn run() -> Accounting {
    let spans = RoutineSpanIndex::from_spans(vec![RoutineSpan {
        routine: CHECK.into(),
        source_unit_id: "ws:src/Order.Table.al".into(),
        start: 12,
        end: 16,
        anchor_line: 13,
        anchor_column: 4,
        declaration_only_display: None,
    }]);
    let files = vec![
        ws("src/Order.Table.al", ORDER),
        ws("src/Calc.Codeunit.al", CALC),
    ];
    account(
        &parse_unified_diff(DIFF),
        &DiffRoot::Workspace,
        &files,
        &spans,
    )
}

fn file<'a>(a: &'a Accounting, path: &str) -> &'a FileAccount {
    a.files.iter().find(|f| f.path == path).expect(path)
}

/// B-1b: per file the buckets sum to its changed lines; the hunk that
/// changes a global variable and the routine below it reports both; a
/// removed field, a removed whole routine (named, not called deleted), a
/// signature change and a body change each land in their own bucket.
///
/// DISCRIMINATION (2026-11-05): clearing a hunk's declaration buckets when
/// it touches a routine (what reporting `hunk-outside-routines` only would
/// do) fails at the per-file sum (`account`'s debug assertion: 9 lines
/// counted of 11); restored, passes.
#[test]
fn every_changed_line_lands_in_one_bucket() {
    let a = run();
    for f in &a.files {
        assert_eq!(f.buckets.total(), f.added + f.removed, "{}", f.path);
    }
    assert_eq!(a.totals.total(), a.changed_lines());
    let o = file(&a, "src/Order.Table.al");
    assert_eq!((o.added, o.removed), (3, 8));
    let b = &o.buckets;
    assert_eq!(b.decl.get(&DeclKind::Field), Some(&1));
    assert_eq!(b.decl.get(&DeclKind::GlobalVar), Some(&2));
    assert_eq!(b.decl.get(&DeclKind::CommentOrBlank), Some(&1));
    assert_eq!((b.signature, b.body), (2, 2));
    assert_eq!(b.removed_routine_text, 3);
    assert_eq!(o.removed_routines, BTreeSet::from(["gone".to_string()]));
    // The second hunk: the global and the routine.
    let h = &o.hunks[1];
    assert_eq!(
        (
            h.buckets.decl.get(&DeclKind::GlobalVar).copied(),
            h.buckets.signature,
            h.buckets.body
        ),
        (Some(2), 2, 2),
        "the global and the routine"
    );
    assert_eq!(h.routines, BTreeSet::from([CHECK.to_string()]));
    let check = a.routines.get(CHECK).expect("Check is changed");
    assert_eq!((check.body, check.signature), (2, 2));
    assert!(!check.added_in_diff(), "{check:?}");
    // The hunks that touch no routine: the field's, the removed routine's,
    // and the other file's.
    let outside: Vec<(String, i32, i32)> = a
        .hunks_outside_routines()
        .map(|(f, h)| (f.path.clone(), h.start, h.end))
        .filter(|(p, _, _)| p.ends_with(".al") && !p.contains("Ghost") && !p.contains("Old."))
        .collect();
    assert_eq!(
        outside,
        vec![
            ("src/Order.Table.al".to_string(), 5, 7),
            ("src/Order.Table.al".to_string(), 18, 18),
            ("src/Calc.Codeunit.al".to_string(), 1, 3),
        ]
    );
}

/// The classes no declaration holds: outside an object, an object header,
/// a non-AL file, a path no workspace file has, a deleted file.
#[test]
fn outside_objects_and_unmapped_files() {
    let a = run();
    let c = &file(&a, "src/Calc.Codeunit.al").buckets;
    assert_eq!(c.outside, 2);
    assert_eq!(c.decl.get(&DeclKind::ObjectHeader), Some(&2));
    let u = |p: &str| {
        let f = file(&a, p);
        (f.unmapped, f.buckets.unmapped.values().sum::<usize>())
    };
    assert_eq!(u("src/notes.txt"), (Some(Unmapped::NotAl), 1));
    assert_eq!(
        u("other/Ghost.Codeunit.al"),
        (Some(Unmapped::NotInWorkspace), 1)
    );
    assert_eq!(u("src/Old.Codeunit.al"), (Some(Unmapped::DeletedFile), 3));
}

/// B-3b's check: the diff's new side against the analyzed text. `Calc`'s
/// third line is `codeunit 50101 Calc` on disk but `"Calc X"` in the diff.
///
/// DISCRIMINATION (2026-11-05): trusting the diff (no `new_side`
/// comparison) leaves `mismatch` `None`: fails; restored, passes.
#[test]
fn a_diff_that_does_not_match_the_analyzed_text_is_reported() {
    let a = run();
    assert_eq!(
        file(&a, "src/Calc.Codeunit.al").mismatch,
        Some(Mismatch { line: 3, lines: 1 })
    );
    assert_eq!(file(&a, "src/Order.Table.al").mismatch, None);
    assert_eq!(a.mismatched().count(), 1);
}

/// A file whose hunk header the parser rejects is unmapped with that
/// reason, never placed.
#[test]
fn a_rejected_hunk_header_unmaps_the_file() {
    let d = "--- a/src/Order.Table.al\n+++ b/src/Order.Table.al\n@@ -1,2 +1,x @@\n+junk\n";
    let parsed = parse_unified_diff(d);
    assert!(!parsed.file_errors.is_empty(), "{parsed:?}");
    let files = vec![ws("src/Order.Table.al", ORDER)];
    let a = account(
        &parsed,
        &DiffRoot::Workspace,
        &files,
        &RoutineSpanIndex::default(),
    );
    for f in &a.files {
        assert_eq!(f.unmapped, Some(Unmapped::DiffParseError), "{f:?}");
        assert_eq!(f.buckets.total(), f.added + f.removed);
    }
}

/// A commit that adds a byte-order mark: the diff's new line 1 carries it,
/// the analyzed text has it stripped. That is no mismatch, and the line is
/// the object header (found on CDO: 54 files read as mismatched before).
///
/// DISCRIMINATION (2026-11-05): comparing without stripping the mark
/// reports `Mismatch { line: 1, .. }`: fails; restored, passes.
#[test]
fn a_byte_order_mark_is_not_a_mismatch() {
    let d = "--- a/src/Calc.Codeunit.al\n+++ b/src/Calc.Codeunit.al\n@@ -1,2 +1,2 @@\n-codeunit 50101 Calc\n+\u{feff}codeunit 50101 Calc\n {\n";
    let text = "codeunit 50101 Calc\n{\n}\n";
    let a = account(
        &parse_unified_diff(d),
        &DiffRoot::Workspace,
        &[ws("src/Calc.Codeunit.al", text)],
        &RoutineSpanIndex::default(),
    );
    let f = file(&a, "src/Calc.Codeunit.al");
    assert_eq!(f.mismatch, None);
    assert_eq!(f.buckets.decl.get(&DeclKind::ObjectHeader), Some(&2));
}
