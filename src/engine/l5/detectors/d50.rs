//! D50 — Advisory (info), opt-in.
//!
//! A Codeunit.Run whose Boolean return value is USED (if-condition, assignment,
//! exit argument, etc.) performs an implicit commit on successful return. When a
//! transaction-managing routine sits in that implicit commit's span, flag it as
//! a possible mid-posting implicit commit.
//!
//! Port of al-sem `src/detectors/d50-checked-run-implicit-commit.ts`.
//!
//! ROUTINE-LEVEL approximation — may include writes not proven to precede the
//! implicit commit (§B.3). NOT refutation-grade.
//!
//! Consumes ONLY `CheckedRunImplicit` seeds from `transaction_spans`.
//! D8/D9 ignore those seeds; D50 ignores `ExplicitCommit` seeds.

use std::collections::{HashMap, HashSet};

use crate::engine::l3::al_attributes::has_attribute;
use crate::engine::l3::l3_workspace::{L3Object, L3Resolved, L3Routine};
use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::anchor_of;
use crate::engine::l5::finding::{
    Evidence, EvidenceStep, Finding, FixOption, SourceAnchor, id_list,
};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::engine::l5::transaction_spans::SeedKind;

const DETECTOR: &str = "d50-checked-run-implicit-commit";
const TRANSACTION_THRESHOLD_TABLES: usize = 3;

/// RootKind values that make a commit-chain root UNTRUSTED for D50's medium tier.
/// Mirrors al-sem's `D50_UNTRUSTED_ROOT_KINDS`.
const D50_UNTRUSTED_ROOT_KINDS: &[&str] = &[
    "event-subscriber",
    "install-codeunit",
    "upgrade-codeunit",
    "api-page",
    "public-procedure",
    "test-procedure",
    "trigger-table",
    "trigger-page",
    "report-trigger",
];

/// Hand-rolled `^(Post|Apply|Release)[A-Z]` check — mirrors al-sem's `POSTING_NAME_RE`.
fn posting_name_matches(name: &str) -> bool {
    for prefix in &["Post", "Apply", "Release"] {
        if let Some(rest) = name.strip_prefix(prefix)
            && let Some(next) = rest.chars().next()
            && next.is_ascii_uppercase()
        {
            return true;
        }
    }
    false
}

/// `isTransactionManaging(routineId)` — routine name matches POSTING_NAME_RE, OR
/// `writes_physical_tables_count_of(summary) >= TRANSACTION_THRESHOLD_TABLES`.
fn is_transaction_managing(routine_id: &str, ctx: &DetectorContext) -> bool {
    let Some(r) = ctx.routine_by_id.get(routine_id) else {
        return false;
    };
    if posting_name_matches(&r.name) {
        return true;
    }
    let Some(summary) = ctx.summaries.get(routine_id) else {
        return false;
    };
    // ⟨C1 Task 2 fix M2⟩ The count alone — avoids resolving and allocating one
    // `String` per table just to discard it. `is_transaction_managing` runs
    // per routine, so this matters at scale.
    //
    // ⟨issue 23⟩ The PHYSICAL count, not the temp-inclusive one: a write to a
    // `temporary` record never dirties the transaction, so counting temp writes
    // toward "this routine manages a transaction" admits routines that manage
    // none. d8 already gates on the physical count (`d8.rs:40-42`); this aligns
    // d50 with it. "Physical" means EXCLUDING known-temp facts — unknown,
    // parameter-dependent and absent temp states still count.
    ctx.cone_derived
        .writes_physical_tables_count_of(&summary.routine_id)
        >= TRANSACTION_THRESHOLD_TABLES
}

/// Returns true when the routine containing an explicit Commit() is eligible for the
/// D50 MEDIUM tier (its explicit Commit() is "proven effective"). Mirrors al-sem's
/// `isExplicitCommitProvenEffective`.
///
/// Four caps:
///   1. primary routine with available body.
///   2. root classification has NO untrusted RootKind.
///   3. routine does NOT carry [TryFunction].
///   4. effective CommitBehavior is "normal" (not "ignore" or "error").
fn is_explicit_commit_proven_effective(
    routine_id: &str,
    ctx: &DetectorContext,
    objects_by_id: &HashMap<&str, &L3Object>,
) -> bool {
    let Some(r) = ctx.routine_by_id.get(routine_id) else {
        return false;
    };

    // Cap 1: primary routine with available body.
    // Source-only: all routines are primary (no dep set). Check body_available.
    if !r.body_available {
        return false;
    }

    // Cap 2: root classification — no untrusted RootKind.
    if let Some(rc) = ctx.root_classifications_by_routine.get(routine_id) {
        for kind in &rc.kinds {
            if D50_UNTRUSTED_ROOT_KINDS.contains(&kind.as_str()) {
                return false;
            }
        }
    }

    // Cap 3: routine must NOT carry [TryFunction].
    let attrs = &r.attributes_parsed;
    if has_attribute(attrs, "TryFunction") {
        return false;
    }

    // Cap 4: effective CommitBehavior must be "normal".
    // Mirrors TS `attrs.find((a) => a.name === "CommitBehavior")` — CASE-SENSITIVE
    // exact match (NOT the shared case-insensitive find_attribute). This faithfully
    // mirrors al-sem's actual code even though a miscased attr name would be a
    // latent al-sem inconsistency.
    //
    // When the routine-level [CommitBehavior] attr IS present:
    //   - If args[0] is present → evaluate value; Ignore/Error → fail cap-4.
    //   - If args is empty → treated as "present but normal" (routine attr wins,
    //     object ICB is NOT consulted). Cap-4 passes.
    // Only when NO routine [CommitBehavior] attr is present → consult object ICB.
    // This mirrors TS d50.ts:133-151 exactly.
    let cb_attr = attrs.iter().find(|a| a.name == "CommitBehavior");
    if let Some(attr) = cb_attr {
        // Routine-level attr present — it wins over the object.
        if let Some(arg) = attr.args.first() {
            let val = if arg.kind == "qualified_enum_value" {
                arg.member.as_deref().unwrap_or("").to_lowercase()
            } else {
                arg.value
                    .as_deref()
                    .or(Some(arg.text.as_str()))
                    .unwrap_or("")
                    .to_lowercase()
            };
            if val == "ignore" || val == "error" {
                return false;
            }
        }
        // Argless [CommitBehavior] or non-suppressing value — cap-4 passes.
    } else {
        // No routine-level attr → object-level InherentCommitBehavior applies.
        let obj = objects_by_id.get(r.object_id.as_str()).copied();
        if let Some(o) = obj
            && let Some(icb) = &o.inherent_commit_behavior
            && (icb == "ignore" || icb == "error")
        {
            return false;
        }
    }

    true
}

/// Build a `SourceAnchor` from a routine's `PAnchor`.
fn routine_anchor(r: &L3Routine) -> SourceAnchor {
    anchor_of(&r.source_anchor, r)
}

pub fn detect_d50(
    resolved: &L3Resolved,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;

    // Build objects_by_id for CommitBehavior lookup (cap 4).
    let objects_by_id: HashMap<&str, &L3Object> =
        ws.objects.iter().map(|o| (o.id.as_str(), o)).collect();

    // Build routinesWithExplicitCommit from ExplicitCommit spans — O(1) membership.
    let mut routines_with_explicit_commit: HashSet<&str> = HashSet::new();
    for span in &ctx.transaction_spans {
        if span.seed_kind == SeedKind::ExplicitCommit {
            routines_with_explicit_commit.insert(span.commit_routine_id.as_str());
        }
    }

    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_other = 0u64;

    for span in &ctx.transaction_spans {
        // D50 only consumes CheckedRunImplicit seeds — D8/D9 own ExplicitCommit.
        if span.seed_kind != SeedKind::CheckedRunImplicit {
            continue;
        }
        let Some(seed_routine) = ctx.routine_by_id.get(span.commit_routine_id.as_str()) else {
            continue;
        };
        // Source-only: all routines are primary — no dep-set check needed.
        candidates_considered += 1;

        // managers = routinesInSpan filtered to isTransactionManaging (includes seed).
        let managers: Vec<&str> = span
            .routines_in_span
            .iter()
            .filter(|id| is_transaction_managing(id.as_str(), ctx))
            .map(|id| id.as_str())
            .collect();

        if managers.is_empty() {
            skipped_other += 1;
            continue;
        }

        let manager_id = managers[0];
        let Some(manager) = ctx.routine_by_id.get(manager_id) else {
            continue;
        };

        // Anchor: use the actual callsite anchor when available, else routine header.
        let callsite_anchor: SourceAnchor = if let Some(cs_id) = &span.seed_callsite_id {
            if let Some(cs) = ctx.call_site_by_id.get(cs_id.as_str()) {
                SourceAnchor {
                    source_unit_id: cs.source_anchor.source_unit_id.clone(),
                    start_line: cs.source_anchor.start_line,
                    start_column: cs.source_anchor.start_column,
                    end_line: cs.source_anchor.end_line,
                    end_column: cs.source_anchor.end_column,
                    enclosing_routine_id: seed_routine.id.clone(),
                    syntax_kind: cs.source_anchor.syntax_kind.clone(),
                    normalized_text_hash: None,
                    leading_context_hash: None,
                    trailing_context_hash: None,
                }
            } else {
                routine_anchor(seed_routine)
            }
        } else {
            routine_anchor(seed_routine)
        };

        // Build evidence path. Two shapes:
        //   Direct (managerId === seedRoutine.id):
        //     step 1 — manager routine header, note: "transaction-managing routine {name} contains the implicit-commit callsite"
        //     step 2 — seed routine's callsite, note: "Codeunit.Run implicit commit (on success) — return value consumed"
        //   Indirect (different routines):
        //     step 1 — manager routine header, note: "transaction-managing routine: {name}"
        //     step 2 — seed routine's callsite, note: "Codeunit.Run implicit commit (on success) in {name} — return value consumed"
        let evidence_path: Vec<EvidenceStep> = if manager_id == seed_routine.id.as_str() {
            vec![
                EvidenceStep {
                    routine_id: manager_id.to_string(),
                    operation_id: None,
                    callsite_id: None,
                    loop_id: None,
                    source_anchor: routine_anchor(manager),
                    note: format!(
                        "transaction-managing routine {} contains the implicit-commit callsite",
                        manager.name
                    ),
                },
                EvidenceStep {
                    routine_id: seed_routine.id.clone(),
                    operation_id: None,
                    callsite_id: None,
                    loop_id: None,
                    source_anchor: callsite_anchor.clone(),
                    note: "Codeunit.Run implicit commit (on success) — return value consumed"
                        .to_string(),
                },
            ]
        } else {
            vec![
                EvidenceStep {
                    routine_id: manager_id.to_string(),
                    operation_id: None,
                    callsite_id: None,
                    loop_id: None,
                    source_anchor: routine_anchor(manager),
                    note: format!("transaction-managing routine: {}", manager.name),
                },
                EvidenceStep {
                    routine_id: seed_routine.id.clone(),
                    operation_id: None,
                    callsite_id: None,
                    loop_id: None,
                    source_anchor: callsite_anchor.clone(),
                    note: format!(
                        "Codeunit.Run implicit commit (on success) in {} — return value consumed",
                        seed_routine.name
                    ),
                },
            ]
        };

        // §0.5 MEDIUM tier: escalate when a proven-effective EXPLICIT Commit() is
        // on the same checked-run-implicit span.
        let has_proven_effective_explicit_commit = span.routines_in_span.iter().any(|rid| {
            routines_with_explicit_commit.contains(rid.as_str())
                && is_explicit_commit_proven_effective(rid.as_str(), ctx, &objects_by_id)
        });
        let severity = if has_proven_effective_explicit_commit {
            "medium"
        } else {
            "info"
        };

        let base_root_cause = format!(
            "A Codeunit.Run whose Boolean return value is used in {} performs an implicit commit \
             on successful return while {} (a transaction-managing routine) is reachable in the \
             routine-level implicit-commit span. Routine-level approximation — may include writes \
             not proven to precede the implicit commit. Review whether the implicit commit splits \
             a posting transaction.",
            seed_routine.name, manager.name
        );
        let root_cause = if severity == "medium" {
            format!(
                "{} A proven-effective explicit Commit() is also on the span — committed writes \
                 may persist partial state before the implicit-commit boundary.",
                base_root_cause
            )
        } else {
            base_root_cause
        };

        // affectedObjects: [seedRoutine.objectId, manager.objectId].sort()
        let mut affected_objects = vec![seed_routine.object_id.clone(), manager.object_id.clone()];
        affected_objects.sort();

        let id = format!("d50/{}", span.commit_operation_id);
        let root_cause_key = id.clone();

        let mut finding = Finding {
            id,
            root_cause_key,
            detector: DETECTOR.to_string(),
            title: "Codeunit.Run implicit commit within a posting span".into(),
            root_cause,
            severity: severity.to_string(),
            confidence: to_confidence(&[], "possible"),
            primary_location: callsite_anchor,
            evidence_path,
            additional_paths: None,
            affected_objects: id_list(affected_objects),
            affected_tables: id_list(span.writes_tables.clone()),
            fix_options: vec![FixOption {
                description:
                    "If atomicity matters, avoid the checked Run mid-transaction or restructure \
                     so the posting completes before the implicit commit."
                        .into(),
                safety: "low".into(),
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

    // Dedupe by id (same checked-Run callsite → one finding).
    findings.sort_by(|a, b| a.id.cmp(&b.id));
    let mut seen: HashSet<String> = HashSet::new();
    let mut emitted: Vec<Finding> = Vec::new();
    for f in findings {
        if seen.insert(f.id.clone()) {
            emitted.push(f);
        }
    }

    let count = emitted.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates_considered, count);
    stats.add_skip("other", skipped_other);
    Ok(DetectorOutput {
        findings: emitted,
        stats,
        diagnostics: vec![],
        d1_cohort_index: None,
    })
}

// ===========================================================================
// Native oracles — D50 MEDIUM tier + cap-4 faithfulness.
//
// These tests cover the CORPUS-UNEXECUTED medium tier and the FIX-1
// cap-4 case-sensitivity / argless-fallthrough edge cases. The 2 corpus
// goldens (ws-d50-pos, ws-d50-neg) are INFO-only and never exercise any of
// this path; the oracles below are the only execution of that code.
// ===========================================================================

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap};

    use super::*;
    use crate::engine::l3::al_attributes::{AttributeArg, AttributeInfo};
    use crate::engine::l3::event_graph::EventGraph;
    use crate::engine::l3::l3_workspace::{L3Object, L3Resolved, L3Routine, L3Workspace};
    use crate::engine::l4::capability_cone::{CapabilityExtra, CapabilityFact};
    use crate::engine::l4::combined_graph::CombinedGraph;
    use crate::engine::l5::detector_context::DetectorContext;
    use crate::engine::l5::event_flow::EventFlowIndexes;
    use crate::engine::l5::full_summary::FullRoutineSummary;
    use crate::engine::l5::test_support::{cone_store_of, fact, routine, summary, ts_known};
    use crate::engine::root_classification::RootClassification;

    // -----------------------------------------------------------------------
    // Attribute / object constructors
    // -----------------------------------------------------------------------

    /// Build a `[CommitBehavior(CommitBehavior::Xxx)]` attribute (qualified form).
    fn cb_attr_qualified(member: &str) -> AttributeInfo {
        AttributeInfo {
            name: "CommitBehavior".to_string(),
            args: vec![AttributeArg {
                kind: "qualified_enum_value".to_string(),
                text: format!("CommitBehavior::{member}"),
                value: None,
                qualifier: Some("CommitBehavior".to_string()),
                member: Some(member.to_string()),
            }],
            raw: format!("[CommitBehavior(CommitBehavior::{member})]"),
        }
    }

    /// Build a `[CommitBehavior(Ignore)]` attribute (plain identifier / ABI "value" path).
    fn cb_attr_plain(value: &str) -> AttributeInfo {
        AttributeInfo {
            name: "CommitBehavior".to_string(),
            args: vec![AttributeArg {
                kind: "identifier".to_string(),
                text: value.to_string(),
                value: Some(value.to_string()),
                qualifier: None,
                member: None,
            }],
            raw: format!("[CommitBehavior({value})]"),
        }
    }

    /// Build a `[CommitBehavior]` attribute with NO args (the argless form).
    fn cb_attr_argless() -> AttributeInfo {
        AttributeInfo {
            name: "CommitBehavior".to_string(),
            args: vec![],
            raw: "[CommitBehavior]".to_string(),
        }
    }

    /// Build a MISCASED `[Commitbehavior(Ignore)]` attribute.
    fn cb_attr_miscased_ignore() -> AttributeInfo {
        AttributeInfo {
            name: "Commitbehavior".to_string(), // wrong case
            args: vec![AttributeArg {
                kind: "qualified_enum_value".to_string(),
                text: "CommitBehavior::Ignore".to_string(),
                value: None,
                qualifier: Some("CommitBehavior".to_string()),
                member: Some("Ignore".to_string()),
            }],
            raw: "[Commitbehavior(CommitBehavior::Ignore)]".to_string(),
        }
    }

    /// Build a `[TryFunction]` attribute.
    fn try_fn_attr() -> AttributeInfo {
        AttributeInfo {
            name: "TryFunction".to_string(),
            args: vec![],
            raw: "[TryFunction]".to_string(),
        }
    }

    /// A minimal `L3Object` with the given id + optional InherentCommitBehavior.
    fn mk_object(id: &str, icb: Option<&str>) -> L3Object {
        L3Object {
            id: id.to_string(),
            app_guid: "app".to_string(),
            object_type: "Codeunit".to_string(),
            object_number: 1,
            name: "TestCU".to_string(),
            source_table_name: None,
            extends_target_name: None,
            implements_interfaces: None,
            object_subtype: None,
            page_type: None,
            inherent_commit_behavior: icb.map(|s| s.to_string()),
            source_table_temporary: None,
            page_controls: Vec::new(),
            single_instance: None,
            editable: None,
            insert_allowed: None,
            modify_allowed: None,
            delete_allowed: None,
            source_anchor: None,
        }
    }

    /// A minimal `RootClassification` with the given kinds.
    fn mk_root_class(routine_id: &str, kinds: &[&str]) -> RootClassification {
        RootClassification {
            routine_id: routine_id.to_string(),
            kinds: kinds.iter().map(|k| k.to_string()).collect(),
            externally_reachable: true,
            source: "ast".to_string(),
            confidence: "static".to_string(),
            config_entry_id: None,
            resolution_status: None,
        }
    }

    /// Build a minimal `DetectorContext` that is sufficient for
    /// `is_explicit_commit_proven_effective` — only `routine_by_id` and
    /// `root_classifications_by_routine` are populated; all other fields are
    /// empty / default.
    ///
    /// Lifetimes: `routines` and `objects` must outlive the returned ctx.
    fn minimal_ctx<'a>(
        routines: &'a [crate::engine::l3::l3_workspace::L3Routine],
        root_classes: Vec<RootClassification>,
    ) -> DetectorContext<'a> {
        let routine_by_id: HashMap<&'a str, &'a crate::engine::l3::l3_workspace::L3Routine> =
            routines.iter().map(|r| (r.id.as_str(), r)).collect();

        let root_classifications_by_routine: HashMap<String, RootClassification> = root_classes
            .into_iter()
            .map(|rc| (rc.routine_id.clone(), rc))
            .collect();

        let empty_graph = CombinedGraph {
            nodes: vec![],
            edges_by_from: HashMap::new(),
            edges_from_order: vec![],
            uncertainty_edges: vec![],
            typed_edges: vec![],
        };

        DetectorContext {
            graph: empty_graph,
            event_graph: EventGraph {
                events: vec![],
                edges: vec![],
            },
            routine_by_id,
            objects_by_id: HashMap::new(),
            table_by_id: HashMap::new(),
            reverse_call_graph: std::collections::BTreeMap::new(),
            entry_points: BTreeSet::new(),
            transaction_spans: vec![],
            resolved_call_edge_by_callsite: HashMap::new(),
            uncertainty_edges_by_from: HashMap::new(),
            uncertainties_by_node: HashMap::new(),
            uncertainties: Default::default(),
            call_site_by_id: HashMap::new(),
            summaries: HashMap::new(),
            // ⟨C1⟩ No summaries ⇒ no derived cone rows.
            cone_derived: Default::default(),
            event_flow_indexes: EventFlowIndexes::default(),
            parameter_roles_by_routine: HashMap::new(),
            upgraded_bindings_by_callsite: HashMap::new(),
            reachable_roots: BTreeSet::new(),
            internal_reachable_externally: false,
            dep_routine_ids: BTreeSet::new(),
            declared_dependencies: Vec::new(),
            app_versions: HashMap::new(),
            root_classifications_by_routine,
            ordering_facts: std::sync::OnceLock::new(),
            ordering_source: None,
            closed_world_temp_params: Default::default(),
            summarize_diagnostics: Vec::new(),
            db_effect_bundle: None,
            reverse_effect_index: None,
            fingerprint_index: crate::engine::l5::fingerprint::FingerprintIndex::build(
                routines,
                &[],
            ),
            cross_extension_subscribers: std::collections::BTreeMap::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Oracle 1: all caps pass → proven_effective TRUE + detect_d50 → MEDIUM.
    //
    // THE PRIORITY: exercises the corpus-unexecuted medium-tier escalation path
    // end-to-end at the detector level.
    //
    // Strategy: hand-build a DetectorContext with pre-populated transaction_spans
    // so we avoid the full call-resolution pipeline.
    //
    //   - r_seed: "RunWorkerChecked" — the CheckedRunImplicit seed routine
    //   - r_manager: "PostDocument" — the transaction manager (name matches Post[A-Z])
    //   - r_committer: "CommitWorker" — has an explicit commit
    //   - CheckedRunImplicit span: seed_kind=CheckedRunImplicit, commit_routine_id=r_seed,
    //     routines_in_span=[r_seed, r_manager, r_committer] (all three are in span)
    //   - ExplicitCommit span: commit_routine_id=r_committer (populates routinesWithExplicitCommit)
    //   - is_explicit_commit_proven_effective(r_committer) = TRUE (all defaults: body_available,
    //     no untrusted root, no TryFunction, no CommitBehavior attr, no object ICB)
    //   → severity MEDIUM + rootCause contains the medium-extra suffix.
    // -----------------------------------------------------------------------
    #[test]
    fn all_caps_pass_medium_severity_at_detector_level() {
        use crate::engine::l2::features::PAnchor;
        use crate::engine::l5::transaction_spans::{SeedKind, TransactionSpan};

        let dummy_anchor = PAnchor {
            source_unit_id: "ws:src/T.al".to_string(),
            start_line: 1,
            start_column: 0,
            end_line: 5,
            end_column: 0,
            syntax_kind: "procedure".to_string(),
        };

        // r_seed: the routine containing the checked Codeunit.Run callsite.
        let mut r_seed = routine("seed", "procedure");
        r_seed.name = "RunWorkerChecked".to_string();
        r_seed.source_anchor = dummy_anchor.clone();

        // r_manager: the transaction-managing routine (name matches Post[A-Z]).
        let mut r_manager = routine("manager", "procedure");
        r_manager.name = "PostDocument".to_string();
        r_manager.source_anchor = dummy_anchor.clone();

        // r_committer: the routine with an explicit Commit().
        // All caps pass by default: body_available=true, no untrusted root,
        // no TryFunction, no CommitBehavior, no object ICB.
        let mut r_committer = routine("committer", "procedure");
        r_committer.name = "CommitWorker".to_string();
        r_committer.source_anchor = dummy_anchor.clone();
        r_committer.object_id = "obj-committer".to_string();

        let routines_slice = vec![r_seed.clone(), r_manager.clone(), r_committer.clone()];

        // Build a minimal object for r_committer (no ICB → cap-4 passes).
        // detect_d50 builds its OWN objects_by_id from resolved.workspace.objects,
        // so we provide the object there (in ws_objects below).
        let obj_committer = mk_object("obj-committer", None);
        let ws_objects = vec![obj_committer.clone()];

        // Manually build the DetectorContext, pre-populating only the fields
        // detect_d50 actually reads:
        //   - transaction_spans (pre-built below)
        //   - routine_by_id
        //   - root_classifications_by_routine (empty → all caps pass for r_committer)
        //   - call_site_by_id (empty → fallback to routine header anchor)
        //   - summaries (empty → is_transaction_managing falls back to name match)
        let routine_by_id: HashMap<&str, &crate::engine::l3::l3_workspace::L3Routine> = [
            (r_seed.id.as_str(), &r_seed),
            (r_manager.id.as_str(), &r_manager),
            (r_committer.id.as_str(), &r_committer),
        ]
        .into();

        // Pre-build the transaction spans:
        //   1. CheckedRunImplicit span: seed=r_seed, routines_in_span includes all 3.
        //   2. ExplicitCommit span: seed=r_committer (puts it in routinesWithExplicitCommit).
        let spans = vec![
            TransactionSpan {
                seed_kind: SeedKind::CheckedRunImplicit,
                commit_operation_id: "seed/cs0".to_string(),
                seed_callsite_id: Some("seed/cs0".to_string()),
                commit_routine_id: "seed".to_string(),
                routines_in_span: vec![
                    "committer".to_string(),
                    "manager".to_string(),
                    "seed".to_string(),
                ],
                writes_tables: vec![],
                publishes_events: vec![],
                span_roots: vec!["manager".to_string()],
                coverage_complete: false,
            },
            TransactionSpan {
                seed_kind: SeedKind::ExplicitCommit,
                commit_operation_id: "committer/op-commit".to_string(),
                seed_callsite_id: None,
                commit_routine_id: "committer".to_string(),
                routines_in_span: vec!["committer".to_string()],
                writes_tables: vec![],
                publishes_events: vec![],
                span_roots: vec!["committer".to_string()],
                coverage_complete: false,
            },
        ];

        let empty_graph = CombinedGraph {
            nodes: vec![],
            edges_by_from: HashMap::new(),
            edges_from_order: vec![],
            uncertainty_edges: vec![],
            typed_edges: vec![],
        };

        let ctx = DetectorContext {
            graph: empty_graph,
            event_graph: EventGraph {
                events: vec![],
                edges: vec![],
            },
            routine_by_id,
            objects_by_id: HashMap::new(), // detect_d50 builds its own from ws.objects
            table_by_id: HashMap::new(),
            reverse_call_graph: std::collections::BTreeMap::new(),
            entry_points: BTreeSet::new(),
            transaction_spans: spans,
            resolved_call_edge_by_callsite: HashMap::new(),
            uncertainty_edges_by_from: HashMap::new(),
            uncertainties_by_node: HashMap::new(),
            uncertainties: Default::default(),
            call_site_by_id: HashMap::new(),
            summaries: HashMap::new(),
            // ⟨C1⟩ No summaries ⇒ no derived cone rows.
            cone_derived: Default::default(),
            event_flow_indexes: EventFlowIndexes::default(),
            parameter_roles_by_routine: HashMap::new(),
            upgraded_bindings_by_callsite: HashMap::new(),
            reachable_roots: BTreeSet::new(),
            internal_reachable_externally: false,
            dep_routine_ids: BTreeSet::new(),
            declared_dependencies: Vec::new(),
            app_versions: HashMap::new(),
            root_classifications_by_routine: HashMap::new(),
            ordering_facts: std::sync::OnceLock::new(),
            ordering_source: None,
            closed_world_temp_params: Default::default(),
            summarize_diagnostics: Vec::new(),
            db_effect_bundle: None,
            reverse_effect_index: None,
            // Borrows routines_slice/ws_objects (not yet moved — resolved.workspace
            // below is built from CLONES of both so this borrow stays valid for the
            // ctx's lifetime through the detect_d50 call).
            fingerprint_index: crate::engine::l5::fingerprint::FingerprintIndex::build(
                &routines_slice,
                &ws_objects,
            ),
            cross_extension_subscribers: std::collections::BTreeMap::new(),
        };

        // Build a minimal L3Resolved for detect_d50 (it uses ws.routines and ws.objects
        // to build its objects_by_id map internally). Clones of routines_slice/ws_objects
        // — the originals stay borrowed by ctx.fingerprint_index above.
        let resolved = L3Resolved {
            workspace: L3Workspace {
                objects: ws_objects.clone(),
                tables: vec![],
                routines: routines_slice.clone(),
            },
            root_classifications: vec![],
            primary_app: None,
            infra_diagnostics: Vec::new(),
        };

        let output = detect_d50(&resolved, &ctx).unwrap();

        // Must produce exactly one finding with severity MEDIUM.
        assert_eq!(
            output.findings.len(),
            1,
            "expected 1 finding; got: {:#?}",
            output.findings
        );
        let f = &output.findings[0];
        assert_eq!(
            f.severity, "medium",
            "severity must be MEDIUM when proven-effective explicit commit is in span; got: {}",
            f.severity
        );
        // rootCause must contain the medium-extra partial-state suffix.
        assert!(
            f.root_cause
                .contains("A proven-effective explicit Commit() is also on the span"),
            "rootCause must contain the medium-extra suffix; got: {}",
            f.root_cause
        );
    }

    // -----------------------------------------------------------------------
    // Oracle 2: cap1 body_available=false → proven_effective FALSE (→ info).
    // -----------------------------------------------------------------------
    #[test]
    fn cap1_body_not_available_returns_false() {
        let mut r = routine("r1", "procedure");
        r.body_available = false;
        let routines = vec![r];
        let ctx = minimal_ctx(&routines, vec![]);
        let objects: HashMap<&str, &L3Object> = HashMap::new();
        assert!(!is_explicit_commit_proven_effective("r1", &ctx, &objects));
    }

    // -----------------------------------------------------------------------
    // Oracle 3: cap2 root has untrusted kind → FALSE.
    //   3a: public-procedure
    //   3b: event-subscriber
    // -----------------------------------------------------------------------
    #[test]
    fn cap2_untrusted_root_public_procedure_returns_false() {
        let r = routine("r2a", "procedure");
        let routines = vec![r];
        let rc = mk_root_class("r2a", &["public-procedure"]);
        let ctx = minimal_ctx(&routines, vec![rc]);
        let objects: HashMap<&str, &L3Object> = HashMap::new();
        assert!(!is_explicit_commit_proven_effective("r2a", &ctx, &objects));
    }

    #[test]
    fn cap2_untrusted_root_event_subscriber_returns_false() {
        let r = routine("r2b", "procedure");
        let routines = vec![r];
        let rc = mk_root_class("r2b", &["event-subscriber"]);
        let ctx = minimal_ctx(&routines, vec![rc]);
        let objects: HashMap<&str, &L3Object> = HashMap::new();
        assert!(!is_explicit_commit_proven_effective("r2b", &ctx, &objects));
    }

    // -----------------------------------------------------------------------
    // Oracle 4: cap3 [TryFunction] → FALSE.
    // -----------------------------------------------------------------------
    #[test]
    fn cap3_try_function_returns_false() {
        let mut r = routine("r3", "procedure");
        r.attributes_parsed = vec![try_fn_attr()];
        let routines = vec![r];
        let ctx = minimal_ctx(&routines, vec![]);
        let objects: HashMap<&str, &L3Object> = HashMap::new();
        assert!(!is_explicit_commit_proven_effective("r3", &ctx, &objects));
    }

    // -----------------------------------------------------------------------
    // Oracle 5: cap4 routine [CommitBehavior(CommitBehavior::Ignore)] qualified → FALSE;
    //           routine [CommitBehavior(CommitBehavior::Error)] qualified → FALSE.
    // -----------------------------------------------------------------------
    #[test]
    fn cap4_qualified_ignore_returns_false() {
        let mut r = routine("r4a", "procedure");
        r.attributes_parsed = vec![cb_attr_qualified("Ignore")];
        r.object_id = "obj-4a".to_string();
        let routines = vec![r];
        let ctx = minimal_ctx(&routines, vec![]);
        let obj = mk_object("obj-4a", None);
        let objects: HashMap<&str, &L3Object> = [("obj-4a", &obj)].into();
        assert!(!is_explicit_commit_proven_effective("r4a", &ctx, &objects));
    }

    #[test]
    fn cap4_qualified_error_returns_false() {
        let mut r = routine("r4b", "procedure");
        r.attributes_parsed = vec![cb_attr_qualified("Error")];
        r.object_id = "obj-4b".to_string();
        let routines = vec![r];
        let ctx = minimal_ctx(&routines, vec![]);
        let obj = mk_object("obj-4b", None);
        let objects: HashMap<&str, &L3Object> = [("obj-4b", &obj)].into();
        assert!(!is_explicit_commit_proven_effective("r4b", &ctx, &objects));
    }

    // -----------------------------------------------------------------------
    // Oracle 6: cap4 bare-ABI [CommitBehavior(Ignore)] (value/text path) → FALSE.
    // -----------------------------------------------------------------------
    #[test]
    fn cap4_abi_value_ignore_returns_false() {
        let mut r = routine("r4c", "procedure");
        r.attributes_parsed = vec![cb_attr_plain("Ignore")];
        r.object_id = "obj-4c".to_string();
        let routines = vec![r];
        let ctx = minimal_ctx(&routines, vec![]);
        let obj = mk_object("obj-4c", None);
        let objects: HashMap<&str, &L3Object> = [("obj-4c", &obj)].into();
        assert!(!is_explicit_commit_proven_effective("r4c", &ctx, &objects));
    }

    // -----------------------------------------------------------------------
    // Oracle 7: cap4 NO routine attr + object ICB "ignore" → FALSE;
    //           + object ICB "error" → FALSE.
    // -----------------------------------------------------------------------
    #[test]
    fn cap4_no_routine_attr_object_icb_ignore_returns_false() {
        let mut r = routine("r4d", "procedure");
        r.object_id = "obj-4d".to_string();
        let routines = vec![r];
        let ctx = minimal_ctx(&routines, vec![]);
        let obj = mk_object("obj-4d", Some("ignore"));
        let objects: HashMap<&str, &L3Object> = [("obj-4d", &obj)].into();
        assert!(!is_explicit_commit_proven_effective("r4d", &ctx, &objects));
    }

    #[test]
    fn cap4_no_routine_attr_object_icb_error_returns_false() {
        let mut r = routine("r4e", "procedure");
        r.object_id = "obj-4e".to_string();
        let routines = vec![r];
        let ctx = minimal_ctx(&routines, vec![]);
        let obj = mk_object("obj-4e", Some("error"));
        let objects: HashMap<&str, &L3Object> = [("obj-4e", &obj)].into();
        assert!(!is_explicit_commit_proven_effective("r4e", &ctx, &objects));
    }

    // -----------------------------------------------------------------------
    // Oracle 8 (FIX 1 regression): cap4 argless [CommitBehavior] (no args) +
    // object ICB "ignore" → proven_effective TRUE.
    //
    // Encodes TS behavior: routine attr IS present → wins → object ICB NOT consulted
    // → no suppression → cap-4 PASSES (routine attr with no args is treated as
    // "normal" override, not a fall-through to object ICB).
    // -----------------------------------------------------------------------
    #[test]
    fn fix1_argless_cb_attr_wins_over_object_icb_ignore_returns_true() {
        let mut r = routine("r_fix1a", "procedure");
        r.attributes_parsed = vec![cb_attr_argless()]; // [CommitBehavior] — argless
        r.object_id = "obj-fix1a".to_string();
        let routines = vec![r];
        let ctx = minimal_ctx(&routines, vec![]);
        let obj = mk_object("obj-fix1a", Some("ignore")); // ICB = ignore, but irrelevant
        let objects: HashMap<&str, &L3Object> = [("obj-fix1a", &obj)].into();
        // FIX 1b: routine attr present → object ICB NOT consulted → TRUE.
        assert!(
            is_explicit_commit_proven_effective("r_fix1a", &ctx, &objects),
            "argless [CommitBehavior] + object ICB ignore must return TRUE (routine attr wins)"
        );
    }

    // -----------------------------------------------------------------------
    // Oracle 9 (FIX 1 regression): cap4 miscased [Commitbehavior(Ignore)] +
    // object ICB absent → proven_effective TRUE.
    //
    // Encodes TS behavior: case-sensitive match misses the routine attr (name is
    // "Commitbehavior" not "CommitBehavior") → falls through to object ICB → absent
    // → cap-4 PASSES → TRUE.
    // -----------------------------------------------------------------------
    #[test]
    fn fix1_miscased_cb_attr_not_matched_no_object_icb_returns_true() {
        let mut r = routine("r_fix1b", "procedure");
        r.attributes_parsed = vec![cb_attr_miscased_ignore()]; // [Commitbehavior(...)] — miscased
        r.object_id = "obj-fix1b".to_string();
        let routines = vec![r];
        let ctx = minimal_ctx(&routines, vec![]);
        let obj = mk_object("obj-fix1b", None); // no object ICB
        let objects: HashMap<&str, &L3Object> = [("obj-fix1b", &obj)].into();
        // FIX 1a: case-sensitive match misses → no object ICB → TRUE.
        assert!(
            is_explicit_commit_proven_effective("r_fix1b", &ctx, &objects),
            "miscased [Commitbehavior(Ignore)] + no object ICB must return TRUE (case-sensitive miss)"
        );
    }

    // =======================================================================
    // A1 (issue 23) — `is_transaction_managing`'s COUNT branch must count
    // PHYSICAL table writes, not temp-inclusive ones.
    //
    // d8 already gates on `writes_physical_tables_count_of`
    // (`d8.rs:40-42`); d50 USED TO gate on `writes_tables_count_of`, which the
    // `cone_derived` module doc (`:23-24`) states is temp-INCLUSIVE -- that is
    // the defect these tests pin, fixed for the count branch by issue 23. A routine
    // whose only writes are to `temporary` records dirties nothing an implicit
    // commit could split.
    //
    // THE VACUITY TRAP these tests are built to avoid: `minimal_ctx` has EMPTY
    // `summaries` and a DEFAULT (row-less) cone store, so
    // `is_transaction_managing` on one returns false at the missing-summary
    // early return (`d50.rs:68-70`) BEFORE the count is ever consulted — a test
    // built on an unmodified `minimal_ctx` would pass identically before and
    // after the fix, for the wrong reason. Every case below therefore populates
    // a REAL summary and a REAL derived cone row, and ASSERTS the
    // inclusive/physical precondition explicitly before asserting the gate.
    // =======================================================================

    /// One `insert` on `table_id`, tagged `temp_state: known(is_temp)`.
    /// `fact_is_known_temp` (`cone_derived.rs:149-155`) reads exactly this
    /// shape, and it is what splits `table_writes_all` from
    /// `physical_table_writes` in the fold (`cone_derived.rs:621-639`).
    fn table_insert_fact(table_id: &str, is_temp: bool) -> CapabilityFact {
        let mut f = fact("insert", "table", Some(table_id));
        f.extra = Some(CapabilityExtra::Table {
            record_variable_id: None,
            temp_state: Some(ts_known(is_temp)),
            op_subtype: None,
        });
        f
    }

    /// A `DetectorContext` whose `summaries` AND `cone_derived` are REALLY
    /// populated for `routine_id` — `cone_store_of` folds the store FROM the
    /// summary, so the two can never silently disagree.
    fn ctx_with_write_summary<'a>(
        routines: &'a [L3Routine],
        routine_id: &str,
        facts: Vec<CapabilityFact>,
    ) -> DetectorContext<'a> {
        let mut summaries: HashMap<String, FullRoutineSummary> = HashMap::new();
        summaries.insert(
            routine_id.to_string(),
            summary(routine_id, facts, vec![], None),
        );
        let mut ctx = minimal_ctx(routines, vec![]);
        ctx.cone_derived = cone_store_of(&summaries);
        ctx.summaries = summaries;
        ctx
    }

    /// Assert the preconditions that make a COUNT-branch case non-vacuous: the
    /// summary EXISTS (so the gate reaches the count rather than returning
    /// false at `d50.rs:68-70`) and the derived row carries exactly
    /// `inclusive` temp-inclusive / `physical` physical writes.
    ///
    /// Asserts only; it returns `()`. Callers keep their own `ctx`.
    fn assert_counts(
        ctx: &DetectorContext<'_>,
        routine_id: &str,
        inclusive: usize,
        physical: usize,
    ) {
        assert!(
            ctx.summaries.contains_key(routine_id),
            "{routine_id} must HAVE a summary, or `is_transaction_managing` returns false \
             at the missing-summary early return and the case proves nothing"
        );
        assert_eq!(
            ctx.cone_derived.writes_tables_count_of(routine_id),
            inclusive,
            "{routine_id}: temp-INCLUSIVE written-table count (what the OLD gate read)"
        );
        assert_eq!(
            ctx.cone_derived.writes_physical_tables_count_of(routine_id),
            physical,
            "{routine_id}: PHYSICAL written-table count (what the gate MUST read)"
        );
    }

    /// A1 — three writes, ALL to `temporary` records: temp-inclusive 3 (AT the
    /// threshold, which is why the OLD inclusive gate answered true), physical 0. The
    /// routine must NOT be transaction-managing.
    #[test]
    fn a1_temp_only_writes_are_not_transaction_managing() {
        const ID: &str = "BufferRowsTemp";
        let r = routine(ID, "procedure");
        assert!(
            !posting_name_matches(&r.name),
            "the fixture routine must NOT match POSTING_NAME_RE, or the NAME branch \
             short-circuits and the COUNT branch under test is never consulted"
        );
        let routines = vec![r];
        let ctx = ctx_with_write_summary(
            &routines,
            ID,
            vec![
                table_insert_fact("t1", true),
                table_insert_fact("t2", true),
                table_insert_fact("t3", true),
            ],
        );
        assert_counts(&ctx, ID, 3, 0);
        assert!(
            ctx.cone_derived.writes_tables_count_of(ID) >= TRANSACTION_THRESHOLD_TABLES,
            "precondition: the temp-INCLUSIVE count is at/over the threshold, so the \
             OLD inclusive gate answered true — that is what makes this case discriminating"
        );

        assert!(
            !is_transaction_managing(ID, &ctx),
            "3 writes to TEMPORARY records and 0 physical ones: a temporary record never \
             dirties the transaction, so the gate must count PHYSICAL writes (as d8 does) \
             and answer false"
        );
    }

    /// A1 — the mixed case: two physical writes plus one temporary is
    /// temp-inclusive 3 but physical 2, below the threshold. The narrowing is
    /// not limited to temp-ONLY routines.
    #[test]
    fn a1_mixed_writes_below_the_physical_threshold_are_not_transaction_managing() {
        const ID: &str = "StageMixedRowsTemp";
        let r = routine(ID, "procedure");
        assert!(!posting_name_matches(&r.name));
        let routines = vec![r];
        let ctx = ctx_with_write_summary(
            &routines,
            ID,
            vec![
                table_insert_fact("t1", false),
                table_insert_fact("t2", false),
                table_insert_fact("t3", true),
            ],
        );
        assert_counts(&ctx, ID, 3, 2);

        assert!(
            !is_transaction_managing(ID, &ctx),
            "2 PHYSICAL writes is below TRANSACTION_THRESHOLD_TABLES ({TRANSACTION_THRESHOLD_TABLES}) \
             even though the temp-inclusive count is 3"
        );
    }

    /// A1 CONTROL (suppression-direction guard) — three PHYSICAL writes still
    /// make a transaction manager through the COUNT branch. Without this,
    /// "always answer false" would pass both cases above, and the two would not
    /// prove the count path is live at all.
    #[test]
    fn a1_control_three_physical_writes_are_transaction_managing() {
        const ID: &str = "StageRowsPhysical";
        let r = routine(ID, "procedure");
        assert!(!posting_name_matches(&r.name));
        let routines = vec![r];
        let ctx = ctx_with_write_summary(
            &routines,
            ID,
            vec![
                table_insert_fact("t1", false),
                table_insert_fact("t2", false),
                table_insert_fact("t3", false),
            ],
        );
        assert_counts(&ctx, ID, 3, 3);

        assert!(
            is_transaction_managing(ID, &ctx),
            "3 PHYSICAL writes must STILL clear the gate — the change narrows the COUNT \
             branch, it does not disable it"
        );
    }
}
