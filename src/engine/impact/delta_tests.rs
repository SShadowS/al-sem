//! `delta.rs`: the key-level delta of two hand-built summaries. Each test
//! states its precondition literally (the two sides' keys), so it holds
//! whatever the summaries' producer does.

use std::collections::BTreeMap;

use super::*;
use crate::engine::impact::keys::{Coverage, RootSummary, Site};

fn site(line: u32, ident: &str) -> Site {
    Site {
        file: "ws:src/A.al".into(),
        line,
        column: 8,
        ident: ident.into(),
    }
}

fn summary(own: &[(&str, &[Site])], cone: &[&str], coverage: Coverage) -> RootSummary {
    RootSummary {
        stable_id: "00000000-0000-0000-0000-0000000000aa:Codeunit:1#ab".into(),
        display: "Codeunit A::R".into(),
        own: own
            .iter()
            .map(|(k, s)| ((*k).to_string(), s.to_vec()))
            .collect::<BTreeMap<_, _>>(),
        cone: cone.iter().map(|k| (*k).to_string()).collect(),
        coverage,
        ..RootSummary::default()
    }
}

fn complete() -> Coverage {
    Coverage {
        status: "complete".into(),
        ..Coverage::default()
    }
}

const T1: &str = "write|00000000-0000-0000-0000-0000000000aa/table/1|Modify|physical";
const T2: &str = "write|00000000-0000-0000-0000-0000000000aa/table/2|Modify|physical";

/// B-3c: the same keys at other lines (a comment inserted above them) give
/// no change.
///
/// Break: compare sites by `file:line` → a removal and an addition.
#[test]
fn moved_sites_are_no_change() {
    let b = summary(
        &[("commit", &[site(10, "commit")])],
        &["COMMIT|"],
        complete(),
    );
    let n = summary(
        &[("commit", &[site(11, "commit")])],
        &["COMMIT|"],
        complete(),
    );
    let (own, cone) = root_delta(&b, &n);
    assert!(own.is_empty(), "{own:?}");
    assert!(cone.is_empty(), "{cone:?}");
}

/// B-3d: a count-preserving table swap shows both tables.
///
/// Break: compare counts per family → nothing.
#[test]
fn a_table_swap_shows_both() {
    let b = summary(&[(T1, &[site(5, "modify/a/0")])], &[], complete());
    let n = summary(&[(T2, &[site(5, "modify/a/0")])], &[], complete());
    let (own, _) = root_delta(&b, &n);
    let words: Vec<(&str, &str)> = own.iter().map(|c| (c.key.as_str(), c.wording)).collect();
    assert_eq!(words, vec![(T1, "removed"), (T2, "added")]);
}

/// B-3e: a key the new side lacks is "no longer observed" when the new
/// side's coverage is incomplete, "removed" when it is complete.
///
/// Break: word every loss "removed".
#[test]
fn a_loss_under_incomplete_coverage_is_no_longer_observed() {
    let b = summary(&[(T1, &[site(5, "modify/a/0")])], &[], complete());
    let n = summary(
        &[],
        &[],
        Coverage {
            status: "complete".into(),
            unresolved: 2,
            ..Coverage::default()
        },
    );
    let (own, _) = root_delta(&b, &n);
    assert_eq!(own.len(), 1);
    assert_eq!(own[0].wording, "no-longer-observed");
    assert!(
        own[0]
            .human()
            .contains("no longer observed (2 unresolved calls"),
        "{}",
        own[0].human()
    );
    let (own, _) = root_delta(&b, &summary(&[], &[], complete()));
    assert_eq!(own[0].wording, "removed");
}

/// A count change names the site, or says the identities cannot.
#[test]
fn a_count_change_pairs_sites_by_identity() {
    let b = summary(
        &[("commit", &[site(5, "commit"), site(9, "commit")])],
        &[],
        complete(),
    );
    let n = summary(&[("commit", &[site(6, "commit")])], &[], complete());
    let (own, _) = root_delta(&b, &n);
    assert_eq!(own[0].wording, "count");
    assert!(
        own[0].ambiguous,
        "two commits became one: which is not decidable"
    );
    assert_eq!(
        own[0].human(),
        "commit: 2 → 1 sites (which site: ambiguous)"
    );
    let b = summary(&[(T1, &[site(5, "modify/a/0")])], &[], complete());
    let n = summary(
        &[(T1, &[site(5, "modify/a/0"), site(8, "modifyall/b/0")])],
        &[],
        complete(),
    );
    let (own, _) = root_delta(&b, &n);
    assert_eq!(own[0].added_sites, vec!["ws:src/A.al:8".to_string()]);
    assert!(!own[0].ambiguous);
}

/// A cone change an own change explains is not printed twice.
#[test]
fn an_explained_cone_change_is_dropped() {
    let table = "00000000-0000-0000-0000-0000000000aa/table/1";
    let b = summary(
        &[(T1, &[site(5, "modify/a/0")])],
        &[&format!("DB_MODIFY|{table}")],
        complete(),
    );
    let n = summary(&[], &["DB_INSERT|x"], complete());
    let (own, cone) = root_delta(&b, &n);
    assert_eq!(own.len(), 1);
    assert_eq!(
        cone.iter().map(|c| c.key.as_str()).collect::<Vec<_>>(),
        vec!["DB_INSERT|x"]
    );
}

/// ENG-09: a new checked run's own commit (`commit|checked-run`) explains its
/// cone row (`COMMIT|checked-run`), so it is printed once; a `Commit()`'s
/// cone row (`COMMIT|`) is another change.
///
/// Break: drop `cone_twin`'s `["commit", r]` arm → the run's cone row is
/// printed beside its own key.
#[test]
fn a_new_checked_runs_commit_is_one_change() {
    let b = summary(&[], &[], complete());
    let n = summary(
        &[("commit|checked-run", &[site(7, "Codeunit.Run")])],
        &["COMMIT|checked-run", "COMMIT|"],
        complete(),
    );
    let (own, cone) = root_delta(&b, &n);
    assert_eq!(
        own.iter().map(|c| c.key.as_str()).collect::<Vec<_>>(),
        vec!["commit|checked-run"]
    );
    assert_eq!(
        cone.iter().map(|c| c.key.as_str()).collect::<Vec<_>>(),
        vec!["COMMIT|"]
    );
}

/// ENG-22: a new checked run's own error is one change: its own key explains
/// its cone row (`ERROR_IMPLICIT|checked-run`); another implicit error's
/// cone row (`ERROR_IMPLICIT|`) is another change.
///
/// Break: drop `cone_twin`'s `["error", "error-implicit", r]` arm → the
/// run's cone row is printed beside its own key.
#[test]
fn a_new_checked_runs_error_is_one_change() {
    let b = summary(&[], &[], complete());
    let n = summary(
        &[(
            "error|error-implicit|checked-run",
            &[site(7, "Codeunit.Run")],
        )],
        &["ERROR_IMPLICIT|checked-run", "ERROR_IMPLICIT|"],
        complete(),
    );
    let (own, cone) = root_delta(&b, &n);
    assert_eq!(
        own.iter().map(|c| c.key.as_str()).collect::<Vec<_>>(),
        vec!["error|error-implicit|checked-run"]
    );
    assert_eq!(
        cone.iter().map(|c| c.key.as_str()).collect::<Vec<_>>(),
        vec!["ERROR_IMPLICIT|"]
    );
}

/// ENG-10: a new try call's refusal is one change: its own key explains its
/// cone row (`ERROR_IMPLICIT|write-in-try`).
///
/// Break: drop `cone_twin`'s `["error", "error-implicit", r]` arm → the
/// try call's cone row is printed beside its own key.
#[test]
fn a_new_try_calls_refusal_is_one_change() {
    let b = summary(&[], &[], complete());
    let n = summary(
        &[("error|error-implicit|write-in-try", &[site(7, "TryInsert")])],
        &["ERROR_IMPLICIT|write-in-try", "ERROR_IMPLICIT|"],
        complete(),
    );
    let (own, cone) = root_delta(&b, &n);
    assert_eq!(
        own.iter().map(|c| c.key.as_str()).collect::<Vec<_>>(),
        vec!["error|error-implicit|write-in-try"]
    );
    assert_eq!(
        cone.iter().map(|c| c.key.as_str()).collect::<Vec<_>>(),
        vec!["ERROR_IMPLICIT|"]
    );
}

/// A call the analyzed body no longer makes is removed even when the new
/// side's cone is incomplete (the call is read off the body, not the cone);
/// an effect key under the same coverage is only "no longer observed".
///
/// Break: drop the early `removed` for `callee|`/`event|` in `loss` → the
/// call reads "no-longer-observed".
#[test]
fn a_dropped_call_is_removed_under_any_cone_coverage() {
    let incomplete = Coverage {
        status: "complete".into(),
        unresolved: 3,
        ..Coverage::default()
    };
    let b = summary(
        &[
            ("callee|g:Codeunit:1#cc", &[site(4, "x/0")]),
            ("commit", &[site(5, "commit")]),
        ],
        &[],
        complete(),
    );
    let n = summary(&[], &[], incomplete);
    let (own, _) = root_delta(&b, &n);
    let words: Vec<(&str, &str)> = own.iter().map(|c| (c.key.as_str(), c.wording)).collect();
    assert_eq!(
        words,
        vec![
            ("callee|g:Codeunit:1#cc", "removed"),
            ("commit", "no-longer-observed")
        ]
    );
}
