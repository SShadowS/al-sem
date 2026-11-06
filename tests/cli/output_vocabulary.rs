//! Output validation V2 (spec 2026-11-06-output-validation-r1 §8.3): the
//! vocabulary invariant. No renderer-owned human line of any tool prints a
//! word the §8.1 word table replaces (`al_sem::engine::vocab::REPLACED`).
//! Checked on live fixture answers of every tool that has a fixture here
//! (explain, prove's existing-effects and callers' context questions, a
//! filtered prove, digest, query effects, touches by table, by field and
//! scoped), so a renderer that puts an old word back fails this test even
//! when its goldens are regenerated. Quoted source (`--show-source`
//! excerpt lines `  >  12 | …`) is not renderer-owned and is skipped.
//!
//! Discrimination (2026-11-06): see the commit message.

use crate::output_fixture::{ISSUE_DOC, ISSUE_DOC_Q, WRITE_LOG, run_alsem, workspace};

/// Whether `line` is a quoted source line (a `--show-source` excerpt).
fn quoted_source(line: &str) -> bool {
    let t = line.trim_start().trim_start_matches('>').trim_start();
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    digits > 0 && t[digits..].starts_with(" | ")
}

/// Every replaced word in `text`'s renderer-owned lines, as
/// `label: word in: line`.
pub(crate) fn vocabulary_failures(label: &str, text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines().filter(|l| !quoted_source(l)) {
        for w in al_sem::engine::vocab::REPLACED {
            if line.contains(w) {
                out.push(format!("{label}: `{w}` in: {}", line.trim()));
            }
        }
    }
    out
}

#[test]
fn no_renderer_prints_a_replaced_word() {
    let ws = workspace().to_string_lossy().into_owned();
    let mut outputs = crate::output_invariants::fixture_human_outputs();
    let extra: Vec<Vec<&str>> = vec![
        vec![
            "explain",
            &ws,
            "--routine",
            ISSUE_DOC,
            "--detail",
            "--show-source",
        ],
        vec!["explain", &ws, "--routine", ISSUE_DOC, "--callers-context"],
        vec!["prove", &ws, ISSUE_DOC, "callers-may-run-noninteractive"],
        vec![
            "prove",
            &ws,
            ISSUE_DOC,
            "callers-intercept-errors",
            "--detail",
        ],
        vec!["prove", &ws, ISSUE_DOC, "callers-have-pending-writes"],
        vec![
            "prove",
            &ws,
            ISSUE_DOC,
            "throws-error",
            "--op",
            "FieldError",
        ],
        vec!["prove", &ws, WRITE_LOG, "reaches-ui"],
        vec![
            "digest",
            &ws,
            "--routine",
            ISSUE_DOC,
            "--format",
            "human",
            "--detail",
        ],
        vec!["query", "effects", &ws, "--routine", ISSUE_DOC_Q, "--all"],
        vec![
            "query",
            "touches",
            &ws,
            "--table",
            "Out Log",
            "--from",
            ISSUE_DOC_Q,
            "--direction",
            "up",
            "--callers",
        ],
        vec![
            "query", "touches", &ws, "--table", "Out Log", "--field", "Message",
        ],
        vec!["query", "touches", &ws, "--table", "unknown"],
    ];
    for mut a in extra {
        let label = a.join(" ").replace(&ws, "{ws}");
        if !a.contains(&"--format") {
            a.extend(["--format", "human"]);
        }
        let (out, err, code) = run_alsem(&a);
        assert_eq!(code, 0, "{label}: {err}");
        outputs.push((label, out));
    }
    assert!(
        outputs.len() >= 20,
        "precondition: {} outputs",
        outputs.len()
    );
    let failures: Vec<String> = outputs
        .iter()
        .flat_map(|(l, t)| vocabulary_failures(l, t))
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
