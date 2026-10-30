//! Intent evidence step 6: `alsem analyze` runs d65-guard-bypass through the
//! real binary, by `--detector` and by `--preset business-rules`, in both
//! universes.
//!
//! This pins the USE of the declared needs: d65 requires `substrate::RULES`,
//! whose index is built from the routines' rules, so analyze's view must build
//! them (`ViewNeeds::analyze_for`). A view built with analyze's default needs
//! has no rules, and filling the index from it panics ("rules not built").
//! It also pins the inline suppression of a d65 finding: the fixture's
//! `ResetDiscount` is a finding of the detector (its r4 golden lists it) and
//! carries `// al-sem-ignore d65-guard-bypass: <reason>` above the write.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

fn ws() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus/ws-d65")
}

/// The routines `alsem analyze <ws> <args>` reports d65 findings in.
fn flagged(args: &[&str]) -> BTreeSet<String> {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("analyze")
        .arg(ws())
        .args(args)
        .args(["--format", "json"])
        .output()
        .expect("run alsem");
    assert!(
        out.status.success(),
        "alsem analyze {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    doc["payload"]["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .map(|f| {
            assert_eq!(f["detector"], "d65-guard-bypass", "{f}");
            f["primaryLocation"]["routineName"]
                .as_str()
                .expect("routine name")
                .to_string()
        })
        .collect()
}

#[test]
fn analyze_runs_d65_and_honours_its_inline_suppression() {
    let want: BTreeSet<String> = [
        "BypassAmount",
        "BypassBranch",
        "BypassModifyAll",
        "CollectNote",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    for args in [
        &["--detector", "d65-guard-bypass"][..],
        &["--preset", "business-rules"][..],
        &["--detector", "d65-guard-bypass", "--single-app"][..],
        &["--detector", "d65-guard-bypass", "--scope", "all"][..],
    ] {
        assert_eq!(flagged(args), want, "{args:?}");
    }
}
