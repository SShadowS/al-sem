//! `mergeByTerminal` — port of al-sem `src/detectors/path-merge.ts`.
//!
//! Collapse N per-path findings that share a terminal anchor (`root_cause_key`)
//! into ONE finding per anchor, with the other paths attached as
//! `additional_paths`. Byte-critical for d1's `additionalPaths`; reproduce the
//! EXACT canonical-path selection + additionalPaths sort + union ordering.
//!
//! Canonical-pick rules (deterministic):
//!   1. Highest severity wins (critical > high > medium > low > info).
//!   2. Tie on severity → earliest `primary_location` (sourceUnitId by
//!      `compareStrings`, then line, then column).
//!   3. Tie on location → smaller `id` lexicographically (`compareStrings`).
//!
//! Merge math:
//!   - `severity` = the canonical's (already the max).
//!   - `confidence.level` = best level (confirmed > likely > possible).
//!   - `confidence.cappedBy` / `evidence` = union (deduped; cappedBy sorted,
//!     evidence keeps first-seen order).
//!   - `affectedObjects` / `affectedTables` = union (deduped, sorted).
//!   - `additionalPaths` = the non-canonical paths sorted by (sourceUnitId, line,
//!     column, routineId) of the first evidence step.
//!   - `rootCause` annotated with the path count when M > 1.
//!
//! Output is sorted by canonical finding `id` (`compareStrings`).

use crate::engine::l5::finding::{
    ConfidenceEvidence, EVIDENCE_SOURCE, EvidenceStep, Finding, FindingConfidence,
};

/// al-sem `compareStrings(a, b)`: `a < b ? -1 : a > b ? 1 : 0`. For `&str` this
/// is exactly `Ord::cmp` (lexicographic by Unicode scalar value, matching JS
/// `<`/`>` on the BMP id strings al-sem compares).
fn compare_strings(a: &str, b: &str) -> std::cmp::Ordering {
    a.cmp(b)
}

/// Severity rank: critical > high > medium > low > info. Unknown severities rank
/// 0 (below info) — al-sem indexes a total `Record<Severity, …>`, so this only
/// affects out-of-contract inputs. `pub(crate)` so d1's RV-6 merge-tie reuses the
/// SAME ranking the canonical-pick uses (single source of truth for severity order).
pub(crate) fn sev_rank(sev: &str) -> i32 {
    match sev {
        "critical" => 5,
        "high" => 4,
        "medium" => 3,
        "low" => 2,
        "info" => 1,
        _ => 0,
    }
}

/// Confidence rank: confirmed > likely > possible. Unknown levels rank 0.
fn conf_rank(level: &str) -> i32 {
    match level {
        "confirmed" => 3,
        "likely" => 2,
        "possible" => 1,
        _ => 0,
    }
}

/// Sort key for a path's first step — orders `additional_paths` deterministically.
/// Mirrors al-sem `pathSortKey`: empty for an empty path; otherwise
/// `${sourceUnitId}|${startLine padded 8}|${startColumn padded 8}|${routineId}`.
fn path_sort_key(path: &[EvidenceStep]) -> String {
    match path.first() {
        None => String::new(),
        Some(step) => {
            let a = &step.source_anchor;
            format!(
                "{}|{:08}|{:08}|{}",
                a.source_unit_id, a.start_line, a.start_column, step.routine_id
            )
        }
    }
}

/// Pick the canonical (worst, then earliest, then smallest-id) finding from a
/// group. Mirrors al-sem `pickCanonical`. Returns the index into `group`.
fn pick_canonical_index(group: &[Finding]) -> usize {
    let mut best = 0usize;
    for i in 1..group.len() {
        let candidate = &group[i];
        let cur_best = &group[best];
        if sev_rank(&candidate.severity) > sev_rank(&cur_best.severity) {
            best = i;
            continue;
        }
        if sev_rank(&candidate.severity) < sev_rank(&cur_best.severity) {
            continue;
        }
        let a = &candidate.primary_location;
        let b = &cur_best.primary_location;
        if a.source_unit_id != b.source_unit_id {
            if compare_strings(&a.source_unit_id, &b.source_unit_id) == std::cmp::Ordering::Less {
                best = i;
            }
            continue;
        }
        if a.start_line != b.start_line {
            if a.start_line < b.start_line {
                best = i;
            }
            continue;
        }
        if a.start_column != b.start_column {
            if a.start_column < b.start_column {
                best = i;
            }
            continue;
        }
        if compare_strings(&candidate.id, &cur_best.id) == std::cmp::Ordering::Less {
            best = i;
        }
    }
    best
}

/// Merge confidence across a group. Mirrors al-sem `mergeConfidence`: best level,
/// unioned cappedBy (sorted), unioned evidence (first-seen order, dedup by full
/// equality). `cappedBy` is `None` when empty (al-sem omits it).
fn merge_confidence(group: &[Finding]) -> FindingConfidence {
    let mut best_level = "possible".to_string();
    let mut capped: Vec<String> = Vec::new();
    let mut capped_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut evidence: Vec<ConfidenceEvidence> = Vec::new();
    let mut evidence_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for f in group {
        if conf_rank(&f.confidence.level) > conf_rank(&best_level) {
            best_level = f.confidence.level.clone();
        }
        if let Some(cap) = &f.confidence.capped_by {
            for c in cap {
                if capped_seen.insert(c.clone()) {
                    capped.push(c.clone());
                }
            }
        }
        for e in &f.confidence.evidence {
            // al-sem keys evidence by JSON.stringify({source, note?}); replicate
            // with a stable composite of the two fields. `source` is no longer
            // STORED on a `ConfidenceEvidence` (it is the one engine-wide
            // constant — see that type's doc), but it is still what the record
            // projects with, so the key is composed from `EVIDENCE_SOURCE` and
            // stays the byte-identical string it was when the field existed.
            let key = format!(
                "{}\u{0}{}",
                EVIDENCE_SOURCE,
                e.note.as_deref().unwrap_or("\u{1}")
            );
            if evidence_seen.insert(key) {
                evidence.push(e.clone());
            }
        }
    }
    capped.sort();
    FindingConfidence {
        level: best_level,
        capped_by: if capped.is_empty() {
            None
        } else {
            Some(capped)
        },
        evidence,
    }
}

/// Union + dedup + sort a set of interned id lists. Mirrors al-sem `unionSorted`.
/// `Arc<str>`'s `Ord` is `str`'s byte order, exactly as `String`'s is, so the
/// emitted order is unchanged by the shared handles; the "clone" is a refcount bump.
fn union_sorted(lists: &[&[std::sync::Arc<str>]]) -> Vec<std::sync::Arc<str>> {
    let mut set: std::collections::BTreeSet<std::sync::Arc<str>> =
        std::collections::BTreeSet::new();
    for list in lists {
        for v in *list {
            set.insert(std::sync::Arc::clone(v));
        }
    }
    set.into_iter().collect()
}

/// Annotate a finding's `rootCause` with the path count when there are multiple
/// reaching traces. Mirrors al-sem `annotateRootCause`. When `path_count <= 1`,
/// returns the input unchanged.
pub fn annotate_root_cause(root_cause: &str, path_count: usize) -> String {
    if path_count <= 1 {
        return root_cause.to_string();
    }
    let others = path_count - 1;
    let noun = if others == 1 { "ancestor" } else { "ancestors" };
    format!("{root_cause} (Also reached from {others} other in-loop {noun}.)")
}

/// Group `findings` by `root_cause_key` and collapse each group to one Finding
/// with `additional_paths` populated. Singleton groups pass through untouched.
/// Output is sorted by canonical finding `id` (`compareStrings`).
///
/// Mirrors al-sem `mergeByTerminal`. Grouping preserves first-seen finding order
/// WITHIN each group (so `merge_confidence`'s first-seen evidence dedup and the
/// canonical scan match al-sem's `Map`-insertion-order iteration). The group
/// ENUMERATION order does not matter — the final output is re-sorted by `id`.
pub fn merge_by_terminal(findings: Vec<Finding>) -> Vec<Finding> {
    // First-seen ordered grouping: a key order vec + per-key finding lists.
    let mut order: Vec<String> = Vec::new();
    let mut groups: std::collections::HashMap<String, Vec<Finding>> =
        std::collections::HashMap::new();
    for f in findings {
        let key = f.root_cause_key.clone();
        match groups.get_mut(&key) {
            Some(list) => list.push(f),
            None => {
                order.push(key.clone());
                groups.insert(key, vec![f]);
            }
        }
    }

    let mut out: Vec<Finding> = Vec::new();
    for key in &order {
        let group = groups.remove(key).expect("key present in order");
        if group.len() == 1 {
            out.push(group.into_iter().next().expect("len 1"));
            continue;
        }
        let canon_idx = pick_canonical_index(&group);
        let group_len = group.len();

        // additionalPaths = the non-canonical evidence paths, sorted by path key.
        // Through `evidence_path_of` for uniformity: `merge_by_terminal`'s only
        // caller is d2, which never sets `cohort_contexts`, so this is a plain
        // borrow-and-clone of the stored field today — routing it through the
        // accessor costs nothing and removes the latent trap of a cohort-bearing
        // finding ever reaching here and silently merging empty paths.
        let mut other_paths: Vec<Vec<EvidenceStep>> = group
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != canon_idx)
            .map(|(_, f)| crate::engine::l5::finding::evidence_path_of(f).into_owned())
            .collect();
        other_paths.sort_by(|a, b| compare_strings(&path_sort_key(a), &path_sort_key(b)));

        let confidence = merge_confidence(&group);
        let affected_objects = union_sorted(
            &group
                .iter()
                .map(|f| f.affected_objects.as_slice())
                .collect::<Vec<_>>(),
        );
        let affected_tables = union_sorted(
            &group
                .iter()
                .map(|f| f.affected_tables.as_slice())
                .collect::<Vec<_>>(),
        );

        let canonical = &group[canon_idx];
        let merged = Finding {
            confidence,
            // `union_sorted` already returns `Vec<Arc<str>>` built from `Arc::clone`
            // (shared handles, refcount bumps only) — the field type matches exactly,
            // so no `id_list` re-materialisation is needed here. `id_list` re-allocates
            // a fresh `Arc<str>` per entry, which would silently undo that sharing on
            // every call to `merge_by_terminal` (d2's path).
            affected_objects,
            affected_tables,
            root_cause: annotate_root_cause(&canonical.root_cause, group_len),
            additional_paths: Some(other_paths),
            ..canonical.clone()
        };
        out.push(merged);
    }

    out.sort_by(|a, b| compare_strings(&a.id, &b.id));
    out
}

// ===========================================================================
// Native oracles — synthetic Findings exercising the merge.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    // Only the test helper below still builds `Finding`s from owned `Vec<String>`
    // ids, so `id_list` is a test-only import now that `merge_by_terminal` itself
    // forwards `union_sorted`'s `Arc<str>` handles directly.
    use crate::engine::l5::finding::SourceAnchor;
    use crate::engine::l5::finding::id_list;

    fn anchor(unit: &str, line: u32, col: u32, routine: &str) -> SourceAnchor {
        SourceAnchor {
            source_unit_id: unit.to_string(),
            start_line: line,
            start_column: col,
            end_line: line,
            end_column: col,
            enclosing_routine_id: routine.to_string(),
            syntax_kind: "x".to_string(),
            normalized_text_hash: None,
            leading_context_hash: None,
            trailing_context_hash: None,
        }
    }

    fn step(unit: &str, line: u32, col: u32, routine: &str) -> EvidenceStep {
        EvidenceStep {
            routine_id: routine.to_string(),
            operation_id: None,
            callsite_id: None,
            loop_id: None,
            source_anchor: anchor(unit, line, col, routine),
            note: "hop".to_string(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn finding(
        id: &str,
        root_cause_key: &str,
        severity: &str,
        conf_level: &str,
        primary: SourceAnchor,
        path: Vec<EvidenceStep>,
        affected_objects: Vec<String>,
        affected_tables: Vec<String>,
    ) -> Finding {
        Finding {
            id: id.to_string(),
            root_cause_key: root_cause_key.to_string(),
            detector: "d1".to_string(),
            title: "t".into(),
            root_cause: "rc".to_string(),
            severity: severity.to_string(),
            confidence: FindingConfidence {
                level: conf_level.to_string(),
                capped_by: None,
                evidence: vec![],
            },
            primary_location: primary,
            evidence_path: path,
            additional_paths: None,
            affected_objects: id_list(affected_objects),
            affected_tables: id_list(affected_tables),
            fix_options: vec![],
            provenance: vec![],
            actionable_anchor: None,
            fingerprint: None,
            event_kind: None,
            cross_extension_subscribers: None,
            cohort_contexts: None,
            loose_subject: None,
        }
    }

    #[test]
    fn two_findings_same_key_fold_to_one_with_additional_paths() {
        // Two findings share root_cause_key "k". f_hi is critical (canonical),
        // f_lo is low. The low path becomes the single additionalPath.
        let primary = anchor("ws:term.al", 10, 4, "r_term");
        let f_hi = finding(
            "d1/b",
            "k",
            "critical",
            "likely",
            primary.clone(),
            vec![step("ws:hi.al", 1, 0, "r_hi")],
            vec!["app/Codeunit/1".to_string()],
            vec!["g/table/18".to_string()],
        );
        let f_lo = finding(
            "d1/a",
            "k",
            "low",
            "confirmed",
            primary.clone(),
            vec![step("ws:lo.al", 2, 0, "r_lo")],
            vec!["app/Codeunit/2".to_string()],
            vec!["g/table/27".to_string()],
        );

        let merged = merge_by_terminal(vec![f_lo, f_hi]);
        assert_eq!(merged.len(), 1);
        let m = &merged[0];
        // Canonical is the critical finding.
        assert_eq!(m.severity, "critical");
        assert_eq!(m.id, "d1/b");
        // best confidence level = confirmed (from the low finding).
        assert_eq!(m.confidence.level, "confirmed");
        // additionalPaths carries the NON-canonical (low) path.
        let ap = m.additional_paths.as_ref().expect("additional paths set");
        assert_eq!(ap.len(), 1);
        assert_eq!(ap[0][0].source_anchor.source_unit_id, "ws:lo.al");
        // unioned + sorted affected tables.
        let tables: Vec<&str> = m.affected_tables.iter().map(|s| &**s).collect();
        assert_eq!(tables, vec!["g/table/18", "g/table/27"]);
        let objects: Vec<&str> = m.affected_objects.iter().map(|s| &**s).collect();
        assert_eq!(objects, vec!["app/Codeunit/1", "app/Codeunit/2"]);
        // rootCause annotated with the other-path count (1 other ancestor).
        assert_eq!(
            m.root_cause,
            "rc (Also reached from 1 other in-loop ancestor.)"
        );
    }

    #[test]
    fn affected_object_ids_are_shared_not_reallocated_across_the_merge() {
        // `union_sorted` clones the SAME `Arc<str>` handle contributed by whichever
        // finding it saw first (a refcount bump, not a byte copy). The merge must
        // forward that handle unchanged; wrapping it back through `id_list` (as the
        // code did before this test) re-materialises a fresh `Arc<str>` from the
        // text and silently undoes the sharing on every call to `merge_by_terminal`.
        let shared: std::sync::Arc<str> = std::sync::Arc::from("app/Codeunit/1");
        let primary = anchor("ws:term.al", 10, 4, "r_term");
        let mut f_hi = finding(
            "d1/b",
            "k",
            "critical",
            "likely",
            primary.clone(),
            vec![step("ws:hi.al", 1, 0, "r_hi")],
            vec![],
            vec![],
        );
        f_hi.affected_objects = vec![std::sync::Arc::clone(&shared)];
        let mut f_lo = finding(
            "d1/a",
            "k",
            "low",
            "confirmed",
            primary,
            vec![step("ws:lo.al", 2, 0, "r_lo")],
            vec![],
            vec![],
        );
        f_lo.affected_objects = vec![std::sync::Arc::clone(&shared)];

        let merged = merge_by_terminal(vec![f_lo, f_hi]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].affected_objects.len(), 1);
        assert!(
            std::sync::Arc::ptr_eq(&merged[0].affected_objects[0], &shared),
            "merge_by_terminal must forward union_sorted's Arc<str> handle unchanged, \
             not re-allocate a new one from its text"
        );
    }

    #[test]
    fn distinct_keys_pass_through_unchanged() {
        let f1 = finding(
            "d1/x",
            "k1",
            "high",
            "likely",
            anchor("ws:a.al", 1, 0, "r1"),
            vec![step("ws:a.al", 1, 0, "r1")],
            vec![],
            vec![],
        );
        let f2 = finding(
            "d1/y",
            "k2",
            "high",
            "likely",
            anchor("ws:b.al", 1, 0, "r2"),
            vec![step("ws:b.al", 1, 0, "r2")],
            vec![],
            vec![],
        );
        let merged = merge_by_terminal(vec![f2, f1]);
        assert_eq!(merged.len(), 2);
        // Sorted by id (compareStrings).
        assert_eq!(merged[0].id, "d1/x");
        assert_eq!(merged[1].id, "d1/y");
        // Singletons keep additionalPaths None.
        assert!(merged[0].additional_paths.is_none());
        assert!(merged[1].additional_paths.is_none());
        // Singletons keep their original rootCause (no annotation).
        assert_eq!(merged[0].root_cause, "rc");
    }

    #[test]
    fn additional_paths_sorted_deterministically() {
        // Three findings same key; canonical chosen by severity. The two
        // non-canonical paths must be sorted by (sourceUnitId, line, col, routine).
        let primary = anchor("ws:term.al", 5, 0, "r_term");
        let canon = finding(
            "d1/canon",
            "k",
            "critical",
            "likely",
            primary.clone(),
            vec![step("ws:canon.al", 0, 0, "rc")],
            vec![],
            vec![],
        );
        // Two low paths whose first-step keys order: "ws:a.al|..." < "ws:z.al|...".
        let p_z = finding(
            "d1/z",
            "k",
            "low",
            "likely",
            primary.clone(),
            vec![step("ws:z.al", 1, 0, "rz")],
            vec![],
            vec![],
        );
        let p_a = finding(
            "d1/a",
            "k",
            "low",
            "likely",
            primary.clone(),
            vec![step("ws:a.al", 1, 0, "ra")],
            vec![],
            vec![],
        );
        // Insert z before a; the additionalPaths sort must reorder to a, z.
        let merged = merge_by_terminal(vec![p_z, canon, p_a]);
        assert_eq!(merged.len(), 1);
        let ap = merged[0].additional_paths.as_ref().unwrap();
        assert_eq!(ap.len(), 2);
        assert_eq!(ap[0][0].source_anchor.source_unit_id, "ws:a.al");
        assert_eq!(ap[1][0].source_anchor.source_unit_id, "ws:z.al");
    }

    #[test]
    fn annotate_pluralizes_ancestors() {
        assert_eq!(annotate_root_cause("rc", 1), "rc");
        assert_eq!(
            annotate_root_cause("rc", 2),
            "rc (Also reached from 1 other in-loop ancestor.)"
        );
        assert_eq!(
            annotate_root_cause("rc", 3),
            "rc (Also reached from 2 other in-loop ancestors.)"
        );
    }
}
