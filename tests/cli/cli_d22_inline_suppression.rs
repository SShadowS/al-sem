//! ENG-101 sol r1/r2: an inline `// al-sem-ignore <detector>` keeps suppressing per
//! SITE after a finding merges several sites. A merged finding is suppressed only
//! when a directive covers EVERY site it lists, each in its own file; a directive at
//! some sites keeps the finding. Run through the real binary, so the analyze
//! pipeline's use (`run.rs`'s sites and scanned files) is pinned, not the helper.
//!
//! - d22 merges the reads of one FlowField (same routine, variable and table),
//!   also across the `#if` declarations of one routine in different files.
//! - d54 keeps one finding per (TryFunction, publisher) across the `#if`
//!   declarations of one TryFunction (`detectors::dedupe_alternate_declarations`).
//!
//! Discrimination (2026-11-25/26), each break alone, restored green after:
//! - `run.rs` passing no sites (primary-line rule alone) -> "only OnlyFirstIgnored
//!   stays" fails (the directive above the first read hid the second).
//! - `apply_inline_suppressions` checking every site against the PRIMARY file ->
//!   "one file covered: the finding stays" fails (the directive in `A.al` also
//!   covered `B.al`'s read on the same line).
//! - `suppression_sites` returning `None` for a deduped finding -> "one arm
//!   covered: the finding stays" fails (the retained arm's directive hid both).

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

const D22: &str = "d22-flowfield-without-calcfields";
const D54: &str = "d54-publish-in-tryfunction-cone";

/// The routines (first word of `rootCause`) `alsem analyze --detector <detector>`
/// reports on a workspace of `fixture`'s `app.json` plus `files` under `src/`.
fn flagged(fixture: &str, files: &[(&str, String)], detector: &str) -> BTreeSet<String> {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus")
        .join(fixture);
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    std::fs::create_dir_all(ws.join("src")).expect("mkdir");
    std::fs::copy(corpus.join("app.json"), ws.join("app.json")).expect("app.json");
    for (name, text) in files {
        std::fs::write(ws.join("src").join(name), text).expect("write");
    }
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("analyze")
        .arg(ws)
        .args(["--detector", detector, "--format", "json"])
        .output()
        .expect("run alsem");
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("json ({e}): {}", String::from_utf8_lossy(&out.stderr)));
    doc["payload"]["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .filter_map(|f| f["rootCause"].as_str())
        .filter_map(|t| t.split(' ').next())
        .map(str::to_string)
        .collect()
}

fn tables() -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus/ws-d22/src/Tables.al"),
    )
    .expect("tables")
}

const CODEUNIT: &str = r#"codeunit 50890 "D22 Inline"
{
    procedure OnlyFirstIgnored()
    var
        Customer: Record Customer;
        Amount: Decimal;
    begin
        Customer.Get('C0001');
        // al-sem-ignore d22-flowfield-without-calcfields: the default value is wanted here
        Amount := Customer."Balance (LCY)";
        Customer.Get('C0002');
        Amount := Customer."Balance (LCY)";
    end;

    procedure EveryReadIgnored()
    var
        Customer: Record Customer;
        Amount: Decimal;
    begin
        Customer.Get('C0001');
        // al-sem-ignore d22-flowfield-without-calcfields: wanted
        Amount := Customer."Balance (LCY)";
        // al-sem-ignore d22-flowfield-without-calcfields: wanted
        Amount += Customer."Balance (LCY)";
    end;
}
"#;

/// Precondition: `OnlyFirstIgnored` reads `Customer."Balance (LCY)"` twice (a `Get`
/// between) with a directive above the first read only; `EveryReadIgnored` reads it
/// twice with a directive above each.
#[test]
fn a_merged_d22_finding_is_suppressed_only_when_every_read_is() {
    let found = flagged(
        "ws-d22",
        &[("Tables.al", tables()), ("Codeunit.al", CODEUNIT.into())],
        D22,
    );
    assert_eq!(
        found,
        BTreeSet::from(["OnlyFirstIgnored".to_string()]),
        "only OnlyFirstIgnored stays"
    );
}

/// One codeunit declared in two files under `#if CLEAN` / `#if not CLEAN` (one
/// routine id), `ReadArm` reading `R."Balance (LCY)"` on the same line (line 10) in
/// both. `ignore` says whether that file puts a directive above the read.
fn arm_file(cond: &str, ignore: bool) -> String {
    let directive = if ignore {
        "        // al-sem-ignore d22-flowfield-without-calcfields: wanted"
    } else {
        "        // no directive here"
    };
    format!(
        "#if {cond}\ncodeunit 50891 \"D22 Arms\"\n{{\n    procedure ReadArm()\n    var\n        R: Record Customer;\n        Amount: Decimal;\n    begin\n{directive}\n        Amount := R.\"Balance (LCY)\";\n    end;\n}}\n#endif\n"
    )
}

/// Precondition: the two declarations of `ReadArm` (files `A.al`, `B.al`) read the
/// same field of the same table on the same line, so d22 merges them into one
/// finding whose reads sit in two files.
#[test]
fn a_d22_finding_merged_across_files_needs_a_directive_in_each() {
    let one = flagged(
        "ws-d22",
        &[
            ("Tables.al", tables()),
            ("A.al", arm_file("CLEAN", true)),
            ("B.al", arm_file("not CLEAN", false)),
        ],
        D22,
    );
    assert_eq!(
        one,
        BTreeSet::from(["ReadArm".to_string()]),
        "one file covered: the finding stays"
    );
    let both = flagged(
        "ws-d22",
        &[
            ("Tables.al", tables()),
            ("A.al", arm_file("CLEAN", true)),
            ("B.al", arm_file("not CLEAN", true)),
        ],
        D22,
    );
    assert!(
        both.is_empty(),
        "both files covered: suppressed, got {both:?}"
    );
}

/// `TryPost` declared in both `#if` arms of one codeunit, each calling the
/// publisher `OnPosted`; `first`/`second` say whether that arm puts a directive
/// directly above its `procedure` line (the finding's anchor).
fn try_arms(first: bool, second: bool) -> String {
    let d = |on: bool| {
        if on {
            "    // al-sem-ignore d54-publish-in-tryfunction-cone: wanted"
        } else {
            "    // no directive here"
        }
    };
    format!(
        "codeunit 50892 \"D54 Arms\"\n{{\n    [IntegrationEvent(false, false)]\n    procedure OnPosted()\n    begin\n    end;\n\n#if CLEAN\n    [TryFunction]\n{}\n    procedure TryPost()\n    begin\n        OnPosted();\n    end;\n#else\n    [TryFunction]\n{}\n    procedure TryPost()\n    begin\n        OnPosted();\n    end;\n#endif\n}}\n",
        d(first),
        d(second)
    )
}

/// Precondition: one (TryPost, OnPosted) pair from two declarations, deduped into
/// one finding on the first. A directive on the retained (first) arm alone must not
/// hide the other arm, which was a finding of its own before the dedupe.
#[test]
fn a_deduped_d54_finding_needs_a_directive_at_every_declaration() {
    let one = flagged("ws-d54", &[("Codeunit.al", try_arms(true, false))], D54);
    assert_eq!(
        one,
        BTreeSet::from(["TryPost".to_string()]),
        "one arm covered: the finding stays"
    );
    let both = flagged("ws-d54", &[("Codeunit.al", try_arms(true, true))], D54);
    assert!(
        both.is_empty(),
        "both arms covered: suppressed, got {both:?}"
    );
}
