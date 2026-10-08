//! Policy engine — port of al-sem `src/policy/policy-engine.ts`.
//!
//! Per rule (sorted by id) × routine (scope-filtered, sorted by id):
//!   1. applicability skip (when→false ⇒ no fact can satisfy ⇒ skip).
//!   2. structural except skip (except→true regardless of facts ⇒ exempt).
//!   3. coverage gate (BEFORE facts).
//!   4. fact loop (per fact: when→true && except not true → matched; when→unknown
//!      → sawUnknown).
//!   5. after: matchedFacts>0 → match finding; else sawUnknown → onUnknown
//!      (fail-closed ⇒ unknown finding; fail-open ⇒ pass).
//!
//! The 3 finding variants reuse the engine's existing `fingerprint_of`.

use std::collections::HashMap;

use crate::engine::gate::policy::policy_types::{PolicyDoc, Rule, RuleRunSummary};
use crate::engine::gate::policy::predicate_evaluator::{
    Tristate, evaluate_applicability, evaluate_result,
};
use crate::engine::gate::policy::predicate_fields::{FieldEvalContext, FieldIndexes};
use crate::engine::l4::capability_cone::CapabilityFact;
use crate::engine::l5::finding::{Evidence, Finding, FindingConfidence, SourceAnchor};
use crate::engine::l5::fingerprint::FingerprintIndex;
use crate::engine::l5::full_summary::FullRoutineSummary;
use crate::program::model::events::EventSymbol;
use crate::program::model::workspace::{ModelObject, ModelRoutine, ModelTable};

/// The full policy-run result (sans envelope). al-sem
/// `Omit<PolicyRunResult, "policySource"|"policyVersion">` plus the version/source
/// carried by [`PolicyRunResult`].
pub struct PolicyRunResult {
    pub policy_source: String,
    pub policy_version: i64,
    pub rule_summaries: Vec<RuleRunSummary>,
    pub findings: Vec<Finding>,
    /// Policy-stage diagnostics (rule-threw warnings). Always empty for the corpus
    /// (no rule throws under the goldens), but carried for envelope parity.
    pub diagnostics: Vec<PolicyDiagnostic>,
}

/// al-sem `Diagnostic` (the subset the policy engine emits).
pub struct PolicyDiagnostic {
    pub severity: String,
    pub stage: String,
    pub message: String,
}

/// `pickCoverage(rule, doc)`.
fn pick_coverage<'a>(rule: &'a Rule, doc: &'a PolicyDoc) -> &'a str {
    rule.require_coverage
        .as_deref()
        .or_else(|| {
            doc.defaults
                .as_ref()
                .and_then(|d| d.require_coverage.as_deref())
        })
        .unwrap_or("any")
}

/// `pickUnknown(rule, doc)`.
fn pick_unknown<'a>(rule: &'a Rule, doc: &'a PolicyDoc) -> &'a str {
    rule.on_unknown
        .as_deref()
        .or_else(|| doc.defaults.as_ref().and_then(|d| d.on_unknown.as_deref()))
        .unwrap_or("fail-closed")
}

/// `passesCoverageGate(coverageStatus, gate)`. NOTE: `coverage_status` is `None`
/// when there is no summary/coverage record (al-sem `undefined`). For gate=partial,
/// al-sem returns `coverageStatus !== "unknown"` — and `undefined !== "unknown"` is
/// TRUE, so a missing coverage PASSES the partial gate.
fn passes_coverage_gate(coverage_status: Option<&str>, gate: &str) -> bool {
    match gate {
        "any" => true,
        "partial" => coverage_status != Some("unknown"),
        // "complete"
        _ => coverage_status == Some("complete"),
    }
}

/// `factSortKey(f)` — joins 8 fields with `|`, undefined→"".
fn fact_sort_key(f: &CapabilityFact) -> String {
    [
        f.op,
        f.resource_kind,
        f.resource_id.as_deref().unwrap_or(""),
        f.witness_operation_id.as_deref().unwrap_or(""),
        f.confidence,
        f.provenance,
        f.via,
        f.witness_callsite_id.as_deref().unwrap_or(""),
    ]
    .join("|")
}

/// Select facts for a rule's `facts` mode. al-sem `selectFacts`.
///
/// ⟨C1 Task 3⟩ This is the ONE consumer that genuinely needs the RAW inherited
/// `CapabilityFact` objects (it matches their `op`/`resource_kind`/`resource_id`/
/// `confidence`/… fields against the rule predicate), so the `policy` pipeline
/// builds its context with `substrate::ALL | substrate::RAW_INHERITED_FACTS`.
///
/// # Panics
/// The `inherited` / `any` modes panic (via `FullRoutineSummary::inherited_raw`)
/// when handed a context built WITHOUT that bit. Deliberate: silently returning
/// the direct-only subset would quietly under-report every `facts: any` rule on
/// every routine — a policy gate that passes because it never saw the evidence is
/// worse than one that fails loudly. `facts: direct` is unaffected: it never
/// reads the inherited half, so it stays correct under either mode.
fn select_facts<'a>(
    rule: &Rule,
    summary: Option<&'a FullRoutineSummary>,
) -> Vec<&'a CapabilityFact> {
    let Some(s) = summary else { return Vec::new() };
    match rule.facts.as_deref().unwrap_or("any") {
        "direct" => s.capability_facts_direct.iter().collect(),
        "inherited" => s.inherited_raw().iter().collect(),
        // "any"
        _ => s
            .capability_facts_direct
            .iter()
            .chain(s.inherited_raw())
            .collect(),
    }
}

/// The model-input view the policy engine reads — built once per run from the
/// already-byte-parity model data. Owns the index maps + the per-routine summary
/// lookup + the fingerprint index.
pub struct PolicyModel<'a> {
    pub routines: &'a [ModelRoutine],
    pub field_indexes: FieldIndexes<'a>,
    pub summaries: &'a HashMap<String, FullRoutineSummary>,
    pub fingerprint_index: FingerprintIndex<'a>,
}

impl<'a> PolicyModel<'a> {
    /// Build the model-input view. `event_names` is the (id → name) map for the
    /// `capability.resource.event.name` field; callers derive it from the event
    /// graph.
    pub fn new(
        routines: &'a [ModelRoutine],
        objects: &'a [ModelObject],
        tables: &'a [ModelTable],
        events: &'a [EventSymbol],
        root_classifications: &'a [crate::engine::root_classification::RootClassification],
        summaries: &'a HashMap<String, FullRoutineSummary>,
    ) -> Self {
        let objects_by_id: HashMap<&str, &ModelObject> =
            objects.iter().map(|o| (o.id.as_str(), o)).collect();
        let root_kinds_by_routine_id: HashMap<&str, &[String]> = root_classifications
            .iter()
            .map(|rc| (rc.routine_id.as_str(), rc.kinds.as_slice()))
            .collect();
        // G-5: REAL table wins an id collision with a tableextension stub.
        let tables_by_id: HashMap<&str, &ModelTable> =
            crate::program::model::workspace::table_by_id_preferring_real(tables);
        let events_by_id: HashMap<&str, &str> = events
            .iter()
            .map(|e| (e.id.as_str(), e.event_name.as_str()))
            .collect();
        let field_indexes = FieldIndexes {
            objects_by_id,
            root_kinds_by_routine_id,
            tables_by_id,
            events_by_id,
        };
        let fingerprint_index = FingerprintIndex::build(routines, objects);
        PolicyModel {
            routines,
            field_indexes,
            summaries,
            fingerprint_index,
        }
    }
}

/// `runPolicyEngine(model, policy, {scope})`. Scope filtering is applied by the
/// caller-built `routines` list (which already excludes dependency routines for
/// scope=primary); the policy fixtures are source-only so every routine is primary.
pub fn run_policy_engine(model: &PolicyModel, policy: &PolicyDoc) -> RunOutput {
    let mut findings: Vec<Finding> = Vec::new();
    let mut rule_summaries: Vec<RuleRunSummary> = Vec::new();
    // Always empty: al-sem pushes a `detect`-stage warning when a rule's predicate
    // evaluation THROWS, but Rust predicate evaluation is total (it cannot throw),
    // so this stays empty. Carried for envelope parity (`diagnostics: []`).
    let diagnostics: Vec<PolicyDiagnostic> = Vec::new();

    // Sort rules by id.
    let mut sorted_rules: Vec<&Rule> = policy.rules.iter().collect();
    sorted_rules.sort_by(|a, b| a.id.cmp(&b.id));

    // Sort routines by id.
    let mut sorted_routines: Vec<&ModelRoutine> = model.routines.iter().collect();
    sorted_routines.sort_by(|a, b| a.id.cmp(&b.id));

    for rule in &sorted_rules {
        let mut summary = RuleRunSummary {
            rule_id: rule.id.clone(),
            routines_evaluated: 0,
            routines_matched: 0,
            routines_skipped_coverage: 0,
            routines_skipped_unknown: 0,
            routines_passed: 0,
            findings_emitted: 0,
            errors: None,
        };
        let coverage_gate = pick_coverage(rule, policy);
        let unknown_policy = pick_unknown(rule, policy);
        let mut rule_findings: Vec<Finding> = Vec::new();

        for routine in &sorted_routines {
            summary.routines_evaluated += 1;

            let summary_for_routine = model.summaries.get(routine.id.as_str());

            // Applicability skip.
            let app_ctx = FieldEvalContext {
                routine,
                fact: None,
                indexes: &model.field_indexes,
            };
            if evaluate_applicability(&rule.when, &app_ctx) == Tristate::False {
                continue;
            }

            // Structural except skip.
            if let Some(except) = &rule.except
                && evaluate_applicability(except, &app_ctx) == Tristate::True
            {
                continue;
            }

            // Coverage gate (BEFORE facts).
            let coverage_status: Option<&str> = summary_for_routine
                .and_then(|s| s.coverage.as_ref().map(|c| c.inherited_status.as_str()));
            if !passes_coverage_gate(coverage_status, coverage_gate) {
                summary.routines_skipped_coverage += 1;
                if unknown_policy == "fail-closed" {
                    rule_findings.push(emit_coverage_finding(
                        rule,
                        routine,
                        model,
                        coverage_status.unwrap_or("unknown"),
                        coverage_gate,
                    ));
                }
                continue;
            }

            // Sorted facts. al-sem memoizes by (routineId, factsMode); the result is
            // output-identical to recomputing, so we recompute (the corpus is tiny).
            // Decorate-sort-undecorate so each sort key is computed once.
            let mut facts = select_facts(rule, summary_for_routine);
            facts.sort_by_cached_key(|f| fact_sort_key(f));
            if facts.is_empty() {
                summary.routines_passed += 1;
                continue;
            }

            let mut matched_facts: Vec<&CapabilityFact> = Vec::new();
            let mut saw_unknown = false;

            for fact in &facts {
                let ctx = FieldEvalContext {
                    routine,
                    fact: Some(fact),
                    indexes: &model.field_indexes,
                };
                let when_result = evaluate_result(&rule.when, &ctx);
                match when_result {
                    Tristate::False => continue,
                    Tristate::Unknown => {
                        saw_unknown = true;
                        continue;
                    }
                    Tristate::True => {
                        // except per-fact.
                        if let Some(except) = &rule.except
                            && evaluate_result(except, &ctx) == Tristate::True
                        {
                            continue;
                        }
                        matched_facts.push(fact);
                    }
                }
            }

            if !matched_facts.is_empty() {
                summary.routines_matched += 1;
                rule_findings.push(emit_match_finding(rule, routine, model, &matched_facts));
            } else if saw_unknown {
                summary.routines_skipped_unknown += 1;
                if unknown_policy == "fail-closed" {
                    rule_findings.push(emit_unknown_finding(rule, routine, model));
                }
            } else {
                summary.routines_passed += 1;
            }
        }

        summary.findings_emitted = rule_findings.len();
        findings.extend(rule_findings);
        rule_summaries.push(summary);
    }

    findings.sort_by(|a, b| a.id.cmp(&b.id));

    RunOutput {
        rule_summaries,
        findings,
        diagnostics,
    }
}

/// `run_policy_engine` output (sans envelope).
pub struct RunOutput {
    pub rule_summaries: Vec<RuleRunSummary>,
    pub findings: Vec<Finding>,
    pub diagnostics: Vec<PolicyDiagnostic>,
}

// ---------------------------------------------------------------------------
// Finding builders (3 variants).
// ---------------------------------------------------------------------------

/// `buildPrimaryLocation(routine)` — `{ ...routine.sourceAnchor, enclosingRoutineId }`.
/// The internal `SourceAnchor` carries the routine's own declaration anchor.
fn build_primary_location(routine: &ModelRoutine) -> SourceAnchor {
    let a = &routine.source_anchor;
    SourceAnchor {
        source_unit_id: a.source_unit_id.clone(),
        start_line: a.start_line,
        start_column: a.start_column,
        end_line: a.end_line,
        end_column: a.end_column,
        enclosing_routine_id: routine.id.clone(),
        syntax_kind: a.syntax_kind.clone(),
        normalized_text_hash: None,
        leading_context_hash: None,
        trailing_context_hash: None,
    }
}

fn emit_match_finding(
    rule: &Rule,
    routine: &ModelRoutine,
    model: &PolicyModel,
    matched: &[&CapabilityFact],
) -> Finding {
    let root_cause_key = format!("policy-{}/{}", rule.id, routine.id);
    let primary_location = build_primary_location(routine);

    let first_note = rule
        .message
        .clone()
        .or_else(|| rule.title.clone())
        .unwrap_or_else(|| format!("Policy rule {} violation", rule.id));

    let mut evidence = Vec::with_capacity(1 + matched.len());
    evidence.push(crate::engine::l5::finding::EvidenceStep {
        routine_id: routine.id.clone(),
        operation_id: None,
        callsite_id: None,
        loop_id: None,
        source_anchor: primary_location.clone(),
        note: first_note,
    });
    for fact in matched {
        let suffix = match &fact.resource_id {
            Some(rid) => format!(", resourceId={rid}"),
            None => String::new(),
        };
        evidence.push(crate::engine::l5::finding::EvidenceStep {
            routine_id: routine.id.clone(),
            operation_id: fact.witness_operation_id.clone(),
            callsite_id: fact.witness_callsite_id.clone(),
            loop_id: None,
            source_anchor: primary_location.clone(),
            note: format!(
                "matched on capability.op={}, resourceKind={}{}",
                fact.op, fact.resource_kind, suffix
            ),
        });
    }

    let root_cause = rule
        .message
        .clone()
        .or_else(|| rule.description.clone())
        .unwrap_or_else(|| {
            format!(
                "Policy {} matched {} fact(s) on routine {}.",
                rule.id,
                matched.len(),
                routine.id
            )
        });

    let mut finding = Finding {
        id: root_cause_key.clone(),
        root_cause_key,
        detector: format!("policy-{}", rule.id),
        title: rule.title.clone().unwrap_or_else(|| rule.id.clone()).into(),
        root_cause,
        severity: rule.severity.clone(),
        confidence: FindingConfidence {
            level: "likely".to_string(),
            capped_by: None,
            evidence: Vec::new(),
        },
        primary_location,
        evidence_path: evidence,
        additional_paths: None,
        affected_objects: vec![routine.object_id.as_str().into()],
        affected_tables: Vec::new(),
        fix_options: Vec::new(),
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
    finding.fingerprint = Some(model.fingerprint_index.fingerprint_of(&finding));
    finding
}

fn emit_coverage_finding(
    rule: &Rule,
    routine: &ModelRoutine,
    model: &PolicyModel,
    status: &str,
    effective_gate: &str,
) -> Finding {
    let root_cause_key = format!("policy-{}/{}", rule.id, routine.id);
    let primary_location = build_primary_location(routine);
    let mut finding = Finding {
        id: root_cause_key.clone(),
        root_cause_key,
        detector: format!("policy-{}", rule.id),
        title: rule.title.clone().unwrap_or_else(|| rule.id.clone()).into(),
        root_cause: format!(
            "Coverage gate (requireCoverage={effective_gate}) failed: routine coverage is {status}."
        ),
        severity: rule.severity.clone(),
        confidence: FindingConfidence {
            level: "possible".to_string(),
            capped_by: None,
            evidence: Vec::new(),
        },
        primary_location: primary_location.clone(),
        evidence_path: vec![crate::engine::l5::finding::EvidenceStep {
            routine_id: routine.id.clone(),
            operation_id: None,
            callsite_id: None,
            loop_id: None,
            source_anchor: primary_location,
            note: format!("coverage={status}"),
        }],
        additional_paths: None,
        affected_objects: vec![routine.object_id.as_str().into()],
        affected_tables: Vec::new(),
        fix_options: Vec::new(),
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
    finding.fingerprint = Some(model.fingerprint_index.fingerprint_of(&finding));
    finding
}

fn emit_unknown_finding(rule: &Rule, routine: &ModelRoutine, model: &PolicyModel) -> Finding {
    let root_cause_key = format!("policy-{}/{}", rule.id, routine.id);
    let primary_location = build_primary_location(routine);
    let mut finding = Finding {
        id: root_cause_key.clone(),
        root_cause_key,
        detector: format!("policy-{}", rule.id),
        title: rule.title.clone().unwrap_or_else(|| rule.id.clone()).into(),
        root_cause: "Predicate could not be resolved on any fact; onUnknown=fail-closed."
            .to_string(),
        severity: rule.severity.clone(),
        confidence: FindingConfidence {
            level: "possible".to_string(),
            capped_by: None,
            evidence: Vec::new(),
        },
        primary_location: primary_location.clone(),
        evidence_path: vec![crate::engine::l5::finding::EvidenceStep {
            routine_id: routine.id.clone(),
            operation_id: None,
            callsite_id: None,
            loop_id: None,
            source_anchor: primary_location,
            note: "policy unknown — fail-closed".to_string(),
        }],
        additional_paths: None,
        affected_objects: vec![routine.object_id.as_str().into()],
        affected_tables: Vec::new(),
        fix_options: Vec::new(),
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
    finding.fingerprint = Some(model.fingerprint_index.fingerprint_of(&finding));
    finding
}
