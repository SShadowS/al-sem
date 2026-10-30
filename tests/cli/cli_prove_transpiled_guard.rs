//! G51: a guard in a dependency that ships only transpiled C# prunes like AL.
//!
//! Continia Core ships no AL. al-sem synthesizes it from the symbols and the
//! C# BC transpiled it into, and used to replace every branch condition with
//! an opaque Boolean. Core's `Notification Mgt.MessageHandler` opens a
//! `Message` under `if ShowMessage then`, and DO reaches it only with
//! `ShowMessage = false`; with the condition opaque, `prove … reaches-ui`
//! answered `yes` for a dialog that cannot open.
//!
//! The dependency here is in the transpiler's shape (no publisher's code):
//! `Check(ShowMessage)` forwards its parameter to `Handler(ShowMessage)`,
//! which shows a message under `if ShowMessage`. `Reassigned(ShowMessage)`
//! sets the parameter to `true` before the same guard: the synthesized body
//! drops assignments, so its condition must stay opaque, or `prove` would
//! prune a message that does open.

use std::path::Path;
use std::process::Command;

const DEP_GUID: &str = "51515151-0000-4000-8000-000000000051";

const CORE_CS: &str = r#"
public sealed class Codeunit50490 : NavCodeunit
{
    [NavName("Check")]
    [MethodId(5101)]
    public async ValueTask Check(bool showMessage)
    {
        ALMethodScope<(bool showMessage, bool γfiller)> γscope = default;
        try
        {
            γscope = new ALMethodScope<(bool showMessage, bool γfiller)>(this, "Check", MethodScopeFlags.None, 5101);
            γscope.ALStart();
            γscope.Target.showMessage = showMessage;
            {
                γscope.StmtHit(0);
                await this.Handler(γscope.Target.showMessage);
            }
        }
        finally
        {
            ALMethodScope.ALFinally(γscope);
        }
    }

    [NavName("Handler")]
    [MethodId(5102)]
    public async ValueTask Handler(bool showMessage)
    {
        ALMethodScope<(bool showMessage, bool γfiller)> γscope = default;
        try
        {
            γscope = new ALMethodScope<(bool showMessage, bool γfiller)>(this, "Handler", MethodScopeFlags.None, 5102);
            γscope.ALStart();
            γscope.Target.showMessage = showMessage;
            {
                if (γscope.CStmtHit(0) & (γscope.Target.showMessage))
                {
                    γscope.StmtHit(1);
                    NavDialog.ALMessage(this.Session, System.Guid.Parse("8da61efd-0002-0003-0507-0b0d1113171d"), new NavText("hi"));
                }
            }
        }
        finally
        {
            ALMethodScope.ALFinally(γscope);
        }
    }

    [NavName("Reassigned")]
    [MethodId(5103)]
    public async ValueTask Reassigned(bool showMessage)
    {
        ALMethodScope<(bool showMessage, bool γfiller)> γscope = default;
        try
        {
            γscope = new ALMethodScope<(bool showMessage, bool γfiller)>(this, "Reassigned", MethodScopeFlags.None, 5103);
            γscope.ALStart();
            γscope.Target.showMessage = showMessage;
            {
                γscope.StmtHit(0);
                γscope.Target.showMessage = true;
                if (γscope.CStmtHit(1) & (γscope.Target.showMessage))
                {
                    γscope.StmtHit(2);
                    NavDialog.ALMessage(this.Session, System.Guid.Parse("8da61efd-0002-0003-0507-0b0d1113171d"), new NavText("hi"));
                }
            }
        }
        finally
        {
            ALMethodScope.ALFinally(γscope);
        }
    }
}
"#;

const WS_AL: &str = r#"codeunit 50491 "G51 Caller"
{
    procedure Quiet()
    var
        Core: Codeunit "G51 Core";
    begin
        Core.Check(false);
    end;

    procedure Loud()
    var
        Core: Codeunit "G51 Core";
    begin
        Core.Check(true);
    end;

    procedure QuietButReassigned()
    var
        Core: Codeunit "G51 Core";
    begin
        Core.Reassigned(false);
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

#[test]
fn a_transpiled_guard_on_a_parameter_prunes_a_literal_false() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        format!(
            r#"{{"id":"52525252-0000-4000-8000-000000000052","name":"G51","publisher":"t","version":"1.0.0.0",
            "dependencies":[{{"id":"{DEP_GUID}","name":"G51Core","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    )
    .unwrap();
    std::fs::write(ws.join("src/Caller.al"), WS_AL).unwrap();
    let bool_param = r#"[{"Name":"ShowMessage","TypeDefinition":{"Name":"Boolean"}}]"#;
    crate::symbol_app::write_source_app(
        &ws.join(".alpackages/probe_G51Core_1.0.0.0.app"),
        DEP_GUID,
        "G51Core",
        "1.0.0.0",
        &format!(
            r#"{{"RuntimeVersion":"13.0","Codeunits":[{{"Id":50490,"Name":"G51 Core","Methods":[
            {{"Id":5101,"Name":"Check","Parameters":{bool_param}}},
            {{"Id":5102,"Name":"Handler","Parameters":{bool_param}}},
            {{"Id":5103,"Name":"Reassigned","Parameters":{bool_param}}}]}}],
            "AppId":"{DEP_GUID}","Name":"G51Core","Publisher":"probe","Version":"1.0.0.0"}}"#
        ),
        &[("bin/COD50490.cs", CORE_CS)],
        "",
    );

    // The synthesized AL the engine reads: `Handler`'s guard written out,
    // `Reassigned`'s kept opaque.
    let dump = t.path().join("dump");
    let out = Command::new(env!("CARGO_BIN_EXE_aldump"))
        .arg("--dump-dependency-source")
        .arg(&ws)
        .arg(&dump)
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let synth = std::fs::read_to_string(dump.join(DEP_GUID).join("transpiled/COD50490.al"))
        .expect("the synthesized codeunit");
    let ifs: Vec<&str> = synth
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("if "))
        .collect();
    assert_eq!(
        ifs,
        [
            "if showMessage then begin // cs 36",
            "if \"alsem branch\" then begin // cs 62",
        ],
        "{synth}"
    );
    assert!(
        synth.contains("\"__alsem_mid_5102\"(showMessage); // cs 16"),
        "Check forwards its flag bare, so the argument binds to the parameter: {synth}"
    );

    let inv = alsem(
        t.path(),
        &["fingerprint", "ws", "--inventory-only", "--format", "json"],
    );
    let id_of = |name: &str| -> String {
        inv["payload"]["routineInventory"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["routineName"] == name)
            .unwrap_or_else(|| panic!("precondition: routine {name} is in the inventory"))
            ["stableRoutineId"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let prove = |name: &str| -> serde_json::Value {
        alsem(
            t.path(),
            &[
                "prove",
                "ws",
                &id_of(name),
                "reaches-ui",
                "--format",
                "json",
                "--deterministic",
            ],
        )["payload"]["result"]
            .clone()
    };

    // The precondition: the message is reached at all, through the
    // synthesized dependency body.
    let loud = prove("Loud");
    assert_eq!(
        loud["answer"], "yes",
        "Check(true) shows the message: {loud}"
    );

    let quiet = prove("Quiet");
    assert_eq!(
        quiet["answer"], "no",
        "Check(false) forwards false to Handler's `if ShowMessage`: {quiet}"
    );

    let reassigned = prove("QuietButReassigned");
    assert_eq!(
        reassigned["answer"], "yes",
        "Reassigned sets ShowMessage before the guard: its condition must stay opaque: {reassigned}"
    );
}
