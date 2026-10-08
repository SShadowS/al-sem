//! D63 — HTML built by string concatenation (OPT-IN heuristic). BCQuality
//! `al-has-no-built-in-htmlencode`: AL has no HtmlEncode; splicing data into
//! HTML literals is an injection (XSS-shaped) risk wherever the string reaches
//! a browser/mail surface. Pure TEXT heuristic over call-site argument source
//! text — one finding per call site (first matching argument), OPT-IN because
//! the engine cannot see where the string ends up.
//!
//! Fires only when a `+`-concatenation splices a NON-LITERAL operand into an
//! HTML literal. A purely-static multi-line HTML template joined with `+`
//! (e.g. a `StrSubstNo` template whose dynamic values enter via `%n`
//! placeholders, not via `+`) is NOT flagged — every `+` operand is a developer
//! literal, so there is no injection vector. See `looks_like_html_concat`.
//!
//! Severity: low. Confidence: possible.

use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::anchor_of;
use crate::engine::l5::finding::{Evidence, EvidenceStep, Finding, FindingConfidence, FixOption};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::model::workspace::Model;

const DETECTOR: &str = "d63-html-concat-injection";

/// Does the literal contain an HTML-tag-ish `<x` / `</x` sequence?
fn html_tagish(lit: &str) -> bool {
    let b = lit.as_bytes();
    b.windows(2)
        .any(|w| w[0] == b'<' && (w[1].is_ascii_alphabetic() || w[1] == b'/'))
}

/// Argument-text heuristic: at least one single-quoted AL literal containing an
/// HTML-tag-ish sequence, at least one `+` OUTSIDE the literals (a real
/// concatenation), AND at least one NON-LITERAL operand outside the literals
/// (an identifier / call / paren — i.e. actual data being spliced in). AL
/// escapes `'` inside literals as `''`.
///
/// The `data_outside` requirement is what separates a genuine data-splice
/// (`'<b>' + UserName`) from a purely-static multi-line HTML template joined
/// with `+` (`'<div>' + '<p>%1</p>' + '</div>'`, the shape of a `StrSubstNo`
/// template whose dynamic values enter via `%n` placeholders, NOT via `+`). The
/// latter is not an injection vector — every concatenated operand is a developer
/// literal — and used to false-positive before this guard.
fn looks_like_html_concat(text: &str) -> bool {
    let mut in_lit = false;
    let mut lit = String::new();
    let mut html_lit = false;
    let mut concat_outside = false;
    let mut data_outside = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if in_lit {
            if c == '\'' {
                if chars.peek() == Some(&'\'') {
                    chars.next();
                    lit.push('\'');
                } else {
                    in_lit = false;
                    if html_tagish(&lit) {
                        html_lit = true;
                    }
                    lit.clear();
                }
            } else {
                lit.push(c);
            }
        } else if c == '\'' {
            in_lit = true;
        } else if c == '+' {
            concat_outside = true;
        } else if !c.is_whitespace() {
            // A non-literal, non-operator token outside the literals: an
            // identifier / call / paren carrying spliced-in data.
            data_outside = true;
        }
    }
    html_lit && concat_outside && data_outside
}

pub fn detect_d63(
    resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;

    for routine in &ws.routines {
        if !routine.body_available || routine.parse_incomplete {
            continue;
        }
        for cs in &routine.call_sites {
            let Some(arg) = cs.argument_texts.iter().find(|t| looks_like_html_concat(t)) else {
                continue;
            };
            candidates_considered += 1;

            let confidence: FindingConfidence = to_confidence(&[], "possible");
            let id = format!("d63/{}/{}", routine.id, cs.id);
            let mut finding = Finding {
                id: id.clone(),
                root_cause_key: id,
                detector: DETECTOR.to_string(),
                title: "HTML built by string concatenation".into(),
                root_cause: format!(
                    "{} concatenates data into an HTML literal ({}) — AL has no built-in \
                     HtmlEncode, so any user-influenced value is an injection risk where \
                     this string reaches a browser or mail body.",
                    routine.name,
                    arg.chars().take(60).collect::<String>()
                ),
                severity: "low".to_string(),
                confidence,
                primary_location: anchor_of(&cs.source_anchor, routine),
                evidence_path: vec![EvidenceStep {
                    routine_id: routine.id.clone(),
                    operation_id: None,
                    callsite_id: Some(cs.id.clone()),
                    loop_id: None,
                    source_anchor: anchor_of(&cs.source_anchor, routine),
                    note: "HTML literal + concatenation in argument".to_string(),
                }],
                additional_paths: None,
                affected_objects: vec![routine.object_id.as_str().into()],
                affected_tables: Vec::new(),
                fix_options: vec![FixOption {
                    description: "Encode interpolated values (replace <, >, &, \" before \
                                  splicing) or build the document with an XmlDocument/\
                                  template API instead of concatenation."
                        .into(),
                    safety: "medium".into(),
                }],
                provenance: vec![Evidence {
                    source: "tree-sitter",
                    note: None,
                }],
                actionable_anchor: None,
                fingerprint: None,
                event_kind: None,
                cross_extension_subscribers: None,
                cohort_contexts: None,
            };
            finding.fingerprint = Some(fp_index.fingerprint_of(&finding));
            findings.push(finding);
        }
    }

    findings.sort_by(|a, b| a.id.cmp(&b.id));
    let emitted = findings.len();
    let stats = DetectorStats::new(DETECTOR, candidates_considered, emitted);
    Ok(DetectorOutput::no_diag(findings, stats))
}

#[cfg(test)]
mod tests {
    use super::{html_tagish, looks_like_html_concat};

    #[test]
    fn html_literal_plus_concat_flags() {
        assert!(looks_like_html_concat("'<b>' + UserName + '</b>'"));
        assert!(looks_like_html_concat("'<div class=x>' + V"));
        assert!(looks_like_html_concat("Body + '</table>'"));
    }

    #[test]
    fn plain_literals_and_math_do_not_flag() {
        assert!(!looks_like_html_concat("'<b>static</b>'")); // no concat
        assert!(!looks_like_html_concat("'a' + 'b'")); // concat, no HTML tag... see note
        assert!(!looks_like_html_concat("X + Y")); // no literal
        assert!(!looks_like_html_concat("'2 < 3 and 4 > 1' + V")); // `< ` not tag-ish
    }

    #[test]
    fn static_html_template_join_does_not_flag() {
        // HTML tags present + `+` present, but every operand is a literal — a
        // multi-line template joined with `+`, NOT a data splice. The DO false
        // positive shape (a StrSubstNo template whose dynamic values enter via
        // %n placeholders inside the literals).
        assert!(!looks_like_html_concat("'<div>' + '</div>'"));
        assert!(!looks_like_html_concat(
            "'<div><p><br></p></div>' + '<b>%1</b> &lt;%2&gt;<br>' + '</div>'"
        ));
        // A non-literal operand tips it back to a real splice.
        assert!(looks_like_html_concat("'<div>' + Body + '</div>'"));
    }

    #[test]
    fn escaped_quotes_inside_literals_handled() {
        assert!(!looks_like_html_concat("'it''s fine' + V"));
        assert!(looks_like_html_concat("'it''s <b>' + V"));
    }

    #[test]
    fn tagish_needs_letter_or_slash_after_lt() {
        assert!(html_tagish("<b>"));
        assert!(html_tagish("</td>"));
        assert!(!html_tagish("2 < 3"));
        assert!(!html_tagish("no tags"));
    }
}
