//! ENG-173: a table procedure named like a record built-in OVERLOADS it, and alc binds each call
//! by its arguments (measured on BC 28.4 with alc 18.0.41: `/work/.al-sem/engine-notes/eng173/
//! alc-bind/RESULTS.md` A1-A9 and `joker/RESULTS.md` J1-J9). The resolver classified every such
//! call as the platform record operation, so the call graph never reached the table's procedure:
//! its writes were a wrong `no`, and a RecordRef it rebinds stayed bound (Fable r3 on ENG-159,
//! Fable r1 on ENG-175). Now the operation gets a call site of its own reaching the procedure,
//! and, when the platform certainly cannot take the call, is no record operation at all.

use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"table 50490 "SB Log"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }

    procedure PlainRecModify()
    begin
        Rec.Modify(true);
    end;

    // Fable r1 on ENG-178 (F1): the implicit `Rec` keeps its table through parentheses.
    procedure ParenRecModify()
    begin
        (Rec).Modify(true);
    end;
}
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

    // sol on ENG-173: `FindLast()` takes no argument, so `E.FindLast(Doc)` runs this; its write
    // goes through the caller's record (a `var` binding), temporary or not.
    procedure FindLast(var X: Record "SB Doc")
    begin
        X.Insert();
    end;

    // Fable on ENG-173 (F1): writes nothing. `ModifyAll(Field, NewValue, RunTrigger?)` takes a
    // field first: a system field is one though no table declares it; a Variant variable is not.
    procedure ModifyAll(A: Variant; B: Variant)
    begin
    end;

    // A6: `E.Get(RR)` ran this on BC, though the platform `Get(Value…)` could take a RecordRef
    // too (alc's ranking): both are kept, the call in one branch of the statement tree.
    // ENG-178 (Fable r2 on ENG-173, H20): alc binds `E.Delete()` to this, though the platform's
    // `Delete(RunTrigger?)` could take it too (no AL0524). It deletes nothing.
    procedure Delete()
    begin
    end;

    // ENG-178: exactly the platform's `Modify(RunTrigger: Boolean)` (AL0524): it hides it, for a
    // parenthesised receiver too. It modifies nothing.
    procedure Modify(B: Boolean)
    begin
    end;

    procedure Get(X: RecordRef): Boolean
    var
        D: Record "SB Doc";
    begin
        X.GetTable(D);
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

    procedure UserFindLastPhysical()
    var
        E: Record "SB Entry";
        Doc: Record "SB Doc";
    begin
        E.FindLast(Doc);
    end;

    procedure UserFindLastTemp()
    var
        E: Record "SB Entry";
        Doc: Record "SB Doc" temporary;
    begin
        E.FindLast(Doc);
    end;

    procedure BothGet()
    var
        E: Record "SB Entry";
        E2: Record "SB Entry";
        R: RecordRef;
    begin
        R.GetTable(E2);
        if E.Get(R) then;
        Guarded(R);
    end;

    procedure PlatformGet()
    var
        E: Record "SB Entry";
        E2: Record "SB Entry";
        R: RecordRef;
    begin
        R.GetTable(E2);
        if E.FindFirst() then;
        Guarded(R);
    end;

    procedure PlainModify()
    var
        E: Record "SB Entry";
    begin
        E.Modify(true);
    end;

    procedure ParenModify()
    var
        E: Record "SB Entry";
    begin
        (E).Modify(true);
    end;

    // No table procedure: the platform's `Modify` runs, on a physical record or a temporary one.
    procedure ParenLogModify()
    var
        L: Record "SB Log";
    begin
        (L).Modify(true);
    end;

    procedure ParenTempLogModify()
    var
        L: Record "SB Log" temporary;
    begin
        (L).Modify(true);
    end;

    procedure ZeroDelete()
    var
        E: Record "SB Entry";
    begin
        E.Delete();
    end;

    procedure PlatformDelete()
    var
        E: Record "SB Entry";
    begin
        E.Delete(true);
    end;

    procedure SystemFieldModifyAll()
    var
        E: Record "SB Entry";
        DT: DateTime;
    begin
        E.ModifyAll(SystemModifiedAt, DT);
    end;

    procedure VariantModifyAll()
    var
        E: Record "SB Entry";
        V: Variant;
        DT: DateTime;
    begin
        E.ModifyAll(V, DT);
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
/// Discrimination (2026-11-23): the body walk ignoring the resolver's decision
/// (`FileBindings::table_procedure`: no call site, the platform operation kept) fails `UserNext`;
/// a name the table scope does not declare read as no field (a system field) fails
/// `SystemFieldModifyAll`; each restored, passes. The hiding rule (AL0524) is pinned by the
/// adapter census of `tests/r0-corpus/ws-report-dataitem` (`GetFilters()`), which fails with it
/// off. ENG-178: the zero-parameter-over-optional rule off fails `ZeroDelete`;
/// `record_ops::receiver_variable` not reading through parentheses fails `ParenModify`;
/// `record_ops::receiver_key_text` keeping the parentheses fails `ParenRecModify`.
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
    // sol on ENG-173: the call site carries its argument bindings, so the procedure's write
    // through a `var` record is the caller's record: physical writes, temporary does not.
    assert_eq!(
        answer("UserFindLastPhysical", "SB Doc"),
        "yes",
        "precondition"
    );
    assert_eq!(answer("UserFindLastTemp", "SB Doc"), "no");
    // Fable on ENG-173 (F1): a system field may be the platform's field argument (the platform
    // `ModifyAll` may run: written); a Variant variable is no field (only the table's empty
    // `ModifyAll` runs: not written).
    assert_eq!(answer("VariantModifyAll", "SB Entry"), "no", "precondition");
    assert_ne!(answer("SystemFieldModifyAll", "SB Entry"), "no");
    // sol r2 on ENG-173: where both may run (A6: alc ranks), the statement tree holds the
    // operation and the call as alternatives, so the call's rebinding of R reaches the guard.
    assert_eq!(answer("PlatformGet", "SB Other"), "no", "precondition");
    assert_ne!(answer("BothGet", "SB Other"), "no");
    // ENG-178: a zero-parameter procedure takes the zero-argument call from an all-optional
    // platform overload (alc binds it): `E.Delete()` deletes nothing; `E.Delete(true)` does.
    assert_eq!(answer("PlatformDelete", "SB Entry"), "yes", "precondition");
    assert_eq!(answer("ZeroDelete", "SB Entry"), "no");
    // ENG-178: `(E).Modify(true)` is `E.Modify(true)`: the table's `Modify(Boolean)` hides the
    // platform's for both (no write).
    assert_eq!(answer("PlainModify", "SB Entry"), "no", "precondition");
    assert_eq!(answer("ParenModify", "SB Entry"), "no");
    // ENG-178: a parenthesised receiver with no table procedure is the platform operation on its
    // record: a physical record writes, a temporary one does not, and the implicit `Rec` keeps its
    // table (Fable r1, F1: the key text `(Rec)` read no table).
    assert_eq!(answer("PlainRecModify", "SB Log"), "yes", "precondition");
    assert_eq!(answer("ParenRecModify", "SB Log"), "yes");
    assert_eq!(answer("ParenLogModify", "SB Log"), "yes");
    assert_eq!(answer("ParenTempLogModify", "SB Log"), "no");
}

/// ENG-178 (Fable r2 on ENG-173): `alsem graph`, which bc-dev-mcp's test selection walks
/// backwards, has the caller's edge to the table's procedure: a resolved one where only the
/// procedure runs (`UserNext` → `Next`), an over-approximate one where the platform operation may
/// run instead (`BothGet` → `Get`, A6: alc ranks).
///
/// Discrimination (2026-11-23): the projection leaving out a both-may site's procedure routes
/// (no `table_procedure` chain in `engine_edges`) loses `BothGet`'s edge; restored, passes.
#[test]
fn the_call_graph_export_reaches_the_table_procedure() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"e173e173-0000-4000-8000-000000000173","name":"ENG173","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/a.al"), SOURCE).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("graph")
        .arg(&ws)
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rows: Vec<serde_json::Value> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let node = |object: &str, member: &str| -> String {
        rows.iter()
            .find(|r| {
                r["t"] == "node" && r["v"]["objectName"] == object && r["v"]["memberName"] == member
            })
            .unwrap_or_else(|| panic!("precondition: node {object}.{member}"))["v"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let confidence = |caller: &str, callee: &str| -> Option<String> {
        let (from, to) = (node("SB Mgt", caller), node("SB Entry", callee));
        rows.iter()
            .find(|r| r["t"] == "edge" && r["v"]["caller"] == from && r["v"]["callee"] == to)
            .map(|r| r["v"]["confidence"].as_str().unwrap().to_string())
    };
    assert_eq!(confidence("UserNext", "Next").as_deref(), Some("Resolved"));
    assert_eq!(confidence("PlatformNext", "Next"), None, "precondition");
    let both = confidence("BothGet", "Get");
    assert!(
        both.as_deref().is_some_and(|c| c != "Resolved"),
        "BothGet -> Get: {both:?}"
    );
}
