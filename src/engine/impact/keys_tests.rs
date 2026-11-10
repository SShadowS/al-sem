//! `keys.rs`: coverage wording.

use super::*;

/// A complete coverage is not incomplete; each gap makes it so, with words.
#[test]
fn coverage_incomplete_has_a_reason() {
    let c = Coverage {
        status: "complete".into(),
        ..Coverage::default()
    };
    assert!(!c.incomplete());
    assert!(c.why().is_empty());
    let c = Coverage {
        status: "complete".into(),
        unresolved: 2,
        ..Coverage::default()
    };
    assert!(c.incomplete());
    assert_eq!(c.why(), vec!["2 unresolved calls in its cone".to_string()]);
}

/// Engine limits B2 (per-arm ids): `Work` is declared in two `#if` arms (arm A
/// inserts `IK A`, arm B, the second row, inserts `IK B` after a call), and
/// `Target` is called from both arms of `Caller`. A summary holds every arm's
/// own sites, each at its own line, and each caller at its own call's line.
/// Breaks: `own_keys` reads the routine's first row only -> arm B's write is
/// missing; `callers` looks the call up in the caller id's last row -> arm
/// A's caller line is the routine's declaration.
#[test]
fn a_two_arm_routine_summarizes_every_arm() {
    const SRC: &str = r#"
table 50380 "IK A"
{
    fields { field(1; "No."; Integer) { } }
}

table 50381 "IK B"
{
    fields { field(1; "No."; Integer) { } }
}

codeunit 50380 "IK Work"
{
    local procedure Touch()
    begin
    end;

#if A
    procedure Work()
    var
        A: Record "IK A";
    begin
        A.Insert();
    end;
#else
    procedure Work()
    var
        B: Record "IK B";
    begin
        Touch();
        B.Insert();
    end;
#endif

    procedure Target()
    begin
    end;

#if A
    procedure Caller()
    begin
        Target();
    end;
#else
    procedure Caller()
    begin
        Touch();
        Target();
    end;
#endif
}
"#;
    let m = crate::program::model::program_calls::assemble_and_resolve_inline_program_default(
        &[("src/T.al".to_string(), SRC.to_string())],
        "22222222-0000-0000-0000-0000000b2a10",
    );
    let rows = |name: &str| -> Vec<&crate::program::model::workspace::ModelRoutine> {
        m.workspace
            .routines
            .iter()
            .filter(|r| r.name == name)
            .collect()
    };
    let (work, caller) = (rows("Work"), rows("Caller"));
    assert_eq!((work.len(), caller.len()), (2, 2), "precondition: two arms");
    let rcx = crate::engine::l5::routine_rollup::RollupCx::new(&m, &[]);
    let names = Names::new(&m, &rcx);
    let s = summarize(&names, &work[0].stable_routine_id, None, None).expect("a summary");
    // 1-based lines of each arm's insert.
    let lines: BTreeSet<u32> = s
        .own
        .iter()
        .filter(|(k, _)| k.starts_with("write|"))
        .flat_map(|(_, v)| v.iter().map(|site| site.line))
        .collect();
    let want: BTreeSet<u32> = work
        .iter()
        .map(|r| r.record_operations[0].source_anchor.start_line + 1)
        .collect();
    assert_eq!(
        lines, want,
        "each arm's insert at its own line: {:?}",
        s.own
    );
    let target = rows("Target")[0];
    let t = summarize(&names, &target.stable_routine_id, None, None).expect("a summary");
    let got: BTreeSet<u32> = t.callers.first.iter().map(|c| c.line).collect();
    let want: BTreeSet<u32> = caller
        .iter()
        .map(|r| {
            r.call_sites
                .iter()
                .find(|c| c.callee_text.contains("Target"))
                .expect("precondition: the arm's call")
                .source_anchor
                .start_line
                + 1
        })
        .collect();
    assert_eq!(got, want, "each caller at its own call's line");
}
