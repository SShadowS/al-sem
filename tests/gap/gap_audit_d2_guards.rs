//! Detector-audit classes A + C for d2 (docs/detector-audit.md): d2 must apply
//! the SAME three guards d1 already has to its subscriber db-op selection:
//!
//! - **Next-terminator (G-1)**: a subscriber whose ONLY db op is the
//!   `until …Next() = 0` terminator of its own `repeat` loop is iterating an
//!   already-positioned cursor — the terminator `Next` is the loop's own
//!   advancement, never an actionable db op → NO d2.
//! - **Virtual/system table (G-6)**: a subscriber reading `AllObjWithCaption` /
//!   `Field` / … hits the platform's in-memory metadata store, not SQL → NO d2.
//! - **Temporary record**: a subscriber whose ops are all on a `temporary`
//!   record (`temp_state` Known(true)) does no physical db work → NO d2.
//!
//! Suppression-direction control: a subscriber with a REAL db op (Modify on a
//! physical record) inside a loop must STILL fire d2.
//!
//! Drives the REAL d2 detector over inline AL workspaces (publisher raising an
//! event inside a loop + an `[EventSubscriber]`; mirrors
//! `tests/r0-corpus/ws-d2` and `tests/gap_audit_b_table_triggers.rs`).

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-000000audd02";

const TABLE_SRC: &str = r#"
table 50170 "AuditD2 Customer"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Name; Text[100]) { }
    }
    keys { key(PK; "No.") { } }
}
"#;

/// Run ONLY d2 over an inline workspace and return its findings.
fn run_d2(files: &[(String, String)]) -> Vec<Finding> {
    let resolved = assemble_and_resolve_inline_program_default(files, APP_GUID);
    let detectors: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == "d2-event-fanout-in-loop")
        .collect();
    assert_eq!(detectors.len(), 1, "d2 must be registered exactly once");
    run_detectors(&resolved, &detectors).findings
}

fn al(name: &str, body: &str) -> (String, String) {
    (format!("src/{name}.al"), body.to_string())
}

/// A publisher codeunit that raises `OnAuditD2Event(var Cust)` inside a loop —
/// the d2 trigger shape (mirrors `tests/r0-corpus/ws-d2/src/publisher.al`).
const PUBLISHER_SRC: &str = r#"
codeunit 50171 "AuditD2 Publisher"
{
    procedure RaiseInLoop()
    var
        Cust: Record "AuditD2 Customer";
        i: Integer;
    begin
        for i := 1 to 10 do
            OnAuditD2Event(Cust);
    end;

    [IntegrationEvent(false, false)]
    procedure OnAuditD2Event(var Cust: Record "AuditD2 Customer")
    begin
    end;
}
"#;

// --- (1) Next-terminator: the subscriber's only db op is its own loop's
// `until …Next()` terminator → NO d2 ------------------------------------------

#[test]
fn subscriber_terminator_next_only_is_suppressed() {
    let subscriber = r#"
codeunit 50172 "AuditD2 Sub Next"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"AuditD2 Publisher", 'OnAuditD2Event', '', true, true)]
    local procedure Handle(var Cust: Record "AuditD2 Customer")
    var
        Total: Integer;
    begin
        repeat
            Total += 1;
        until Cust.Next() = 0;
    end;
}
"#;
    let findings = run_d2(&[
        al("AuditD2Customer", TABLE_SRC),
        al("AuditD2Publisher", PUBLISHER_SRC),
        al("AuditD2SubNext", subscriber),
    ]);
    assert!(
        findings.is_empty(),
        "a subscriber whose only db op is its own loop's `until …Next()` terminator \
         must not make d2 fire. findings: {findings:#?}"
    );
}

// --- (2) Virtual/system table: the subscriber only reads platform metadata
// tables (AllObjWithCaption / Field) in a loop → NO d2 -------------------------

#[test]
fn subscriber_virtual_system_table_reads_are_suppressed() {
    let subscriber = r#"
codeunit 50173 "AuditD2 Sub Virtual"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"AuditD2 Publisher", 'OnAuditD2Event', '', true, true)]
    local procedure Handle(var Cust: Record "AuditD2 Customer")
    var
        AllObjWithCaption: Record AllObjWithCaption;
        FieldRec: Record Field;
        i: Integer;
    begin
        for i := 1 to 5 do begin
            AllObjWithCaption.FindFirst();
            FieldRec.FindSet();
        end;
    end;
}
"#;
    let findings = run_d2(&[
        al("AuditD2Customer", TABLE_SRC),
        al("AuditD2Publisher", PUBLISHER_SRC),
        al("AuditD2SubVirtual", subscriber),
    ]);
    assert!(
        findings.is_empty(),
        "a subscriber reading only virtual/system tables (AllObjWithCaption, Field) \
         must not make d2 fire. findings: {findings:#?}"
    );
}

// --- (3) Temporary record: the subscriber's loop ops are all on a `temporary`
// record (Known(true)) → NO d2 -------------------------------------------------

#[test]
fn subscriber_temp_record_ops_are_suppressed() {
    let subscriber = r#"
codeunit 50174 "AuditD2 Sub Temp"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"AuditD2 Publisher", 'OnAuditD2Event', '', true, true)]
    local procedure Handle(var Cust: Record "AuditD2 Customer")
    var
        TempCust: Record "AuditD2 Customer" temporary;
        i: Integer;
    begin
        for i := 1 to 5 do begin
            TempCust.Init();
            TempCust.Insert();
        end;
    end;
}
"#;
    let findings = run_d2(&[
        al("AuditD2Customer", TABLE_SRC),
        al("AuditD2Publisher", PUBLISHER_SRC),
        al("AuditD2SubTemp", subscriber),
    ]);
    assert!(
        findings.is_empty(),
        "a subscriber whose db ops are all on a Known(true) temporary record must \
         not make d2 fire. findings: {findings:#?}"
    );
}

// --- CONTROL: a REAL db op (Modify on a physical record) in the subscriber's
// loop → d2 must STILL fire ----------------------------------------------------

#[test]
fn control_subscriber_real_db_op_still_fires() {
    let subscriber = r#"
codeunit 50175 "AuditD2 Sub Real"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"AuditD2 Publisher", 'OnAuditD2Event', '', true, true)]
    local procedure Handle(var Cust: Record "AuditD2 Customer")
    var
        Other: Record "AuditD2 Customer";
        i: Integer;
    begin
        for i := 1 to 5 do begin
            Other.Get('X');
            Other.Name := 'changed';
            Other.Modify();
        end;
    end;
}
"#;
    let findings = run_d2(&[
        al("AuditD2Customer", TABLE_SRC),
        al("AuditD2Publisher", PUBLISHER_SRC),
        al("AuditD2SubReal", subscriber),
    ]);
    assert_eq!(
        findings.len(),
        1,
        "a subscriber doing a REAL db op (Get + Modify on a physical record) must \
         still make d2 fire. findings: {findings:#?}"
    );
    let f = &findings[0];
    assert_eq!(f.detector, "d2-event-fanout-in-loop");
    assert!(
        f.root_cause.contains("OnAuditD2Event"),
        "control finding must name the event. root_cause: {}",
        f.root_cause
    );
}
