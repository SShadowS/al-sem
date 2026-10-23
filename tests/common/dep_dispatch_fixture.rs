//! #70: a workspace and an embedded-source dependency whose PUBLISHER the
//! workspace raises (the probe fixture of
//! `docs/superpowers/specs/2026-10-15-issue-70-cross-app-follow-ups-probes.md`).
//!
//! Dependency `XBase` (`Dep Lib`, `Dep Sub`): `Outer()` calls `Inner()`, which
//! inserts `Customer` and commits; `[IntegrationEvent] OnDepEvent()`;
//! `Dep Sub.HandleDep` subscribes to it and inserts `Customer`.
//! Workspace `Ws70` (`Ws Work`, `Ws Sub`): `CallChain()` calls `Lib.Outer()`,
//! `RaiseDep()` calls `Lib.OnDepEvent()`, `TouchCust()` inserts `Customer`;
//! `Ws Sub.HandleInWs` subscribes to `OnDepEvent` and inserts `Ws Flag`.

use std::path::Path;

pub const WS_GUID: &str = "aaaa7070-0000-0000-0000-000000000070";
pub const DEP_GUID: &str = "bbbb7070-0000-0000-0000-000000000070";

const CUSTOMER_AL: &str = "table 18 Customer\n{\n    fields\n    {\n        field(1; \"No.\"; Code[20])\n        {\n        }\n    }\n}\n";

const DEP_LIB_AL: &str = r#"codeunit 10 "Dep Lib"
{
    procedure Outer()
    begin
        Inner();
    end;

    procedure Inner()
    var
        Cust: Record Customer;
    begin
        Cust.Insert();
        Commit();
    end;

    [IntegrationEvent(false, false)]
    procedure OnDepEvent()
    begin
    end;
}

codeunit 11 "Dep Sub"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Dep Lib", 'OnDepEvent', '', false, false)]
    local procedure HandleDep()
    var
        Cust: Record Customer;
    begin
        Cust.Insert();
    end;
}
"#;

/// The workspace source. Every routine of the workspace lives here (plus any
/// `extra_al` the caller adds as `src/Extra.al`).
pub const WS_AL: &str = r#"table 50170 "Ws Flag"
{
    fields
    {
        field(1; "Entry No."; Integer)
        {
        }
    }
}

codeunit 50170 "Ws Work"
{
    procedure CallChain()
    var
        Lib: Codeunit "Dep Lib";
    begin
        Lib.Outer();
    end;

    procedure RaiseDep()
    var
        Lib: Codeunit "Dep Lib";
    begin
        Lib.OnDepEvent();
    end;

    procedure TouchCust()
    var
        Cust: Record Customer;
    begin
        Cust.Insert();
    end;
}

codeunit 50171 "Ws Sub"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Dep Lib", 'OnDepEvent', '', false, false)]
    local procedure HandleInWs()
    var
        Flag: Record "Ws Flag";
    begin
        Flag.Insert();
    end;
}
"#;

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// `app.json` (declares `XBase`, no `platform`) + `src/Work.al`, and
/// `src/Extra.al` when `extra_al` is non-empty.
pub fn write_workspace(dir: &Path, extra_al: &str) {
    write(
        &dir.join("app.json"),
        &format!(
            r#"{{"id":"{WS_GUID}","name":"Ws70","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50100,"to":50199}}],"dependencies":[{{"id":"{DEP_GUID}","name":"XBase","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    );
    write(&dir.join("src/Work.al"), WS_AL);
    if !extra_al.is_empty() {
        write(&dir.join("src/Extra.al"), extra_al);
    }
}

/// The `XBase` dependency `.app` (embedded source) at `app_path`. `revision`
/// is appended to `Lib.al` as a comment, so two revisions differ in bytes
/// and in nothing the analysis reads.
pub fn write_dependency(app_path: &Path, revision: u32) {
    write_dependency_lib(app_path, revision, DEP_LIB_AL);
}

/// [`write_dependency`] with `Lib.al` = `lib_al`.
fn write_dependency_lib(app_path: &Path, revision: u32, lib_al: &str) {
    let symbols = format!(
        r#"{{"RuntimeVersion":"13.0","Tables":[{{"Id":18,"Name":"Customer","Fields":[{{"Id":1,"Name":"No.","TypeDefinition":{{"Name":"Code"}}}}]}}],"Codeunits":[{{"Id":10,"Name":"Dep Lib","Methods":[{{"Name":"Outer","Parameters":[]}},{{"Name":"Inner","Parameters":[]}},{{"Name":"OnDepEvent","Parameters":[],"Attributes":[{{"Name":"IntegrationEvent","Arguments":[{{"Value":"False"}},{{"Value":"False"}}]}}]}}]}},{{"Id":11,"Name":"Dep Sub","Methods":[]}}],"AppId":"{DEP_GUID}","Name":"XBase","Publisher":"probe","Version":"1.0.0.0"}}"#
    );
    let lib = format!("{lib_al}// revision {revision}\n");
    crate::symbol_app::write_source_app(
        app_path,
        DEP_GUID,
        "XBase",
        "1.0.0.0",
        &symbols,
        &[("src/Customer.al", CUSTOMER_AL), ("src/Lib.al", &lib)],
        "",
    );
}

/// The `.app` file name the loader expects in a package cache.
pub const DEP_APP_FILE: &str = "probe_XBase_1.0.0.0.app";

/// The workspace with the dependency in its OWN `.alpackages`.
pub fn write_own(dir: &Path, extra_al: &str) {
    write_workspace(dir, extra_al);
    write_dependency(&dir.join(".alpackages").join(DEP_APP_FILE), 0);
}

/// #161's text: [`write_own`] whose `Inner` does `Cust.Modify()` instead of
/// `Cust.Insert()`.
pub fn write_own_modify(dir: &Path, extra_al: &str) {
    let lib = DEP_LIB_AL.replacen(
        "Cust.Insert();\n        Commit();",
        "Cust.Modify();\n        Commit();",
        1,
    );
    assert_ne!(lib, DEP_LIB_AL, "fixture: Inner's insert is replaced");
    write_workspace(dir, extra_al);
    write_dependency_lib(&dir.join(".alpackages").join(DEP_APP_FILE), 0, &lib);
}
