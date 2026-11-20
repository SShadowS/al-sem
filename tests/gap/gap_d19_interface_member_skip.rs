//! ENG-151 — d19-unused-parameter must not flag a parameter of a method that
//! implements an interface member: the interface fixes the signature (alc
//! 18.0.41 refuses an implementer missing a member, AL0582), so an unused
//! parameter there is not the author's to drop. ENG-142 (interface inheritance)
//! made more of these visible on CDO `--scope all` (System Application's
//! "Default Email Connector v2", whose `Send`/`GetAccounts` implement members of
//! "Email Connector", which its interfaces extend).
//!
//! Each case drives REAL workspace assembly and the REGISTERED detector, and
//! checks the assembly produced the routine it depends on.
//!
//! Mutations (2026-11-20, each restored, green):
//! - the `implements_member` check removed from `detect_d19` -> every case's
//!   member is flagged (red);
//! - the base closure not followed (no `stack.extend`) -> the inherited member
//!   is flagged (`an_inherited_member_is_not_flagged`, red);
//! - an interface absent from the model answering `false` -> the
//!   `IMissing` implementer is flagged (`a_member_of_an_interface_not_in_the_model_is_not_flagged`, red);
//! - the `local` exception dropped from the unknown arm -> `Helper` is not
//!   flagged (same test, red);
//! - the routine's object found by a last-wins id lookup, not
//!   `DeclaringObjects::of` -> `if_arms_do_not_borrow_each_others_implements`
//!   red;
//! - no `unescape_al_identifier` in d19's fold -> `"I""Q"` reads as missing,
//!   `Other` is skipped (`an_escaped_quote_in_implements_finds_the_interface`, red).

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-0000eng151ab";
const DETECTOR: &str = "d19-unused-parameter";

/// d19's findings and its `interfaceMember` skip count over one workspace,
/// after checking the routines the case names were assembled with a body.
fn run_d19(src: &str, routines: &[&str]) -> (Vec<Finding>, u64) {
    let (f, s, _) = run_d19_keys(src, routines);
    (f, s)
}

/// [`run_d19`] plus the `interfaceUnknown` skip count.
fn run_d19_keys(src: &str, routines: &[&str]) -> (Vec<Finding>, u64, u64) {
    let files = vec![("src/Eng151.al".to_string(), src.to_string())];
    let resolved = assemble_and_resolve_inline_program_default(&files, APP_GUID);
    for name in routines {
        let r = resolved
            .workspace
            .routines
            .iter()
            .find(|r| r.name == *name && r.body_available)
            .unwrap_or_else(|| panic!("precondition: a routine {name:?} with a body"));
        assert!(
            !r.parameters.is_empty(),
            "precondition: {name:?} has parameters"
        );
    }
    let d19: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == DETECTOR)
        .collect();
    assert_eq!(d19.len(), 1);
    let out = run_detectors(&resolved, &d19);
    let skip = |key: &str| {
        out.detector_stats
            .iter()
            .find(|s| s.detector == DETECTOR)
            .and_then(|s| s.skipped.get(key).copied())
            .unwrap_or(0)
    };
    (
        out.findings.clone(),
        skip("interfaceMember"),
        skip("interfaceUnknown"),
    )
}

/// The routine names d19 flagged (each finding's root cause names it first).
fn flagged(findings: &[Finding]) -> Vec<String> {
    let mut v: Vec<String> = findings
        .iter()
        .map(|f| {
            f.root_cause
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_string()
        })
        .collect();
    v.sort();
    v
}

/// A direct member (`Do`) is skipped; a method no interface declares (`Other`)
/// is still flagged: the control.
#[test]
fn a_direct_member_is_not_flagged() {
    let src = "interface IFoo\n{\n    procedure Do(X: Integer);\n}\n\ncodeunit 50300 \"Impl\" implements IFoo\n{\n    procedure Do(X: Integer)\n    begin\n    end;\n\n    procedure Other(Y: Integer)\n    begin\n    end;\n}\n";
    let (findings, skipped) = run_d19(src, &["Do", "Other"]);
    assert_eq!(flagged(&findings), ["Other"], "{findings:#?}");
    assert_eq!(skipped, 1);
}

/// A member the implemented interface inherits (`IDer extends IBase`): `Do` is
/// declared on `IBase` only, `Extra` on `IDer`; neither is flagged.
#[test]
fn an_inherited_member_is_not_flagged() {
    let src = "interface IBase\n{\n    procedure Do(X: Integer);\n}\n\ninterface IDer extends IBase\n{\n    procedure Extra(Z: Integer);\n}\n\ncodeunit 50301 \"Impl\" implements IDer\n{\n    procedure Do(X: Integer)\n    begin\n    end;\n\n    procedure Extra(Z: Integer)\n    begin\n    end;\n}\n";
    let (findings, skipped) = run_d19(src, &["Do", "Extra"]);
    assert!(findings.is_empty(), "{findings:#?}");
    assert_eq!(skipped, 2);
}

/// An implemented interface the model does not hold (its members unknown):
/// its implementer's public methods are not flagged, and are counted apart
/// (`interfaceUnknown`); a `local` one cannot be a member and still is
/// (review, sol).
#[test]
fn a_member_of_an_interface_not_in_the_model_is_not_flagged() {
    let src = "codeunit 50302 \"Impl\" implements IMissing\n{\n    procedure Do(X: Integer)\n    begin\n    end;\n\n    local procedure Helper(Y: Integer)\n    begin\n    end;\n}\n";
    let (findings, member, unknown) = run_d19_keys(src, &["Do", "Helper"]);
    assert_eq!(flagged(&findings), ["Helper"], "{findings:#?}");
    assert_eq!((member, unknown), (0, 1));
}

/// An `implements` naming the interface with an escaped quote (`"I""Q"`, the
/// interface `I"Q`) finds it: `Do` is a member, `Other` is flagged, and
/// nothing reads as unknown (review, sol).
#[test]
fn an_escaped_quote_in_implements_finds_the_interface() {
    let src = "interface \"I\"\"Q\"\n{\n    procedure Do(X: Integer);\n}\n\ncodeunit 50304 \"Impl\" implements \"I\"\"Q\"\n{\n    procedure Do(X: Integer)\n    begin\n    end;\n\n    procedure Other(Y: Integer)\n    begin\n    end;\n}\n";
    let (findings, member, unknown) = run_d19_keys(src, &["Do", "Other"]);
    assert_eq!(flagged(&findings), ["Other"], "{findings:#?}");
    assert_eq!((member, unknown), (1, 0));
}

/// Whole-object `#if` arms of one codeunit, only one implementing `IFoo`: each
/// method is judged by its OWN arm's `implements`, in both arm orders (a
/// last-wins id map lent one arm's to the other; review, sol).
#[test]
fn if_arms_do_not_borrow_each_others_implements() {
    let iface = "interface IFoo\n{\n    procedure Do(X: Integer);\n}\n\n";
    let with = "codeunit 50305 \"Impl\" implements IFoo\n{\n    procedure Do(X: Integer)\n    begin\n    end;\n}\n";
    let without =
        "codeunit 50305 \"Impl\"\n{\n    procedure Do(X: Integer)\n    begin\n    end;\n}\n";
    for (first, second, flagged_arm) in [(with, without, "#else"), (without, with, "#if")] {
        let src = format!("{iface}#if ARM\n{first}#else\n{second}#endif\n");
        let (findings, member, _) = run_d19_keys(&src, &["Do"]);
        assert_eq!(
            flagged(&findings),
            ["Do"],
            "only the {flagged_arm} arm's Do: {findings:#?}"
        );
        assert_eq!(member, 1, "{flagged_arm}");
    }
}
