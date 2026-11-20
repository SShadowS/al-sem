//! `alsem analyze --focus` (ENG-168, spec
//! `docs/superpowers/specs/2026-11-20-eng168-analyze-focus-design.md` §3.3-§3.4).
//!
//! A view, never a filter that hides silently: the findings of the concerns not
//! named are removed AFTER `--min-severity`, the allow-list and `--scope`, and
//! BEFORE `--limit`, the baseline and inline suppression, and every format states
//! what was removed ([`FocusStatement`]). The detectors and their run are
//! unchanged, so a shown finding is byte-equal with and without `--focus`.

use std::collections::HashMap;

use crate::engine::l5::detectors::registered_detectors;
use crate::engine::l5::registry::Concern;

/// When the hidden counts are taken, as JSON states it.
pub const BEFORE: &str = "limit, baseline, suppressions";

/// What the no-model path says instead of a count.
pub const UNAVAILABLE: &str = "focus requested, analysis unavailable";

/// Parse `--focus <concern>[,<concern>...]`. The result is in [`Concern::ALL`]
/// order without repeats. An unknown or empty value is a config error naming the
/// known values.
pub fn parse_focus(s: &str) -> Result<Vec<Concern>, String> {
    let mut named = Vec::new();
    for v in s.split(',') {
        match Concern::parse(v) {
            Some(c) => named.push(c),
            None => {
                return Err(format!(
                    "invalid --focus '{}'. Expected one or more of: performance, behaviour (or behavior), maintainability",
                    v.trim()
                ));
            }
        }
    }
    Ok(Concern::ALL
        .into_iter()
        .filter(|c| named.contains(c))
        .collect())
}

/// The combinations `--focus` refuses (spec §3.3), checked before any analysis or
/// write. Called from the CLI's `plan_analyze` AND from `AnalyzeJob::new`, so no
/// entry can write a focused baseline or gate on a focused view.
pub fn check_focus_conflicts(
    focus: Option<&[Concern]>,
    update_baseline: bool,
    fail_on: Option<&str>,
) -> Result<(), String> {
    if focus.is_none() {
        return Ok(());
    }
    if update_baseline {
        return Err(
            "--update-baseline cannot be combined with --focus: the baseline would \
                    lose every hidden finding"
                .to_string(),
        );
    }
    if fail_on.is_some() {
        return Err(
            "--fail-on cannot be combined with --focus: --focus is an investigation \
                    view, not a gate"
                .to_string(),
        );
    }
    Ok(())
}

/// One hidden detector's count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HiddenCount {
    pub detector: String,
    pub concern: Concern,
    pub count: usize,
}

/// What `--focus` did, handed to every renderer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FocusStatement {
    /// The findings removed: the total and per detector (registry order, zero
    /// counts omitted).
    Hidden {
        focus: Vec<Concern>,
        total: usize,
        by_detector: Vec<HiddenCount>,
    },
    /// No model was built: nothing was counted, so no count is stated.
    Unavailable { focus: Vec<Concern> },
}

/// Remove from `items` every finding whose detector's concern is not in `focus`,
/// and count them. A detector the registry does not know is KEPT (fail open:
/// never hide what cannot be classified).
pub fn apply_focus<T>(
    items: &mut Vec<T>,
    detector_of: impl Fn(&T) -> &str,
    focus: &[Concern],
) -> FocusStatement {
    let registry: HashMap<String, (usize, Concern)> = registered_detectors()
        .into_iter()
        .enumerate()
        .map(|(i, d)| (d.name, (i, d.concern)))
        .collect();
    let mut hidden: HashMap<&str, (usize, Concern, usize)> = HashMap::new();
    items.retain(|item| match registry.get_key_value(detector_of(item)) {
        Some((name, &(order, concern))) if !focus.contains(&concern) => {
            hidden.entry(name.as_str()).or_insert((order, concern, 0)).2 += 1;
            false
        }
        _ => true,
    });
    let mut rows: Vec<_> = hidden.into_iter().collect();
    rows.sort_by_key(|(_, (order, ..))| *order);
    let by_detector: Vec<HiddenCount> = rows
        .into_iter()
        .map(|(name, (_, concern, count))| HiddenCount {
            detector: name.to_string(),
            concern,
            count,
        })
        .collect();
    FocusStatement::Hidden {
        focus: focus.to_vec(),
        total: by_detector.iter().map(|h| h.count).sum(),
        by_detector,
    }
}

fn focus_list(focus: &[Concern]) -> String {
    focus
        .iter()
        .map(|c| c.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

/// `1743` -> `1,743`.
fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

impl FocusStatement {
    /// The JSON object (`payload.summary.hidden`, SARIF `runs[0].properties.hidden`).
    pub fn to_json_value(&self) -> serde_json::Value {
        match self {
            FocusStatement::Hidden {
                focus,
                total,
                by_detector,
            } => serde_json::json!({
                "focus": focus.iter().map(|c| c.as_str()).collect::<Vec<_>>(),
                "before": BEFORE,
                "total": total,
                "byDetector": by_detector.iter().map(|h| serde_json::json!({
                    "detector": h.detector,
                    "concern": h.concern.as_str(),
                    "count": h.count,
                })).collect::<Vec<_>>(),
            }),
            FocusStatement::Unavailable { focus } => serde_json::json!({
                "focus": focus.iter().map(|c| c.as_str()).collect::<Vec<_>>(),
                "status": UNAVAILABLE,
            }),
        }
    }

    /// The one line terminal, `--group-by`, pr-summary and html print.
    pub fn line(&self) -> String {
        match self {
            FocusStatement::Hidden {
                focus,
                total,
                by_detector,
            } => {
                let head = format!(
                    "hidden by --focus {} (before limit, baseline and suppressions): {} finding{}",
                    focus_list(focus),
                    thousands(*total),
                    if *total == 1 { "" } else { "s" }
                );
                let groups: Vec<String> = Concern::ALL
                    .into_iter()
                    .filter_map(|c| {
                        let parts: Vec<String> = by_detector
                            .iter()
                            .filter(|h| h.concern == c)
                            .map(|h| {
                                let short = h.detector.split('-').next().unwrap_or(&h.detector);
                                format!("{short} {}", thousands(h.count))
                            })
                            .collect();
                        (!parts.is_empty()).then(|| format!("{}: {}", c.as_str(), parts.join(", ")))
                    })
                    .collect();
                if groups.is_empty() {
                    head
                } else {
                    format!("{head} ({})", groups.join("; "))
                }
            }
            FocusStatement::Unavailable { focus } => {
                format!("--focus {}: {UNAVAILABLE}", focus_list(focus))
            }
        }
    }

    /// The stderr warning a focused SARIF run prints: consumers treat a missing
    /// alert as fixed.
    pub fn sarif_warning(&self) -> String {
        let what = match self {
            FocusStatement::Hidden { total, .. } => format!(
                "{} finding{} hidden",
                thousands(*total),
                if *total == 1 { "" } else { "s" }
            ),
            FocusStatement::Unavailable { .. } => UNAVAILABLE.to_string(),
        };
        format!("--focus: a partial report ({what}); do not upload it as the full scan")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::gate::projection::{FindingLocation, FindingSummary};

    fn summary(detector: &str, fingerprint: &str) -> FindingSummary {
        FindingSummary {
            id: format!("id-{fingerprint}"),
            fingerprint: fingerprint.to_string(),
            loose_fingerprint: format!("loose-{fingerprint}"),
            detector: detector.to_string(),
            title: "t".to_string(),
            root_cause: "r".to_string(),
            severity: "high".to_string(),
            confidence_level: "high".to_string(),
            confidence_capped_by: None,
            primary_location: FindingLocation {
                file: "ws:a.al".to_string(),
                line: 1,
                column: 1,
                object_id: None,
                object_name: None,
                routine_id: None,
                routine_name: None,
            },
            terminal_location: None,
            affected_objects: vec![],
            affected_tables: vec![],
            fix_hint: None,
            path_count: 1,
        }
    }

    /// Hand-stated d1 (performance) x2, d34 (behaviour), d14 (maintainability).
    fn rows() -> Vec<FindingSummary> {
        vec![
            summary("d1-db-op-in-loop", "fp-d1-a"),
            summary("d1-db-op-in-loop", "fp-d1-b"),
            summary("d14-dead-routine", "fp-d14"),
            summary("d34-commit-in-loop", "fp-d34"),
        ]
    }

    /// `--focus behaviour` keeps d34 byte-equal and hides d1 and d14, each counted
    /// once per finding, in registry order.
    ///
    /// Mutation (2026-11-20): `!focus.contains(&concern)` -> `focus.contains(&concern)`
    /// -> kept was [d1, d1, d14], not [d34]; the performance, maintainability and
    /// fail-open tests failed too (red); restored, green.
    #[test]
    fn behaviour_keeps_behaviour_and_counts_the_rest() {
        let unfiltered = rows();
        let mut kept = rows();
        let st = apply_focus(&mut kept, |s| &s.detector, &[Concern::Behaviour]);
        assert_eq!(kept, vec![unfiltered[3].clone()]);
        assert_eq!(
            st,
            FocusStatement::Hidden {
                focus: vec![Concern::Behaviour],
                total: 3,
                by_detector: vec![
                    HiddenCount {
                        detector: "d1-db-op-in-loop".into(),
                        concern: Concern::Performance,
                        count: 2,
                    },
                    HiddenCount {
                        detector: "d14-dead-routine".into(),
                        concern: Concern::Maintainability,
                        count: 1,
                    },
                ],
            }
        );
    }

    /// `--focus performance` the other way round.
    #[test]
    fn performance_keeps_performance() {
        let mut kept = rows();
        let st = apply_focus(&mut kept, |s| &s.detector, &[Concern::Performance]);
        assert_eq!(
            kept.iter()
                .map(|s| s.fingerprint.as_str())
                .collect::<Vec<_>>(),
            ["fp-d1-a", "fp-d1-b"]
        );
        let FocusStatement::Hidden { total, .. } = st else {
            panic!("hidden")
        };
        assert_eq!(total, 2);
    }

    /// Maintainability is its own concern: hidden by `behaviour`, kept by
    /// `behaviour,maintainability`.
    ///
    /// Mutation (2026-11-20): d14's registry entry `concern: Concern::Maintainability`
    /// -> `Concern::Behaviour` -> `--focus behaviour` kept d14 here and in
    /// `behaviour_keeps_behaviour_and_counts_the_rest`, and the registry table test
    /// said "d14: registry says Behaviour, table Maintainability" (red); restored, green.
    #[test]
    fn maintainability_is_hidden_by_behaviour_and_kept_when_named() {
        let mut kept = rows();
        apply_focus(&mut kept, |s| &s.detector, &[Concern::Behaviour]);
        assert!(kept.iter().all(|s| s.detector != "d14-dead-routine"));

        let mut kept = rows();
        let st = apply_focus(
            &mut kept,
            |s| &s.detector,
            &parse_focus("behaviour,maintainability").unwrap(),
        );
        assert_eq!(
            kept.iter()
                .map(|s| s.fingerprint.as_str())
                .collect::<Vec<_>>(),
            ["fp-d14", "fp-d34"]
        );
        assert_eq!(
            st.line(),
            "hidden by --focus behaviour,maintainability (before limit, baseline and \
             suppressions): 2 findings (performance: d1 2)"
        );
    }

    /// A detector the registry does not know is kept, never hidden or counted.
    ///
    /// Mutation (2026-11-20): the `_ => true` arm split into `Some(_) => true,
    /// None => false` -> `kept.len()` 0, not 1: the unknown detector's finding was
    /// dropped (red); restored, green.
    #[test]
    fn unknown_detector_fails_open() {
        let mut kept = vec![
            summary("d999-not-registered", "fp-x"),
            summary("d1-db-op-in-loop", "fp-d1"),
        ];
        let st = apply_focus(&mut kept, |s| &s.detector, &[Concern::Behaviour]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].detector, "d999-not-registered");
        let FocusStatement::Hidden { total, .. } = st else {
            panic!("hidden")
        };
        assert_eq!(total, 1);
    }

    /// Every registered detector's concern is the spec §3.2 table: a
    /// reclassification, or a detector added or removed, is a reviewed diff here.
    ///
    /// Mutation (2026-11-20): d42's registry entry `Concern::Performance` ->
    /// `Concern::Behaviour` -> "d42: registry says Behaviour, table Performance" (red);
    /// restored, green.
    #[test]
    fn registry_concerns_are_the_spec_table() {
        use Concern::{Behaviour as B, Maintainability as M, Performance as P};
        let table: &[(&str, Concern)] = &[
            ("d1", P),
            ("d2", P),
            ("d3", P),
            ("d4", P),
            ("d5", P),
            ("d18", P),
            ("d36", P),
            ("d42", P),
            ("d48", P),
            ("d55", P),
            ("d56", P),
            ("d57", P),
            ("d60", P),
            ("d7", B),
            ("d8", B),
            ("d9", B),
            ("d10", B),
            ("d11", B),
            ("d21", B),
            ("d22", B),
            ("d29", B),
            ("d33", B),
            ("d34", B),
            ("d35", B),
            ("d37", B),
            ("d39", B),
            ("d40", B),
            ("d41", B),
            ("d43", B),
            ("d44", B),
            ("d45", B),
            ("d46", B),
            ("d47", B),
            ("d49", B),
            ("d50", B),
            ("d51", B),
            ("d52", B),
            ("d53", B),
            ("d54", B),
            ("d58", B),
            ("d59", B),
            ("d61", B),
            ("d62", B),
            ("d63", B),
            ("d64", B),
            ("d65", B),
            ("d12", M),
            ("d13", M),
            ("d14", M),
            ("d16", M),
            ("d17", M),
            ("d19", M),
            ("d20", M),
            ("d32", M),
            ("d38", M),
        ];
        assert_eq!(table.len(), 55);
        let registry = registered_detectors();
        assert_eq!(registry.len(), table.len(), "detector count");
        for d in &registry {
            let short = d.name.split('-').next().unwrap();
            let want = table
                .iter()
                .find(|(n, _)| *n == short)
                .unwrap_or_else(|| panic!("{short}: not in the table"))
                .1;
            assert_eq!(
                d.concern, want,
                "{short}: registry says {:?}, table {want:?}",
                d.concern
            );
        }
    }

    #[test]
    fn parse_accepts_behavior_and_refuses_unknown() {
        assert_eq!(parse_focus("behavior"), Ok(vec![Concern::Behaviour]));
        assert_eq!(
            parse_focus("maintainability, performance,performance"),
            Ok(vec![Concern::Performance, Concern::Maintainability])
        );
        let e = parse_focus("speed").unwrap_err();
        assert!(
            e.contains("'speed'") && e.contains("performance, behaviour"),
            "{e}"
        );
        assert!(parse_focus("").is_err());
    }

    #[test]
    fn line_and_json_shapes() {
        let st = FocusStatement::Hidden {
            focus: vec![Concern::Behaviour],
            total: 1743,
            by_detector: vec![
                HiddenCount {
                    detector: "d1-db-op-in-loop".into(),
                    concern: Concern::Performance,
                    count: 1612,
                },
                HiddenCount {
                    detector: "d14-dead-routine".into(),
                    concern: Concern::Maintainability,
                    count: 131,
                },
            ],
        };
        assert_eq!(
            st.line(),
            "hidden by --focus behaviour (before limit, baseline and suppressions): 1,743 \
             findings (performance: d1 1,612; maintainability: d14 131)"
        );
        assert_eq!(
            st.to_json_value(),
            serde_json::json!({"focus": ["behaviour"], "before": BEFORE, "total": 1743,
                "byDetector": [
                    {"detector": "d1-db-op-in-loop", "concern": "performance", "count": 1612},
                    {"detector": "d14-dead-routine", "concern": "maintainability", "count": 131}]})
        );
        let none = FocusStatement::Unavailable {
            focus: vec![Concern::Behaviour],
        };
        assert_eq!(
            none.line(),
            "--focus behaviour: focus requested, analysis unavailable"
        );
        assert!(none.sarif_warning().contains(UNAVAILABLE));
    }
}
