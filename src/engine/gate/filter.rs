//! `filter_findings` + the scope + limit filters — port of al-sem
//! `src/projection/finding-filters.ts` and the scope/limit steps of the `analyze`
//! command (`src/cli/index.ts`).
//!
//! Order, matching the CLI:
//!   1. `filter_findings` (min-severity, then detector allow-list).
//!   2. scope filter (`primary` drops dependency-anchored findings; `all` keeps them).
//!   3. limit (first N, after scope).

use std::collections::HashSet;

use crate::engine::gate::projection::FindingSummary;

/// Severity rank (finding-filters.ts `SEV_RANK`).
fn sev_rank(sev: &str) -> u8 {
    match sev {
        "critical" => 5,
        "high" => 4,
        "medium" => 3,
        "low" => 2,
        "info" => 1,
        _ => 0,
    }
}

/// `FilterOptions` — only the gate-relevant subset (min-severity + detectors).
#[derive(Debug, Clone, Default)]
pub struct FilterOptions {
    pub min_severity: Option<String>,
    pub detectors: Option<Vec<String>>,
}

/// `filterFindings(findings, opts)` — drop below `min_severity`, then keep only the
/// allow-listed detectors (when a non-empty `detectors` list is given).
pub fn filter_findings(findings: Vec<FindingSummary>, opts: &FilterOptions) -> Vec<FindingSummary> {
    let mut out = findings;
    if let Some(min) = &opts.min_severity {
        let min_rank = sev_rank(min);
        out.retain(|f| sev_rank(&f.severity) >= min_rank);
    }
    if let Some(detectors) = &opts.detectors
        && !detectors.is_empty()
    {
        let allow: HashSet<&str> = detectors.iter().map(|s| s.as_str()).collect();
        out.retain(|f| allow.contains(f.detector.as_str()));
    }
    out
}

/// `Scope` — `primary` (default) drops findings whose primary object is a dependency;
/// `all` keeps them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Primary,
    All,
}

/// The scope filter. `is_dependency_object(object_id) -> bool` decides whether a
/// finding's primary object lives in a dependency. An unknown object (`object_id`
/// `None`) is KEPT (mirrors al-sem: "unknown object — keep, don't silently drop").
///
/// In the source-only gate path every object is primary, so the predicate always
/// returns `false` and this keeps everything — matching al-sem's behavior when no
/// `analysisRole === "dependency"` object exists.
pub fn scope_filter<F>(
    findings: Vec<FindingSummary>,
    scope: Scope,
    is_dependency_object: F,
) -> Vec<FindingSummary>
where
    F: Fn(&str) -> bool,
{
    match scope {
        Scope::All => findings,
        Scope::Primary => findings
            .into_iter()
            .filter(|f| match &f.primary_location.object_id {
                None => true,
                Some(obj_id) => !is_dependency_object(obj_id),
            })
            .collect(),
    }
}

/// `--limit <n>` — keep the first `n` findings (applied after scope).
pub fn apply_limit(findings: Vec<FindingSummary>, limit: Option<usize>) -> Vec<FindingSummary> {
    match limit {
        Some(n) => findings.into_iter().take(n).collect(),
        None => findings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::gate::projection::FindingLocation;

    fn fs(detector: &str, severity: &str, object_id: Option<&str>) -> FindingSummary {
        FindingSummary {
            id: format!("{detector}-id"),
            fingerprint: "fp".to_string(),
            loose_fingerprint: String::new(),
            detector: detector.to_string(),
            title: "t".to_string(),
            root_cause: "rc".to_string(),
            severity: severity.to_string(),
            confidence_level: "confirmed".to_string(),
            confidence_capped_by: None,
            primary_location: FindingLocation {
                file: "ws:x.al".to_string(),
                line: 1,
                column: 1,
                object_id: object_id.map(|s| s.to_string()),
                object_name: None,
                routine_id: None,
                routine_name: None,
            },
            terminal_location: None,
            affected_objects: vec![],
            affected_tables: vec![],
            path_count: 1,
            fix_hint: None,
        }
    }

    #[test]
    fn min_severity_drops_below() {
        let fs_in = vec![fs("d1", "low", None), fs("d2", "high", None)];
        let out = filter_findings(
            fs_in,
            &FilterOptions {
                min_severity: Some("medium".to_string()),
                detectors: None,
            },
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].detector, "d2");
    }

    #[test]
    fn detector_allow_list() {
        let fs_in = vec![fs("d1", "high", None), fs("d2", "high", None)];
        let out = filter_findings(
            fs_in,
            &FilterOptions {
                min_severity: None,
                detectors: Some(vec!["d2".to_string()]),
            },
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].detector, "d2");
    }

    #[test]
    fn scope_primary_drops_dependency_objects() {
        let fs_in = vec![
            fs("d1", "high", Some("dep-obj")),
            fs("d2", "high", Some("ws-obj")),
            fs("d3", "high", None),
        ];
        let out = scope_filter(fs_in, Scope::Primary, |id| id == "dep-obj");
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|f| f.detector != "d1"));
    }

    #[test]
    fn scope_all_keeps_everything() {
        let fs_in = vec![fs("d1", "high", Some("dep-obj"))];
        let out = scope_filter(fs_in, Scope::All, |_| true);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn limit_caps() {
        let fs_in = vec![fs("d1", "high", None), fs("d2", "high", None)];
        assert_eq!(apply_limit(fs_in, Some(1)).len(), 1);
    }
}
