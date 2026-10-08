//! PR-summary renderer — port of al-sem `src/cli/format-pr-summary.ts`.
//!
//! Concise, severity-grouped, app-attributed markdown for PR comments:
//!   - Grouped by severity (critical → high → medium → low → info).
//!   - Each bucket SORTED by fingerprint (`fingerprint ?? id`) for determinism.
//!   - Truncated to `TOP_N_PER_SEVERITY` per group; remainder shown as
//!     "… M more {sev} finding(s) — see SARIF".
//!   - Per finding: severity label + `[detector] title`; the app-attribution line
//!     `App: {publisher}/{name} {version}  —  "{ObjectName}".{Routine}()`; one line per
//!     evidence step (`{sourceUnitId}:{line}  {note}`); the coverage note from
//!     `confidence.cappedBy`; the cross-app blame line when applicable.
//!   - NO scalar/numeric risk score; NO embedded version (byte-stable).

use crate::engine::gate::app_attribution::{
    App, AttributionIndex, app_for_finding, blame_for_finding,
};
use crate::engine::gate::projection::FindingSummary;
use crate::engine::l5::finding::Finding;

const TOP_N_PER_SEVERITY: usize = 10;

const SEV_ORDER: &[&str] = &["critical", "high", "medium", "low", "info"];

/// Severity label prefix (al-sem `SEV_LABEL`).
fn sev_label(sev: &str) -> &'static str {
    match sev {
        "critical" => "CRITICAL",
        "high" => "HIGH",
        "medium" => "MEDIUM",
        "low" => "LOW",
        "info" => "INFO",
        _ => "INFO",
    }
}

/// `appLabel` — `"{publisher}/{name} {version}"`, or `"(unknown app)"`.
fn app_label(app: Option<&App>) -> String {
    match app {
        Some(a) => format!("{}/{} {}", a.publisher, a.name, a.version),
        None => "(unknown app)".to_string(),
    }
}

/// Render the PR-comment markdown. `pairs` are the post-filter `(summary, raw)` findings
/// (the SAME order/set the SARIF formatter receives); `apps` is the resolved workspace
/// app registry (source-only ⇒ one app).
///
/// Returns the string WITHOUT a trailing newline (the CLI / caller appends `"\n"`, like
/// al-sem's `process.stdout.write(`${formatPrSummary(...)}\n`)`).
pub fn format_pr_summary(
    pairs: &[(FindingSummary, &Finding)],
    routines: &[crate::program::model::workspace::ModelRoutine],
    apps: &[App],
) -> String {
    let idx = AttributionIndex::build(routines, apps);

    // Group raw findings by severity (only the recognised severities).
    let mut by_sev: Vec<(&str, Vec<&(FindingSummary, &Finding)>)> =
        SEV_ORDER.iter().map(|s| (*s, Vec::new())).collect();
    for pair in pairs {
        let sev = pair.1.severity.as_str();
        if let Some(slot) = by_sev.iter_mut().find(|(s, _)| *s == sev) {
            slot.1.push(pair);
        }
    }

    // Sort each bucket by fingerprint (`fingerprint ?? id`).
    for (_, bucket) in by_sev.iter_mut() {
        bucket.sort_by(|a, b| {
            let fa = a.1.fingerprint.as_deref().unwrap_or(a.1.id.as_str());
            let fb = b.1.fingerprint.as_deref().unwrap_or(b.1.id.as_str());
            fa.cmp(fb)
        });
    }

    // --- header summary line ---
    let mut count_parts: Vec<String> = Vec::new();
    for (sev, bucket) in &by_sev {
        if !bucket.is_empty() {
            count_parts.push(format!("{} {}", bucket.len(), sev));
        }
    }

    let mut lines: Vec<String> = Vec::new();

    if count_parts.is_empty() {
        lines.push("### Transaction integrity — no findings".to_string());
        lines.push(String::new());
        lines.push("No transaction-integrity findings detected.".to_string());
        return lines.join("\n");
    }

    lines.push(format!(
        "### ⛔ Transaction integrity — {}",
        count_parts.join(", ")
    ));
    lines.push(String::new());

    // --- per-severity groups ---
    for (sev, bucket) in &by_sev {
        if bucket.is_empty() {
            continue;
        }
        let shown_n = bucket.len().min(TOP_N_PER_SEVERITY);
        let overflow = bucket.len() - shown_n;

        for pair in &bucket[..shown_n] {
            let summary = &pair.0;
            let finding = pair.1;

            // F1 FIX: use the RAW finding's primary_location.enclosing_routine_id for
            // app attribution / blame, NOT the projected summary's routine_id (which
            // may be swapped to the actionable_anchor for cross-app findings).
            // Mirrors al-sem app-attribution.ts: `finding.primaryLocation.enclosingRoutineId`.
            let raw_primary_routine_id = finding.primary_location.enclosing_routine_id.as_str();
            let owner_app = app_for_finding(raw_primary_routine_id, &idx);
            // Through `evidence_path_of` (twice below): a cohort-bearing d1
            // finding derives its path from `cohort_contexts[0].witness` rather
            // than storing it. Materialised ONCE here and reused for the witness
            // lines further down.
            let evidence_path = crate::engine::l5::finding::evidence_path_of(finding);
            let evidence_routine_ids: Vec<String> =
                evidence_path.iter().map(|s| s.routine_id.clone()).collect();
            let blame = blame_for_finding(raw_primary_routine_id, &evidence_routine_ids, &idx);

            // --- finding header line ---
            lines.push(format!(
                "**{}**  [{}] {}",
                sev_label(sev),
                finding.detector,
                summary.title
            ));

            // --- app identity + object/routine context ---
            let app_str = app_label(owner_app);
            let obj_name = match &summary.primary_location.object_name {
                Some(n) => format!("\"{n}\""),
                None => "(unknown)".to_string(),
            };
            let routine_name = summary
                .primary_location
                .routine_name
                .as_deref()
                .unwrap_or("(unknown)");
            lines.push(format!("  App: {app_str}  —  {obj_name}.{routine_name}()"));

            // --- evidence witness path (file:line  note) ---
            for step in evidence_path.iter() {
                let anchor = &step.source_anchor;
                let file_line = format!("{}:{}", anchor.source_unit_id, anchor.start_line + 1);
                lines.push(format!("  {file_line}  {}", step.note));
            }

            // --- coverage / confidence note ---
            match &finding.confidence.capped_by {
                Some(capped) if !capped.is_empty() => {
                    lines.push(format!(
                        "  coverage: partial — capped by: {}",
                        capped.join(", ")
                    ));
                }
                _ => {
                    lines.push("  coverage: complete".to_string());
                }
            }

            // --- cross-app blame note ---
            if blame.cross_app && !blame.other_apps.is_empty() {
                let other_names = blame
                    .other_apps
                    .iter()
                    .map(|a| app_label(Some(a)))
                    .collect::<Vec<_>>()
                    .join(", ");
                lines.push(format!(
                    "  cross-app: hazard in {app_str}, via: {other_names}"
                ));
            }

            lines.push(String::new());
        }

        if overflow > 0 {
            lines.push(format!(
                "… {overflow} more {sev} finding{} — see SARIF",
                if overflow == 1 { "" } else { "s" }
            ));
            lines.push(String::new());
        }
    }

    // Remove trailing blank line(s).
    while lines.last().map(|l| l.is_empty()).unwrap_or(false) {
        lines.pop();
    }

    lines.join("\n")
}

// ===========================================================================
// Unit oracles — corpus-invisible cells covered by native #[cfg(test)] only.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::gate::projection::{FindingLocation, FindingSummary};
    use crate::engine::l5::finding::{EvidenceStep, Finding, FindingConfidence, SourceAnchor};
    use crate::program::model::workspace::{ModelRoutine, RoutineVariables};

    /// Build a minimal `SourceAnchor` keyed on `enclosing_routine_id`.
    fn anchor(routine_id: &str) -> SourceAnchor {
        SourceAnchor {
            source_unit_id: "src/Foo.al".to_string(),
            start_line: 9,
            start_column: 0,
            end_line: 9,
            end_column: 10,
            enclosing_routine_id: routine_id.to_string(),
            syntax_kind: "method_call".to_string(),
            normalized_text_hash: None,
            leading_context_hash: None,
            trailing_context_hash: None,
        }
    }

    /// Build a minimal `EvidenceStep` keyed on `routine_id`.
    fn step(routine_id: &str, note: &str) -> EvidenceStep {
        EvidenceStep {
            routine_id: routine_id.to_string(),
            operation_id: None,
            callsite_id: None,
            loop_id: None,
            source_anchor: anchor(routine_id),
            note: note.to_string(),
        }
    }

    /// Build a minimal `Finding` with `severity`, `primary_location` keyed on
    /// `primary_routine_id`, and optional `actionable_anchor`.
    fn make_finding(
        id: &str,
        severity: &str,
        primary_routine_id: &str,
        actionable_routine_id: Option<&str>,
        capped_by: Option<Vec<String>>,
    ) -> Finding {
        Finding {
            id: id.to_string(),
            root_cause_key: id.to_string(),
            detector: "d-test".to_string(),
            title: "Test finding".into(),
            root_cause: "test".to_string(),
            severity: severity.to_string(),
            confidence: FindingConfidence {
                level: "high".to_string(),
                capped_by,
                evidence: vec![],
            },
            primary_location: anchor(primary_routine_id),
            evidence_path: vec![step(primary_routine_id, "test step")],
            additional_paths: None,
            affected_objects: vec![],
            affected_tables: vec![],
            fix_options: vec![],
            provenance: vec![],
            actionable_anchor: actionable_routine_id.map(anchor),
            fingerprint: None,
            event_kind: None,
            cross_extension_subscribers: None,
            cohort_contexts: None,
        }
    }

    /// Build a minimal `FindingSummary` with `id`, `severity`, and a
    /// `primary_location.routine_id` (the PROJECTED/actionable one).
    fn make_summary(
        id: &str,
        severity: &str,
        projected_routine_id: Option<&str>,
    ) -> FindingSummary {
        FindingSummary {
            id: id.to_string(),
            fingerprint: id.to_string(),
            detector: "d-test".to_string(),
            title: "Test finding".to_string(),
            root_cause: "test".to_string(),
            severity: severity.to_string(),
            confidence_level: "high".to_string(),
            confidence_capped_by: None,
            primary_location: FindingLocation {
                file: "src/Foo.al".to_string(),
                line: 10,
                column: 1,
                object_id: None,
                object_name: Some("FooCodeunit".to_string()),
                routine_id: projected_routine_id.map(|s| s.to_string()),
                routine_name: Some("DoThing".to_string()),
            },
            terminal_location: None,
            affected_objects: vec![],
            affected_tables: vec![],
            path_count: 1,
            fix_hint: None,
        }
    }

    /// Build a minimal `ModelRoutine` mapping `routine_id` → `object_id`.
    fn make_routine(routine_id: &str, object_id: &str) -> ModelRoutine {
        use crate::program::body::features::PAnchor;
        ModelRoutine {
            id: routine_id.to_string(),
            stable_routine_id: routine_id.to_string(),
            object_id: object_id.to_string(),
            object_type: "Codeunit".to_string(),
            name: "DoThing".to_string(),
            kind: "procedure".to_string(),
            attributes_parsed: vec![],
            app_guid: object_id.split('/').next().unwrap_or("").to_string(),
            object_number: 50000,
            normalized_signature_hash: "hash".to_string(),
            body_available: true,
            parse_incomplete: false,
            record_variables: vec![],
            record_operations: vec![],
            field_accesses: vec![],
            variables: RoutineVariables::default(),
            parameters: vec![],
            access_modifier: None,
            return_type: None,
            call_sites: vec![],
            operation_sites: vec![],
            statement_tree: None,
            loops: vec![],
            source_anchor: PAnchor {
                source_unit_id: "src/Foo.al".to_string(),
                start_line: 0,
                start_column: 0,
                end_line: 0,
                end_column: 0,
                syntax_kind: "procedure".to_string(),
            },
            identifier_references: vec![],
            unreachable_statements: vec![],
            has_branching: false,
            var_assignments: vec![],
            condition_references: vec![],
            enclosing_member: None,
            originating_object: None,
            enclosing_member_range: None,
            entry_temp_guard_receiver: None,
        }
    }

    /// Build a minimal `App`.
    fn make_app(guid: &str, publisher: &str, name: &str, version: &str) -> App {
        App {
            app_guid: guid.to_string(),
            publisher: publisher.to_string(),
            name: name.to_string(),
            version: version.to_string(),
        }
    }

    // -----------------------------------------------------------------------
    // Oracle 1 — F1 cross-app attribution (corpus-invisible cell)
    //
    // A Finding whose RAW primary_location.enclosing_routine_id belongs to a
    // DEP app, but whose actionable_anchor belongs to a PRIMARY app.
    // The projected summary's primary_location.routine_id will be the actionable
    // (primary-app) routine — the wrong one. The F1 fix uses the raw routine id,
    // so app_for_finding / blame_for_finding must resolve to the DEP app.
    // -----------------------------------------------------------------------
    #[test]
    fn oracle_f1_cross_app_attribution_uses_raw_primary_location() {
        let dep_guid = "dep-app-guid-0000-0000-0000-000000000000";
        let primary_guid = "primary-app-guid-0000-0000-0000-000000000000";

        // The RAW primary_location.enclosing_routine_id → dep app
        let dep_routine_id = format!("mii/{dep_guid}-routine-hash");
        let dep_object_id = format!("{dep_guid}/Codeunit/50000");

        // The actionable_anchor.enclosing_routine_id → primary app
        let primary_routine_id = format!("mii/{primary_guid}-routine-hash");
        let primary_object_id = format!("{primary_guid}/Codeunit/50001");

        let dep_app = make_app(dep_guid, "Dep", "DepLib", "1.0.0.0");
        let primary_app = make_app(primary_guid, "Me", "MyApp", "2.0.0.0");
        let apps = vec![dep_app, primary_app];

        let dep_routine = make_routine(&dep_routine_id, &dep_object_id);
        let primary_routine = make_routine(&primary_routine_id, &primary_object_id);
        let routines = vec![dep_routine, primary_routine];

        // Finding: raw primary → dep; actionable → primary
        let finding = make_finding(
            "f1",
            "high",
            &dep_routine_id,
            Some(&primary_routine_id),
            None,
        );
        // Summary: projected primary (actionable) → primary_routine_id
        let summary = make_summary("f1", "high", Some(&primary_routine_id));

        let idx = AttributionIndex::build(&routines, &apps);

        // F1: app_for_finding uses RAW primary_location.enclosing_routine_id
        let raw_rid = finding.primary_location.enclosing_routine_id.as_str();
        let owner = app_for_finding(raw_rid, &idx);
        assert_eq!(
            owner.map(|a| a.app_guid.as_str()),
            Some(dep_guid),
            "F1: owner app must be the DEP app (raw primary_location), not the actionable-anchor app"
        );

        // Confirm the WRONG path (projected summary's routine_id) would give the primary app
        let projected_rid = summary.primary_location.routine_id.as_deref().unwrap_or("");
        let projected_owner = app_for_finding(projected_rid, &idx);
        assert_eq!(
            projected_owner.map(|a| a.app_guid.as_str()),
            Some(primary_guid),
            "sanity: projected routine_id resolves to the primary app (the pre-F1 wrong answer)"
        );

        // The format_pr_summary output must use the dep app label, NOT the primary app label
        let pairs: Vec<(FindingSummary, &Finding)> = vec![(summary, &finding)];
        let md = format_pr_summary(&pairs, &routines, &apps);
        assert!(
            md.contains("Dep/DepLib 1.0.0.0"),
            "F1: PR-summary must attribute the finding to the DEP app (raw primary_location):\n{md}"
        );
        assert!(
            !md.contains("Me/MyApp 2.0.0.0"),
            "F1: PR-summary must NOT attribute the finding to the primary (actionable) app:\n{md}"
        );
    }

    // -----------------------------------------------------------------------
    // Oracle 2 — TOP_N>10 truncation (corpus-invisible cell)
    //
    // Build 12 findings in one severity bucket → assert exactly 10 shown + the
    // "… {M} more {sev} findings — see SARIF" line with M=2 + plural.
    // -----------------------------------------------------------------------
    #[test]
    fn oracle_top_n_truncation() {
        let guid = "aaaa0000-0000-0000-0000-000000000000";
        let routine_id = format!("mii/{guid}-rh");
        let object_id = format!("{guid}/Codeunit/50000");
        let app = make_app(guid, "Me", "MyApp", "1.0.0.0");
        let routines = vec![make_routine(&routine_id, &object_id)];
        let apps = vec![app];

        // 12 high-severity findings — fingerprints ensure stable sort order
        let findings: Vec<Finding> = (0..12)
            .map(|i| {
                let mut f = make_finding(&format!("f{i:02}"), "high", &routine_id, None, None);
                f.fingerprint = Some(format!("fp{i:02}"));
                f.id = format!("f{i:02}");
                f
            })
            .collect();
        let pairs: Vec<(FindingSummary, &Finding)> = findings
            .iter()
            .map(|f| {
                let mut s = make_summary(&f.id, "high", Some(&routine_id));
                s.fingerprint = f.fingerprint.clone().unwrap();
                s.id = f.id.clone();
                (s, f)
            })
            .collect();

        let md = format_pr_summary(&pairs, &routines, &apps);

        // Exactly 10 "coverage:" lines (one per shown finding)
        let coverage_count = md.matches("  coverage:").count();
        assert_eq!(
            coverage_count, 10,
            "expected exactly 10 findings shown (TOP_N=10), got {coverage_count} in:\n{md}"
        );

        // The overflow line: "… 2 more high findings — see SARIF"
        assert!(
            md.contains("… 2 more high findings — see SARIF"),
            "expected the overflow line for 2 excess findings:\n{md}"
        );
    }

    // -----------------------------------------------------------------------
    // Oracle 3 — cappedBy partial-coverage note (corpus-invisible cell)
    //
    // A finding with non-empty confidence.cappedBy → assert the
    // `coverage: partial — capped by: …` line appears. Every golden is
    // "complete", so this path is untested by the differential.
    // -----------------------------------------------------------------------
    #[test]
    fn oracle_capped_by_partial_coverage_note() {
        let guid = "bbbb0000-0000-0000-0000-000000000000";
        let routine_id = format!("mii/{guid}-rh");
        let object_id = format!("{guid}/Codeunit/50000");
        let app = make_app(guid, "Me", "MyApp", "1.0.0.0");
        let routines = vec![make_routine(&routine_id, &object_id)];
        let apps = vec![app];

        let finding = make_finding(
            "f-capped",
            "high",
            &routine_id,
            None,
            Some(vec!["ExternalLib".to_string(), "OpaqueApp".to_string()]),
        );
        let mut summary = make_summary("f-capped", "high", Some(&routine_id));
        summary.confidence_capped_by =
            Some(vec!["ExternalLib".to_string(), "OpaqueApp".to_string()]);

        let pairs: Vec<(FindingSummary, &Finding)> = vec![(summary, &finding)];
        let md = format_pr_summary(&pairs, &routines, &apps);

        assert!(
            md.contains("  coverage: partial — capped by: ExternalLib, OpaqueApp"),
            "expected 'coverage: partial — capped by: ...' line for a capped finding:\n{md}"
        );
        assert!(
            !md.contains("coverage: complete"),
            "must NOT emit 'coverage: complete' when capped_by is non-empty:\n{md}"
        );
    }
}
