//! D14 — dead routine (unreachable from any entry point). Port of al-sem
//! `src/detectors/d14-dead-routine.ts`.
//!
//! Forward-reachability BFS from `ctx.reachable_roots` over `graph.edges_by_from`
//! (following `edge.to`). A routine is flagged DEAD when it is:
//!   - primary (source-only ⇒ always),
//!   - `body_available`,
//!   - NOT in the reachable set,
//!   - access-flaggable (`local`, or `internal` when
//!     `!internal_reachable_externally`),
//!   - NOT on a Test/Tests object,
//!   - NOT on a property-expression host object (Page / PageExtension / Report /
//!     XmlPort / Query — whose call graph the resolver does not fully model),
//!   - NOT itself a reachable root.
//!
//! NOTE: al-sem does NOT gate d14 on `parse_incomplete` (unlike d46) — only
//! `roleOf == primary` and `body_available`. Faithfully reproduced here.
//!
//! Within-detector sort by `compareStrings(id)` (byte order); fingerprint computed
//! pre-projection over the internal ids.

use std::collections::{HashSet, VecDeque};

use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::finding::{Evidence, EvidenceStep, Finding, FixOption, SourceAnchor};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::model::workspace::Model;

const DETECTOR: &str = "d14-dead-routine";

/// Object types whose call graph the resolver does not fully model — property
/// expressions (`Caption = MyCaption()`, action `OnAction`, layout bindings, …)
/// can reference same-object procedures without forming call edges, so a routine
/// on one of these cannot be proven unreachable. al-sem
/// `OBJECT_TYPES_WITHOUT_FULL_CALL_GRAPH`.
const OBJECT_TYPES_WITHOUT_FULL_CALL_GRAPH: [&str; 5] =
    ["Page", "PageExtension", "Report", "XmlPort", "Query"];

/// `obj.name.match(/Tests?$/i)` — the name ends in `Test` or `Tests`
/// (case-insensitive), anchored at the end. JS `$` here is end-of-string.
fn ends_with_test_ci(name: &str) -> bool {
    let nb = name.as_bytes();
    // Try "tests" (5) first, then "test" (4) — both are accepted by `/Tests?$/i`.
    for suf in [b"tests".as_slice(), b"test".as_slice()] {
        if nb.len() >= suf.len() && nb[nb.len() - suf.len()..].eq_ignore_ascii_case(suf) {
            return true;
        }
    }
    false
}

/// The forward-reachable set: BFS from `ctx.reachable_roots` over
/// `graph.edges_by_from` (following `edge.to`).
fn compute_reachable_set(ctx: &DetectorContext) -> HashSet<String> {
    let roots = &ctx.reachable_roots;
    let mut reachable: HashSet<String> = roots.iter().cloned().collect();
    let mut queue: VecDeque<String> = roots.iter().cloned().collect();
    while let Some(id) = queue.pop_front() {
        if let Some(edges) = ctx.graph.edges_by_from.get(&id) {
            for e in edges {
                if !reachable.contains(&e.to) {
                    reachable.insert(e.to.clone());
                    queue.push_back(e.to.clone());
                }
            }
        }
    }
    reachable
}

/// Per-routine d14 classification — the SINGLE place the dead criteria live.
/// `detect_d14` maps each variant to its stats bucket / finding; the G-7
/// cross-detector helper (`provably_dead_routine_ids`) collects only `Dead`, so
/// both consumers apply EXACTLY the same criteria by construction. Variants are
/// checked in the original detector order (reachable → access → Test object →
/// property-expression host → root) to keep the stats buckets byte-identical.
enum D14Class {
    /// `!body_available` — never a candidate (not counted).
    NotCandidate,
    /// In the forward-reachable closure → skip "other".
    Reachable,
    /// Not `local` / app-scoped `internal` → skip "nonLocal".
    NonFlaggableAccess,
    /// Owning object name matches `/Tests?$/i` → skip "other".
    TestObject,
    /// Owning object's call graph is not fully modeled → skip
    /// "propertyExpressionHost".
    PropertyExpressionHost,
    /// Itself a reachable root → skip "other".
    Root,
    /// Provably dead per every d14 criterion — d14 emits a finding.
    Dead,
}

fn classify_routine(
    r: &crate::program::model::workspace::ModelRoutine,
    reachable: &HashSet<String>,
    ctx: &DetectorContext,
) -> D14Class {
    // roleOf(r) === "primary": source-only ⇒ always true.
    if !r.body_available {
        return D14Class::NotCandidate;
    }
    if reachable.contains(&r.id) {
        return D14Class::Reachable;
    }
    // `local` is always flaggable; `internal` only when no app is granted
    // internal access (source-only default: internal_reachable_externally=false).
    // "protected" / default (public) → None entry in access_modifier ⇒ never
    // flaggable. Read the modifier off the routine directly (mirrors al-sem's
    // `r.accessModifier`).
    let access = r.access_modifier.as_deref();
    let access_flaggable = access == Some("local")
        || (access == Some("internal") && !ctx.internal_reachable_externally);
    if !access_flaggable {
        return D14Class::NonFlaggableAccess;
    }
    if let Some(obj) = ctx.objects_by_id.get(r.object_id.as_str()).copied() {
        if ends_with_test_ci(&obj.name) {
            return D14Class::TestObject;
        }
        if OBJECT_TYPES_WITHOUT_FULL_CALL_GRAPH.contains(&obj.object_type.as_str()) {
            return D14Class::PropertyExpressionHost;
        }
    }
    if ctx.reachable_roots.contains(&r.id) {
        return D14Class::Root;
    }
    D14Class::Dead
}

/// G-7 (docs/engine-gaps.md): the set of routine ids d14 would flag DEAD —
/// EXACTLY its emission criteria, via the shared `classify_routine`. Other
/// detectors consult this to DOWN-CONFIDENCE (never suppress) findings rooted
/// only in provably-dead routines. The signal is deliberately strong: a routine
/// is in this set only when it is unreachable from the entry-point closure
/// (triggers, event subscribers, every non-`local` procedure — the open-world
/// surface), is `local`/app-scoped-`internal`, and sits on an object whose call
/// graph the resolver fully models.
pub(crate) fn provably_dead_routine_ids(
    resolved: &Model,
    ctx: &DetectorContext,
) -> HashSet<String> {
    let reachable = compute_reachable_set(ctx);
    resolved
        .workspace
        .routines
        .iter()
        .filter(|r| matches!(classify_routine(r, &reachable, ctx), D14Class::Dead))
        .map(|r| r.id.clone())
        .collect()
}

pub fn detect_d14(
    resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;

    // Forward BFS from the reachable roots over graph.edges_by_from (follow edge.to).
    let reachable = compute_reachable_set(ctx);

    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_other = 0u64;
    let mut skipped_non_local = 0u64;
    let mut skipped_property_expression_host = 0u64;

    for r in &ws.routines {
        match classify_routine(r, &reachable, ctx) {
            D14Class::NotCandidate => continue,
            D14Class::Reachable | D14Class::TestObject | D14Class::Root => {
                candidates_considered += 1;
                skipped_other += 1;
                continue;
            }
            D14Class::NonFlaggableAccess => {
                candidates_considered += 1;
                skipped_non_local += 1;
                continue;
            }
            D14Class::PropertyExpressionHost => {
                candidates_considered += 1;
                skipped_property_expression_host += 1;
                continue;
            }
            D14Class::Dead => {
                candidates_considered += 1;
            }
        }

        let access = r.access_modifier.as_deref();
        let obj = ctx.objects_by_id.get(r.object_id.as_str()).copied();
        let access_note = if access == Some("internal") {
            " The workspace's app.json has no `internalsVisibleTo` entries, so no other app can call it."
        } else {
            ""
        };
        let obj_name = obj.map(|o| o.name.as_str()).unwrap_or(r.object_id.as_str());

        let id = format!("d14/{}", r.id);
        let root_cause_key = id.clone();
        let primary_location = anchor_from(&r.source_anchor, &r.id);

        let evidence_path = vec![EvidenceStep {
            routine_id: r.id.clone(),
            operation_id: None,
            callsite_id: None,
            loop_id: None,
            source_anchor: anchor_from(&r.source_anchor, &r.id),
            note: format!("{} (no inbound edges from entry-point closure)", r.name),
        }];

        let mut finding = Finding {
            id,
            root_cause_key,
            detector: DETECTOR.to_string(),
            title: "Routine is unreachable from any entry point".into(),
            root_cause: format!(
                "{} on {} is not called from any page action, trigger, OnRun, web service, \
                 or event subscriber in this app \u{2014} appears to be dead code.{}",
                r.name, obj_name, access_note
            ),
            severity: "info".to_string(),
            confidence: to_confidence(&[], "possible"),
            primary_location,
            evidence_path,
            additional_paths: None,
            affected_objects: vec![r.object_id.as_str().into()],
            affected_tables: Vec::new(),
            fix_options: vec![FixOption {
                description: "Remove the routine if truly unused, or wire it up to an entry \
                              point if intended to be invoked."
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

    findings.sort_by(|a, b| a.id.cmp(&b.id));

    let emitted = findings.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates_considered, emitted);
    stats.add_skip("other", skipped_other);
    stats.add_skip("nonLocal", skipped_non_local);
    stats.add_skip("propertyExpressionHost", skipped_property_expression_host);
    Ok(DetectorOutput {
        findings,
        stats,
        diagnostics: vec![],
        d1_cohort_index: None,
    })
}

/// Build a `SourceAnchor` from a `PAnchor` with the routine's own id as the
/// enclosing routine. Hash fields default to `None`.
fn anchor_from(a: &crate::program::body::features::PAnchor, routine_id: &str) -> SourceAnchor {
    SourceAnchor {
        source_unit_id: a.source_unit_id.clone(),
        start_line: a.start_line,
        start_column: a.start_column,
        end_line: a.end_line,
        end_column: a.end_column,
        enclosing_routine_id: routine_id.to_string(),
        syntax_kind: a.syntax_kind.clone(),
        normalized_text_hash: None,
        leading_context_hash: None,
        trailing_context_hash: None,
    }
}
