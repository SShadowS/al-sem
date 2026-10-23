//! The 5 diff passes. Port of al-sem `diff-abi.ts`, `diff-schema.ts`,
//! `diff-events.ts`, `diff-capabilities.ts`, `diff-permissions.ts`.
//!
//! Each pass iterates the index maps in INSERTION order, builds findings, and
//! sorts its OWN output by `id` (the engine re-sorts the union by severity /
//! category / kind / id afterward — but each pass's internal id-sort is kept for
//! byte-faithful ordering of equal-rank findings).

use std::collections::BTreeSet;

use indexmap::{IndexMap, IndexSet};

use crate::engine::gate::cbor::CborValue;

use super::fingerprint::{DiffCategory, DiffKind, compute_diff_fingerprint};
use super::indexes::DiffIndexes;
use super::{DiffFinding, DiffSubject, Severity, get_array, get_str};

// ── shared finding construction ─────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn make_finding(
    category: DiffCategory,
    kind: DiffKind,
    severity: Severity,
    subject_id: &str,
    secondary_key: Option<&str>,
    comparison_cone: Vec<String>,
    details: Vec<(String, CborValue)>,
    indexes: &DiffIndexes,
) -> DiffFinding {
    let origin = indexes.origin_for(subject_id);
    let display = indexes.display_for(subject_id);
    DiffFinding {
        id: compute_diff_fingerprint(category, kind, subject_id, secondary_key),
        category,
        kind,
        severity,
        subject: DiffSubject {
            normalized_stable_id: subject_id.to_string(),
            old_original_stable_id: origin.old_original_stable_id,
            new_stable_id: origin.new_stable_id,
            display_name: display,
        },
        comparison_cone,
        details,
        coverage_state: None,
    }
}

/// A `details` map with just `{kind}`.
fn details_kind(kind: DiffKind) -> Vec<(String, CborValue)> {
    vec![("kind".into(), CborValue::Text(kind.as_str().into()))]
}

fn sort_by_id(findings: &mut [DiffFinding]) {
    findings.sort_by(|a, b| a.id.cmp(&b.id));
}

// ===========================================================================
// ABI (diff-abi.ts)
// ===========================================================================

const VISIBILITY_ORDER: [&str; 4] = ["local", "internal", "protected", "public"];

fn visibility_index(v: &str) -> i32 {
    VISIBILITY_ORDER
        .iter()
        .position(|x| *x == v)
        .map(|i| i as i32)
        .unwrap_or(-1)
}

fn is_narrowed(old_v: &str, new_v: &str) -> bool {
    let o = visibility_index(old_v);
    let n = visibility_index(new_v);
    o > n && n >= 0
}

fn is_procedure_like(fact: &CborValue) -> bool {
    matches!(
        get_str(fact, "kind"),
        Some("routine") | Some("event-publisher")
    )
}

fn abi_severity(kind: DiffKind) -> Severity {
    match kind {
        DiffKind::ObjectRemoved
        | DiffKind::ObjectAccessibilityNarrowed
        | DiffKind::ProcedureRemoved
        | DiffKind::ProcedureSignatureChanged
        | DiffKind::ProcedureVarDirectionChanged => Severity::Critical,
        DiffKind::ProcedureObsoletionRegressed => Severity::High,
        DiffKind::ProcedureObsoletionProgressed
        | DiffKind::ObjectAdded
        | DiffKind::ProcedureAdded => Severity::Info,
        _ => Severity::Medium,
    }
}

/// Event attributes whose publisher other apps (or, for `ExternalBusinessEvent`,
/// Power Automate / Dataverse) subscribe to, even when the procedure is `local`.
const EXTERNAL_EVENT_ATTRIBUTES: [&str; 3] =
    ["IntegrationEvent", "BusinessEvent", "ExternalBusinessEvent"];

/// #156: whether a contract fact is part of the app's ABI — callable or
/// subscribable from another app. Not ABI: triggers, and `local` / `internal`
/// routines that publish no external event. A `local` or `internal` publisher
/// stays ABI: dependent extensions can subscribe to it (AppSourceCop AS0025: the
/// access modifier only limits raising the event). Shortcut: `internal` is
/// treated as in-app although `internalsVisibleTo` friend apps can call it; a
/// fact without `routineKind` (an older snapshot) counts as not a trigger.
pub(super) fn is_abi_surface(fact: &CborValue) -> bool {
    if get_str(fact, "kind") != Some("routine") {
        return true;
    }
    if get_str(fact, "routineKind") == Some("trigger") {
        return false;
    }
    carries_attribute(fact, &EXTERNAL_EVENT_ATTRIBUTES)
        || !matches!(get_str(fact, "visibility"), Some("local" | "internal"))
}

/// Whether the fact carries one of `names` (case-insensitive).
fn carries_attribute(fact: &CborValue, names: &[&str]) -> bool {
    get_array(fact, "attributes").is_some_and(|attrs| {
        attrs.iter().any(|a| {
            get_str(a, "name").is_some_and(|n| names.iter().any(|e| n.eq_ignore_ascii_case(e)))
        })
    })
}

/// #236: a `local` / `internal` `[IntegrationEvent]` / `[BusinessEvent]`
/// publisher whose old `parameters` list is a strict prefix of the new one.
/// AppSourceCop AS0025 allows adding a parameter to such an event (subscribers
/// bind by name and need not take every parameter); removing, renaming,
/// retyping or flipping `var` breaks them. Stricter than AS0025 on purpose: a
/// middle insertion or a reorder is not an append here and stays breaking. A
/// fact without `parameters` (an older snapshot) is never an append. Like
/// `is_abi_surface`, `internal` ignores `internalsVisibleTo` friend apps that
/// may raise the event; an append breaks them, which AS0025 accepts too.
pub(super) fn is_non_breaking_event_append(old: &CborValue, new: &CborValue) -> bool {
    let eligible = |f: &CborValue| {
        get_str(f, "kind") == Some("routine")
            && matches!(get_str(f, "visibility"), Some("local" | "internal"))
            && carries_attribute(f, &["IntegrationEvent", "BusinessEvent"])
    };
    let (Some(old_params), Some(new_params)) =
        (get_array(old, "parameters"), get_array(new, "parameters"))
    else {
        return false;
    };
    eligible(old)
        && eligible(new)
        && old_params.len() < new_params.len()
        && old_params
            .iter()
            .zip(new_params)
            .all(|p| matches!(p, (CborValue::Text(o), CborValue::Text(n)) if o == n))
}

pub fn diff_abi(indexes: &DiffIndexes) -> Vec<DiffFinding> {
    let mut out: Vec<DiffFinding> = Vec::new();

    for (subject, old_fact) in &indexes.old_contracts_by_subject {
        let new_fact = indexes.new_contracts_by_subject.get(subject);
        let Some(new_fact) = new_fact else {
            if !is_abi_surface(old_fact) {
                continue;
            }
            // Removal — object vs procedure-like.
            let removal_kind = if is_procedure_like(old_fact) {
                DiffKind::ProcedureRemoved
            } else {
                DiffKind::ObjectRemoved
            };
            out.push(make_finding(
                DiffCategory::Abi,
                removal_kind,
                abi_severity(removal_kind),
                subject,
                None,
                vec![subject.clone()],
                details_kind(removal_kind),
                indexes,
            ));
            continue;
        };

        if !is_abi_surface(old_fact) {
            // Not ABI before: becoming ABI is new surface; otherwise nothing.
            if is_abi_surface(new_fact) {
                let kind = DiffKind::ProcedureAdded;
                out.push(make_finding(
                    DiffCategory::Abi,
                    kind,
                    abi_severity(kind),
                    subject,
                    None,
                    vec![subject.clone()],
                    details_kind(kind),
                    indexes,
                ));
            }
            continue;
        }

        // Visibility narrowing.
        let old_vis = get_str(old_fact, "visibility").unwrap_or("");
        let new_vis = get_str(new_fact, "visibility").unwrap_or("");
        if is_narrowed(old_vis, new_vis) {
            let kind = DiffKind::ObjectAccessibilityNarrowed;
            out.push(make_finding(
                DiffCategory::Abi,
                kind,
                abi_severity(kind),
                subject,
                None,
                vec![subject.clone()],
                vec![
                    ("kind".into(), CborValue::Text(kind.as_str().into())),
                    ("oldAccessibility".into(), CborValue::Text(old_vis.into())),
                    ("newAccessibility".into(), CborValue::Text(new_vis.into())),
                ],
                indexes,
            ));
        }

        // Obsoletion transition (flat obsoleteState).
        let old_obs = get_str(old_fact, "obsoleteState");
        let new_obs = get_str(new_fact, "obsoleteState");
        if old_obs != new_obs {
            let rank = |s: Option<&str>| match s {
                Some("Pending") => 1,
                Some("Removed") => 2,
                _ => 0,
            };
            let old_rank = rank(old_obs);
            let new_rank = rank(new_obs);
            if new_rank > old_rank {
                let kind = DiffKind::ProcedureObsoletionProgressed;
                out.push(make_finding(
                    DiffCategory::Abi,
                    kind,
                    abi_severity(kind),
                    subject,
                    None,
                    vec![subject.clone()],
                    obsoletion_details(kind, old_obs, new_obs),
                    indexes,
                ));
            } else if new_rank < old_rank {
                let kind = DiffKind::ProcedureObsoletionRegressed;
                out.push(make_finding(
                    DiffCategory::Abi,
                    kind,
                    abi_severity(kind),
                    subject,
                    None,
                    vec![subject.clone()],
                    obsoletion_details(kind, old_obs, new_obs),
                    indexes,
                ));
            }
        }

        // Signature fingerprint change.
        let old_sig = get_str(old_fact, "signatureFingerprint").unwrap_or("");
        let new_sig = get_str(new_fact, "signatureFingerprint").unwrap_or("");
        if !old_sig.is_empty()
            && !new_sig.is_empty()
            && old_sig != new_sig
            && !is_non_breaking_event_append(old_fact, new_fact)
        {
            let kind = DiffKind::ProcedureSignatureChanged;
            out.push(make_finding(
                DiffCategory::Abi,
                kind,
                abi_severity(kind),
                subject,
                None,
                vec![subject.clone()],
                vec![
                    ("kind".into(), CborValue::Text(kind.as_str().into())),
                    ("oldSignatureHash".into(), CborValue::Text(old_sig.into())),
                    ("newSignatureHash".into(), CborValue::Text(new_sig.into())),
                ],
                indexes,
            ));
        }
    }

    // Additions.
    for (subject, new_fact) in &indexes.new_contracts_by_subject {
        if indexes.old_contracts_by_subject.contains_key(subject) || !is_abi_surface(new_fact) {
            continue;
        }
        let add_kind = if is_procedure_like(new_fact) {
            DiffKind::ProcedureAdded
        } else {
            DiffKind::ObjectAdded
        };
        out.push(make_finding(
            DiffCategory::Abi,
            add_kind,
            abi_severity(add_kind),
            subject,
            None,
            vec![subject.clone()],
            details_kind(add_kind),
            indexes,
        ));
    }

    sort_by_id(&mut out);
    out
}

fn obsoletion_details(
    kind: DiffKind,
    old_obs: Option<&str>,
    new_obs: Option<&str>,
) -> Vec<(String, CborValue)> {
    let mut d = vec![("kind".into(), CborValue::Text(kind.as_str().into()))];
    // al-sem sets `oldObsoleteState: oldObs` / `newObsoleteState: newObs`; when the
    // value is undefined the JSON serializer drops the key. So only emit when Some.
    if let Some(o) = old_obs {
        d.push(("oldObsoleteState".into(), CborValue::Text(o.into())));
    }
    if let Some(n) = new_obs {
        d.push(("newObsoleteState".into(), CborValue::Text(n.into())));
    }
    d
}

// ===========================================================================
// Schema (diff-schema.ts)
// ===========================================================================

fn data_class_rank(dc: &str) -> i32 {
    match dc {
        "SystemMetadata" => 0,
        "AccountData" => 1,
        "OrganizationIdentifiableInformation" => 2,
        "EndUserPseudonymousIdentifiers" => 3,
        "EndUserIdentifiableInformation" => 4,
        "CustomerContent" => 5,
        _ => 0,
    }
}

fn schema_severity(kind: DiffKind) -> Severity {
    match kind {
        DiffKind::TableFieldRemoved
        | DiffKind::TableFieldTypeNarrowed
        | DiffKind::EnumValueRemoved
        | DiffKind::EnumValueRenumbered => Severity::Critical,
        DiffKind::TableFieldDataClassificationTightened => Severity::High,
        DiffKind::TableFieldTypeWidened => Severity::Low,
        DiffKind::TableFieldDataClassificationRelaxed
        | DiffKind::TableFieldAdded
        | DiffKind::EnumValueAdded => Severity::Info,
        _ => Severity::Medium,
    }
}

pub fn diff_schema(indexes: &DiffIndexes) -> Vec<DiffFinding> {
    let mut out: Vec<DiffFinding> = Vec::new();

    for (stable_id, old_facts) in &indexes.old_schema_by_subject {
        let Some(old_fact) = old_facts.first() else {
            continue;
        };
        let new_fact = indexes
            .new_schema_by_subject
            .get(stable_id)
            .and_then(|v| v.first());

        let old_kind = get_str(old_fact, "kind").unwrap_or("");

        let Some(new_fact) = new_fact else {
            // Removal.
            if old_kind == "field" {
                let kind = DiffKind::TableFieldRemoved;
                out.push(make_finding(
                    DiffCategory::Schema,
                    kind,
                    schema_severity(kind),
                    stable_id,
                    None,
                    vec![stable_id.clone()],
                    details_kind(kind),
                    indexes,
                ));
            } else if old_kind == "enum-value" {
                let kind = DiffKind::EnumValueRemoved;
                out.push(make_finding(
                    DiffCategory::Schema,
                    kind,
                    schema_severity(kind),
                    stable_id,
                    None,
                    vec![stable_id.clone()],
                    details_kind(kind),
                    indexes,
                ));
            }
            continue;
        };

        let old_fp = get_str(old_fact, "shapeFingerprint").unwrap_or("");
        let new_fp = get_str(new_fact, "shapeFingerprint").unwrap_or("");

        if old_kind == "field" {
            if old_fp != new_fp {
                let kind = DiffKind::TableFieldTypeNarrowed;
                out.push(make_finding(
                    DiffCategory::Schema,
                    kind,
                    schema_severity(kind),
                    stable_id,
                    None,
                    vec![stable_id.clone()],
                    vec![
                        ("kind".into(), CborValue::Text(kind.as_str().into())),
                        ("oldShapeFingerprint".into(), CborValue::Text(old_fp.into())),
                        ("newShapeFingerprint".into(), CborValue::Text(new_fp.into())),
                    ],
                    indexes,
                ));
            }

            let old_dc = get_str(old_fact, "dataClassification");
            let new_dc = get_str(new_fact, "dataClassification");
            if let (Some(o), Some(n)) = (old_dc, new_dc)
                && o != n
            {
                let or = data_class_rank(o);
                let nr = data_class_rank(n);
                if nr > or {
                    let kind = DiffKind::TableFieldDataClassificationTightened;
                    out.push(make_finding(
                        DiffCategory::Schema,
                        kind,
                        schema_severity(kind),
                        stable_id,
                        None,
                        vec![stable_id.clone()],
                        data_class_details(kind, o, n),
                        indexes,
                    ));
                } else if nr < or {
                    let kind = DiffKind::TableFieldDataClassificationRelaxed;
                    out.push(make_finding(
                        DiffCategory::Schema,
                        kind,
                        schema_severity(kind),
                        stable_id,
                        None,
                        vec![stable_id.clone()],
                        data_class_details(kind, o, n),
                        indexes,
                    ));
                }
            }
        } else if old_kind == "enum-value" && old_fp != new_fp {
            let kind = DiffKind::EnumValueRenumbered;
            out.push(make_finding(
                DiffCategory::Schema,
                kind,
                schema_severity(kind),
                stable_id,
                None,
                vec![stable_id.clone()],
                vec![
                    ("kind".into(), CborValue::Text(kind.as_str().into())),
                    ("oldShapeFingerprint".into(), CborValue::Text(old_fp.into())),
                    ("newShapeFingerprint".into(), CborValue::Text(new_fp.into())),
                ],
                indexes,
            ));
        }
    }

    // Additions.
    for (stable_id, new_facts) in &indexes.new_schema_by_subject {
        if indexes.old_schema_by_subject.contains_key(stable_id) {
            continue;
        }
        let Some(new_fact) = new_facts.first() else {
            continue;
        };
        let new_kind = get_str(new_fact, "kind").unwrap_or("");
        if new_kind == "field" {
            let kind = DiffKind::TableFieldAdded;
            out.push(make_finding(
                DiffCategory::Schema,
                kind,
                schema_severity(kind),
                stable_id,
                None,
                vec![stable_id.clone()],
                details_kind(kind),
                indexes,
            ));
        } else if new_kind == "enum-value" {
            let kind = DiffKind::EnumValueAdded;
            out.push(make_finding(
                DiffCategory::Schema,
                kind,
                schema_severity(kind),
                stable_id,
                None,
                vec![stable_id.clone()],
                details_kind(kind),
                indexes,
            ));
        }
    }

    sort_by_id(&mut out);
    out
}

fn data_class_details(kind: DiffKind, old_dc: &str, new_dc: &str) -> Vec<(String, CborValue)> {
    vec![
        ("kind".into(), CborValue::Text(kind.as_str().into())),
        (
            "oldDataClassification".into(),
            CborValue::Text(old_dc.into()),
        ),
        (
            "newDataClassification".into(),
            CborValue::Text(new_dc.into()),
        ),
    ]
}

// ===========================================================================
// Events (diff-events.ts)
// ===========================================================================

fn event_severity(kind: DiffKind) -> Severity {
    match kind {
        DiffKind::EventPublisherSignatureChanged
        | DiffKind::EventContractChangedWithAffectedSubscribers => Severity::Critical,
        DiffKind::EventPublisherRemoved | DiffKind::EventSubscriberInducedCapabilityGained => {
            Severity::Medium
        }
        DiffKind::EventPublisherAdded => Severity::Info,
        DiffKind::EventSubscriberInducedCapabilityLost => Severity::Low,
        _ => Severity::Medium,
    }
}

/// #236: whether the publisher `routine` (a NEW id; both contract maps are keyed
/// by it once #156 paired the signature change) only appended a parameter. No
/// pair found means not an append, so the finding stays Critical.
fn is_append_at(indexes: &DiffIndexes, routine: &str) -> bool {
    match (
        indexes.old_contracts_by_subject.get(routine),
        indexes.new_contracts_by_subject.get(routine),
    ) {
        (Some(old), Some(new)) => is_non_breaking_event_append(old, new),
        _ => false,
    }
}

struct EventIdentity {
    publisher_object: String,
    event_name: String,
}

fn parse_event_identity(event_id: &str) -> EventIdentity {
    let parts: Vec<&str> = event_id.split("::").collect();
    if parts.len() < 3 {
        return EventIdentity {
            publisher_object: parts.first().copied().unwrap_or("").to_string(),
            event_name: parts.get(1).copied().unwrap_or("").to_string(),
        };
    }
    let event_name = parts[parts.len() - 2].to_string();
    let publisher_object = parts[..parts.len() - 2].join("::");
    EventIdentity {
        publisher_object,
        event_name,
    }
}

fn publisher_identity_key(decl: &CborValue) -> String {
    let id = get_str(decl, "eventId").unwrap_or("");
    let p = parse_event_identity(id);
    format!("{}::{}", p.publisher_object, p.event_name)
}

#[allow(clippy::too_many_arguments)]
fn event_finding(
    kind: DiffKind,
    subject_id: &str,
    secondary_key: &str,
    details: Vec<(String, CborValue)>,
    indexes: &DiffIndexes,
) -> DiffFinding {
    make_finding(
        DiffCategory::Events,
        kind,
        event_severity(kind),
        subject_id,
        Some(secondary_key),
        vec![subject_id.to_string()],
        details,
        indexes,
    )
}

fn event_details(
    kind: DiffKind,
    publisher_object: &str,
    event_name: &str,
    old_event_id: Option<&str>,
    new_event_id: Option<&str>,
) -> Vec<(String, CborValue)> {
    let mut d = vec![
        ("kind".into(), CborValue::Text(kind.as_str().into())),
        (
            "publisherObject".into(),
            CborValue::Text(publisher_object.into()),
        ),
        ("eventName".into(), CborValue::Text(event_name.into())),
    ];
    if let Some(o) = old_event_id {
        d.push(("oldEventId".into(), CborValue::Text(o.into())));
    }
    if let Some(n) = new_event_id {
        d.push(("newEventId".into(), CborValue::Text(n.into())));
    }
    d
}

pub fn diff_events(indexes: &DiffIndexes) -> Vec<DiffFinding> {
    let mut out: Vec<DiffFinding> = Vec::new();

    // publisher maps keyed by identity (object::name), last-wins (Map.set semantics).
    let mut old_publishers: IndexMap<String, &CborValue> = IndexMap::new();
    let mut new_publishers: IndexMap<String, &CborValue> = IndexMap::new();
    for decls in indexes.old_events_by_subject.values() {
        for decl in decls {
            if get_str(decl, "kind") != Some("publisher") {
                continue;
            }
            old_publishers.insert(publisher_identity_key(decl), decl);
        }
    }
    for decls in indexes.new_events_by_subject.values() {
        for decl in decls {
            if get_str(decl, "kind") != Some("publisher") {
                continue;
            }
            new_publishers.insert(publisher_identity_key(decl), decl);
        }
    }

    for (key, old_decl) in &old_publishers {
        let new_decl = new_publishers.get(key);
        let old_id = get_str(old_decl, "eventId").unwrap_or("");
        let parsed = parse_event_identity(old_id);
        match new_decl {
            None => {
                let routine = indexes.normalize_old(get_str(old_decl, "routine").unwrap_or(""));
                out.push(event_finding(
                    DiffKind::EventPublisherRemoved,
                    &routine,
                    &parsed.event_name,
                    event_details(
                        DiffKind::EventPublisherRemoved,
                        &parsed.publisher_object,
                        &parsed.event_name,
                        Some(old_id),
                        None,
                    ),
                    indexes,
                ));
            }
            Some(new_decl) => {
                let new_id = get_str(new_decl, "eventId").unwrap_or("");
                if old_id != new_id {
                    let routine = get_str(new_decl, "routine").unwrap_or("");
                    let mut f = event_finding(
                        DiffKind::EventPublisherSignatureChanged,
                        routine,
                        &parsed.event_name,
                        event_details(
                            DiffKind::EventPublisherSignatureChanged,
                            &parsed.publisher_object,
                            &parsed.event_name,
                            Some(old_id),
                            Some(new_id),
                        ),
                        indexes,
                    );
                    if is_append_at(indexes, routine) {
                        f.severity = Severity::Low;
                    }
                    out.push(f);
                }
            }
        }
    }

    for (key, new_decl) in &new_publishers {
        if old_publishers.contains_key(key) {
            continue;
        }
        let new_id = get_str(new_decl, "eventId").unwrap_or("");
        let parsed = parse_event_identity(new_id);
        let routine = get_str(new_decl, "routine").unwrap_or("");
        out.push(event_finding(
            DiffKind::EventPublisherAdded,
            routine,
            &parsed.event_name,
            event_details(
                DiffKind::EventPublisherAdded,
                &parsed.publisher_object,
                &parsed.event_name,
                None,
                Some(new_id),
            ),
            indexes,
        ));
    }

    // Phase 3: subscribers-by-event.
    let mut old_subs_by_event: IndexMap<String, Vec<&CborValue>> = IndexMap::new();
    let mut new_subs_by_event: IndexMap<String, Vec<&CborValue>> = IndexMap::new();
    for decls in indexes.old_events_by_subject.values() {
        for d in decls {
            if get_str(d, "kind") != Some("subscriber") {
                continue;
            }
            let p = parse_event_identity(get_str(d, "eventId").unwrap_or(""));
            let k = format!("{}::{}", p.publisher_object, p.event_name);
            old_subs_by_event.entry(k).or_default().push(d);
        }
    }
    for decls in indexes.new_events_by_subject.values() {
        for d in decls {
            if get_str(d, "kind") != Some("subscriber") {
                continue;
            }
            let p = parse_event_identity(get_str(d, "eventId").unwrap_or(""));
            let k = format!("{}::{}", p.publisher_object, p.event_name);
            new_subs_by_event.entry(k).or_default().push(d);
        }
    }

    // Specialization: signature-change WITH subscribers → ContractChanged…, suppress
    // the generic signature-changed for that event key.
    let mut suppress_keys: IndexSet<String> = IndexSet::new();
    let mut contract_findings: Vec<DiffFinding> = Vec::new();
    for (key, old_decl) in &old_publishers {
        let Some(new_decl) = new_publishers.get(key) else {
            continue;
        };
        let old_id = get_str(old_decl, "eventId").unwrap_or("");
        let new_id = get_str(new_decl, "eventId").unwrap_or("");
        if old_id == new_id {
            continue;
        }
        let has_subs = old_subs_by_event.get(key).map(|v| v.len()).unwrap_or(0)
            + new_subs_by_event.get(key).map(|v| v.len()).unwrap_or(0)
            > 0;
        if !has_subs {
            continue;
        }
        suppress_keys.insert(key.clone());
        let parsed = parse_event_identity(new_id);
        let routine = get_str(new_decl, "routine").unwrap_or("");
        let mut f = event_finding(
            DiffKind::EventContractChangedWithAffectedSubscribers,
            routine,
            &parsed.event_name,
            event_details(
                DiffKind::EventContractChangedWithAffectedSubscribers,
                &parsed.publisher_object,
                &parsed.event_name,
                Some(old_id),
                Some(new_id),
            ),
            indexes,
        );
        if is_append_at(indexes, routine) {
            f.severity = Severity::Low;
        }
        contract_findings.push(f);
    }

    // Filter out suppressed signature findings, then append contract findings.
    let filtered: Vec<DiffFinding> = out
        .into_iter()
        .filter(|f| {
            if f.kind != DiffKind::EventPublisherSignatureChanged {
                return true;
            }
            let po = detail_str(&f.details, "publisherObject");
            let en = detail_str(&f.details, "eventName");
            let k = format!("{po}::{en}");
            !suppress_keys.contains(&k)
        })
        .collect();
    out = filtered;
    out.extend(contract_findings);

    // Subscriber-induced capability delta.
    let matched_event_keys: IndexSet<String> = old_subs_by_event
        .keys()
        .chain(new_subs_by_event.keys())
        .cloned()
        .collect();

    for key in &matched_event_keys {
        let old_writes = writes_of(
            old_subs_by_event.get(key),
            &indexes.old_capability_facts_by_subject,
            |r| indexes.normalize_old(r),
        );
        let new_writes = writes_of(
            new_subs_by_event.get(key),
            &indexes.new_capability_facts_by_subject,
            str::to_string,
        );
        let mut split = key.splitn(2, "::");
        let publisher_object = split.next().unwrap_or("");
        let event_name = split.next().unwrap_or("");
        let subject = match (new_publishers.get(key), old_publishers.get(key)) {
            (Some(d), _) => get_str(d, "routine").map(str::to_string),
            (None, Some(d)) => get_str(d, "routine").map(|r| indexes.normalize_old(r)),
            (None, None) => None,
        };
        let Some(subject) = subject else {
            continue;
        };
        let subject = subject.as_str();

        // gained: new not in old.
        for (table, ops) in &new_writes {
            let old_ops = old_writes.get(table);
            for op in ops {
                if old_ops.map(|s| s.contains(op)).unwrap_or(false) {
                    continue;
                }
                let kind = DiffKind::EventSubscriberInducedCapabilityGained;
                let sk = format!("{event_name}|{table}|{op}");
                out.push(event_finding(
                    kind,
                    subject,
                    &sk,
                    vec![
                        ("kind".into(), CborValue::Text(kind.as_str().into())),
                        (
                            "publisherObject".into(),
                            CborValue::Text(publisher_object.into()),
                        ),
                        ("eventName".into(), CborValue::Text(event_name.into())),
                    ],
                    indexes,
                ));
            }
        }
        // lost: old not in new.
        for (table, ops) in &old_writes {
            let new_ops = new_writes.get(table);
            for op in ops {
                if new_ops.map(|s| s.contains(op)).unwrap_or(false) {
                    continue;
                }
                let kind = DiffKind::EventSubscriberInducedCapabilityLost;
                let sk = format!("{event_name}|{table}|{op}");
                out.push(event_finding(
                    kind,
                    subject,
                    &sk,
                    vec![
                        ("kind".into(), CborValue::Text(kind.as_str().into())),
                        (
                            "publisherObject".into(),
                            CborValue::Text(publisher_object.into()),
                        ),
                        ("eventName".into(), CborValue::Text(event_name.into())),
                    ],
                    indexes,
                ));
            }
        }
    }

    sort_by_id(&mut out);
    out
}

/// tableId → ordered set of write ops over a subscriber set's capability facts.
/// `key_of` maps a declaration's `routine` to the key `cap_map` uses.
fn writes_of(
    subs: Option<&Vec<&CborValue>>,
    cap_map: &IndexMap<String, Vec<&CborValue>>,
    key_of: impl Fn(&str) -> String,
) -> IndexMap<String, BTreeSet<String>> {
    let mut m: IndexMap<String, BTreeSet<String>> = IndexMap::new();
    let Some(subs) = subs else {
        return m;
    };
    for s in subs {
        let routine = key_of(get_str(s, "routine").unwrap_or(""));
        let Some(facts) = cap_map.get(&routine) else {
            continue;
        };
        for f in facts {
            if get_str(f, "resourceKind") != Some("table") {
                continue;
            }
            let op = get_str(f, "op").unwrap_or("");
            if !matches!(op, "insert" | "modify" | "delete") {
                continue;
            }
            let Some(rid) = get_str(f, "resourceId") else {
                continue;
            };
            m.entry(rid.to_string()).or_default().insert(op.to_string());
        }
    }
    m
}

fn detail_str(details: &[(String, CborValue)], key: &str) -> String {
    for (k, v) in details {
        if k == key
            && let CborValue::Text(s) = v
        {
            return s.clone();
        }
    }
    String::new()
}

// ===========================================================================
// Capabilities (diff-capabilities.ts)
// ===========================================================================

fn capability_severity(kind: DiffKind) -> Severity {
    match kind {
        DiffKind::CapabilityGainedCommit | DiffKind::CapabilityGainedDynamicDispatch => {
            Severity::High
        }
        DiffKind::CapabilityGainedWrite
        | DiffKind::CapabilityGainedRead
        | DiffKind::CapabilityGainedHttp
        | DiffKind::CapabilityGainedTelemetry
        | DiffKind::CapabilityGainedIsolatedStorage
        | DiffKind::CapabilityGainedFile
        | DiffKind::CapabilityGainedEventPublish
        | DiffKind::CapabilityLost => Severity::Medium,
        DiffKind::CapabilityLostUnderCoverage => Severity::Low,
        _ => Severity::Medium,
    }
}

/// The "gained" kind for a new capability fact, or None when it's not a tracked
/// gain. Mirrors `gainedKindFor`.
fn gained_kind_for(fact: &CborValue) -> Option<DiffKind> {
    let kind = get_str(fact, "resourceKind").unwrap_or("");
    let op = get_str(fact, "op").unwrap_or("");
    if kind == "table" {
        if op == "read" {
            return Some(DiffKind::CapabilityGainedRead);
        }
        if matches!(op, "insert" | "modify" | "delete") {
            return Some(DiffKind::CapabilityGainedWrite);
        }
    }
    if kind == "transaction" && op == "commit" {
        return Some(DiffKind::CapabilityGainedCommit);
    }
    if kind == "http" {
        return Some(DiffKind::CapabilityGainedHttp);
    }
    if kind == "telemetry" {
        return Some(DiffKind::CapabilityGainedTelemetry);
    }
    if kind == "isolated-storage" {
        return Some(DiffKind::CapabilityGainedIsolatedStorage);
    }
    if kind == "file" {
        return Some(DiffKind::CapabilityGainedFile);
    }
    if kind == "event" && op == "publish" {
        return Some(DiffKind::CapabilityGainedEventPublish);
    }
    if matches!(kind, "codeunit" | "page" | "report") && op == "execute" {
        // extra.kind == "dispatch" && resourceId === undefined.
        let resource_id_absent = get_str(fact, "resourceId").is_none();
        let extra_kind = match fact {
            CborValue::Map(m) => match m.get("extra") {
                Some(extra) => get_str(extra, "kind"),
                None => None,
            },
            _ => None,
        };
        if extra_kind == Some("dispatch") && resource_id_absent {
            return Some(DiffKind::CapabilityGainedDynamicDispatch);
        }
    }
    None
}

/// factKey = `op|resourceKind|resourceId` (resourceId stringified, empty when absent).
fn cap_fact_key(fact: &CborValue) -> String {
    let op = get_str(fact, "op").unwrap_or("");
    let rk = get_str(fact, "resourceKind").unwrap_or("");
    let rid = get_str(fact, "resourceId").unwrap_or("");
    format!("{op}|{rk}|{rid}")
}

fn cap_details(fact: &CborValue) -> Vec<(String, CborValue)> {
    let mut d = vec![("kind".into(), CborValue::Null)]; // kind filled by caller
    let rk = get_str(fact, "resourceKind").unwrap_or("");
    d.push(("resourceKind".into(), CborValue::Text(rk.into())));
    if let Some(rid) = get_str(fact, "resourceId") {
        d.push(("resourceId".into(), CborValue::Text(rid.into())));
    }
    let op = get_str(fact, "op").unwrap_or("");
    d.push(("op".into(), CborValue::Text(op.into())));
    d
}

pub fn diff_capabilities(indexes: &DiffIndexes) -> Vec<DiffFinding> {
    let mut out: Vec<DiffFinding> = Vec::new();

    // allSubjects = union of old+new keys (Set insertion order: old keys, then new
    // keys not already present).
    let mut all_subjects: IndexSet<String> = IndexSet::new();
    for s in indexes.old_capability_facts_by_subject.keys() {
        all_subjects.insert(s.clone());
    }
    for s in indexes.new_capability_facts_by_subject.keys() {
        all_subjects.insert(s.clone());
    }

    for subject in &all_subjects {
        let empty: Vec<&CborValue> = Vec::new();
        let old_facts = indexes
            .old_capability_facts_by_subject
            .get(subject)
            .unwrap_or(&empty);
        let new_facts = indexes
            .new_capability_facts_by_subject
            .get(subject)
            .unwrap_or(&empty);
        let mut old_by_key: IndexMap<String, &CborValue> = IndexMap::new();
        let mut new_by_key: IndexMap<String, &CborValue> = IndexMap::new();
        for f in old_facts {
            old_by_key.insert(cap_fact_key(f), f);
        }
        for f in new_facts {
            new_by_key.insert(cap_fact_key(f), f);
        }

        // Gains.
        for (key, fact) in &new_by_key {
            if old_by_key.contains_key(key) {
                continue;
            }
            let Some(gain_kind) = gained_kind_for(fact) else {
                continue;
            };
            let mut details = cap_details(fact);
            details[0] = ("kind".into(), CborValue::Text(gain_kind.as_str().into()));
            out.push(make_finding(
                DiffCategory::Capabilities,
                gain_kind,
                capability_severity(gain_kind),
                subject,
                Some(key),
                vec![subject.clone()],
                details,
                indexes,
            ));
        }

        // Losses → provisional CapabilityLost.
        for (key, fact) in &old_by_key {
            if new_by_key.contains_key(key) {
                continue;
            }
            let kind = DiffKind::CapabilityLost;
            let mut details = cap_details(fact);
            details[0] = ("kind".into(), CborValue::Text(kind.as_str().into()));
            out.push(make_finding(
                DiffCategory::Capabilities,
                kind,
                capability_severity(kind),
                subject,
                Some(key),
                vec![subject.clone()],
                details,
                indexes,
            ));
        }
    }

    sort_by_id(&mut out);
    out
}

// ===========================================================================
// Permissions (diff-permissions.ts) — RequiredPermissionFact only.
// ===========================================================================

fn permission_severity(kind: DiffKind) -> Severity {
    match kind {
        DiffKind::PermissionRightsExpanded | DiffKind::PermissionTargetAdded => Severity::High,
        DiffKind::PermissionRightsContracted | DiffKind::PermissionTargetRemoved => Severity::Low,
        _ => Severity::Medium,
    }
}

fn rights_of(fact: &CborValue) -> Vec<String> {
    get_array(fact, "rights")
        .map(|arr| {
            arr.iter()
                .filter_map(|v| match v {
                    CborValue::Text(s) => Some(s.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn is_expansion(old_rights: &[String], new_rights: &[String]) -> bool {
    let old_set: BTreeSet<&String> = old_rights.iter().collect();
    let new_set: BTreeSet<&String> = new_rights.iter().collect();
    if new_set.len() <= old_set.len() {
        return false;
    }
    old_set.iter().all(|r| new_set.contains(*r))
}

fn is_contraction(old_rights: &[String], new_rights: &[String]) -> bool {
    let old_set: BTreeSet<&String> = old_rights.iter().collect();
    let new_set: BTreeSet<&String> = new_rights.iter().collect();
    if old_set.len() <= new_set.len() {
        return false;
    }
    new_set.iter().all(|r| old_set.contains(*r))
}

fn perm_details(
    kind: DiffKind,
    target_kind: &str,
    target_id: &str,
    old_rights: Option<&[String]>,
    new_rights: Option<&[String]>,
) -> Vec<(String, CborValue)> {
    let mut d = vec![
        ("kind".into(), CborValue::Text(kind.as_str().into())),
        ("targetKind".into(), CborValue::Text(target_kind.into())),
        ("targetId".into(), CborValue::Text(target_id.into())),
    ];
    if let Some(o) = old_rights {
        d.push((
            "oldRights".into(),
            CborValue::Array(o.iter().map(|s| CborValue::Text(s.clone())).collect()),
        ));
    }
    if let Some(n) = new_rights {
        d.push((
            "newRights".into(),
            CborValue::Array(n.iter().map(|s| CborValue::Text(s.clone())).collect()),
        ));
    }
    d
}

struct PermEntry {
    rights: Vec<String>,
}

pub fn diff_permissions(indexes: &DiffIndexes) -> Vec<DiffFinding> {
    let mut out: Vec<DiffFinding> = Vec::new();

    let mut all_subjects: IndexSet<String> = IndexSet::new();
    for s in indexes.old_permissions_by_subject.keys() {
        all_subjects.insert(s.clone());
    }
    for s in indexes.new_permissions_by_subject.keys() {
        all_subjects.insert(s.clone());
    }

    for subject in &all_subjects {
        let empty: Vec<&CborValue> = Vec::new();
        let old_facts = indexes
            .old_permissions_by_subject
            .get(subject)
            .unwrap_or(&empty);
        let new_facts = indexes
            .new_permissions_by_subject
            .get(subject)
            .unwrap_or(&empty);

        let mut old_by_target: IndexMap<String, PermEntry> = IndexMap::new();
        let mut new_by_target: IndexMap<String, PermEntry> = IndexMap::new();
        for f in old_facts {
            if get_str(f, "kind") != Some("required") {
                continue;
            }
            let tk = get_str(f, "targetKind").unwrap_or("");
            let tid = get_str(f, "target").unwrap_or("");
            old_by_target.insert(
                format!("{tk}|{tid}"),
                PermEntry {
                    rights: rights_of(f),
                },
            );
        }
        for f in new_facts {
            if get_str(f, "kind") != Some("required") {
                continue;
            }
            let tk = get_str(f, "targetKind").unwrap_or("");
            let tid = get_str(f, "target").unwrap_or("");
            new_by_target.insert(
                format!("{tk}|{tid}"),
                PermEntry {
                    rights: rights_of(f),
                },
            );
        }

        // Old targets — removals + rights changes.
        for (key, old_entry) in &old_by_target {
            let sep = key.find('|').unwrap_or(0);
            let tk = &key[..sep];
            let tid = &key[sep + 1..];
            let new_entry = new_by_target.get(key);
            let Some(new_entry) = new_entry else {
                out.push(make_finding(
                    DiffCategory::Permissions,
                    DiffKind::PermissionTargetRemoved,
                    permission_severity(DiffKind::PermissionTargetRemoved),
                    subject,
                    Some(key),
                    vec![subject.clone()],
                    perm_details(
                        DiffKind::PermissionTargetRemoved,
                        tk,
                        tid,
                        Some(&old_entry.rights),
                        None,
                    ),
                    indexes,
                ));
                continue;
            };

            // identical sets → skip.
            let old_set: BTreeSet<&String> = old_entry.rights.iter().collect();
            let new_set: BTreeSet<&String> = new_entry.rights.iter().collect();
            if old_set == new_set {
                continue;
            }

            let kind = if is_expansion(&old_entry.rights, &new_entry.rights) {
                DiffKind::PermissionRightsExpanded
            } else if is_contraction(&old_entry.rights, &new_entry.rights) {
                DiffKind::PermissionRightsContracted
            } else {
                // mixed → treated as expansion.
                DiffKind::PermissionRightsExpanded
            };
            out.push(make_finding(
                DiffCategory::Permissions,
                kind,
                permission_severity(kind),
                subject,
                Some(key),
                vec![subject.clone()],
                perm_details(
                    kind,
                    tk,
                    tid,
                    Some(&old_entry.rights),
                    Some(&new_entry.rights),
                ),
                indexes,
            ));
        }

        // New targets — additions.
        for (key, new_entry) in &new_by_target {
            if old_by_target.contains_key(key) {
                continue;
            }
            let sep = key.find('|').unwrap_or(0);
            let tk = &key[..sep];
            let tid = &key[sep + 1..];
            out.push(make_finding(
                DiffCategory::Permissions,
                DiffKind::PermissionTargetAdded,
                permission_severity(DiffKind::PermissionTargetAdded),
                subject,
                Some(key),
                vec![subject.clone()],
                perm_details(
                    DiffKind::PermissionTargetAdded,
                    tk,
                    tid,
                    None,
                    Some(&new_entry.rights),
                ),
                indexes,
            ));
        }
    }

    sort_by_id(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::ids::{ParamSpec, canonical_param_spec};

    /// One `parameters` entry, rendered by the same formatter `derive_contracts` uses.
    fn param(name: &str, type_text: &str, is_var: bool) -> CborValue {
        let spec = ParamSpec {
            type_text: type_text.into(),
            is_var,
        };
        CborValue::Text(canonical_param_spec(&spec, Some(name)))
    }

    /// A routine contract fact; `attr` empty means no attribute at all.
    fn fact(vis: &str, attr: &str, params: Option<Vec<CborValue>>) -> CborValue {
        let mut m: IndexMap<String, CborValue> = IndexMap::new();
        m.insert("kind".into(), CborValue::Text("routine".into()));
        m.insert("visibility".into(), CborValue::Text(vis.into()));
        let attrs = if attr.is_empty() {
            vec![]
        } else {
            let mut a: IndexMap<String, CborValue> = IndexMap::new();
            a.insert("name".into(), CborValue::Text(attr.into()));
            vec![CborValue::Map(a)]
        };
        m.insert("attributes".into(), CborValue::Array(attrs));
        if let Some(p) = params {
            m.insert("parameters".into(), CborValue::Array(p));
        }
        CborValue::Map(m)
    }

    /// #236 test 1: which publisher signature changes are a non-breaking append.
    /// The rename, retype and `var`-flip rows also append, so only the
    /// per-parameter compare can reject them.
    #[test]
    fn non_breaking_event_append_matrix() {
        let a = || param("Handled", "Boolean", true);
        let b = || param("Qty", "Decimal", false);
        let c = || param("Code", "Code[20]", false);
        let check = |vis: &str, attr: &str, old: Vec<CborValue>, new: Vec<CborValue>| {
            is_non_breaking_event_append(&fact(vis, attr, Some(old)), &fact(vis, attr, Some(new)))
        };
        for vis in ["local", "internal"] {
            for attr in ["IntegrationEvent", "BusinessEvent", "integrationevent"] {
                assert!(
                    check(vis, attr, vec![a()], vec![a(), b()]),
                    "{vis} [{attr}]"
                );
            }
        }
        for (vis, attr, why) in [
            ("public", "IntegrationEvent", "public"),
            ("protected", "IntegrationEvent", "protected"),
            ("local", "ExternalBusinessEvent", "ExternalBusinessEvent"),
            ("local", "", "plain local routine"),
        ] {
            assert!(!check(vis, attr, vec![a()], vec![a(), b()]), "{why}");
        }
        let local = |old, new| check("local", "IntegrationEvent", old, new);
        assert!(!local(vec![a()], vec![a()]), "unchanged");
        assert!(!local(vec![a(), b()], vec![a()]), "removed");
        assert!(!local(vec![a(), b()], vec![a(), c(), b()]), "inserted");
        assert!(!local(vec![a(), b()], vec![b(), a(), c()]), "reordered");
        let renamed = param("IsHandled", "Boolean", true);
        assert!(!local(vec![a()], vec![renamed, b()]), "rename+append");
        let retyped = param("Handled", "Integer", true);
        assert!(!local(vec![a()], vec![retyped, b()]), "retype+append");
        let unvar = param("Handled", "Boolean", false);
        assert!(!local(vec![a()], vec![unvar, b()]), "var+append");
        let i = "IntegrationEvent";
        assert!(
            !is_non_breaking_event_append(
                &fact("local", i, None),
                &fact("local", i, Some(vec![a()]))
            ),
            "old without parameters"
        );
        assert!(
            !is_non_breaking_event_append(&fact("local", i, Some(vec![])), &fact("local", i, None)),
            "new without parameters"
        );
    }
}
