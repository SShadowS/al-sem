//! Owned-IR L2 cutover — robustness / anti-regression suite.
//!
//! The L2 emitter is now driven entirely by the owned `al_syntax` IR (no tree-sitter
//! CST walk, no legacy fallback). Byte-for-byte parity over the well-formed corpus
//! proves the happy path; these tests lock down the edge cases that corpus parity
//! structurally CANNOT (external review, gpt-5.5 + gemini-3.1-pro, flagged each as a
//! potential silent-correctness hole):
//!
//!   1. Malformed routines must NOT be dropped from the emitted set, and a malformed
//!      routine must not swallow following routines (the binary trust-boundary risk).
//!   2. Quoted-identifier names with embedded doubled quotes (`""`) must normalize
//!      IDENTICALLY to the legacy `strip_quotes` (outer-quote strip only) — the
//!      stable-routine-id hash depends on it.
//!   3. An extension object's number is the extension's OWN id, never a numeric target.
//!   4. `ir_object_type` must equal the legacy `object_type_for` for every object kind.
//!   5. `al_syntax::parse` + the L2 entry points must not panic on garbage input.

use al_sem::engine::l2::l2_workspace::project_named_routine;

/// A parse error in one routine must not drop a FOLLOWING well-formed routine (the
/// binary trust-boundary risk: a malformed routine swallowing the rest of the
/// object). Each fixture pairs a malformed routine with a well-formed follower that
/// MUST still appear in the IR routine set.
#[test]
fn malformed_routines_are_not_dropped() {
    // (source, names that MUST be present). Families: stray token, unterminated call,
    // missing inner end, missing semicolon, malformed case branch, malformed params.
    let fixtures: &[(&str, &[&str])] = &[
        (
            "codeunit 50100 A\n{\n procedure Broken() begin Foo(); @@@ end;\n procedure After() begin Bar(); end;\n}\n",
            &["After"],
        ),
        (
            // Unterminated nested call + missing inner `end`: the parser CANNOT
            // recover the follower `Q` (it is consumed into P's broken body) — in
            // tree-sitter too. The honest invariant here is only that the malformed
            // routine `P` itself is still emitted (not silently dropped).
            "codeunit 50101 B\n{\n procedure P() begin Foo(); if X then begin Bar(  Baz(); end;\n procedure Q() begin Ok(); end;\n}\n",
            &["P"],
        ),
        (
            "codeunit 50102 C\n{\n procedure P() begin Foo() Bar(); end;\n procedure R() begin Fine(); end;\n}\n",
            &["R"],
        ),
        (
            "codeunit 50103 D\n{\n procedure P() begin case X of 1: ; 2 Bad(); end; end;\n procedure S() begin Good(); end;\n}\n",
            &["S"],
        ),
        (
            "codeunit 50104 E\n{\n procedure P(var : ) begin end;\n procedure T() begin Last(); end;\n}\n",
            &["T"],
        ),
    ];

    for (src, required) in fixtures {
        let file = al_syntax::parse(src);
        let names: std::collections::HashSet<String> = file
            .objects
            .iter()
            .flat_map(|o| o.routines.iter().map(|r| r.name.clone()))
            .collect();
        for want in *required {
            assert!(
                names.contains(*want),
                "well-formed routine `{want}` was DROPPED by the IR (a malformed sibling swallowed it).\n  src: {src:?}\n  got: {names:?}"
            );
        }
    }
}

/// Quoted identifiers with embedded doubled quotes must match the legacy
/// `strip_quotes` (outer strip only) — NOT a semantic unescape. If the IR unescaped
/// `"A ""B"""` to `A "B"`, the stable-routine-id hash would silently drift.
#[test]
fn quoted_identifier_doubled_quotes_match_legacy_strip() {
    let src = concat!(
        "codeunit 50100 \"My \"\"Obj\"\"\"\n",
        "{\n",
        "    procedure \"A \"\"B\"\"\"()\n",
        "    begin\n",
        "    end;\n",
        "}\n",
    );
    let file = al_syntax::parse(src);
    assert_eq!(file.objects.len(), 1);
    let o = &file.objects[0];
    // Legacy strip_quotes strips only the OUTER pair, preserving doubled inner quotes.
    assert_eq!(o.name, "My \"\"Obj\"\"", "object name outer-strip only");
    assert_eq!(o.routines.len(), 1);
    assert_eq!(
        o.routines[0].name, "A \"\"B\"\"",
        "routine name outer-strip only"
    );

    // And the routine is resolvable by that exact (legacy-form) name through the
    // production entry point — i.e. the stable id is computed from the legacy form.
    let r = project_named_routine(src, "A \"\"B\"\"", "app", "ws:t.al");
    assert!(r.is_some(), "routine not found by its legacy-stripped name");
}

/// An extension object's number is the extension's OWN id, even when the target
/// could be read as numeric — never the target.
#[test]
fn extension_object_number_is_declaration_id() {
    let cases = [
        (
            "tableextension 50100 \"My Ext\" extends Customer\n{\n}\n",
            50100,
        ),
        (
            "pageextension 50111 PExt extends \"Customer Card\"\n{\n}\n",
            50111,
        ),
        (
            "enumextension 50122 EExt extends \"My Enum\"\n{\n}\n",
            50122,
        ),
    ];
    for (src, want) in cases {
        let file = al_syntax::parse(src);
        assert_eq!(file.objects.len(), 1, "one object: {src:?}");
        assert_eq!(
            file.objects[0].id,
            Some(want),
            "extension number must be its own id: {src:?}"
        );
    }
}

/// `ir_object_type(ObjectKind)` maps every object kind to its expected L2 label
/// (and skips — `None` — the kinds the L2 projection excludes). The owned-IR object
/// classifier is the single source of truth now that the tree-sitter node-kind map
/// is retired.
#[test]
fn ir_object_type_labels_every_kind() {
    use al_sem::engine::l2::ir_walk::ir_object_type;
    // (source declaring exactly one object, expected L2 label or None when excluded)
    let cases: &[(&str, Option<&str>)] = &[
        ("codeunit 50100 X\n{\n}\n", Some("Codeunit")),
        ("table 50100 X\n{\n}\n", Some("Table")),
        (
            "tableextension 50100 X extends Customer\n{\n}\n",
            Some("TableExtension"),
        ),
        ("page 50100 X\n{\n}\n", Some("Page")),
        (
            "pageextension 50100 X extends \"Customer Card\"\n{\n}\n",
            Some("PageExtension"),
        ),
        ("report 50100 X\n{\n}\n", Some("Report")),
        (
            "reportextension 50100 X extends \"Customer List\"\n{\n}\n",
            Some("ReportExtension"),
        ),
        ("query 50100 X\n{\n}\n", Some("Query")),
        ("xmlport 50100 X\n{\n}\n", Some("XMLport")),
        ("enum 50100 X\n{\n}\n", Some("Enum")),
        (
            "enumextension 50100 X extends \"My Enum\"\n{\n}\n",
            Some("EnumExtension"),
        ),
        ("interface X\n{\n}\n", Some("Interface")),
        ("controladdin X\n{\n}\n", Some("ControlAddIn")),
        ("permissionset 50100 X\n{\n}\n", Some("PermissionSet")),
    ];
    for (src, want) in cases {
        let file = al_syntax::parse(src);
        assert_eq!(file.objects.len(), 1, "one object: {src:?}");
        let ir = ir_object_type(&file.objects[0].kind);
        assert_eq!(ir, *want, "object-type label for {src:?}");
    }
}

/// Neither `al_syntax::parse` nor the L2 entry points may panic on garbage input.
#[test]
fn no_panic_on_garbage() {
    let garbage = [
        "",
        "\u{0}\u{1}\u{2}",
        "procedure",
        "codeunit",
        "}}}{{{",
        "codeunit 50100 \"unterminated",
        "table\n{\nfield(1;Foo;",
        "🦀 procedure begin end",
        "codeunit 9 A { procedure P() begin ",
    ];
    for src in garbage {
        let f = al_syntax::parse(src);
        // Touch the routines so any lazy panic surfaces.
        let _ = f.objects.iter().map(|o| o.routines.len()).sum::<usize>();
        let _ = project_named_routine(src, "P", "app", "ws:g.al");
    }
}

/// Report dataitems are modelled in the owned IR (Phase-3 prep). A dataitem trigger's
/// implicit `Rec` is typed to its enclosing dataitem's source table, and every report
/// routine sees the dataitem NAMES as record vars typed to their source tables. This
/// exercises `project_routine_features_ir` directly (the IR-driven L2 path), so the
/// modeling is validated independent of the L3 emitter.
#[test]
fn report_dataitem_implicit_rec_and_name_vars_seeded() {
    use al_sem::engine::l2::ir_walk;
    use al_sem::engine::l2::scope::{ParameterSymbol, compute_routine_id};
    let src = concat!(
        "report 50100 \"My Report\"\n{\n",
        "    dataset\n    {\n",
        "        dataitem(Cust; Customer)\n        {\n",
        "            trigger OnAfterGetRecord() begin Rec.TableCaption(); end;\n",
        "        }\n    }\n",
        "    procedure Helper() begin end;\n",
        "}\n",
    );
    let f = al_syntax::parse(src);
    let o = &f.objects[0];
    // The report carries its dataitem (name, source-table) in the IR.
    assert_eq!(
        o.dataitems,
        vec![("Cust".to_string(), "Customer".to_string())]
    );

    let oi = 0usize;
    let no_params: Vec<ParameterSymbol> = vec![];
    // The dataitem trigger: implicit `Rec` typed to Customer + a `Cust` dataitem-name var.
    let trig = o
        .routines
        .iter()
        .find(|r| r.name == "OnAfterGetRecord")
        .expect("trigger");
    assert_eq!(trig.dataitem_source_table.as_deref(), Some("Customer"));
    // `enclosing_member` = the dataitem wrapper — the real discriminator this
    // trigger's id carries (`ir_enclosing_member`, Task 3).
    let trig_member = ir_walk::ir_enclosing_member(trig);
    assert_eq!(trig_member.as_deref(), Some("Cust"));
    let rid = compute_routine_id(
        "a",
        "Report",
        50100,
        "trigger",
        &trig.name,
        trig_member.as_deref(),
        &no_params,
        None,
        "a",
    );
    let feats = ir_walk::project_routine_features_ir(&f, oi, trig, &rid, src, "a", None);
    let rec = feats
        .record_variables
        .iter()
        .find(|rv| rv.name.eq_ignore_ascii_case("Rec"))
        .expect("implicit Rec seeded in the dataitem trigger");
    assert_eq!(
        rec.table_name.as_deref(),
        Some("Customer"),
        "dataitem trigger Rec typed to the dataitem source table"
    );
    assert!(
        feats
            .record_variables
            .iter()
            .any(|rv| rv.name == "Cust" && rv.table_name.as_deref() == Some("Customer")),
        "dataitem name `Cust` seeded as a record var typed to Customer"
    );

    // A report-level procedure (NOT a dataitem trigger): no implicit dataitem Rec, but
    // the dataitem NAME var is still in scope across the whole report.
    let helper = o
        .routines
        .iter()
        .find(|r| r.name == "Helper")
        .expect("Helper");
    assert_eq!(helper.dataitem_source_table, None);
    let rid2 = compute_routine_id(
        "a",
        "Report",
        50100,
        "procedure",
        &helper.name,
        ir_walk::ir_enclosing_member(helper).as_deref(),
        &no_params,
        None,
        "a",
    );
    let feats2 = ir_walk::project_routine_features_ir(&f, oi, helper, &rid2, src, "a", None);
    assert!(
        !feats2
            .record_variables
            .iter()
            .any(|rv| rv.name.eq_ignore_ascii_case("Rec")),
        "a report-level procedure has no implicit dataitem Rec"
    );
    assert!(
        feats2.record_variables.iter().any(|rv| rv.name == "Cust"),
        "the dataitem name var is in scope in report-level procedures too"
    );
}
