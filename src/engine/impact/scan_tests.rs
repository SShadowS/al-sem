//! The line scanner on hand-written AL: each line's label is stated here,
//! next to the line.

use super::*;

fn labels(text: &str) -> Vec<(String, LineKind)> {
    let lines: Vec<&str> = text.lines().collect();
    let l = Scanner::new().scan(lines.iter().copied(), |_, _| {});
    lines.iter().map(|s| s.trim().to_string()).zip(l).collect()
}

fn d(k: DeclKind) -> LineKind {
    LineKind::Decl(k)
}

fn r(name: Option<&str>) -> LineKind {
    LineKind::Routine {
        name: name.map(str::to_string),
    }
}

const TABLE: &str = r#"namespace Acme.Sales;
using System;

table 50100 "Acme Order"
{
    Caption = 'Order';
    DataClassification = CustomerContent;

    fields
    {
        field(1; "No."; Code[20])
        {
            Caption = 'No. {not a brace}';
            trigger OnValidate()
            begin
                Check();
            end;
        }
    }
    keys
    {
        key(PK; "No.")
        {
            Clustered = true;
        }
    }
    fieldgroups
    {
        fieldgroup(DropDown; "No.") { }
    }

    var
        Setup: Record "Acme Setup";

    [IntegrationEvent(false, false)]
    local procedure OnCheck()
    begin
    end;

    // a comment
    procedure Check()
    var
        X: Integer;
    begin
        if X = 1 then begin
            X := 2;
        end;
        case X of
            1:
                X := 3;
        end;
    end;
}
"#;

/// Every line of a table: fields, a field trigger, keys, field groups, a
/// global var, properties, an attributed procedure, a comment, nested
/// `begin`/`case` blocks, and the lines outside the object.
///
/// DISCRIMINATION (2026-11-05): dropping the `{`-head relabel (the `for l in
/// out…` loop) leaves `fields` and `keys` as `property`: fails at line 9.
#[test]
fn a_table_is_labelled_line_by_line() {
    use DeclKind::*;
    let want = vec![
        LineKind::Outside,
        LineKind::Outside,
        LineKind::Outside,
        d(ObjectHeader),
        d(ObjectHeader),
        d(Property),
        d(Property),
        d(CommentOrBlank),
        d(Field), // fields
        d(Field),
        d(Field), // field(1; …)
        d(Field),
        d(Field), // Caption with a brace in a string
        r(Some("onvalidate")),
        r(Some("onvalidate")),
        r(Some("onvalidate")),
        r(Some("onvalidate")),
        d(Field), // }
        d(Field), // } fields
        d(Key),
        d(Key),
        d(Key),
        d(Key),
        d(Key),
        d(Key),
        d(Key),
        d(FieldGroup),
        d(FieldGroup),
        d(FieldGroup),
        d(FieldGroup),
        d(CommentOrBlank),
        d(GlobalVar),
        d(GlobalVar),
        d(CommentOrBlank),
        r(None),
        r(Some("oncheck")),
        r(Some("oncheck")),
        r(Some("oncheck")),
        d(CommentOrBlank),
        d(CommentOrBlank),
        r(Some("check")),
        r(Some("check")),
        r(Some("check")),
        r(Some("check")),
        r(Some("check")),
        r(Some("check")),
        r(Some("check")),
        r(Some("check")),
        r(Some("check")),
        r(Some("check")),
        r(Some("check")),
        r(Some("check")),
        d(ObjectHeader),
    ];
    let got = labels(TABLE);
    for (i, (line, k)) in got.iter().enumerate() {
        assert_eq!(Some(k), want.get(i), "line {}: {line:?}", i + 1);
    }
    assert_eq!(got.len(), want.len());
}

/// A page's layout and actions, an enum's values, and an interface's
/// bodiless procedures.
#[test]
fn layout_actions_enum_values_and_interfaces() {
    use DeclKind::*;
    let page = "page 50101 P\n{\n    layout\n    {\n        area(Content)\n        {\n            field(Name; Rec.Name) { }\n        }\n    }\n    actions\n    {\n        area(Processing)\n        {\n            action(Post)\n            {\n                Caption = 'Post';\n            }\n        }\n    }\n}\n";
    let got: Vec<LineKind> = labels(page).into_iter().map(|x| x.1).collect();
    assert_eq!(got[2..9], vec![d(Layout); 7][..]);
    assert_eq!(got[9..19], vec![d(Action); 10][..]);
    let en = "enum 50102 E\n{\n    Extensible = true;\n    value(0; None) { Caption = 'None'; }\n    value(1; Some)\n    {\n    }\n}\n";
    let got: Vec<LineKind> = labels(en).into_iter().map(|x| x.1).collect();
    assert_eq!(
        got,
        vec![
            d(ObjectHeader),
            d(ObjectHeader),
            d(Property),
            d(EnumValue),
            d(EnumValue),
            d(EnumValue),
            d(EnumValue),
            d(ObjectHeader)
        ]
    );
    let itf = "interface I\n{\n    procedure A();\n\n    procedure B(X: Integer): Text;\n}\n";
    let got: Vec<LineKind> = labels(itf).into_iter().map(|x| x.1).collect();
    assert_eq!(
        got,
        vec![
            d(ObjectHeader),
            d(ObjectHeader),
            r(Some("a")),
            d(CommentOrBlank),
            r(Some("b")),
            d(ObjectHeader)
        ]
    );
}

/// The body starts at the routine's `var` or `begin` line; lines above it
/// (attributes, header, a parameter list over several lines) are the
/// signature.
#[test]
fn body_starts_at_var_or_begin() {
    let lines: Vec<&str> = TABLE.lines().collect();
    // `procedure Check()` is line 41 (0-based 40); its `var` is 41.
    assert_eq!(body_start(&lines, 40, 51), Some(41));
    // `local procedure OnCheck()` (0-based 35): `begin` at 36.
    assert_eq!(body_start(&lines, 35, 37), Some(36));
    let multi = [
        "procedure P(A: Integer;",
        "    B: Integer)",
        "begin",
        "end;",
    ];
    assert_eq!(body_start(&multi, 0, 3), Some(2));
    assert_eq!(body_start(&["procedure X();"], 0, 0), None);
}
