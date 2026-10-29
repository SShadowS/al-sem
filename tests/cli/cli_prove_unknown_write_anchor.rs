//! A write whose table is not known is witnessed at ITS OWN operation. Digest's
//! producer-proof search matched facts with `fact_equivalent`, which treats a
//! missing resource as equal to any resource, so an unresolved write inside a
//! dependency (its own paths end at the dependency boundary, rule B) was proved
//! by a workspace parameter-bound producer's path instead: `F.Insert()` on a
//! `var Record` parameter became its evidence, and `prove`'s
//! `unknown-table-write` blocker pointed at that typed insert (Continia DO:
//! three such blockers anchored at a typed `CDO File` insert).
//!
//! Proved at its own operation, the write is a dependency boundary group's,
//! and a group listed only the resources its facts name: a write with none
//! vanished from it, so `prove` could answer `no` past an unknown write. The
//! group now counts them (`unresolvedResources`), and `prove` blocks on it.

use std::process::Command;

use crate::dep_dispatch_fixture as dd;

/// `Dep Lib.Outer` inserts into a table the dependency does not declare.
const LIB: &str = r#"codeunit 10 "Dep Lib"
{
    procedure Outer()
    var
        R: Record "Dep Missing";
    begin
        R.Insert();
    end;

    [IntegrationEvent(false, false)]
    procedure OnDepEvent()
    begin
    end;
}
"#;

/// A workspace producer (an insert on a `var Record` parameter) and a root
/// whose one call (`Mid`) reaches both it, with the root's own record
/// parameter, and the dependency: the two facts share the root's first hop,
/// which the producer proof starts from, and neither is known temporary.
const EXTRA: &str = r#"table 50180 "UW T"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }
}
codeunit 50181 "UW Root"
{
    procedure Producer(var F: Record "UW T")
    begin
        F.Insert();
    end;

    procedure Go(var F: Record "UW T")
    begin
        Mid(F);
    end;

    procedure Mid(var F: Record "UW T")
    var
        Lib: Codeunit "Dep Lib";
    begin
        Producer(F);
        Lib.Outer();
    end;
}
"#;

#[test]
fn an_unknown_table_write_is_never_witnessed_by_a_typed_producer() {
    let t = tempfile::tempdir().unwrap();
    dd::write_own_lib(t.path(), EXTRA, LIB);
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("prove")
        .arg(t.path())
        .args(["Go", "writes-table:UW T", "--deterministic"])
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    let r =
        serde_json::from_slice::<serde_json::Value>(&out.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)))["payload"]
            ["result"]
            .clone();
    // The dependency writes a table that is not known, so the answer is not
    // `no`: the boundary group that holds the write says so.
    assert_eq!(r["answer"], "unknown", "{r}");
    let anchors: Vec<String> = r["blockedBy"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["kind"] == "unknown-table-write")
        .map(|b| b["anchor"]["excerpt"].as_str().unwrap_or("").to_string())
        .collect();
    assert!(!anchors.is_empty(), "an unknown-table-write blocker: {r}");
    assert!(
        !anchors.iter().any(|a| a == "F.Insert"),
        "the typed producer is no unknown-table write's evidence: {r}"
    );
}
