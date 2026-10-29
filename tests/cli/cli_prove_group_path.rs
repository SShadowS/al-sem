//! G35: a `prove` answer on a dependency boundary group shows how the
//! dependency's code reaches the asked table (`groupPath`), along a path that
//! can run.
//!
//! Under rule B a witness ends where it enters dependency code, and the group
//! only lists the tables reached: Continia DO's `writes-table:CDO E-Mail Log`
//! was `yes` through `Base Application.CalcInvDiscForHeader` with no path to
//! check. The path comes from the guarded search G33 runs, so a branch it
//! proves dead is never shown: here the shortest route to the `Dep Log` write
//! goes through a `case RecRef.Number` branch for another table (a `Dep Line`
//! delete trigger hands its own `Rec`), and the path shown is the longer live
//! one.

use std::process::Command;

use crate::dep_dispatch_fixture as dd;

const LIB: &str = r#"table 20 "Dep Line"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }

    trigger OnDelete()
    var
        Approvals: Codeunit "Dep Approvals";
    begin
        Approvals.Prevent(Rec);
    end;
}
table 21 "Dep Batch"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }
}
table 22 "Dep Log"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }
}
codeunit 11 "Dep Approvals"
{
    procedure Prevent(Variant: Variant)
    var
        RecRef: RecordRef;
    begin
        RecRef.GetTable(Variant);
        case RecRef.Number of
            Database::"Dep Batch":
                WriteLog();
        end;
    end;

    procedure WriteLog()
    var
        Log: Record "Dep Log";
    begin
        Log.Code := 'X';
        Log.Insert();
    end;
}
codeunit 10 "Dep Lib"
{
    procedure Outer()
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnDepEvent()
    begin
    end;

    procedure Entry()
    var
        Line: Record "Dep Line";
    begin
        Line.Get('A');
        Line.Delete(true);
        Long1();
    end;

    local procedure Long1()
    begin
        Long2();
    end;

    local procedure Long2()
    begin
        Long3();
    end;

    local procedure Long3()
    var
        Approvals: Codeunit "Dep Approvals";
    begin
        Approvals.WriteLog();
    end;
}
"#;

const EXTRA: &str = r#"codeunit 50500 "GP Root"
{
    procedure Go()
    var
        Lib: Codeunit "Dep Lib";
    begin
        Lib.Entry();
    end;
}
"#;

#[test]
fn a_boundary_group_answer_shows_a_live_path_to_the_table() {
    let t = tempfile::tempdir().unwrap();
    dd::write_own_lib(t.path(), EXTRA, LIB);
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("prove")
        .arg(t.path())
        .args(["Go", "writes-table:Dep Log", "--deterministic"])
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
    let r = &v["payload"]["result"];
    assert_eq!(r["answer"], "yes", "{r}");
    let paths: Vec<Vec<String>> = r["evidence"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e["groupPath"]["hops"].as_array())
        .map(|h| {
            h.iter()
                .map(|x| x.as_str().unwrap_or("").to_string())
                .collect()
        })
        .collect();
    assert!(!paths.is_empty(), "a groupPath: {r}");
    for hops in &paths {
        assert!(hops[0].ends_with("GP Root.Go"), "from the root: {hops:?}");
        assert!(
            hops.last().unwrap().ends_with("Dep Approvals.WriteLog"),
            "to the write: {hops:?}"
        );
        assert!(
            hops.iter().any(|h| h.ends_with("Dep Lib.Long2")),
            "the live route: {hops:?}"
        );
        assert!(
            !hops.iter().any(|h| h.contains("Prevent")),
            "never the dead branch: {hops:?}"
        );
    }
}
