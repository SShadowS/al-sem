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
table 51362 "PW Keep One" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 51363 "PW Keep Else" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 51364 "PW Bool Keep" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 51365 "PW Blob" { fields { field(1; Code; Code[20]) { } field(2; Data; Blob) { } } keys { key(PK; Code) { } } }
table 51366 "PW Arm A" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 51367 "PW Arm Else" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 51368 "PW Arm Bool" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 51369 "PW Arm Rec" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 51371 "PW Tab Line" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 51372 "PW Tab Batch" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 51373 "PW Tab Log" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 51374 "PW Ref Log" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
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
        D.Add(1, 2);
        D.Get(1, I);
        case I of
            1:
                A.Insert();
            else
                E.Insert();
        end;
    end;

    // ---- E1, Option: InStream.Read(var X) writes O (a value written to a Blob, read back) ----
    procedure OptRoot()
    begin
        OptCase(Opt::A);
    end;

    local procedure OptCase(O: Option A,B)
    var
        Buf: Record "PW Blob" temporary;
        OS: OutStream;
        S: InStream;
        A: Record "PW Opt A";
        B: Record "PW Opt B";
    begin
        Buf.Data.CreateOutStream(OS);
        OS.Write(1);
        Buf.Data.CreateInStream(S);
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
        L.Add(Enum::"PW Kind"::Beta);
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
        D.Add(1, 5);
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
        D.Add(1, true);
        D.Get(1, B);
        if B then
            T.Insert();
    end;

    // ---- E2 keeps: a local at Dictionary.Get's KEY position (not var) keeps its value ----
    procedure FlowKeep()
    var
        D: Dictionary of [Integer, Integer];
        L: Integer;
        V: Integer;
        One: Record "PW Keep One";
        Other: Record "PW Keep Else";
    begin
        L := 1;
        D.Get(L, V);
        case L of
            1:
                One.Insert();
            else
                Other.Insert();
        end;
    end;

    // ---- guard frames keep: Format reads a Boolean ----
    procedure BoolKeepRoot()
    begin
        BoolKeep(false);
    end;

    local procedure BoolKeep(B: Boolean)
    var
        T: Record "PW Bool Keep";
    begin
        Message(Format(B));
        if B then
            T.Insert();
    end;

    // ---- `#if` arms: only the compiled `#else` arm writes I before passing it on ----
    procedure ArmRoot()
    begin
        ArmPass(1);
    end;

#if PW_NEVER
    local procedure ArmPass(I: Integer)
    begin
        ArmLeaf(I);
    end;
#else
    local procedure ArmPass(I: Integer)
    var
        D: Dictionary of [Integer, Integer];
    begin
        D.Add(1, 2);
        D.Get(1, I);
        ArmLeaf(I);
    end;
#endif

    procedure ArmBoolRoot()
    begin
        ArmBoolPass(false);
    end;

#if PW_NEVER
    local procedure ArmBoolPass(B: Boolean)
    begin
        ArmBoolLeaf(B);
    end;
#else
    local procedure ArmBoolPass(B: Boolean)
    var
        D: Dictionary of [Integer, Boolean];
    begin
        D.Add(1, true);
        D.Get(1, B);
        ArmBoolLeaf(B);
    end;
#endif

    local procedure ArmBoolLeaf(B: Boolean)
    var
        T: Record "PW Arm Bool";
    begin
        if B then
            T.Insert();
    end;

    // The same inside a recursive SCC, where the cone keeps no guard and `literal_reach`'s own
    // guard frames decide (as G23's `DocSaveRecursive`).
    procedure ArmRecRoot()
    begin
        ArmRecPass(false, 0);
    end;

#if PW_NEVER
    local procedure ArmRecPass(B: Boolean; Depth: Integer)
    begin
        ArmRecLeaf(B, Depth);
    end;
#else
    local procedure ArmRecPass(B: Boolean; Depth: Integer)
    var
        D: Dictionary of [Integer, Boolean];
    begin
        D.Add(1, true);
        D.Get(1, B);
        ArmRecLeaf(B, Depth);
    end;
#endif

    local procedure ArmRecLeaf(B: Boolean; Depth: Integer)
    var
        T: Record "PW Arm Rec";
    begin
        if Depth < 3 then
            ArmRecPass(B, Depth + 1);
        if B then
            T.Insert();
    end;

    // And a record in a Variant: the compiled arm hands on a different record. Both the table
    // frame (G33: `ArmTabPass` reads its table with GetTable) and the RecordRef flow (G34: the
    // Variant's possible tables) must see every arm.
    procedure ArmTabRoot()
    var
        Line: Record "PW Tab Line";
    begin
        ArmTabPass(Line);
    end;

#if PW_NEVER
    local procedure ArmTabPass(V: Variant)
    var
        RecRef: RecordRef;
    begin
        RecRef.GetTable(V);
        ArmTabLeaf(V);
    end;
#else
    local procedure ArmTabPass(V: Variant)
    var
        RecRef: RecordRef;
        Batch: Record "PW Tab Batch";
    begin
        V := Batch;
        RecRef.GetTable(V);
        ArmTabLeaf(V);
    end;
#endif

    // The same where the value flow cannot follow the new value (a function result), so the
    // table frame alone decides whether the entry table is forwarded.
    procedure ArmTabOpaqueRoot()
    var
        Line: Record "PW Tab Line";
    begin
        ArmTabOpaquePass(Line);
    end;

#if PW_NEVER
    local procedure ArmTabOpaquePass(V: Variant)
    var
        RecRef: RecordRef;
    begin
        RecRef.GetTable(V);
        ArmTabLeaf(V);
    end;
#else
    local procedure ArmTabOpaquePass(V: Variant)
    var
        RecRef: RecordRef;
    begin
        V := TabOther();
        RecRef.GetTable(V);
        ArmTabLeaf(V);
    end;
#endif

    // No arms: a parameter the caller reassigns from a function result is not forwarded with
    // its entry table (master forwarded it and pruned the Batch arm).
    procedure TabFwdRoot()
    var
        Line: Record "PW Tab Line";
    begin
        TabFwdPass(Line);
    end;

    local procedure TabFwdPass(V: Variant)
    begin
        V := TabOther();
        ArmTabLeaf(V);
    end;

    // The RecordRef flow (G34) reads what a call leaves in a `var` RecordRef: a routine in three
    // `#if` arms, only the middle one leaving a Batch. The engine answers for every arm's build
    // (engine limits B2), and a build defining PW_NEVER2 runs the insert. A call used to target
    // only the first and the last row.
    procedure RefRoot()
    var
        RecRef: RecordRef;
    begin
        RefSet(RecRef);
        RefGuarded(RecRef);
    end;

    local procedure RefGuarded(var R: RecordRef)
    var
        Log: Record "PW Ref Log";
    begin
        if R.Number <> Database::"PW Tab Batch" then
            exit;
        Log.Insert();
    end;

#if PW_NEVER
    local procedure RefSet(var R: RecordRef)
    var
        L: Record "PW Tab Line";
    begin
        R.GetTable(L);
    end;
#elif PW_NEVER2
    local procedure RefSet(var R: RecordRef)
    var
        B: Record "PW Tab Batch";
    begin
        R.GetTable(B);
    end;
#else
    local procedure RefSet(var R: RecordRef)
    var
        L: Record "PW Tab Line";
    begin
        R.GetTable(L);
    end;
#endif

    local procedure TabOther(): Variant
    var
        Batch: Record "PW Tab Batch";
    begin
        exit(Batch);
    end;

    local procedure ArmTabLeaf(V: Variant)
    var
        RecRef: RecordRef;
        Log: Record "PW Tab Log";
    begin
        RecRef.GetTable(V);
        case RecRef.Number of
            Database::"PW Tab Batch":
                Log.Insert();
        end;
    end;

    local procedure ArmLeaf(I: Integer)
    var
        A: Record "PW Arm A";
        E: Record "PW Arm Else";
    begin
        case I of
            1:
                A.Insert();
            else
                E.Insert();
        end;
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

/// The value a platform `var` writes reaches every arm: none is pruned. Each read is seeded, so
/// the written value is the one the arm on the OTHER side of the bound literal takes. (On master
/// that other arm was a wrong `no`: the bound literal kept its value through the write.)
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

/// sol: a routine whose `#if` arms differ, only one writing the parameter through a platform
/// `var`. The routine's frame takes the parameters usable in EVERY arm; the writing arm has
/// none, so the merge must leave the parameter unbound rather than skip that arm.
#[test]
fn an_arm_that_writes_the_parameter_unbinds_it() {
    expect(&[
        ("ArmRoot", "PW Arm A", "yes"),
        ("ArmRoot", "PW Arm Else", "yes"),
        ("ArmBoolRoot", "PW Arm Bool", "yes"),
        ("ArmRecRoot", "PW Arm Rec", "yes"),
        ("ArmTabRoot", "PW Tab Log", "yes"),
        ("ArmTabOpaqueRoot", "PW Tab Log", "yes"),
        ("TabFwdRoot", "PW Tab Log", "yes"),
        ("RefRoot", "PW Ref Log", "yes"),
    ]);
}

/// A read-only platform position keeps the value, so the arm it cannot take is pruned: a Text
/// passed to `DelStr` (ENG-30 unbound every Text passed to a function outside its 15 names), a
/// local at `Dictionary.Get`'s key position (E2's old list havocked every argument of `Get`),
/// and a Boolean passed to `Format`.
#[test]
fn a_read_only_platform_position_keeps_the_value() {
    expect(&[
        ("TxtRoot", "PW Txt A", "yes"),
        ("TxtRoot", "PW Txt Else", "no"),
        ("FlowKeep", "PW Keep One", "yes"),
        ("FlowKeep", "PW Keep Else", "no"),
        ("BoolKeepRoot", "PW Bool Keep", "no"),
    ]);
}
