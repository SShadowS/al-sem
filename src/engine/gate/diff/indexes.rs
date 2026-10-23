//! `buildDiffIndexes` — the per-snapshot rename-normalized fact lookups + origin
//! tracking. Port of al-sem `src/diff/diff-indexes.ts`.
//!
//! Facts are kept as borrowed `&CborValue` references into the deserialized
//! snapshot trees (the passes read fields by key). The OLD side is rename-
//! normalized; the NEW side is keyed verbatim.

use std::collections::HashSet;

use indexmap::IndexMap;

use crate::engine::gate::cbor::CborValue;

use super::renames::{RenameTable, validate_overlay_against_snapshots};
use super::{DiffDiagnostic, get_array, get_str, snapshot_array};

/// Origin tracking for one normalized subject id.
#[derive(Debug, Clone, Default)]
pub struct SubjectOrigin {
    pub old_original_stable_id: Option<String>,
    pub new_stable_id: Option<String>,
}

pub struct DiffIndexes<'a> {
    pub old_display_by_stable_id: IndexMap<String, String>,
    pub new_display_by_stable_id: IndexMap<String, String>,
    pub origin_by_normalized: IndexMap<String, SubjectOrigin>,

    pub old_contracts_by_subject: IndexMap<String, &'a CborValue>,
    pub new_contracts_by_subject: IndexMap<String, &'a CborValue>,
    pub old_schema_by_subject: IndexMap<String, Vec<&'a CborValue>>,
    pub new_schema_by_subject: IndexMap<String, Vec<&'a CborValue>>,
    pub old_permissions_by_subject: IndexMap<String, Vec<&'a CborValue>>,
    pub new_permissions_by_subject: IndexMap<String, Vec<&'a CborValue>>,
    pub old_events_by_subject: IndexMap<String, Vec<&'a CborValue>>,
    pub new_events_by_subject: IndexMap<String, Vec<&'a CborValue>>,
    pub old_capability_facts_by_subject: IndexMap<String, Vec<&'a CborValue>>,
    pub new_capability_facts_by_subject: IndexMap<String, Vec<&'a CborValue>>,
    pub old_coverage_by_subject: IndexMap<String, &'a CborValue>,
    pub new_coverage_by_subject: IndexMap<String, &'a CborValue>,

    pub rename_diagnostics: Vec<DiffDiagnostic>,

    /// The rename table the OLD side was normalized with (user overlay + #156
    /// signature pairs), for old ids read out of fact bodies.
    rename_table: RenameTable,
}

fn normalize(id: &str, table: &RenameTable) -> String {
    match table.get(id) {
        Some(e) => e.new_id.clone(),
        None => id.to_string(),
    }
}

fn push_by_key<'a>(
    map: &mut IndexMap<String, Vec<&'a CborValue>>,
    key: String,
    value: &'a CborValue,
) {
    map.entry(key).or_default().push(value);
}

/// Extract the primary subject id of a PermissionFact: declared→permissionSet,
/// required→subject.
fn permission_fact_subject(fact: &CborValue) -> Option<&str> {
    match get_str(fact, "kind") {
        Some("required") => get_str(fact, "subject"),
        _ => get_str(fact, "permissionSet"),
    }
}

pub fn build_diff_indexes<'a>(
    old_snap: &'a CborValue,
    new_snap: &'a CborValue,
    rename_table: &RenameTable,
) -> DiffIndexes<'a> {
    let mut old_display_by_stable_id: IndexMap<String, String> = IndexMap::new();
    let mut new_display_by_stable_id: IndexMap<String, String> = IndexMap::new();
    let mut origin_by_normalized: IndexMap<String, SubjectOrigin> = IndexMap::new();

    // Old identities — normalize.
    let old_ids_arr = identities_field(old_snap, "stableIds");
    let old_names_arr = identities_field(old_snap, "displayNames");
    for (i, id_v) in old_ids_arr.iter().enumerate() {
        let id = match id_v {
            CborValue::Text(s) => s.as_str(),
            _ => "",
        };
        if id.is_empty() {
            continue;
        }
        let display = old_names_arr
            .get(i)
            .and_then(|v| match v {
                CborValue::Text(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_default();
        let normalized = normalize(id, rename_table);
        old_display_by_stable_id.insert(normalized.clone(), display);
        // A #156 signature pair is the same routine, not a rename.
        let rename_applied = rename_table.get(id).is_some_and(|e| !e.auto);
        origin_by_normalized.insert(
            normalized,
            SubjectOrigin {
                old_original_stable_id: if rename_applied {
                    Some(id.to_string())
                } else {
                    None
                },
                new_stable_id: None,
            },
        );
    }

    // New identities — never renamed; populate display + fold into origin.
    let new_ids_arr = identities_field(new_snap, "stableIds");
    let new_names_arr = identities_field(new_snap, "displayNames");
    for (i, id_v) in new_ids_arr.iter().enumerate() {
        let id = match id_v {
            CborValue::Text(s) => s.as_str(),
            _ => "",
        };
        if id.is_empty() {
            continue;
        }
        let display = new_names_arr
            .get(i)
            .and_then(|v| match v {
                CborValue::Text(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_default();
        new_display_by_stable_id.insert(id.to_string(), display);
        match origin_by_normalized.get_mut(id) {
            Some(origin) => origin.new_stable_id = Some(id.to_string()),
            None => {
                origin_by_normalized.insert(
                    id.to_string(),
                    SubjectOrigin {
                        old_original_stable_id: None,
                        new_stable_id: Some(id.to_string()),
                    },
                );
            }
        }
    }

    // ContractFacts — keyed by stableId.
    let mut old_contracts_by_subject: IndexMap<String, &CborValue> = IndexMap::new();
    for fact in snapshot_array(old_snap, "contractFacts") {
        if let Some(sid) = get_str(fact, "stableId") {
            old_contracts_by_subject.insert(normalize(sid, rename_table), fact);
        }
    }
    let mut new_contracts_by_subject: IndexMap<String, &CborValue> = IndexMap::new();
    for fact in snapshot_array(new_snap, "contractFacts") {
        if let Some(sid) = get_str(fact, "stableId") {
            new_contracts_by_subject.insert(sid.to_string(), fact);
        }
    }

    // SchemaFacts — keyed by stableId.
    // A field fact's `name` (#156) is its display name; identities still win.
    let mut old_schema_by_subject: IndexMap<String, Vec<&CborValue>> = IndexMap::new();
    for fact in snapshot_array(old_snap, "schemaFacts") {
        if let Some(sid) = get_str(fact, "stableId") {
            let normalized = normalize(sid, rename_table);
            if let Some(name) = get_str(fact, "name") {
                old_display_by_stable_id
                    .entry(normalized.clone())
                    .or_insert_with(|| name.to_string());
            }
            push_by_key(&mut old_schema_by_subject, normalized, fact);
        }
    }
    let mut new_schema_by_subject: IndexMap<String, Vec<&CborValue>> = IndexMap::new();
    for fact in snapshot_array(new_snap, "schemaFacts") {
        if let Some(sid) = get_str(fact, "stableId") {
            if let Some(name) = get_str(fact, "name") {
                new_display_by_stable_id
                    .entry(sid.to_string())
                    .or_insert_with(|| name.to_string());
            }
            push_by_key(&mut new_schema_by_subject, sid.to_string(), fact);
        }
    }

    // PermissionFacts — keyed by permission_fact_subject.
    let mut old_permissions_by_subject: IndexMap<String, Vec<&CborValue>> = IndexMap::new();
    for fact in snapshot_array(old_snap, "permissionFacts") {
        if let Some(sub) = permission_fact_subject(fact) {
            push_by_key(
                &mut old_permissions_by_subject,
                normalize(sub, rename_table),
                fact,
            );
        }
    }
    let mut new_permissions_by_subject: IndexMap<String, Vec<&CborValue>> = IndexMap::new();
    for fact in snapshot_array(new_snap, "permissionFacts") {
        if let Some(sub) = permission_fact_subject(fact) {
            push_by_key(&mut new_permissions_by_subject, sub.to_string(), fact);
        }
    }

    // EventDeclarations — keyed by routine.
    let mut old_events_by_subject: IndexMap<String, Vec<&CborValue>> = IndexMap::new();
    for decl in snapshot_array(old_snap, "eventDeclarations") {
        if let Some(routine) = get_str(decl, "routine") {
            push_by_key(
                &mut old_events_by_subject,
                normalize(routine, rename_table),
                decl,
            );
        }
    }
    let mut new_events_by_subject: IndexMap<String, Vec<&CborValue>> = IndexMap::new();
    for decl in snapshot_array(new_snap, "eventDeclarations") {
        if let Some(routine) = get_str(decl, "routine") {
            push_by_key(&mut new_events_by_subject, routine.to_string(), decl);
        }
    }

    // CapabilityFacts — keyed by subject.
    let mut old_capability_facts_by_subject: IndexMap<String, Vec<&CborValue>> = IndexMap::new();
    for fact in snapshot_array(old_snap, "capabilityFacts") {
        if let Some(sub) = get_str(fact, "subject") {
            push_by_key(
                &mut old_capability_facts_by_subject,
                normalize(sub, rename_table),
                fact,
            );
        }
    }
    let mut new_capability_facts_by_subject: IndexMap<String, Vec<&CborValue>> = IndexMap::new();
    for fact in snapshot_array(new_snap, "capabilityFacts") {
        if let Some(sub) = get_str(fact, "subject") {
            push_by_key(&mut new_capability_facts_by_subject, sub.to_string(), fact);
        }
    }

    // CoverageRecords — keyed by subject.
    let mut old_coverage_by_subject: IndexMap<String, &CborValue> = IndexMap::new();
    for rec in snapshot_array(old_snap, "coverage") {
        if let Some(sub) = get_str(rec, "subject") {
            old_coverage_by_subject.insert(normalize(sub, rename_table), rec);
        }
    }
    let mut new_coverage_by_subject: IndexMap<String, &CborValue> = IndexMap::new();
    for rec in snapshot_array(new_snap, "coverage") {
        if let Some(sub) = get_str(rec, "subject") {
            new_coverage_by_subject.insert(sub.to_string(), rec);
        }
    }

    // Stale-rename diagnostics now that snapshot membership is known.
    let old_ids: HashSet<String> = old_ids_arr
        .iter()
        .filter_map(|v| match v {
            CborValue::Text(s) => Some(s.clone()),
            _ => None,
        })
        .collect();
    let new_ids: HashSet<String> = new_ids_arr
        .iter()
        .filter_map(|v| match v {
            CborValue::Text(s) => Some(s.clone()),
            _ => None,
        })
        .collect();
    let rename_diagnostics = validate_overlay_against_snapshots(rename_table, &old_ids, &new_ids);

    DiffIndexes {
        old_display_by_stable_id,
        new_display_by_stable_id,
        origin_by_normalized,
        old_contracts_by_subject,
        new_contracts_by_subject,
        old_schema_by_subject,
        new_schema_by_subject,
        old_permissions_by_subject,
        new_permissions_by_subject,
        old_events_by_subject,
        new_events_by_subject,
        old_capability_facts_by_subject,
        new_capability_facts_by_subject,
        old_coverage_by_subject,
        new_coverage_by_subject,
        rename_diagnostics,
        rename_table: rename_table.clone(),
    }
}

/// Read `identities.<field>` as an array slice (empty when absent).
pub(super) fn identities_field<'a>(snap: &'a CborValue, field: &str) -> &'a [CborValue] {
    match snap {
        CborValue::Map(m) => match m.get("identities") {
            Some(ident) => get_array(ident, field).unwrap_or(&[]),
            None => &[],
        },
        _ => &[],
    }
}

impl<'a> DiffIndexes<'a> {
    /// The display name for a normalized subject id (new ?? old ?? id), mirroring
    /// the `makeFinding` display lookup shared by all passes.
    pub fn display_for(&self, subject_id: &str) -> String {
        self.new_display_by_stable_id
            .get(subject_id)
            .or_else(|| self.old_display_by_stable_id.get(subject_id))
            .cloned()
            .unwrap_or_else(|| subject_id.to_string())
    }

    /// An OLD-side id read from a fact body (e.g. an event declaration's
    /// `routine`), normalized the way the index keys are.
    pub fn normalize_old(&self, id: &str) -> String {
        normalize(id, &self.rename_table)
    }

    /// The origin (oldOriginalStableId / newStableId) for a normalized subject.
    pub fn origin_for(&self, subject_id: &str) -> SubjectOrigin {
        self.origin_by_normalized
            .get(subject_id)
            .cloned()
            .unwrap_or_default()
    }
}
