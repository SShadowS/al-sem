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

use crate::engine::gate::policy::policy_types::{PolicyDoc, Predicate, Rule, RuleRunSummary};
use crate::engine::gate::policy::predicate_evaluator::{
    Tristate, evaluate_applicability, evaluate_result,
};
use crate::engine::gate::policy::predicate_fields::{
    FieldEvalContext, FieldIndexes, RuleFact, WriteFact,
};
use crate::engine::l4::capability_cone::CapabilityFact;
use crate::engine::l5::detectors::d65::{RuleKey, applied, table_key};
use crate::engine::l5::finding::{Evidence, Finding, FindingConfidence, SourceAnchor};
use crate::engine::l5::fingerprint::FingerprintIndex;
use crate::engine::l5::full_summary::FullRoutineSummary;
use crate::engine::l5::rules_index::{RulesIndex, WriteSite};
use crate::program::model::declaring::DeclaringObjects;
use crate::program::model::events::EventSymbol;
use crate::program::model::workspace::{ModelEntities, ModelRoutine, ModelTable};

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
/// The ONE consumer of RAW inherited `CapabilityFact` objects (it matches their
/// `op`/`resource_kind`/`resource_id`/`confidence`/… fields against the rule
/// predicate). `direct` is the routine's direct facts, `raw` its inherited ones:
/// the policy view's witness pass (#160 step 6 Task 3, `View::raw`).
fn select_facts<'a>(
    rule: &Rule,
    direct: &'a [CapabilityFact],
    raw: &'a [CapabilityFact],
) -> Vec<&'a CapabilityFact> {
    match rule.facts.as_deref().unwrap_or("any") {
        "direct" => direct.iter().collect(),
        "inherited" => raw.iter().collect(),
        // "any"
        _ => direct.iter().chain(raw).collect(),
    }
}

/// The model-input view the policy engine reads — built once per run from the
/// already-byte-parity model data. Owns the index maps + the per-routine summary
/// lookup + the fingerprint index.
pub struct PolicyModel<'a> {
    pub routines: &'a [ModelRoutine],
    pub field_indexes: FieldIndexes<'a>,
    pub summaries: &'a HashMap<String, FullRoutineSummary>,
    /// Every evaluated routine's raw inherited facts, by id (the view's
    /// `Cell::Raw` under `WitnessRequest::primary()`).
    pub raw: &'a HashMap<String, Vec<CapabilityFact>>,
    pub fingerprint_index: FingerprintIndex<'a>,
    /// The pins' read of the writer index: `Some` only for a policy with a
    /// pin ([`PolicyDoc::has_pins`]), whose view filled `substrate::RULES`.
    pub pins: Option<PinIndex<'a>>,
}

/// One writer of one field, as a pin reads it: the field's names, the
/// writer's writes of it, and the rule keys it applies (d65's `applied`).
pub struct PinFact<'a> {
    pub table_id: &'a str,
    pub table: &'a str,
    pub field: &'a str,
    pub sites: &'a [WriteSite],
    pub keys: Vec<RuleKey>,
}

/// The writer index as pins read it (rules-to-policy spec §2.1): per routine
/// (stable id), each field it writes, with the keys it applies there. The
/// keys are d65's, computed by d65's own `applied` over the same index d65
/// reads, so a pin and d65 agree on what a writer applies.
pub struct PinIndex<'a> {
    by_writer: HashMap<&'a str, Vec<PinFact<'a>>>,
    /// The cross-call walk was cut short: a writer with no matching key is
    /// unknown, not a violation.
    pub truncated: bool,
}

impl<'a> PinIndex<'a> {
    #[must_use]
    pub fn new(index: &'a RulesIndex) -> PinIndex<'a> {
        let mut by_writer: HashMap<&'a str, Vec<PinFact<'a>>> = HashMap::new();
        for f in &index.fields {
            let t = table_key(&f.table);
            for w in &f.writers {
                by_writer
                    .entry(w.stable_id.as_str())
                    .or_default()
                    .push(PinFact {
                        table_id: &f.table_id,
                        table: &f.table,
                        field: &f.field,
                        sites: &w.sites,
                        keys: applied(w, &t).into_keys().collect(),
                    });
            }
        }
        PinIndex {
            by_writer,
            truncated: index.truncated,
        }
    }

    /// The fields `routine` writes, in index order (table id, field).
    fn facts_of(&self, routine: &ModelRoutine) -> &[PinFact<'a>] {
        self.by_writer
            .get(routine.stable_routine_id.as_str())
            .map_or(&[], Vec::as_slice)
    }
}

impl<'a> PolicyModel<'a> {
    /// Build the model-input view. `event_names` is the (id → name) map for the
    /// `capability.resource.event.name` field; callers derive it from the event
    /// graph. `workspace` gives the object and table rows (`routines` is the
    /// evaluated subset); `declaring` is built over `workspace.objects`, the
    /// same index the caller gave `classify_roots` (#279: one per policy
    /// build).
    pub fn new(
        routines: &'a [ModelRoutine],
        workspace: &'a ModelEntities,
        declaring: DeclaringObjects<'a>,
        events: &'a [EventSymbol],
        root_classifications: &'a [crate::engine::root_classification::RootClassification],
        summaries: &'a HashMap<String, FullRoutineSummary>,
        raw: &'a HashMap<String, Vec<CapabilityFact>>,
    ) -> Self {
        // One classification per id (class P, #160 step 11b §4.6).
        let root_kinds_by_routine_id: HashMap<&str, &[String]> =
            crate::engine::root_classification::classification_per_id(
                root_classifications,
                routines,
                &declaring,
            )
            .into_iter()
            .map(|(id, rc)| (id, rc.kinds.as_slice()))
            .collect();
        // G-5: REAL table wins an id collision with a tableextension stub.
        let tables_by_id: HashMap<&str, &ModelTable> =
            crate::program::model::workspace::table_by_id_preferring_real(&workspace.tables);
        let events_by_id: HashMap<&str, &str> = events
            .iter()
            .map(|e| (e.id.as_str(), e.event_name.as_str()))
            .collect();
        let field_indexes = FieldIndexes {
            declaring,
            root_kinds_by_routine_id,
            tables_by_id,
            events_by_id,
        };
        let fingerprint_index = FingerprintIndex::build(routines, &workspace.objects);
        PolicyModel {
            routines,
            field_indexes,
            summaries,
            raw,
            fingerprint_index,
            pins: None,
        }
    }

    /// The model with the pins' read of `rules` (a policy with a pin; the
    /// view filled `substrate::RULES` for it).
    #[must_use]
    pub fn with_pins(self, rules: &'a RulesIndex) -> Self {
        PolicyModel {
            pins: Some(PinIndex::new(rules)),
            ..self
        }
    }
}

/// `runPolicyEngine(model, policy, {scope})`. Scope filtering is applied by the
/// caller-built `routines` list: `run_policy_check` passes the workspace's routines
/// only, so a cross-app model's dependency routines are never evaluated (#70) and
/// every counter counts workspace routines.
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
    // One evaluation per id (#160 step 11b §4.6): the rows of a collided id are
    // one routine, read through its LAST row (the stable sort keeps row order).
    sorted_routines.dedup_by(|later, kept| {
        let same = later.id == kept.id;
        if same {
            *kept = *later;
        }
        same
    });

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

        if let Some(requires) = &rule.requires {
            run_pin(
                rule,
                requires,
                unknown_policy,
                &sorted_routines,
                model,
                &mut summary,
                &mut rule_findings,
            );
            summary.findings_emitted = rule_findings.len();
            findings.extend(rule_findings);
            rule_summaries.push(summary);
            continue;
        }

        for routine in &sorted_routines {
            summary.routines_evaluated += 1;

            let summary_for_routine = model.summaries.get(routine.id.as_str());

            // Applicability skip.
            let app_ctx = FieldEvalContext {
                routine,
                fact: None,
                write: None,
                rule: None,
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
            let direct =
                summary_for_routine.map_or(&[][..], |s| s.capability_facts_direct.as_slice());
            // R6 (fail loud): a routine with a summary is a cone member, and
            // the witness pass gives every member in scope a raw entry (empty
            // when it inherits nothing). A missing entry is a scope that missed
            // an evaluated routine; reading it as "no inherited facts" would
            // let `facts: inherited|any` pass unseen. `direct` never reads it.
            let raw = match model.raw.get(routine.id.as_str()) {
                Some(v) => v.as_slice(),
                None if summary_for_routine.is_none()
                    || rule.facts.as_deref() == Some("direct") =>
                {
                    &[][..]
                }
                None => panic!(
                    "policy: routine {} has a summary but no raw inherited facts \
                     (the view's witness scope missed an evaluated routine)",
                    routine.id
                ),
            };
            let mut facts = select_facts(rule, direct, raw);
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
                    write: None,
                    rule: None,
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

/// One pin over the routines (rules-to-policy spec §2.2). Per routine, as a
/// fact rule: the applicability and structural `except` skips on its routine
/// fields; then per write fact (one field it writes), `when` and `except` in
/// full. A selected write is satisfied when some key the writer applies makes
/// `requires` true; a routine with an unsatisfied write violates the pin. No
/// coverage gate (the loader refuses `requireCoverage` on a pin: it is about
/// capability facts). When the writer index was cut short, a write with no
/// matching key is unknown, and `onUnknown` decides.
fn run_pin(
    rule: &Rule,
    requires: &Predicate,
    unknown_policy: &str,
    routines: &[&ModelRoutine],
    model: &PolicyModel,
    summary: &mut RuleRunSummary,
    out: &mut Vec<Finding>,
) {
    let pins = model.pins.as_ref().expect(
        "policy: a pin needs the writer index (declare substrate::RULES in the policy plan)",
    );
    for routine in routines {
        summary.routines_evaluated += 1;
        let base = FieldEvalContext {
            routine,
            fact: None,
            write: None,
            rule: None,
            indexes: &model.field_indexes,
        };
        if evaluate_applicability(&rule.when, &base) == Tristate::False {
            continue;
        }
        if let Some(except) = &rule.except
            && evaluate_applicability(except, &base) == Tristate::True
        {
            continue;
        }
        let mut violated: Vec<&PinFact> = Vec::new();
        let mut saw_unknown = false;
        for fact in pins.facts_of(routine) {
            let write = Some(WriteFact {
                table: fact.table,
                field: fact.field,
            });
            let wctx = FieldEvalContext { write, ..base };
            match evaluate_result(&rule.when, &wctx) {
                Tristate::False => continue,
                Tristate::Unknown => {
                    saw_unknown = true;
                    continue;
                }
                Tristate::True => {}
            }
            if let Some(except) = &rule.except
                && evaluate_result(except, &wctx) == Tristate::True
            {
                continue;
            }
            let mut satisfied = false;
            for k in &fact.keys {
                let rctx = FieldEvalContext {
                    rule: Some(RuleFact {
                        kind: k.kind,
                        atoms: &k.atoms,
                    }),
                    ..wctx
                };
                match evaluate_result(requires, &rctx) {
                    Tristate::True => {
                        satisfied = true;
                        break;
                    }
                    Tristate::Unknown => saw_unknown = true,
                    Tristate::False => {}
                }
            }
            if satisfied {
                continue;
            }
            if pins.truncated {
                saw_unknown = true;
            } else {
                violated.push(fact);
            }
        }
        if !violated.is_empty() {
            summary.routines_matched += 1;
            out.push(emit_pin_finding(rule, routine, model, &violated));
        } else if saw_unknown {
            summary.routines_skipped_unknown += 1;
            if unknown_policy == "fail-closed" {
                out.push(emit_unknown_finding(rule, routine, model));
            }
        } else {
            summary.routines_passed += 1;
        }
    }
}

/// A pin's finding: at the writer's first violating write (where d65
/// anchors), one evidence step per violating field.
fn emit_pin_finding(
    rule: &Rule,
    routine: &ModelRoutine,
    model: &PolicyModel,
    violated: &[&PinFact],
) -> Finding {
    let root_cause_key = format!("policy-{}/{}", rule.id, routine.id);
    let anchor_of =
        |f: &PinFact| crate::engine::l5::detectors::anchor_of(&f.sites[0].anchor, routine);
    let primary_location = anchor_of(violated[0]);
    let fields: Vec<String> = violated
        .iter()
        .map(|f| format!("{}.{}", f.table, f.field))
        .collect();
    let root_cause = rule.message.clone().unwrap_or_else(|| {
        format!(
            "{} writes {} without the rule this policy pins{}",
            routine.name,
            fields.join(", "),
            rule.description
                .as_deref()
                .map(|d| format!(": {d}"))
                .unwrap_or_default()
        )
    });
    let evidence = violated
        .iter()
        .map(|f| crate::engine::l5::finding::EvidenceStep {
            routine_id: routine.id.clone(),
            operation_id: None,
            callsite_id: None,
            loop_id: None,
            source_anchor: anchor_of(f),
            note: format!(
                "writes {}.{} ({}) without a rule that matches requires",
                f.table,
                f.field,
                f.sites[0].kind.as_str()
            ),
        })
        .collect();
    let mut tables: Vec<&str> = violated.iter().map(|f| f.table_id).collect();
    tables.dedup();
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
        affected_tables: tables.into_iter().map(Into::into).collect(),
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
        loose_subject: None,
    };
    finding.fingerprint = Some(model.fingerprint_index.fingerprint_of(&finding));
    finding
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
        loose_subject: None,
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
        loose_subject: None,
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
        loose_subject: None,
    };
    finding.fingerprint = Some(model.fingerprint_index.fingerprint_of(&finding));
    finding
}

#[cfg(test)]
#[path = "policy_facts_oracle_tests.rs"]
mod facts_oracle_tests;
