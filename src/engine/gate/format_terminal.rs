//! `format_terminal` — port of al-sem `src/cli/format-terminal.ts` +
//! `src/projection/rollup-findings.ts` + `src/projection/finding-groups.ts`.
//!
//! Three public entry points:
//!   - `rollup_findings`   — mirror of `rollupFindings`.
//!   - `format_terminal`   — mirror of `formatTerminal`.
//!   - `group_findings`    — mirror of `groupFindings`.
//!
//! ## Rollup semantics
//! Multiple detectors firing at the same `(file, line, column, sortedAffectedTables)`
//! are rolled up into a single `RolledOrSingle::Rolled` entry. Singleton groups pass
//! through as `RolledOrSingle::Single`. The final group sort is NON-TOTAL (ties on
//! severity/file/line/column fall back to insertion order), so the groups map uses
//! `IndexMap` (insertion-ordered).
//!
//! ## compareStrings
//! All string comparisons that mirror al-sem's `compareStrings` use Rust `str::cmp`
//! (byte-order). For the BMP-only identifier strings in the corpus this is identical
//! to JS's UTF-16 code-unit order, so the goldens byte-match.
//!
//! TRACKED FOLLOW-UP (engine-wide UTF-16 `compareStrings`): al-sem's `compareStrings`
//! is true UTF-16 code-unit order. For non-BMP scalars (astral plane, e.g. emoji)
//! Rust `str::cmp` (UTF-8 byte order) DIVERGES from JS. When the engine-wide swap to a
//! shared UTF-16 comparator lands, these sites must use `a.encode_utf16().cmp(b.encode_utf16())`
//! (true UTF-16 code-unit order) — NOT byte order, which would still diverge for non-BMP.
//! No corpus string exercises this today; behavior is intentionally left as `str::cmp`.

use indexmap::IndexMap;

use crate::engine::gate::projection::{FindingLocation, FindingSummary};
use crate::engine::l5::registry::Diagnostic;
use crate::program::model::coverage::AnalysisCoverage;

// ---------------------------------------------------------------------------
// SEV_RANK — mirrors `rollup-findings.ts`
// ---------------------------------------------------------------------------

fn sev_rank(sev: &str) -> i32 {
    match sev {
        "critical" => 5,
        "high" => 4,
        "medium" => 3,
        "low" => 2,
        "info" => 1,
        _ => 0,
    }
}

// SAFETY_RANK for fix options in renderRolled
fn safety_rank(safety: &str) -> i32 {
    match safety {
        "high" => 3,
        "medium" => 2,
        "low" => 1,
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// RolledOrSingle — mirrors the TS union type
// ---------------------------------------------------------------------------

/// A single finding (no rollup) or a multi-contributor rolled group.
#[derive(Debug, Clone)]
// Variant size spread is inherent (Single vs the richer Rolled); boxing would churn callers.
#[allow(clippy::large_enum_variant)]
pub enum RolledOrSingle {
    Single(FindingSummary),
    Rolled {
        primary_location: FindingLocation,
        affected_tables: Vec<String>,
        /// Max severity across contributors.
        severity: String,
        /// Sorted: highest-severity first, then by detector id ascending.
        contributors: Vec<FindingSummary>,
    },
}

// ---------------------------------------------------------------------------
// rollup_findings — mirrors `rollupFindings` in rollup-findings.ts
// ---------------------------------------------------------------------------

/// Build the grouping key: `file|line|column|sortedAffectedTables.join(",")`.
fn rollup_key(f: &FindingSummary) -> String {
    let mut tables = f.affected_tables.clone();
    tables.sort();
    let tables_str = tables.join(",");
    format!(
        "{}|{}|{}|{}",
        f.primary_location.file, f.primary_location.line, f.primary_location.column, tables_str
    )
}

/// Group findings by rollup key, sort contributors, produce `RolledOrSingle`.
/// Final sort: severity desc, file (compareStrings), line, column — STABLE.
/// Uses `IndexMap` so that ties fall back to insertion order (non-total sort).
pub fn rollup_findings(findings: &[FindingSummary]) -> Vec<RolledOrSingle> {
    // insertion-ordered map: key → list of findings
    let mut groups: IndexMap<String, Vec<FindingSummary>> = IndexMap::new();
    for f in findings {
        let k = rollup_key(f);
        groups.entry(k).or_default().push(f.clone());
    }

    let mut out: Vec<RolledOrSingle> = Vec::new();
    for (_k, list) in groups {
        if list.is_empty() {
            continue;
        }
        if list.len() == 1 {
            out.push(RolledOrSingle::Single(list.into_iter().next().unwrap()));
            continue;
        }
        // Multi-contributor: sort by sev desc then detector id asc (STABLE).
        let mut sorted = list;
        sorted.sort_by(|a, b| {
            let r = sev_rank(&b.severity).cmp(&sev_rank(&a.severity));
            if r != std::cmp::Ordering::Equal {
                return r;
            }
            a.detector.cmp(&b.detector)
        });
        let canonical = sorted[0].clone();
        out.push(RolledOrSingle::Rolled {
            primary_location: canonical.primary_location,
            affected_tables: canonical.affected_tables,
            severity: canonical.severity,
            contributors: sorted,
        });
    }

    // Final group sort: sev desc, file (compareStrings), line, col — STABLE.
    out.sort_by(|a, b| {
        let sev_a = match a {
            RolledOrSingle::Single(f) => sev_rank(&f.severity),
            RolledOrSingle::Rolled { severity, .. } => sev_rank(severity),
        };
        let sev_b = match b {
            RolledOrSingle::Single(f) => sev_rank(&f.severity),
            RolledOrSingle::Rolled { severity, .. } => sev_rank(severity),
        };
        let r = sev_b.cmp(&sev_a);
        if r != std::cmp::Ordering::Equal {
            return r;
        }
        let loc_a = match a {
            RolledOrSingle::Single(f) => &f.primary_location,
            RolledOrSingle::Rolled {
                primary_location, ..
            } => primary_location,
        };
        let loc_b = match b {
            RolledOrSingle::Single(f) => &f.primary_location,
            RolledOrSingle::Rolled {
                primary_location, ..
            } => primary_location,
        };
        let r = loc_a.file.cmp(&loc_b.file);
        if r != std::cmp::Ordering::Equal {
            return r;
        }
        let r = loc_a.line.cmp(&loc_b.line);
        if r != std::cmp::Ordering::Equal {
            return r;
        }
        loc_a.column.cmp(&loc_b.column)
    });

    out
}

// ---------------------------------------------------------------------------
// Rendering helpers — mirrors format-terminal.ts
// ---------------------------------------------------------------------------

fn loc_str(loc: &FindingLocation) -> String {
    let where_part = match (&loc.object_name, &loc.routine_name) {
        (Some(obj), Some(rtn)) => format!(" in {obj} :: {rtn}"),
        _ => String::new(),
    };
    format!("{}:{}:{}{}", loc.file, loc.line, loc.column, where_part)
}

fn render_single(f: &FindingSummary, lines: &mut Vec<String>) {
    lines.push(format!(
        "  [{}] {} \u{2014} {}",
        f.detector, f.title, f.root_cause
    ));
    lines.push(format!("    {}", loc_str(&f.primary_location)));
    if let Some(ref tl) = f.terminal_location {
        lines.push(format!("    terminal: {}", loc_str(tl)));
    }
    // TS uses a bare truthy check (`f.confidence.cappedBy ? ...`). The `!is_empty()`
    // guard is equivalent because `cappedBy` is NEVER `[]` — producers emit `undefined`
    // (None) or a non-empty array (confidence.ts:64 / path-merge.ts:106). An empty array
    // would diverge from TS's ` (capped by )`; the guard keeps the safer behavior and is
    // consistent with `render_rolled`. NOT sorted here (TS `renderSingle` joins as-is —
    // only `renderRolled` sorts its deduped union).
    let cap_str = match &f.confidence_capped_by {
        Some(cb) if !cb.is_empty() => format!(" (capped by {})", cb.join(", ")),
        _ => String::new(),
    };
    lines.push(format!("    confidence: {}{}", f.confidence_level, cap_str));
    let pc = f.path_count;
    if pc > 1 {
        let noun = if pc - 1 == 1 {
            "other path"
        } else {
            "other paths"
        };
        lines.push(format!(
            "    also reached from {} {} (full traces in SARIF output / explain_path MCP tool)",
            pc - 1,
            noun
        ));
    }
    if let Some((ref desc, ref safety)) = f.fix_hint {
        lines.push(format!("    fix ({}): {}", safety, desc));
    }
}

fn render_rolled(
    primary_location: &FindingLocation,
    contributors: &[FindingSummary],
    lines: &mut Vec<String>,
) {
    let n = contributors.len();
    lines.push(format!(
        "  {} \u{2014} {} detectors agree:",
        loc_str(primary_location),
        n
    ));
    for c in contributors {
        lines.push(format!("    [{}] {} ({})", c.detector, c.title, c.severity));
    }
    // Worst-case confidence (lowest): confirmed > likely > possible
    let conf_order = ["confirmed", "likely", "possible"];
    let worst_conf = conf_order
        .iter()
        .find(|&&lvl| contributors.iter().any(|c| c.confidence_level == lvl))
        .copied()
        .unwrap_or("possible");
    // Union of cappedBys — sorted
    let mut capped_bys: Vec<String> = Vec::new();
    for c in contributors {
        if let Some(ref cb) = c.confidence_capped_by {
            for s in cb {
                if !capped_bys.contains(s) {
                    capped_bys.push(s.clone());
                }
            }
        }
    }
    capped_bys.sort();
    let cap_str = if !capped_bys.is_empty() {
        format!(" (capped by {})", capped_bys.join(", "))
    } else {
        String::new()
    };
    lines.push(format!("    confidence: {}{}", worst_conf, cap_str));

    // Aggregate fixes sorted by safety (high→low) — STABLE sort.
    let mut fixes: Vec<(&str, &str, &str)> = contributors
        .iter()
        .filter_map(|c| {
            c.fix_hint
                .as_ref()
                .map(|(desc, safety)| (c.detector.as_str(), desc.as_str(), safety.as_str()))
        })
        .collect();
    fixes.sort_by_key(|x| std::cmp::Reverse(safety_rank(x.2)));

    if !fixes.is_empty() {
        lines.push("    fix options (safest first):".to_string());
        for (detector, desc, safety) in &fixes {
            lines.push(format!(
                "      \u{2022} ({}) {}  [{}]",
                safety, desc, detector
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// format_terminal — mirrors `formatTerminal` in format-terminal.ts
// ---------------------------------------------------------------------------

const SEV_ORDER: &[&str] = &["critical", "high", "medium", "low", "info"];

/// Render the full terminal output for an analyze result.
/// Returns the string WITHOUT the trailing `\n` (the caller appends it,
/// matching al-sem's `process.stdout.write(`${formatTerminal(...)}\n`)`).
pub fn format_terminal(
    findings: &[FindingSummary],
    coverage: &AnalysisCoverage,
    diagnostics: &[Diagnostic],
) -> String {
    let rolled = rollup_findings(findings);
    let mut lines: Vec<String> = Vec::new();

    let rollup_count = rolled
        .iter()
        .filter(|r| matches!(r, RolledOrSingle::Rolled { .. }))
        .count();
    let rollup_note = if rollup_count > 0 {
        format!(
            "; {} location(s) flagged by multiple detectors (rolled up below).",
            rollup_count
        )
    } else {
        ".".to_string()
    };
    lines.push(format!(
        "Analysed {} routines ({} with bodies, {} parse-incomplete); {}/{} source units parsed; {} opaque app(s){}",
        coverage.routines_total,
        coverage.routines_body_available,
        coverage.routines_parse_incomplete.len(),
        coverage.source_units_parsed,
        coverage.source_units_total,
        coverage.opaque_apps.len(),
        rollup_note
    ));

    if findings.is_empty() {
        lines.push(String::new());
        lines.push(
            "No findings. (Absence of a finding is not absence of a problem — see coverage above.)"
                .to_string(),
        );
    }

    for &sev in SEV_ORDER {
        let group: Vec<&RolledOrSingle> = rolled
            .iter()
            .filter(|r| match r {
                RolledOrSingle::Single(f) => f.severity == sev,
                RolledOrSingle::Rolled { severity, .. } => severity == sev,
            })
            .collect();
        if group.is_empty() {
            continue;
        }
        lines.push(String::new());
        lines.push(format!("{} ({}):", sev.to_uppercase(), group.len()));
        for item in group {
            match item {
                RolledOrSingle::Single(f) => render_single(f, &mut lines),
                RolledOrSingle::Rolled {
                    primary_location,
                    contributors,
                    ..
                } => render_rolled(primary_location, contributors, &mut lines),
            }
        }
    }

    if !diagnostics.is_empty() {
        lines.push(String::new());
        lines.push(format!("Diagnostics ({}):", diagnostics.len()));
        for d in diagnostics {
            lines.push(format!("  [{}/{}] {}", d.severity, d.stage, d.message));
        }
    }

    lines.join("\n")
}

// ---------------------------------------------------------------------------
// group_findings + format_terminal_grouped — mirrors finding-groups.ts + index.ts
// ---------------------------------------------------------------------------

/// GroupBy variant — mirrors `GroupBy` in finding-groups.ts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBy {
    Object,
    Routine,
    Table,
    Detector,
    File,
}

impl GroupBy {
    /// Parse from the string value stored in `AnalyzeArgs::group_by`.
    /// Named `parse` (not `from_str`) to avoid the `std::str::FromStr` trait
    /// convention collision — `FromStr::from_str` returns `Result`, this returns
    /// `Option`, and the inherent name trips clippy `should_implement_trait`.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "object" => Some(GroupBy::Object),
            "routine" => Some(GroupBy::Routine),
            "table" => Some(GroupBy::Table),
            "detector" => Some(GroupBy::Detector),
            "file" => Some(GroupBy::File),
            _ => None,
        }
    }
}

fn key_for(f: &FindingSummary, by: GroupBy) -> String {
    match by {
        GroupBy::Object => f
            .primary_location
            .object_id
            .clone()
            .unwrap_or_else(|| "(unknown)".to_string()),
        GroupBy::Routine => f
            .primary_location
            .routine_id
            .clone()
            .unwrap_or_else(|| "(unknown)".to_string()),
        GroupBy::Table => f
            .affected_tables
            .first()
            .cloned()
            .unwrap_or_else(|| "(none)".to_string()),
        GroupBy::Detector => f.detector.clone(),
        GroupBy::File => f.primary_location.file.clone(),
    }
}

/// Group findings by `by`, sort largest-first then alphabetical key (STABLE).
/// Returns `(key, count)` pairs in sorted order.
pub fn group_findings(findings: &[FindingSummary], by: GroupBy) -> Vec<(String, usize)> {
    let mut map: IndexMap<String, usize> = IndexMap::new();
    for f in findings {
        let k = key_for(f, by);
        *map.entry(k).or_insert(0) += 1;
    }
    let mut groups: Vec<(String, usize)> = map.into_iter().collect();
    // largest first, then alphabetical for determinism — STABLE sort
    // Note: the TS uses raw `<` string compare (not compareStrings) for the key tiebreak.
    // Both are equivalent for BMP strings; use `str::cmp` here.
    groups.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    groups
}

/// Render the grouped terminal output (mirror of index.ts L331-348).
/// Returns the string WITHOUT trailing `\n`.
pub fn format_terminal_grouped(
    findings: &[FindingSummary],
    coverage: &AnalysisCoverage,
    by: GroupBy,
) -> String {
    let groups = group_findings(findings, by);
    let mut lines: Vec<String> = Vec::new();

    // Coverage header (NO rollup note in group-by path — always ends with `.`)
    lines.push(format!(
        "Analysed {} routines ({} with bodies, {} parse-incomplete); {}/{} source units parsed; {} opaque app(s).",
        coverage.routines_total,
        coverage.routines_body_available,
        coverage.routines_parse_incomplete.len(),
        coverage.source_units_parsed,
        coverage.source_units_total,
        coverage.opaque_apps.len(),
    ));
    lines.push(String::new());

    let by_str = match by {
        GroupBy::Object => "object",
        GroupBy::Routine => "routine",
        GroupBy::Table => "table",
        GroupBy::Detector => "detector",
        GroupBy::File => "file",
    };
    lines.push(format!("Grouped by {} (top {}):", by_str, groups.len()));
    for (key, count) in &groups {
        lines.push(format!("  {}: {}", key, count));
    }

    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::gate::projection::{FindingLocation, FindingSummary};

    fn make_finding(
        id: &str,
        detector: &str,
        severity: &str,
        file: &str,
        line: u32,
        col: u32,
        tables: Vec<String>,
    ) -> FindingSummary {
        FindingSummary {
            id: id.to_string(),
            fingerprint: id.to_string(),
            detector: detector.to_string(),
            title: format!("Title {id}"),
            root_cause: format!("RootCause {id}"),
            severity: severity.to_string(),
            confidence_level: "likely".to_string(),
            confidence_capped_by: None,
            primary_location: FindingLocation {
                file: file.to_string(),
                line,
                column: col,
                object_id: None,
                object_name: None,
                routine_id: None,
                routine_name: None,
            },
            terminal_location: None,
            affected_objects: vec![],
            affected_tables: tables,
            fix_hint: None,
            path_count: 1,
        }
    }

    #[test]
    fn rollup_multi_contributor_group() {
        // Three findings at the same (file, line, col, tables) — should roll up.
        let findings = vec![
            make_finding(
                "f1",
                "d1-detector",
                "high",
                "ws:src/Test.al",
                10,
                5,
                vec!["Customer".to_string()],
            ),
            make_finding(
                "f2",
                "d10-detector",
                "high",
                "ws:src/Test.al",
                10,
                5,
                vec!["Customer".to_string()],
            ),
            make_finding(
                "f3",
                "d5-detector",
                "info",
                "ws:src/Test.al",
                10,
                5,
                vec!["Customer".to_string()],
            ),
        ];
        let rolled = rollup_findings(&findings);
        assert_eq!(rolled.len(), 1, "should produce exactly 1 rolled group");
        match &rolled[0] {
            RolledOrSingle::Rolled {
                severity,
                contributors,
                ..
            } => {
                assert_eq!(severity, "high", "worst severity should be high");
                assert_eq!(contributors.len(), 3, "all 3 contributors");
                // Contributors: sorted by sev desc then detector asc
                // high: d1, d10; info: d5 → order: d1, d10, d5
                assert_eq!(contributors[0].detector, "d1-detector");
                assert_eq!(contributors[1].detector, "d10-detector");
                assert_eq!(contributors[2].detector, "d5-detector");
            }
            RolledOrSingle::Single(_) => panic!("expected rolled, got single"),
        }
    }

    #[test]
    fn rollup_different_tables_stay_separate() {
        // Two findings at same position but different tables → must NOT roll up.
        let findings = vec![
            make_finding(
                "f1",
                "d1",
                "high",
                "ws:src/Test.al",
                10,
                5,
                vec!["TableA".to_string()],
            ),
            make_finding(
                "f2",
                "d1",
                "high",
                "ws:src/Test.al",
                10,
                5,
                vec!["TableB".to_string()],
            ),
        ];
        let rolled = rollup_findings(&findings);
        assert_eq!(rolled.len(), 2, "different tables → separate groups");
    }

    #[test]
    fn rollup_final_sort_sev_desc() {
        // Mix of high and medium → high comes first.
        let findings = vec![
            make_finding("f1", "d1", "medium", "ws:src/A.al", 1, 1, vec![]),
            make_finding("f2", "d2", "high", "ws:src/B.al", 2, 1, vec![]),
        ];
        let rolled = rollup_findings(&findings);
        assert_eq!(rolled.len(), 2);
        let sev0 = match &rolled[0] {
            RolledOrSingle::Single(f) => f.severity.as_str(),
            RolledOrSingle::Rolled { severity, .. } => severity.as_str(),
        };
        assert_eq!(sev0, "high");
    }

    #[test]
    fn group_findings_count_sort() {
        let findings = vec![
            make_finding("f1", "d1", "high", "ws:file1.al", 1, 1, vec![]),
            make_finding("f2", "d2", "high", "ws:file2.al", 2, 1, vec![]),
            make_finding("f3", "d1", "high", "ws:file3.al", 3, 1, vec![]),
        ];
        let groups = group_findings(&findings, GroupBy::Detector);
        // d1 has 2, d2 has 1 → d1 first
        assert_eq!(groups[0].0, "d1");
        assert_eq!(groups[0].1, 2);
        assert_eq!(groups[1].0, "d2");
        assert_eq!(groups[1].1, 1);
    }

    // -----------------------------------------------------------------------
    // Render sub-branch oracles — these branches are NOT exercised by the 27
    // differential goldens (corpus-invisible). They read correct on inspection;
    // these tests lock them against regression (same discipline as A1/A2 oracles).
    // -----------------------------------------------------------------------

    /// Build a `FindingLocation` with object + routine names (so `loc_str`
    /// emits the ` in Obj :: Rtn` suffix).
    fn loc(file: &str, line: u32, col: u32) -> FindingLocation {
        FindingLocation {
            file: file.to_string(),
            line,
            column: col,
            object_id: Some("11111111/Codeunit/50000".to_string()),
            object_name: Some("Obj".to_string()),
            routine_id: Some("rid".to_string()),
            routine_name: Some("Rtn".to_string()),
        }
    }

    /// Oracle 1: render_single emits the `terminal:` line when terminal_location is Some.
    #[test]
    fn oracle_render_single_terminal_line() {
        let mut f = make_finding("f1", "d1", "high", "ws:src/A.al", 10, 5, vec![]);
        f.primary_location = loc("ws:src/A.al", 10, 5);
        f.terminal_location = Some(loc("ws:src/B.al", 20, 7));
        let mut lines = Vec::new();
        render_single(&f, &mut lines);
        assert!(
            lines
                .iter()
                .any(|l| l == "    terminal: ws:src/B.al:20:7 in Obj :: Rtn"),
            "expected terminal: line, got:\n{lines:#?}"
        );
    }

    /// Oracle 2: path_count == 2 → SINGULAR "1 other path" (not "paths").
    #[test]
    fn oracle_render_single_singular_other_path() {
        let mut f = make_finding("f1", "d1", "high", "ws:src/A.al", 10, 5, vec![]);
        f.path_count = 2;
        let mut lines = Vec::new();
        render_single(&f, &mut lines);
        let line = lines
            .iter()
            .find(|l| l.contains("also reached from"))
            .expect("expected 'also reached from' line");
        assert!(
            line.contains("1 other path ") && !line.contains("other paths"),
            "expected SINGULAR '1 other path', got: {line:?}"
        );
    }

    /// Oracle 3: render_single capped-by suffix (comma-joined, join-as-is order).
    #[test]
    fn oracle_render_single_capped_by() {
        let mut f = make_finding("f1", "d1", "high", "ws:src/A.al", 10, 5, vec![]);
        f.confidence_level = "possible".to_string();
        // TS renderSingle does NOT sort — joins in array order. Use a non-sorted
        // order to prove we mirror the join-as-is behavior.
        f.confidence_capped_by = Some(vec!["zeta".to_string(), "alpha".to_string()]);
        let mut lines = Vec::new();
        render_single(&f, &mut lines);
        assert!(
            lines
                .iter()
                .any(|l| l == "    confidence: possible (capped by zeta, alpha)"),
            "expected join-as-is capped-by suffix, got:\n{lines:#?}"
        );
    }

    /// Build a rolled group from contributors directly (canonical = first).
    fn rolled_from(contributors: Vec<FindingSummary>) -> (FindingLocation, Vec<FindingSummary>) {
        let primary = contributors[0].primary_location.clone();
        (primary, contributors)
    }

    /// Oracle 4: render_rolled worst-confidence selection.
    #[test]
    fn oracle_render_rolled_worst_confidence() {
        // Case A: includes "confirmed" + "possible" → worst (lowest in order found
        // first: confirmed) — al-sem `order.find` returns the FIRST level present in
        // [confirmed, likely, possible], i.e. the BEST present, labeled "worst-case".
        let mut a1 = make_finding("a1", "d1", "high", "ws:src/A.al", 1, 1, vec![]);
        a1.confidence_level = "confirmed".to_string();
        let mut a2 = make_finding("a2", "d2", "high", "ws:src/A.al", 1, 1, vec![]);
        a2.confidence_level = "possible".to_string();
        let (loc_a, contrib_a) = rolled_from(vec![a1, a2]);
        let mut lines = Vec::new();
        render_rolled(&loc_a, &contrib_a, &mut lines);
        assert!(
            lines.iter().any(|l| l == "    confidence: confirmed"),
            "case A: expected 'confidence: confirmed', got:\n{lines:#?}"
        );

        // Case B: only "possible" present → "possible".
        let mut b1 = make_finding("b1", "d1", "high", "ws:src/A.al", 1, 1, vec![]);
        b1.confidence_level = "possible".to_string();
        let mut b2 = make_finding("b2", "d2", "high", "ws:src/A.al", 1, 1, vec![]);
        b2.confidence_level = "possible".to_string();
        let (loc_b, contrib_b) = rolled_from(vec![b1, b2]);
        let mut lines_b = Vec::new();
        render_rolled(&loc_b, &contrib_b, &mut lines_b);
        assert!(
            lines_b.iter().any(|l| l == "    confidence: possible"),
            "case B: expected 'confidence: possible', got:\n{lines_b:#?}"
        );

        // Case C: all-unknown confidence levels → unwrap_or("possible") fallback.
        let mut c1 = make_finding("c1", "d1", "high", "ws:src/A.al", 1, 1, vec![]);
        c1.confidence_level = "weird".to_string();
        let mut c2 = make_finding("c2", "d2", "high", "ws:src/A.al", 1, 1, vec![]);
        c2.confidence_level = "alsoweird".to_string();
        let (loc_c, contrib_c) = rolled_from(vec![c1, c2]);
        let mut lines_c = Vec::new();
        render_rolled(&loc_c, &contrib_c, &mut lines_c);
        assert!(
            lines_c.iter().any(|l| l == "    confidence: possible"),
            "case C: expected fallback 'confidence: possible', got:\n{lines_c:#?}"
        );
    }

    /// Oracle 5: render_rolled SAFETY_RANK reorder (high → medium → low).
    #[test]
    fn oracle_render_rolled_safety_reorder() {
        // Contributors added in low/high/medium order; fix options must come out
        // ordered high → medium → low.
        let mut c_low = make_finding("clow", "d-low", "high", "ws:src/A.al", 1, 1, vec![]);
        c_low.fix_hint = Some(("LOW FIX".to_string(), "low".to_string()));
        let mut c_high = make_finding("chigh", "d-high", "high", "ws:src/A.al", 1, 1, vec![]);
        c_high.fix_hint = Some(("HIGH FIX".to_string(), "high".to_string()));
        let mut c_med = make_finding("cmed", "d-med", "high", "ws:src/A.al", 1, 1, vec![]);
        c_med.fix_hint = Some(("MED FIX".to_string(), "medium".to_string()));
        let (loc_r, contrib) = rolled_from(vec![c_low, c_high, c_med]);
        let mut lines = Vec::new();
        render_rolled(&loc_r, &contrib, &mut lines);
        // Find the fix-option lines (after the "fix options (safest first):" header).
        let header_idx = lines
            .iter()
            .position(|l| l == "    fix options (safest first):")
            .expect("expected fix options header");
        let fix_lines = &lines[header_idx + 1..];
        assert_eq!(
            fix_lines[0], "      \u{2022} (high) HIGH FIX  [d-high]",
            "first fix must be high-safety"
        );
        assert_eq!(
            fix_lines[1], "      \u{2022} (medium) MED FIX  [d-med]",
            "second fix must be medium-safety"
        );
        assert_eq!(
            fix_lines[2], "      \u{2022} (low) LOW FIX  [d-low]",
            "third fix must be low-safety"
        );
    }

    /// Oracle 6: render_rolled cappedBy union — deduped + sorted across contributors.
    #[test]
    fn oracle_render_rolled_capped_by_union() {
        let mut c1 = make_finding("c1", "d1", "high", "ws:src/A.al", 1, 1, vec![]);
        c1.confidence_level = "likely".to_string();
        c1.confidence_capped_by = Some(vec!["zeta".to_string(), "beta".to_string()]);
        let mut c2 = make_finding("c2", "d2", "high", "ws:src/A.al", 1, 1, vec![]);
        c2.confidence_level = "likely".to_string();
        // overlapping "beta" (dedup) + new "alpha".
        c2.confidence_capped_by = Some(vec!["beta".to_string(), "alpha".to_string()]);
        let (loc_r, contrib) = rolled_from(vec![c1, c2]);
        let mut lines = Vec::new();
        render_rolled(&loc_r, &contrib, &mut lines);
        // Union deduped {zeta, beta, alpha} → sorted → alpha, beta, zeta.
        assert!(
            lines
                .iter()
                .any(|l| l == "    confidence: likely (capped by alpha, beta, zeta)"),
            "expected deduped+sorted union, got:\n{lines:#?}"
        );
    }
}
