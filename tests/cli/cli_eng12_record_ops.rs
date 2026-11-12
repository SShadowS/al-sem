//! ENG-12 (engine limits L35, L36, ENG-06 review gap (b)): record operations
//! the engine did not see. Probe on master `bdb7aab5`:
//! docs/superpowers/notes/2026-11-12-eng12-record-ops-probe.md. Every
//! routine below answered a definite `no` there.

use std::process::Command;

const SOURCE: &str = r#"table 52600 "E12 Row"
{
    fields { field(1; K; Integer) { } field(2; V; Integer) { } }
    keys { key(PK; K) { } }
    trigger OnModify()
    begin
        Commit();
    end;
    procedure BareSysId(Id: Guid)
    begin
        if GetBySystemId(Id) then;
    end;
}
codeunit 52601 "E12 Ops"
{
    procedure SysIdMember(Id: Guid)
    var
        R: Record "E12 Row";
    begin
        if R.GetBySystemId(Id) then;
    end;
    procedure RecRefSysId(Id: Guid)
    var
        RR: RecordRef;
    begin
        RR.Open(Database::"E12 Row");
        if RR.GetBySystemId(Id) then;
    end;
    procedure ArrMember()
    var
        Recs: array[2] of Record "E12 Row";
    begin
        if Recs[1].FindFirst() then;
    end;
    procedure ArrWith()
    var
        Recs: array[2] of Record "E12 Row";
    begin
        with Recs[1] do
            if FindFirst() then;
    end;
    procedure ArrModifyTrue()
    var
        Recs: array[2] of Record "E12 Row";
    begin
        Recs[1].Modify(true);
    end;
    procedure RRArrModifyTrue()
    var
        RRs: array[2] of RecordRef;
    begin
        RRs[1].Open(Database::"E12 Row");
        RRs[1].Modify(true);
    end;
    procedure GetRefModifyTrue()
    begin
        GetRef().Modify(true);
    end;
    local procedure GetRef(): Record "E12 Row"
    var
        R: Record "E12 Row";
    begin
        exit(R);
    end;
}
"#;

fn workspace() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"e12e12e0-0000-4000-8000-0000000000aa","name":"E12","publisher":"t","version":"1.0.0.0","dependencies":[]}"#,
    )
    .unwrap();
    std::fs::write(t.path().join("src/a.al"), SOURCE).unwrap();
    t
}

fn run(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8")
}

/// The `answer:` line of `query touches --table "E12 Row" --direction down`.
fn touches_down(ws: &str, from: &str) -> String {
    let h = run(&[
        "query",
        "touches",
        "--table",
        "E12 Row",
        "--from",
        from,
        "--direction",
        "down",
        ws,
    ]);
    h.lines()
        .find(|l| l.starts_with("answer:"))
        .unwrap_or_default()
        .to_string()
}

/// L35: `GetBySystemId` reads its table (on a record, bare in the table,
/// on a RecordRef opened on it).
/// Break (2026-11-12): drop `getbysystemid` from `record_ops::record_op_type`
/// -> each answers `down: no (every candidate decided)`; restored, passes.
#[test]
fn get_by_system_id_reads_its_table() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    for from in [
        "\"E12 Row\".BareSysId",
        "\"E12 Ops\".SysIdMember",
        "\"E12 Ops\".RecRefSysId",
    ] {
        let a = touches_down(ws, from);
        assert!(a.starts_with("answer: down: yes"), "{from}: {a}");
    }
}
