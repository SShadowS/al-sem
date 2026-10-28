//! G30: an interface call on an object global that only its own codeunit
//! writes, from an `internal` setter whose every caller passes a codeunit
//! variable, dispatches over those codeunits alone: a closed site, in the
//! default world, with no assumption. On Continia DO: `CDO Template Variant
//! Mgt`'s `FieldChainEvaluator`, set by `SetFieldChainEvaluator`.
//!
//! Here a second implementer (`G30 Writing Evaluator`) writes the table, but
//! no setter call stores it: the site's routes narrow to `G30 Real
//! Evaluator`, so `writes-table` is `no`. The site stays open (an open-world
//! row, and a MAY `yes` through the writing implementer) when the setter is
//! public (code that is not loaded may call it), or when a caller passes an
//! interface value (any implementation).

use std::path::Path;
use std::process::Command;

fn source(setter_access: &str, caller: &str) -> String {
    format!(
        r#"table 50310 "G30 T"
{{
    fields {{ field(1; Code; Code[20]) {{ }} }}
    keys {{ key(PK; Code) {{ }} }}
}}
interface "G30 Evaluator"
{{
    procedure Evaluate(): Text;
}}
codeunit 50311 "G30 Real Evaluator" implements "G30 Evaluator"
{{
    procedure Evaluate(): Text
    begin
        exit('x');
    end;
}}
codeunit 50312 "G30 Writing Evaluator" implements "G30 Evaluator"
{{
    procedure Evaluate(): Text
    var
        T: Record "G30 T";
    begin
        T.Code := 'X';
        T.Insert();
        exit('w');
    end;
}}
codeunit 50313 "G30 Holder"
{{
    Access = Internal;

    var
        Evaluator: Interface "G30 Evaluator";

    {setter_access} procedure SetEvaluator(NewEvaluator: Interface "G30 Evaluator")
    begin
        Evaluator := NewEvaluator;
    end;

    internal procedure Lookup(): Text
    begin
        exit(Evaluator.Evaluate());
    end;
}}
codeunit 50314 "G30 Caller"
{{
{caller}
}}
"#
    )
}

const PASSES_CODEUNIT: &str = r#"    procedure Go(): Text
    var
        Real: Codeunit "G30 Real Evaluator";
        Holder: Codeunit "G30 Holder";
    begin
        Holder.SetEvaluator(Real);
        exit(Holder.Lookup());
    end;"#;

/// One caller passes a codeunit, another an interface value: the second alone
/// keeps the site open.
const PASSES_INTERFACE: &str = r#"    procedure Go(): Text
    var
        Real: Codeunit "G30 Real Evaluator";
        Holder: Codeunit "G30 Holder";
    begin
        Holder.SetEvaluator(Real);
        exit(Holder.Lookup());
    end;

    procedure GoAny(Any: Interface "G30 Evaluator"): Text
    var
        Holder: Codeunit "G30 Holder";
    begin
        Holder.SetEvaluator(Any);
        exit(Holder.Lookup());
    end;"#;

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

/// `prove <routine> writes-table:G30 T` on a workspace of `source(..)`.
fn prove(setter_access: &str, caller: &str, routine: &str) -> serde_json::Value {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"30303030-0000-4000-8000-000000000030","name":"G30","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/Shapes.al"), source(setter_access, caller)).unwrap();
    let inv = alsem(
        t.path(),
        &["fingerprint", "ws", "--inventory-only", "--format", "json"],
    );
    let id = inv["payload"]["routineInventory"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["routineName"] == routine)
        .unwrap_or_else(|| panic!("precondition: {routine} is in the inventory"))["stableRoutineId"]
        .as_str()
        .unwrap()
        .to_string();
    alsem(
        t.path(),
        &["prove", "ws", &id, "writes-table:G30 T", "--deterministic"],
    )["payload"]["result"]
        .clone()
}

fn blocks_on_dispatch(r: &serde_json::Value) -> bool {
    r["blockedBy"].as_array().is_some_and(|a| {
        a.iter()
            .any(|b| b["kind"].as_str() == Some("open-world-dispatch"))
    })
}

#[test]
fn a_global_set_only_from_codeunit_variables_is_closed() {
    for routine in ["Go", "Lookup"] {
        let r = prove("internal", PASSES_CODEUNIT, routine);
        assert_eq!(r["answer"], "no", "{routine}: {r}");
        assert!(!blocks_on_dispatch(&r), "{routine}: {r}");
        assert!(r.get("assumptions").is_none(), "{routine}: {r}");
    }
}

#[test]
fn a_public_setter_or_an_interface_argument_keeps_it_open() {
    for (case, r) in [
        ("public setter", prove("", PASSES_CODEUNIT, "Lookup")),
        (
            "interface argument",
            prove("internal", PASSES_INTERFACE, "Lookup"),
        ),
    ] {
        assert_eq!(r["answer"], "yes", "{case}: every implementer: {r}");
        assert_eq!(r["obligations"]["openWorldRows"], 1, "{case}: {r}");
    }
}
