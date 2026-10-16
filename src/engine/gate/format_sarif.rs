//! `format_sarif` — port of al-sem `src/cli/format-sarif.ts`.
//!
//! Emits SARIF 2.1.0 JSON whose object-key ORDER + 2-space pretty form byte-match
//! `JSON.stringify(sarif, null, 2)`. The struct field declaration order below mirrors
//! al-sem's object-literal construction order EXACTLY; `serde_json` serializes struct
//! fields in declaration order, and `skip_serializing_if` reproduces the
//! `undefined`-field omission (`logicalLocations` / `codeFlows` absent when empty).
//!
//! The `tool.driver.rules` catalog is derived from the detector registry
//! (`registered_detectors()`, default and opt-in alike): one rule per registered
//! detector, carrying its `rule_name` and `short_description`, in natural
//! detector-number order. A result's `ruleId` therefore always names a rule.
//!
//! Determinism: the `results` array preserves the pre-sorted `FindingSummary` order
//! (the findings come pre-sorted out of `run_detectors`); no map iteration leaks in.

use serde::Serialize;

use crate::engine::gate::projection::FindingSummary;
use crate::engine::l5::finding::{EvidenceStep, Finding};

/// SARIF level for a finding severity (`SARIF_LEVEL`). critical/high → error,
/// medium → warning, low/info → note. Unknown → warning (`?? "warning"`).
fn sarif_level(severity: &str) -> &'static str {
    match severity {
        "critical" | "high" => "error",
        "medium" => "warning",
        "low" | "info" => "note",
        _ => "warning",
    }
}

// ---------------------------------------------------------------------------
// Rule catalog — one rule per registered detector (the registry is the source).
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ShortDescription {
    text: &'static str,
}

#[derive(Serialize)]
struct SarifRule {
    id: String,
    name: &'static str,
    #[serde(rename = "shortDescription")]
    short_description: ShortDescription,
}

/// Every registered detector as a SARIF rule, sorted by natural detector number
/// (`d2` before `d10`). The registry's own order is not numeric, so the sort is
/// explicit. Results carry only `ruleId` (no `ruleIndex`), so order is cosmetic.
fn rules() -> Vec<SarifRule> {
    let mut rules: Vec<SarifRule> = crate::engine::l5::detectors::registered_detectors()
        .into_iter()
        .map(|d| SarifRule {
            id: d.name,
            name: d.rule_name,
            short_description: ShortDescription {
                text: d.short_description,
            },
        })
        .collect();
    rules.sort_by(|a, b| crate::engine::l5::registry::compare_natural(&a.id, &b.id));
    rules
}

// ---------------------------------------------------------------------------
// SARIF document shape (key order = al-sem object-literal order).
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ArtifactLocation {
    uri: String,
}

#[derive(Serialize)]
struct Region {
    #[serde(rename = "startLine")]
    start_line: u32,
    #[serde(rename = "startColumn")]
    start_column: u32,
}

#[derive(Serialize)]
struct PhysicalLocation {
    #[serde(rename = "artifactLocation")]
    artifact_location: ArtifactLocation,
    region: Region,
}

#[derive(Serialize)]
struct LogicalLocation {
    name: String,
}

#[derive(Serialize)]
struct MessageText {
    text: String,
}

/// A `locations[]` entry on a result: `{ physicalLocation, logicalLocations? }`.
#[derive(Serialize)]
struct ResultLocation {
    #[serde(rename = "physicalLocation")]
    physical_location: PhysicalLocation,
    #[serde(rename = "logicalLocations", skip_serializing_if = "Option::is_none")]
    logical_locations: Option<Vec<LogicalLocation>>,
}

/// A threadFlow location: `{ location: { physicalLocation, message } }`.
#[derive(Serialize)]
struct ThreadFlowLocationInner {
    #[serde(rename = "physicalLocation")]
    physical_location: PhysicalLocation,
    message: MessageText,
}

#[derive(Serialize)]
struct ThreadFlowLocation {
    location: ThreadFlowLocationInner,
}

#[derive(Serialize)]
struct ThreadFlow {
    locations: Vec<ThreadFlowLocation>,
}

#[derive(Serialize)]
struct CodeFlow {
    /// SARIF `codeFlow.message` — for a d1 COHORT finding, the per-class summary
    /// ("reached by N loops — <verdict>, M total hops") so the compressed
    /// cohort's `loop_count`/`total_hops` metadata rides on its representative
    /// code-flow. `None` (skipped) for every non-cohort finding, keeping their
    /// SARIF byte-identical.
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<MessageText>,
    #[serde(rename = "threadFlows")]
    thread_flows: Vec<ThreadFlow>,
}

/// A SARIF result. Field order: ruleId, level, message, fingerprints, locations,
/// [codeFlows] — exactly al-sem's `result_` insertion order.
#[derive(Serialize)]
struct SarifResult {
    #[serde(rename = "ruleId")]
    rule_id: String,
    level: &'static str,
    message: MessageText,
    fingerprints: Fingerprints,
    locations: Vec<ResultLocation>,
    #[serde(rename = "codeFlows", skip_serializing_if = "Option::is_none")]
    code_flows: Option<Vec<CodeFlow>>,
}

/// `fingerprints: { "al-sem/v1": <fp> }`.
#[derive(Serialize)]
struct Fingerprints {
    #[serde(rename = "al-sem/v1")]
    al_sem_v1: String,
}

#[derive(Serialize)]
struct Driver {
    name: &'static str,
    version: String,
    #[serde(rename = "informationUri")]
    information_uri: &'static str,
    rules: Vec<SarifRule>,
}

#[derive(Serialize)]
struct Tool {
    driver: Driver,
}

#[derive(Serialize)]
struct Run {
    tool: Tool,
    results: Vec<SarifResult>,
}

#[derive(Serialize)]
struct SarifDocument {
    #[serde(rename = "$schema")]
    schema: &'static str,
    version: &'static str,
    runs: Vec<Run>,
}

/// `pathToThreadFlow(path)` — one `EvidenceStep` → one threadFlowLocation. The step's
/// `note` becomes the location's message; the anchor (1-based) is the physical region.
fn path_to_thread_flow(path: &[EvidenceStep]) -> ThreadFlow {
    ThreadFlow {
        locations: path
            .iter()
            .map(|step| ThreadFlowLocation {
                location: ThreadFlowLocationInner {
                    physical_location: PhysicalLocation {
                        artifact_location: ArtifactLocation {
                            uri: step.source_anchor.source_unit_id.clone(),
                        },
                        region: Region {
                            start_line: step.source_anchor.start_line + 1,
                            start_column: step.source_anchor.start_column + 1,
                        },
                    },
                    message: MessageText {
                        text: step.note.clone(),
                    },
                },
            })
            .collect(),
    }
}

/// Build one SARIF result for a projected finding + its raw `Finding` (for the
/// evidence/additional paths). Mirrors format-sarif.ts `results.map`.
fn build_result(summary: &FindingSummary, raw: &Finding) -> SarifResult {
    // logicalLocations: present only when objectName or routineName is defined.
    let logical_locations = if summary.primary_location.object_name.is_some()
        || summary.primary_location.routine_name.is_some()
    {
        let object = summary
            .primary_location
            .object_name
            .clone()
            .unwrap_or_default();
        let routine = summary
            .primary_location
            .routine_name
            .clone()
            .unwrap_or_default();
        Some(vec![LogicalLocation {
            name: format!("{object} :: {routine}"),
        }])
    } else {
        None
    };

    // codeFlows. For a d1 COHORT finding (`cohort_contexts` set), emit ONE
    // code-flow per verdict-class cohort — its bounded representative witness +
    // a `message` carrying the class's `loop_count`/verdict/`total_hops`. For
    // every other finding, the unchanged `[evidencePath, ...additionalPaths]`
    // list (message-less, byte-identical to before the cutover).
    let code_flows = if let Some(cohorts) = &raw.cohort_contexts {
        let flows: Vec<CodeFlow> = cohorts
            .iter()
            .map(|c| {
                let path = crate::engine::l5::d1_witness::flatten_witness(&c.witness);
                let loops = if c.loop_count == 1 { "loop" } else { "loops" };
                CodeFlow {
                    message: Some(MessageText {
                        text: format!(
                            "Reached by {} {loops} — {} verdict, {} total hops",
                            c.loop_count, c.verdict, c.witness.total_hops
                        ),
                    }),
                    thread_flows: vec![path_to_thread_flow(&path)],
                }
            })
            .filter(|cf| !cf.thread_flows[0].locations.is_empty())
            .collect();
        if flows.is_empty() { None } else { Some(flows) }
    } else {
        let mut all_paths: Vec<&Vec<EvidenceStep>> = vec![&raw.evidence_path];
        if let Some(extra) = &raw.additional_paths {
            for p in extra {
                all_paths.push(p);
            }
        }
        let non_empty: Vec<&Vec<EvidenceStep>> =
            all_paths.into_iter().filter(|p| !p.is_empty()).collect();
        if non_empty.is_empty() {
            None
        } else {
            Some(
                non_empty
                    .into_iter()
                    .map(|p| CodeFlow {
                        message: None,
                        thread_flows: vec![path_to_thread_flow(p)],
                    })
                    .collect(),
            )
        }
    };

    SarifResult {
        rule_id: summary.detector.clone(),
        level: sarif_level(&summary.severity),
        message: MessageText {
            text: format!("{} — {}", summary.title, summary.root_cause),
        },
        fingerprints: Fingerprints {
            al_sem_v1: summary.fingerprint.clone(),
        },
        locations: vec![ResultLocation {
            physical_location: PhysicalLocation {
                artifact_location: ArtifactLocation {
                    uri: summary.primary_location.file.clone(),
                },
                region: Region {
                    start_line: summary.primary_location.line,
                    start_column: summary.primary_location.column,
                },
            },
            logical_locations,
        }],
        code_flows,
    }
}

const INFORMATION_URI: &str = "https://github.com/SShadowS/al-sem";
const SCHEMA: &str = "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/master/Schemata/sarif-schema-2.1.0.json";

/// `formatSarif(result)` — render the projected findings + their raw `Finding`s as
/// SARIF 2.1.0. `summaries[i]` MUST correspond to `raws[i]` (same order). `version`
/// is the `driver.version` (e.g. the engine version, or the pinned "gate-sarif-v1").
///
/// Output is `serde_json::to_string_pretty(...)` — 2-space indent matching
/// `JSON.stringify(_, null, 2)`. The caller appends the trailing newline.
pub fn format_sarif(summaries: &[FindingSummary], raws: &[&Finding], version: &str) -> String {
    debug_assert_eq!(summaries.len(), raws.len());
    let results: Vec<SarifResult> = summaries
        .iter()
        .zip(raws.iter())
        .map(|(s, r)| build_result(s, r))
        .collect();

    let doc = SarifDocument {
        schema: SCHEMA,
        version: "2.1.0",
        runs: vec![Run {
            tool: Tool {
                driver: Driver {
                    name: "al-sem",
                    version: version.to_string(),
                    information_uri: INFORMATION_URI,
                    rules: rules(),
                },
            },
            results,
        }],
    };

    serde_json::to_string_pretty(&doc).expect("SARIF serialization cannot fail")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_mapping() {
        assert_eq!(sarif_level("critical"), "error");
        assert_eq!(sarif_level("high"), "error");
        assert_eq!(sarif_level("medium"), "warning");
        assert_eq!(sarif_level("low"), "note");
        assert_eq!(sarif_level("info"), "note");
        assert_eq!(sarif_level("bogus"), "warning");
    }

    /// Issue #87 (T1): the SARIF rule catalog is exactly the registry. Every
    /// registered detector has a rule and every rule names a registered
    /// detector; each rule carries its registry entry's name and description
    /// (both non-empty); ids are unique; and the order is natural detector
    /// number (`d2` before `d10`).
    #[test]
    fn rules_are_the_registry() {
        use crate::engine::l5::detectors::registered_detectors;
        use crate::engine::l5::registry::compare_natural;
        use std::collections::BTreeSet;

        let registry = registered_detectors();
        let rules = rules();
        let rule_ids: BTreeSet<&str> = rules.iter().map(|r| r.id.as_str()).collect();
        let reg_ids: BTreeSet<&str> = registry.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(rule_ids.len(), rules.len(), "duplicate rule ids");
        assert_eq!(
            rule_ids, reg_ids,
            "SARIF rules and registered detectors differ"
        );
        for r in &rules {
            let d = registry
                .iter()
                .find(|d| d.name == r.id)
                .expect("rule names a registered detector");
            assert_eq!(r.name, d.rule_name, "{}", r.id);
            assert_eq!(r.short_description.text, d.short_description, "{}", r.id);
            assert!(!d.rule_name.is_empty(), "{}: empty name", r.id);
            assert!(!d.short_description.is_empty(), "{}: empty text", r.id);
        }
        for w in rules.windows(2) {
            assert_eq!(
                compare_natural(&w[0].id, &w[1].id),
                std::cmp::Ordering::Less,
                "rules out of order: {} before {}",
                w[0].id,
                w[1].id
            );
        }
    }

    /// Issue #87 (T2): the 21 rules the catalog listed before it was derived
    /// from the registry keep their exact `(id, name, shortDescription)`, so a
    /// careless rewrite of an old description is caught.
    #[test]
    fn pre_existing_rules_are_unchanged() {
        const OLD: &[(&str, &str, &str)] = &[
            (
                "d1-db-op-in-loop",
                "DbOpInLoop",
                "Database operation inside a loop",
            ),
            (
                "d2-event-fanout-in-loop",
                "EventFanoutInLoop",
                "Event raised inside a loop with DB-touching subscribers",
            ),
            (
                "d3-missing-setloadfields",
                "MissingSetLoadFields",
                "Missing SetLoadFields before record retrieval",
            ),
            (
                "d4-repeated-lookup-in-loop",
                "RepeatedLookupInLoop",
                "Repeated identical lookup inside a loop",
            ),
            (
                "d5-set-based-opportunity",
                "SetBasedOpportunity",
                "Loop-and-Modify candidate for ModifyAll",
            ),
            (
                "d7-recursive-event-expansion",
                "RecursiveEventExpansion",
                "Event subscriber chain forms a cycle",
            ),
            (
                "d8-commit-in-transaction",
                "CommitInTransaction",
                "Commit inside a posting transaction span",
            ),
            (
                "d9-transaction-span-summary",
                "TransactionSpanSummary",
                "Transaction span summary (info)",
            ),
            (
                "d10-self-modifying-loop",
                "SelfModifyingLoop",
                "Self-modifying loop",
            ),
            (
                "d11-modify-without-get",
                "ModifyWithoutGet",
                "Modify without prior Get",
            ),
            (
                "d12-dead-integration-event",
                "DeadIntegrationEvent",
                "Integration event has no subscribers",
            ),
            (
                "d13-cross-app-internal-call",
                "CrossAppInternalCall",
                "Cross-extension call into an internal procedure",
            ),
            (
                "d14-dead-routine",
                "DeadRoutine",
                "Routine unreachable from any entry point",
            ),
            (
                "d16-obsolete-routine-call",
                "ObsoleteRoutineCall",
                "Call to an obsolete routine",
            ),
            (
                "d17-min-version-drift",
                "MinVersionDrift",
                "Call into API newer than declared MinVersion",
            ),
            (
                "d46-commit-in-lifecycle",
                "CommitInLifecycle",
                "Commit reachable from an Install/Upgrade codeunit trigger",
            ),
            (
                "d47-io-unsafe-txn",
                "IoUnsafeTxn",
                "External IO inside an open write transaction / before commit",
            ),
            (
                "d48-io-in-loop",
                "IoInLoop",
                "External IO (HTTP/FILE) inside a loop",
            ),
            (
                "d49-uncommitted-write-before-ui",
                "UncommittedWriteBeforeUi",
                "DB write pending at a window-opening UI call (BC runtime error)",
            ),
            (
                "d50-checked-run-implicit-commit",
                "CheckedRunImplicitCommit",
                "Checked Codeunit.Run implicit commit within a posting span (advisory)",
            ),
            (
                "d51-retry-side-effect-duplication",
                "RetrySideEffectDuplication",
                "Write-direction external request before an escaping error — may duplicate on retry (advisory)",
            ),
        ];
        let rules = rules();
        let mut last = None;
        for (id, name, text) in OLD {
            let pos = rules
                .iter()
                .position(|r| r.id == *id)
                .unwrap_or_else(|| panic!("{id} missing from rules()"));
            let r = &rules[pos];
            assert_eq!(r.name, *name, "{id} name");
            assert_eq!(r.short_description.text, *text, "{id} text");
            // Their relative order is unchanged.
            assert!(
                last.is_none_or(|l| l < pos),
                "{id} moved before its predecessor"
            );
            last = Some(pos);
        }
    }

    #[test]
    fn empty_findings_emits_empty_results_array() {
        let s = format_sarif(&[], &[], "gate-sarif-v1");
        assert!(s.contains("\"results\": []"));
        assert!(s.contains("\"version\": \"gate-sarif-v1\""));
    }
}
