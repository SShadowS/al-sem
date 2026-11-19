//! ENG-135: literal pruning knows which platform methods write an argument, from the AL
//! compiler's own var-ness (`program::resolve::platform_writes`). Before, E1 (a parameter's
//! literal binding) took only `Evaluate` and `Clear` as writers of an Integer, Option or Enum
//! argument, and E2 (the within-routine flow) a partial hand list, so a parameter or local a
//! platform method writes through `var` kept its literal and `case` arms that DO run were pruned
//! (a wrong "no"). Each shape below writes the bound value through a `var` the old lists did not
//! know; the arm that the write can reach must stay live. Text keeps ENG-30's rule made exact: a
//! read-only platform function outside its old 15-name list no longer unbinds the parameter.

use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"table 51350 "PW Int A" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 51351 "PW Int Else" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 51352 "PW Opt A" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 51353 "PW Opt B" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 51354 "PW Enum Alpha" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 51355 "PW Enum Beta" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 51356 "PW Set One" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 51357 "PW Set Else" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 51358 "PW Txt A" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 51359 "PW Txt Else" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 51361 "PW Bool If" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
enum 51360 "PW Kind"
{
    value(0; Alpha) { }
    value(1; Beta) { }
}
codeunit 51370 "PW Shapes"
{
    var
        Opt: Option A,B;

    // ---- E1, Integer: Dictionary.Get(K, var V) writes I ----
    procedure IntRoot()
    begin
        IntCase(1);
    end;

    local procedure IntCase(I: Integer)
    var
        D: Dictionary of [Integer, Integer];
        A: Record "PW Int A";
        E: Record "PW Int Else";
    begin
        D.Get(1, I);
        case I of
            1:
                A.Insert();
            else
                E.Insert();
        end;
    end;

    // ---- E1, Option: InStream.Read(var X) writes O ----
    procedure OptRoot()
    begin
        OptCase(Opt::A);
    end;

    local procedure OptCase(O: Option A,B)
    var
        S: InStream;
        A: Record "PW Opt A";
        B: Record "PW Opt B";
    begin
        S.Read(O);
        case O of
            O::A:
                A.Insert();
            O::B:
                B.Insert();
        end;
    end;

    // ---- E1, Enum: List.Get(I, var V) writes K ----
    procedure EnumRoot()
    begin
        EnumCase(Enum::"PW Kind"::Alpha);
    end;

    local procedure EnumCase(K: Enum "PW Kind")
    var
        L: List of [Enum "PW Kind"];
        A: Record "PW Enum Alpha";
        B: Record "PW Enum Beta";
    begin
        L.Get(1, K);
        case K of
            K::Alpha:
                A.Insert();
            K::Beta:
                B.Insert();
        end;
    end;

    // ---- E2, a local: Dictionary.Set(K, V, var Old) writes Old ----
    procedure SetFlow()
    var
        D: Dictionary of [Integer, Integer];
        Old: Integer;
        One: Record "PW Set One";
        Other: Record "PW Set Else";
    begin
        Old := 1;
        D.Set(1, 2, Old);
        case Old of
            1:
                One.Insert();
            else
                Other.Insert();
        end;
    end;

    // ---- guard frames, Boolean: Dictionary.Get(K, var V) writes B ----
    procedure BoolRoot()
    begin
        BoolGuard(false);
    end;

    local procedure BoolGuard(B: Boolean)
    var
        D: Dictionary of [Integer, Boolean];
        T: Record "PW Bool If";
    begin
        D.Get(1, B);
        if B then
            T.Insert();
    end;

    // ---- Text: DelStr reads its text (outside ENG-30's 15 names): T keeps 'A' ----
    procedure TxtRoot()
    begin
        TxtCase('A');
    end;

    local procedure TxtCase(T: Text)
    var
        A: Record "PW Txt A";
        E: Record "PW Txt Else";
    begin
        if DelStr(T, 1, 0) = '' then
            exit;
        case T of
            'A':
                A.Insert();
            else
                E.Insert();
        end;
    end;
}
"#;

fn alsem(cwd: &Path, args: &[&str]) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .current_dir(cwd)
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    crate::as_v3(serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{args:?}: {e}; stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        )
    }))
}

/// `prove <routine> writes-table:<table>` for every case; all mismatches in one message.
fn expect(cases: &[(&str, &str, &str)]) {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"13513513-0000-4000-8000-000000000135","name":"PW","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/Shapes.al"), SOURCE).unwrap();
    let inv = alsem(
        dir.path(),
        &["fingerprint", "ws", "--inventory-only", "--format", "json"],
    );
    let id_of = |name: &str| -> String {
        inv["payload"]["routineInventory"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["routineName"] == name)
            .unwrap_or_else(|| panic!("precondition: {name} is in the inventory"))["stableRoutineId"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let wrong: Vec<String> = cases
        .iter()
        .filter_map(|(r, t, want)| {
            let got = alsem(
                dir.path(),
                &[
                    "prove",
                    "ws",
                    &id_of(r),
                    &format!("writes-table:{t}"),
                    "--deterministic",
                ],
            )["payload"]["result"]["answer"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            (got != *want).then(|| format!("{r} writes {t}: got {got}, want {want}"))
        })
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The value a platform `var` writes reaches every arm: none is pruned. (Each `yes` was a wrong
/// `no` on master: the bound literal kept its value through the write.)
#[test]
fn a_platform_method_writing_through_var_keeps_every_arm_live() {
    expect(&[
        ("IntRoot", "PW Int A", "yes"),
        ("IntRoot", "PW Int Else", "yes"),
        ("OptRoot", "PW Opt A", "yes"),
        ("OptRoot", "PW Opt B", "yes"),
        ("EnumRoot", "PW Enum Alpha", "yes"),
        ("EnumRoot", "PW Enum Beta", "yes"),
        ("SetFlow", "PW Set One", "yes"),
        ("SetFlow", "PW Set Else", "yes"),
        ("BoolRoot", "PW Bool If", "yes"),
    ]);
}

/// A read-only platform function keeps a Text parameter's literal, so the arm it cannot take
/// is pruned (ENG-30 unbound every Text passed to a function outside its 15 names).
#[test]
fn a_read_only_platform_function_keeps_a_text_literal() {
    expect(&[
        ("TxtRoot", "PW Txt A", "yes"),
        ("TxtRoot", "PW Txt Else", "no"),
    ]);
}
