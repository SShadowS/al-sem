//! Round-3 m04: `prove … throws-error` follows the literal Boolean arguments
//! that guard an `Error(..)`, as `writes-table` and `may-commit` already did
//! for writes and commits (G23).
//!
//! On Continia DO, `PupulateAppCache` calls `HasValidCredentials(false)`, and
//! the error fires only under `if ShowError and not IsClientCredentialValid
//! then Error(..)`; prove still listed that ERROR_THROW as a witness. Two
//! defects stacked:
//! - an `Error(..)` statement's statement-tree leaf carries its call-site id
//!   while its `error-throw` fact names its `error-call` operation id, so no
//!   guard ever reached an explicit error, not even `if ShowError then
//!   Error(..)` (`param_guard::alias_error_ops`);
//! - only a whole-condition guard (`P`, `not P`, `P = false`) was read, not
//!   the guards among the operands of an `and` chain (THEN branch) or an `or`
//!   chain (ELSE branch and what follows `if … then exit;`)
//!   (`PCFNNode::junction_guards`).
//!
//! The cone's guard (singleton SCCs) and the per-root literal reach
//! (recursive SCCs, `literal_reach`) read the same frames; both are asked.
//! Controls keep every error that can still run a `yes`: a literal `true`, an
//! `or` whose other operand is unknown under a literal `false`, a guard on an
//! unknown local, and a root that takes the flag as its own parameter.
//!
//! Discrimination (2026-11-07), each break made with Edit, run with
//! `cargo test --test cli cli_prove_literal_error_guards::`, then reverted:
//! - dropping `alias_error_ops(r, &mut by_site)` in `frame_guards`: fails at
//!   `SimpleFalse` (answer `yes`);
//! - `branch_reqs` ignoring `JunctionGuards::All`: fails at `AndFalse`;
//! - `branch_reqs` ignoring `JunctionGuards::Any`: fails at `OrExitTrue`;
//! - `branch_reqs` giving `All` its conjuncts on the ELSE branch too (an
//!   unsound rule): fails at the control `AndElseFalse`.

use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"codeunit 50250 "M04 Cred"
{
    procedure SimpleFalse()
    begin
        CheckSimple(false);
    end;

    procedure SimpleTrue()
    begin
        CheckSimple(true);
    end;

    procedure AndFalse()
    begin
        CheckAnd(false);
    end;

    procedure AndTrue()
    begin
        CheckAnd(true);
    end;

    procedure AndRecursiveFalse()
    begin
        RecA(false, 0);
    end;

    procedure AndRecursiveTrue()
    begin
        RecA(true, 0);
    end;

    procedure OrExitTrue()
    begin
        CheckOrExit(true);
    end;

    procedure OrExitFalse()
    begin
        CheckOrExit(false);
    end;

    procedure AndElseFalse()
    begin
        CheckAndElse(false);
    end;

    procedure OwnFlag(ShowError: Boolean)
    begin
        CheckAnd(ShowError);
    end;

    local procedure CheckSimple(ShowError: Boolean)
    begin
        if ShowError then
            Error('Invalid');
    end;

    local procedure CheckAnd(ShowError: Boolean): Boolean
    var
        IsValid: Boolean;
    begin
        IsValid := Valid();
        if ShowError and not IsValid then
            Error('Invalid')
        else
            exit(IsValid);
    end;

    local procedure RecA(ShowError: Boolean; Depth: Integer)
    begin
        RecB(ShowError, Depth);
    end;

    local procedure RecB(ShowError: Boolean; Depth: Integer)
    begin
        if Depth < 3 then
            RecA(ShowError, Depth + 1);
        if (ShowError) and (Depth > 0) then
            Error('Invalid');
    end;

    local procedure CheckOrExit(Silent: Boolean)
    begin
        if Silent or Valid() then
            exit;
        Error('Invalid');
    end;

    // The ELSE of an `and` is not constrained: `Strict = false` runs it.
    local procedure CheckAndElse(Strict: Boolean)
    begin
        if Strict and Valid() then
            exit
        else
            Error('Invalid');
    end;

    local procedure Valid(): Boolean
    begin
        exit(Random(2) = 1);
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

#[test]
fn an_error_behind_a_literal_false_guard_is_not_thrown() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"50250000-0000-4000-8000-000000000004","name":"M04","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/Cred.al"), SOURCE).unwrap();
    let prove = |name: &str| -> serde_json::Value {
        alsem(
            t.path(),
            &[
                "prove",
                "ws",
                &format!("Codeunit \"M04 Cred\"::{name}"),
                "throws-error",
                "--deterministic",
            ],
        )["payload"]["result"]
            .clone()
    };
    for routine in ["SimpleFalse", "AndFalse", "AndRecursiveFalse", "OrExitTrue"] {
        let r = prove(routine);
        assert_eq!(r["answer"], "no", "{routine}: the only Error is dead: {r}");
    }
    for routine in [
        "SimpleTrue",
        "AndTrue",
        "AndRecursiveTrue",
        "OrExitFalse",
        "AndElseFalse",
        "OwnFlag",
    ] {
        let r = prove(routine);
        assert_eq!(r["answer"], "yes", "{routine}: the Error can run: {r}");
        let ev = r["evidence"].as_array().cloned().unwrap_or_default();
        assert!(
            ev.iter().any(|e| e["type"] == "ERROR_THROW"),
            "{routine}: the witness is the explicit Error: {r}"
        );
    }
}
