//! E1 — L3 enclosing-member / originating-object / wrapper-range capture.
//!
//! These tests exercise the additive `L3Routine` fields populated at L3 assembly
//! (`l3_workspace.rs`). They are Rust-only model assertions — `L3Routine` is NOT
//! `Serialize`-derived, so these fields never reach an R0–R3 golden; the parity
//! contract is guarded by the FULL differential suite, not here.
//!
//! Coverage (spec Revision-2 clauses):
//!   (a) RE-9 — routine set/order INVARIANT: the `(id, source_anchor.start_line)`
//!       sequence on a multi-trigger fixture matches a frozen expectation, proving the
//!       `collect_routine_nodes` `(parent, routine)` change did not perturb traversal.
//!   (b) RE-1/RE-2 — a two-field-`OnValidate` table → two routines with DISTINCT
//!       `stable_routine_id` (since task 4 folded the member into the hash),
//!       DISTINCT `enclosing_member` and DISTINCT wrapper ranges.
//!   (b2) #41 — the same, for two action `modify()` blocks in one pageextension.
//!   (c) RE-3 — a `report_dataitem` `OnAfterGetRecord` → member = the dataitem name.
//!   (d) RE-4 — an escaped-quote / mixed-case field name → the unescaped logical name.
//!   (e) a true object-level trigger (`OnRun`) → `enclosing_member` is `None`.

use al_sem::engine::l3::l3_workspace::{L3Routine, L3Workspace, assemble_workspace};

const APP_GUID: &str = "11111111-1111-1111-1111-111111111111";

fn assemble(files: &[(&str, &str)]) -> L3Workspace {
    let owned: Vec<(String, String)> = files
        .iter()
        .map(|(n, s)| ((*n).to_string(), (*s).to_string()))
        .collect();
    assemble_workspace(&owned, APP_GUID, "r0")
}

fn find<'a>(ws: &'a L3Workspace, name: &str) -> Vec<&'a L3Routine> {
    ws.routines.iter().filter(|r| r.name == name).collect()
}

// ---------------------------------------------------------------------------
// (a) Routine set/order invariant (RE-9).
// ---------------------------------------------------------------------------

const MULTI_TRIGGER_TABLE: &str = r#"
table 50100 "Multi Trigger"
{
    fields
    {
        field(1; "First Field"; Integer)
        {
            trigger OnValidate()
            begin
            end;
        }
        field(2; "Second Field"; Integer)
        {
            trigger OnValidate()
            begin
            end;
        }
    }

    trigger OnInsert()
    begin
    end;

    procedure DoStuff()
    begin
    end;
}
"#;

#[test]
fn routine_set_and_order_invariant() {
    let ws = assemble(&[("multi.al", MULTI_TRIGGER_TABLE)]);

    // Four routines: two field OnValidate triggers, the OnInsert object trigger,
    // and the DoStuff procedure — in document (traversal) order.
    let seq: Vec<(String, u32)> = ws
        .routines
        .iter()
        .map(|r| (r.name.clone(), r.source_anchor.start_line))
        .collect();

    // Frozen expectation. start_line is 0-based (tree-sitter rows). The leading newline
    // in the raw string makes `table` line 1; the triggers/procedure follow in source
    // order. If this sequence moves, the collect_routine_nodes change perturbed traversal.
    let expected = vec![
        ("OnValidate".to_string(), 7u32),
        ("OnValidate".to_string(), 13u32),
        ("OnInsert".to_string(), 19u32),
        ("DoStuff".to_string(), 23u32),
    ];
    assert_eq!(
        seq, expected,
        "routine (name, start_line) sequence must be unchanged by the (parent, routine) collect change"
    );
    assert_eq!(ws.routines.len(), 4, "exactly four routines expected");
}

// ---------------------------------------------------------------------------
// (b) Two-field OnValidate: DISTINCT stable_routine_id, distinct member + range
//     (RE-1/RE-2, and the Task-4 stable-id discriminator).
// ---------------------------------------------------------------------------

/// ⟨task 4⟩ **This assertion is inverted from what it was, deliberately.** It used
/// to read *"both field OnValidate triggers collapse to the SAME
/// stable_routine_id"* — a frozen recording of the defect, from the era when
/// `enclosing_member` was a work-AROUND for a collapse that the id itself could
/// not express. `to_stable_routine_id_from_parts` now folds the member into the
/// hash, so the collapse is gone and the two siblings' findings no longer share a
/// fingerprint. Rebaselined under the project's standing rule (correctness over
/// compatibility, all downstream consumers are ours) rather than preserved.
#[test]
fn two_field_on_validate_distinct_member_and_stable_id() {
    let ws = assemble(&[("multi.al", MULTI_TRIGGER_TABLE)]);

    let validates = find(&ws, "OnValidate");
    assert_eq!(validates.len(), 2, "two OnValidate triggers expected");

    // Distinct StableRoutineId — the collapse is closed at the id itself.
    assert_ne!(
        validates[0].stable_routine_id, validates[1].stable_routine_id,
        "two field OnValidate triggers must NOT share a stable_routine_id"
    );
    // …and the member is what separates them: re-minting both without the
    // discriminator (the pre-Task-4 form) collides, so this cannot pass for an
    // unrelated reason.
    let legacy: Vec<String> = validates
        .iter()
        .map(|r| {
            al_sem::engine::ids::to_stable_routine_id_from_parts(
                &al_sem::engine::ids::to_stable_object_id(&r.object_id),
                &r.normalized_signature_hash,
                None,
            )
        })
        .collect();
    assert_eq!(
        legacy[0], legacy[1],
        "precondition: without the member discriminator both collapse to one stable id"
    );

    // Distinct enclosing members (the unescaped logical field names).
    let members: Vec<&str> = validates
        .iter()
        .map(|r| r.enclosing_member.as_deref().expect("member present"))
        .collect();
    assert!(
        members.contains(&"First Field") && members.contains(&"Second Field"),
        "members must be the two field names, got {members:?}"
    );
    assert_ne!(members[0], members[1], "members must be distinct");

    // Distinct wrapper ranges (the position discriminator boundary).
    let r0 = validates[0]
        .enclosing_member_range
        .as_ref()
        .expect("wrapper range present");
    let r1 = validates[1]
        .enclosing_member_range
        .as_ref()
        .expect("wrapper range present");
    assert_ne!(
        (r0.start_line, r0.end_line),
        (r1.start_line, r1.end_line),
        "the two field wrappers occupy distinct source ranges"
    );

    // originating_object = the StableObjectId of the declaring table, identical for both.
    assert!(validates[0].originating_object.is_some());
    assert_eq!(
        validates[0].originating_object, validates[1].originating_object,
        "originating_object is the declaring object (same table)"
    );
}

// ---------------------------------------------------------------------------
// (b2) Two action `modify()` blocks in one pageextension (issue #41).
// ---------------------------------------------------------------------------

const TWO_ACTION_MODIFIES: &str = r#"
pageextension 50104 "Cust Card Ext" extends "Customer Card"
{
    actions
    {
        modify(FirstAction)
        {
            trigger OnAfterAction()
            begin
            end;
        }
        modify(SecondAction)
        {
            trigger OnAfterAction()
            begin
            end;
        }
    }
}
"#;

/// Issue #41 — the de-collision this fix exists for. Before it, an action
/// modification's trigger had NO enclosing member, so two sibling
/// `modify()` blocks each declaring `OnAfterAction()` minted one shared
/// `stable_routine_id` and one of them was lost to run-collapse.
///
/// The `syntax_kind` assert is deliberate, not decoration: it is the
/// executable join to `root_classification`'s hand-stated
/// `action_modification_trigger_is_page_action`, which matches on exactly
/// this string. Anchoring the origin at the trigger instead of the wrapper
/// would keep the ids distinct and silently break classification.
#[test]
fn two_action_modifies_distinct_member_and_stable_id() {
    let ws = assemble(&[("pext.al", TWO_ACTION_MODIFIES)]);

    let actions = find(&ws, "OnAfterAction");
    assert_eq!(actions.len(), 2, "two OnAfterAction triggers expected");

    assert_ne!(
        actions[0].stable_routine_id, actions[1].stable_routine_id,
        "two action modify() triggers must NOT share a stable_routine_id"
    );
    // …and the member is what separates them: re-minting both without the
    // discriminator collides, so this cannot pass for an unrelated reason.
    let legacy: Vec<String> = actions
        .iter()
        .map(|r| {
            al_sem::engine::ids::to_stable_routine_id_from_parts(
                &al_sem::engine::ids::to_stable_object_id(&r.object_id),
                &r.normalized_signature_hash,
                None,
            )
        })
        .collect();
    assert_eq!(
        legacy[0], legacy[1],
        "precondition: without the member discriminator both collapse to one stable id"
    );

    let members: Vec<&str> = actions
        .iter()
        .map(|r| r.enclosing_member.as_deref().expect("member present"))
        .collect();
    assert!(
        members.contains(&"FirstAction") && members.contains(&"SecondAction"),
        "members must be the two action names, got {members:?}"
    );

    for r in &actions {
        assert_eq!(
            r.enclosing_member_range
                .as_ref()
                .expect("wrapper range present")
                .syntax_kind,
            "modify_action_modification",
            "the anchor must be the modify wrapper — the string is_page_action_wrapper matches"
        );
    }
}

// ---------------------------------------------------------------------------
// (c) report_dataitem OnAfterGetRecord → member = dataitem name (RE-3).
// ---------------------------------------------------------------------------

const REPORT_DATAITEM: &str = r#"
report 50101 "Cust Report"
{
    dataset
    {
        dataitem(Customer; Customer)
        {
            trigger OnAfterGetRecord()
            begin
            end;
        }
    }
}
"#;

#[test]
fn report_dataitem_member_is_dataitem_name() {
    let ws = assemble(&[("rep.al", REPORT_DATAITEM)]);
    let r = find(&ws, "OnAfterGetRecord");
    assert_eq!(r.len(), 1, "one OnAfterGetRecord trigger expected");
    assert_eq!(
        r[0].enclosing_member.as_deref(),
        Some("Customer"),
        "report dataitem member = the dataitem name"
    );
    assert!(r[0].enclosing_member_range.is_some());
    assert!(r[0].originating_object.is_some());
}

// ---------------------------------------------------------------------------
// (d) Escaped-quote / mixed-case field name → unescaped logical name (RE-4).
// ---------------------------------------------------------------------------

const ESCAPED_QUOTE_FIELD: &str = r#"
table 50102 "Quote Table"
{
    fields
    {
        field(1; "Sell-to ""Custom"" No."; Code[20])
        {
            trigger OnValidate()
            begin
            end;
        }
    }
}
"#;

#[test]
fn escaped_quote_member_is_unescaped_logical_name() {
    let ws = assemble(&[("q.al", ESCAPED_QUOTE_FIELD)]);
    let r = find(&ws, "OnValidate");
    assert_eq!(r.len(), 1);
    // strip_quotes trims the boundary quotes; unescape_al_identifier collapses the
    // inner "" → ". The logical name matches the profiler display form.
    assert_eq!(
        r[0].enclosing_member.as_deref(),
        Some(r#"Sell-to "Custom" No."#),
        "member must be the unescaped logical identifier"
    );
}

// ---------------------------------------------------------------------------
// (e) Object-level trigger (OnRun) → member is None.
// ---------------------------------------------------------------------------

const OBJECT_LEVEL_TRIGGER: &str = r#"
codeunit 50103 "Runner"
{
    trigger OnRun()
    begin
    end;

    procedure Helper()
    begin
    end;
}
"#;

#[test]
fn object_level_trigger_has_no_member() {
    let ws = assemble(&[("cu.al", OBJECT_LEVEL_TRIGGER)]);

    let onrun = find(&ws, "OnRun");
    assert_eq!(onrun.len(), 1);
    assert_eq!(
        onrun[0].enclosing_member, None,
        "object-level OnRun has no enclosing member"
    );
    assert_eq!(onrun[0].enclosing_member_range, None);
    assert_eq!(onrun[0].originating_object, None);

    // A plain procedure likewise has no member.
    let helper = find(&ws, "Helper");
    assert_eq!(helper.len(), 1);
    assert_eq!(helper[0].enclosing_member, None);
}
