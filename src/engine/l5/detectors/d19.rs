//! D19 — declared procedure parameter never referenced in the routine body.
//! Port of al-sem `src/detectors/d19-unused-parameter.ts`.
//!
//! Restricted to `procedure` (NOT triggers, NOT event subscribers — the latter's
//! signature is dictated by the publisher and must keep every parameter declared,
//! even unused — and NOT event publishers). bodyAvailable + !parseIncomplete; skip
//! when there are 0 params.
//!
//! The event-publisher exclusion (issue 25) is a CONSTRUCTION argument, not a
//! heuristic: an `[IntegrationEvent]`/`[BusinessEvent]` body is empty by language
//! definition — the compiler generates the dispatch — so a publisher parameter is
//! unreferenced in its own body *always*. Its readers are the SUBSCRIBERS, which
//! this body-local predicate cannot see, making d19 100% false-positive on that
//! population. This is NOT a claim that a publisher parameter can never deserve
//! removal; that question needs subscriber-use evidence and belongs to the
//! event-graph detectors (d12 already covers the dead-event case).
//!
//! `refs = set(routine.identifier_references)` (the L2 lowercased / sorted /
//! deduped set). A parameter whose lowercased name is absent from that set is
//! unreferenced. Unnamed slots (empty name) are never flagged.
//!
//! Within-detector sort by `compareStrings(a.id, b.id)` (byte order).

use std::collections::{HashMap, HashSet};

use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::anchor_of;
use crate::engine::l5::finding::{Evidence, EvidenceStep, Finding, FindingConfidence, FixOption};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::model::workspace::{Model, ModelEntities, ModelObject, ModelRoutine};

pub(crate) const DETECTOR: &str = "d19-unused-parameter";

/// ENG-151: whether a routine implements an interface member: its object
/// implements an interface that is, or extends (transitively, ENG-142's
/// `interface_bases`), one declaring a member of the routine's name (the
/// model keeps no arity for a source interface's members). alc makes an
/// implementer keep the member's signature (AL0582 when a
/// member is missing), so its unused parameters are not the author's to drop.
/// An interface not in the model leaves its members unknown
/// ([`IfaceMember::Unknown`]); the caller skips only a method that could be a
/// member then (review, sol: not a `local` one). Interfaces are matched by
/// name, as the model's `implements_interfaces` lists them, unescaped as the
/// object names are (`"I""Foo"` names `I"Foo`). The routine's object is the
/// copy that DECLARES it (`DeclaringObjects::of`), so whole-object `#if` arms
/// do not borrow each other's `implements` (review, sol).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IfaceMember {
    /// An interface the object implements (or one of its bases) declares it.
    Member,
    /// None declares it, but one of them is not in the model.
    Unknown,
    /// None declares it, and all of them are in the model (or none at all).
    No,
}

fn interface_member_test(
    ws: &ModelEntities,
) -> impl Fn(Option<&ModelObject>, &ModelRoutine) -> IfaceMember + '_ {
    let fold = |s: &str| {
        al_syntax::fold_identifier(&crate::program::body::scope::unescape_al_identifier(
            s.trim().trim_matches('"'),
        ))
    };
    let objects: HashMap<&str, &ModelObject> =
        ws.objects.iter().map(|o| (o.id.as_str(), o)).collect();
    let mut bases: HashMap<String, Vec<String>> = HashMap::new();
    for o in ws.objects.iter().filter(|o| o.object_type == "Interface") {
        bases
            .entry(fold(&o.name))
            .or_default()
            .extend(o.interface_bases.iter().map(|b| fold(b)));
    }
    // A source interface's members are `interface_members`, ALL of them: not
    // `declaration_only_routines`, which the cross-app model cuts to the demanded
    // population, and a dependency interface's bodyless members are never
    // demanded (ENG-151's CDO measurement: System Application's "Email
    // Connector" read as memberless). A symbol-only interface has its members as
    // routines. Names only: a same-named method of another arity (an overload)
    // is skipped too, the safe side.
    let mut members: HashMap<String, HashSet<String>> = HashMap::new();
    for o in ws.objects.iter().filter(|o| o.object_type == "Interface") {
        members
            .entry(fold(&o.name))
            .or_default()
            .extend(o.interface_members.iter().map(|m| fold(m)));
    }
    for r in &ws.routines {
        if let Some(o) = objects.get(r.object_id.as_str())
            && o.object_type == "Interface"
        {
            members
                .entry(fold(&o.name))
                .or_default()
                .insert(fold(&r.name));
        }
    }
    move |object: Option<&ModelObject>, r: &ModelRoutine| {
        let Some(impls) = object.and_then(|o| o.implements_interfaces.as_ref()) else {
            return IfaceMember::No;
        };
        let key = fold(&r.name);
        let mut unknown = false;
        let mut seen: HashSet<String> = HashSet::new();
        let mut stack: Vec<String> = impls.iter().map(|i| fold(i)).collect();
        while let Some(i) = stack.pop() {
            if !seen.insert(i.clone()) {
                continue;
            }
            let Some(more) = bases.get(&i) else {
                unknown = true; // not in the model: its members are unknown
                continue;
            };
            if members.get(&i).is_some_and(|m| m.contains(&key)) {
                return IfaceMember::Member;
            }
            stack.extend(more.iter().cloned());
        }
        if unknown {
            IfaceMember::Unknown
        } else {
            IfaceMember::No
        }
    }
}

pub fn detect_d19(
    resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_trigger = 0u64;
    let mut skipped_event_subscriber = 0u64;
    let mut skipped_event_publisher = 0u64;
    let mut skipped_interface_member = 0u64;
    let mut skipped_interface_unknown = 0u64;
    let implements_member = interface_member_test(ws);
    let declaring = crate::program::model::declaring::DeclaringObjects::new(&ws.objects);

    for routine in &ws.routines {
        // roleOf(routine) !== "primary" → skip. Source-only: every routine is
        // primary, so this never skips (mirrors al-sem semantics).
        if !routine.body_available {
            continue;
        }
        if routine.parse_incomplete {
            continue;
        }
        // procedure only — skip triggers, event subscribers + event publishers.
        if routine.kind == "trigger" {
            skipped_trigger += 1;
            continue;
        }
        if routine.kind == "event-subscriber" {
            skipped_event_subscriber += 1;
            continue;
        }
        // A routine carrying BOTH a subscriber and a publisher attribute is booked
        // as a SUBSCRIBER — but that precedence is decided upstream, in
        // `ir_walk::ir_routine_kind`, not here: by the time d19 runs, `routine.kind`
        // is already one single string. These three checks are mutually-exclusive
        // equality tests on that one scalar, so REORDERING THEM CHANGES NOTHING —
        // measured, by swapping them and watching every test stay green (issue 25).
        // The guard that actually matters is `ir_routine_kind`'s branch order, and
        // `gap_d19_event_publisher_skip::dual_attribute_routine_books_the_subscriber_counter`
        // is what pins it (inverting that branch order fails the test).
        if routine.kind == "event-publisher" {
            skipped_event_publisher += 1;
            continue;
        }
        if routine.parameters.is_empty() {
            continue;
        }
        // ENG-151: a method implementing an interface member keeps the
        // interface's signature, so an unused parameter there is no defect.
        // An interface not in the model leaves this unknown: skip then only a
        // method that could be a member (a `local` one cannot).
        match implements_member(declaring.of(routine), routine) {
            IfaceMember::Member => {
                skipped_interface_member += 1;
                continue;
            }
            IfaceMember::Unknown if routine.access_modifier.as_deref() != Some("local") => {
                skipped_interface_unknown += 1;
                continue;
            }
            IfaceMember::Unknown | IfaceMember::No => {}
        }
        candidates_considered += 1;

        let refs: HashSet<&str> = routine
            .identifier_references
            .iter()
            .map(|s| s.as_str())
            .collect();

        for param in &routine.parameters {
            let lc = al_syntax::fold_identifier(&param.name);
            if lc.is_empty() {
                continue; // unnamed slot — never flag
            }
            if refs.contains(lc.as_str()) {
                continue;
            }

            let path = vec![EvidenceStep {
                routine_id: routine.id.clone(),
                operation_id: None,
                callsite_id: None,
                loop_id: None,
                source_anchor: anchor_of(&routine.source_anchor, routine),
                note: format!(
                    "parameter '{}: {}' declared but never referenced",
                    param.name, param.type_text
                ),
            }];

            let id = format!("d19/{}/p{}", routine.id, param.index);
            let root_cause_key = id.clone();

            let confidence: FindingConfidence = to_confidence(&[], "likely");

            let root_cause = format!(
                "{} declares parameter '{}' ({}) at position {} but the body never references it.",
                routine.name, param.name, param.type_text, param.index
            );

            let mut finding = Finding {
                id,
                root_cause_key,
                detector: DETECTOR.to_string(),
                title: "Procedure parameter is never used".into(),
                root_cause,
                severity: "info".to_string(),
                confidence,
                primary_location: anchor_of(&routine.source_anchor, routine),
                evidence_path: path,
                additional_paths: None,
                affected_objects: vec![routine.object_id.as_str().into()],
                affected_tables: Vec::new(),
                fix_options: vec![FixOption {
                    description:
                        "Remove the parameter, or wire it into the procedure body. If callers \
                         must keep the existing signature, leave it and silence with an `_` \
                         prefix on the name."
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
                loose_subject: None,
            };
            finding.fingerprint = Some(fp_index.fingerprint_of(&finding));
            findings.push(finding);
        }
    }

    findings.sort_by(|a, b| a.id.cmp(&b.id));

    let emitted = findings.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates_considered, emitted);
    stats.add_skip("trigger", skipped_trigger);
    stats.add_skip("eventSubscriber", skipped_event_subscriber);
    stats.add_skip("eventPublisher", skipped_event_publisher);
    stats.add_skip("interfaceMember", skipped_interface_member);
    stats.add_skip("interfaceUnknown", skipped_interface_unknown);
    Ok(DetectorOutput {
        findings,
        stats,
        diagnostics: vec![],
        d1_cohort_index: None,
    })
}
