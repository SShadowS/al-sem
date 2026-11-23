//! ENG-173: a table procedure named like a record built-in OVERLOADS it, and alc binds each call
//! by its arguments (measured on BC 28.4 with alc 18.0.41: `/work/.al-sem/engine-notes/eng173/
//! alc-bind/RESULTS.md` A1-A9 and `joker/RESULTS.md` J1-J9). The resolver classified every such
//! call as the platform record operation, so the call graph never reached the table's procedure:
//! its writes were a wrong `no`, and a RecordRef it rebinds stayed bound (Fable r3 on ENG-159,
//! Fable r1 on ENG-175). Now the operation gets a call site of its own reaching the procedure,
//! and, when the platform certainly cannot take the call, is no record operation at all.

use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"table 50490 "SB Log" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
table 50491 "SB Doc" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
table 50494 "SB Other" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
table 50492 "SB Entry"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }

    // A1: `Next(Steps?)` takes at most one argument, so `E.Next(RR, true)` runs this.
    procedure Next(var R: RecordRef; Really: Boolean)
    var
        L: Record "SB Log";
    begin
        L.Insert();
    end;

    // A4: `Count()` takes none, so `E.Count(R)` runs this: it rebinds R (a by-value RecordRef
    // shares the caller's handle, BC 28.4).
    procedure Count(X: RecordRef)
    var
        D: Record "SB Doc";
    begin
        X.GetTable(D);
    end;

    // Fable r3 on ENG-159 (TP7): `Find(Which?)` takes one Text, so `E.Find(RR, true)` runs this.
    procedure Find(var R: RecordRef; Really: Boolean): Boolean
    var
        L: Record "SB Log";
    begin
        R.GetTable(L);
    end;

    // A6/J4: a value Joker refuses a RecordRef, so `E.Insert(RR)` runs this, which inserts
    // nothing: the platform `Insert` never runs.
    procedure Insert(X: RecordRef)
    begin
    end;
}
codeunit 50493 "SB Mgt"
{
    procedure UserNext()
    var
        E: Record "SB Entry";
        RR: RecordRef;
    begin
        E.Next(RR, true);
    end;

    procedure PlatformNext()
    var
        E: Record "SB Entry";
        N: Integer;
    begin
        N := E.Next();
    end;

    procedure UserCount()
    var
        E: Record "SB Entry";
        E2: Record "SB Entry";
        R: RecordRef;
    begin
        R.GetTable(E2);
        E.Count(R);
        Guarded(R);
    end;

    procedure PlatformCount()
    var
        E: Record "SB Entry";
        E2: Record "SB Entry";
        R: RecordRef;
        N: Integer;
    begin
        R.GetTable(E2);
        N := E.Count();
        Guarded(R);
    end;

    // TP7: RR is a temporary Log until the table's `Find` rebinds it to the physical Log.
    procedure UserFindTemp()
    var
        RR: RecordRef;
        Tmp: Record "SB Log" temporary;
        E: Record "SB Entry";
    begin
        RR.GetTable(Tmp);
        E.Find(RR, true);
        RR.Insert();
    end;

    procedure PlatformFindTemp()
    var
        RR: RecordRef;
        Tmp: Record "SB Log" temporary;
        E: Record "SB Entry";
    begin
        RR.GetTable(Tmp);
        E.Find('-');
        RR.Insert();
    end;

    procedure UserInsert()
    var
        E: Record "SB Entry";
        RR: RecordRef;
    begin
        E.Insert(RR);
    end;

    procedure PlatformInsert()
    var
        E: Record "SB Entry";
    begin
        E.Insert();
    end;

    local procedure Guarded(var R: RecordRef)
    var
        O: Record "SB Other";
    begin
        if R.Number <> Database::"SB Doc" then
            exit;
        O.Insert();
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
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{args:?}: {e}; stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

/// Each routine against its platform control (the same code calling the platform method).
///
/// Discrimination (2026-11-23): the model not appending the procedure's call site fails
/// `UserNext`; the record-type pass ignoring a shadowed operation's holder arguments fails
/// `UserFindTemp`; the platform-cannot operation kept as a record operation fails `UserInsert`;
/// each restored, passes.
#[test]
fn a_table_procedure_named_like_a_record_builtin_is_called() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"e173e173-0000-4000-8000-000000000173","name":"ENG173","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/a.al"), SOURCE).unwrap();
    let inv = alsem(
        t.path(),
        &["fingerprint", "ws", "--inventory-only", "--format", "json"],
    );
    let answer = |name: &str, table: &str| -> String {
        let id = inv["payload"]["routineInventory"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["routineName"] == name)
            .unwrap_or_else(|| panic!("precondition: {name} is in the inventory"))
            ["stableRoutineId"]
            .as_str()
            .unwrap()
            .to_string();
        let q = format!("writes-table:{table}");
        alsem(t.path(), &["prove", "ws", &id, &q, "--deterministic"])["payload"]["result"]["answer"]
            .as_str()
            .unwrap()
            .to_string()
    };
    // The call graph reaches the table's procedure (A1); the platform shape does not.
    assert_eq!(answer("PlatformNext", "SB Log"), "no", "precondition");
    assert_eq!(answer("UserNext", "SB Log"), "yes");
    // A RecordRef the table's procedure rebinds (A4; Fable r1 on ENG-175's `T.Count(R)`).
    assert_eq!(answer("PlatformCount", "SB Other"), "no", "precondition");
    assert_eq!(answer("UserCount", "SB Other"), "yes");
    // The temp pass: the table's `Find` moves RR to the physical Log (Fable's TP7).
    assert_eq!(answer("PlatformFindTemp", "SB Log"), "no", "precondition");
    assert_ne!(answer("UserFindTemp", "SB Log"), "no");
    // No phantom operation: `E.Insert(RR)` runs only the table's empty `Insert`.
    assert_eq!(answer("PlatformInsert", "SB Entry"), "yes", "precondition");
    assert_eq!(answer("UserInsert", "SB Entry"), "no");
}
