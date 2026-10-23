//! The rename overlay → rename table, with chain/overlap/stale diagnostics.
//! Port of al-sem `src/diff/diff-renames.ts`.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;

use crate::engine::gate::cbor::CborValue;

use super::DiffDiagnostic;

/// A raw rename overlay: `{oldId: newId}`. Insertion order preserved (mirrors the
/// JS object iteration order al-sem relies on for chain/overlap diagnostics).
pub type RenameOverlay = IndexMap<String, String>;

#[derive(Debug, Clone)]
pub struct RenameEntry {
    pub new_id: String,
    /// #156: minted by [`pair_signature_changes`], not by the user overlay. An auto
    /// entry is not a rename: it sets no `oldOriginalStableId` and is not counted in
    /// `renamesApplied`.
    pub auto: bool,
}

/// `oldId -> RenameEntry`. Identity mappings (`oldId == newId`) are dropped.
pub type RenameTable = IndexMap<String, RenameEntry>;

/// Load a rename overlay from JSON file contents into an insertion-ordered map.
/// Engine-never-throws: parse failure is surfaced as `Err`, not a panic.
pub fn parse_rename_overlay(text: &str) -> Result<RenameOverlay, String> {
    // serde_json with preserve_order keeps key order; if not enabled the order is
    // sorted — chain/overlap diagnostics are corpus-invisible (the rename corpus has
    // 2 non-overlapping, non-chaining entries), so either order is acceptable here.
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("rename overlay JSON parse error: {e}"))?;
    let obj = value
        .as_object()
        .ok_or_else(|| "rename overlay must be a JSON object".to_string())?;
    let mut overlay: RenameOverlay = IndexMap::new();
    for (k, v) in obj {
        let s = v
            .as_str()
            .ok_or_else(|| format!("rename overlay value for '{k}' must be a string"))?;
        overlay.insert(k.clone(), s.to_string());
    }
    Ok(overlay)
}

/// Build the normalized rename table from a raw overlay. Detects chains
/// (`A→B`, `B→C`) and overlaps (multiple olds → one new). Mirrors `buildRenameTable`.
pub fn build_rename_table(overlay: &RenameOverlay) -> (RenameTable, Vec<DiffDiagnostic>) {
    let mut table: RenameTable = IndexMap::new();
    let mut diagnostics: Vec<DiffDiagnostic> = Vec::new();
    let mut new_to_olds: IndexMap<String, Vec<String>> = IndexMap::new();

    for (old_id, new_id) in overlay {
        if old_id == new_id {
            continue; // identity mapping
        }
        table.insert(
            old_id.clone(),
            RenameEntry {
                new_id: new_id.clone(),
                auto: false,
            },
        );
        new_to_olds
            .entry(new_id.clone())
            .or_default()
            .push(old_id.clone());
    }

    // Chains: any newId that also appears as an oldId.
    for (old_id, entry) in &table {
        if let Some(next) = table.get(&entry.new_id) {
            diagnostics.push(DiffDiagnostic {
                kind: "rename-overlay-chain".into(),
                fields: vec![
                    (
                        "kind".into(),
                        CborValue::Text("rename-overlay-chain".into()),
                    ),
                    ("from".into(), CborValue::Text(old_id.clone())),
                    ("via".into(), CborValue::Text(entry.new_id.clone())),
                    ("to".into(), CborValue::Text(next.new_id.clone())),
                ],
            });
        }
    }

    // Overlaps: any newId with multiple oldIds.
    for (new_id, olds) in &new_to_olds {
        if olds.len() > 1 {
            let mut targets: Vec<CborValue> =
                olds.iter().map(|o| CborValue::Text(o.clone())).collect();
            targets.push(CborValue::Text(new_id.clone()));
            diagnostics.push(DiffDiagnostic {
                kind: "rename-overlay-overlap".into(),
                fields: vec![
                    (
                        "kind".into(),
                        CborValue::Text("rename-overlay-overlap".into()),
                    ),
                    ("targets".into(), CborValue::Array(targets)),
                ],
            });
        }
    }

    (table, diagnostics)
}

/// Validate the overlay against actual snapshot stable-id sets, emitting stale
/// diagnostics. Mirrors `validateOverlayAgainstSnapshots`.
pub fn validate_overlay_against_snapshots(
    table: &RenameTable,
    old_ids: &HashSet<String>,
    new_ids: &HashSet<String>,
) -> Vec<DiffDiagnostic> {
    let mut diagnostics: Vec<DiffDiagnostic> = Vec::new();
    for (old_id, entry) in table {
        if !old_ids.contains(old_id) {
            diagnostics.push(stale(old_id, "not-in-old"));
        }
        if !new_ids.contains(&entry.new_id) {
            diagnostics.push(stale(&entry.new_id, "not-in-new"));
        }
    }
    diagnostics
}

fn stale(stale_id: &str, reason: &str) -> DiffDiagnostic {
    DiffDiagnostic {
        kind: "rename-overlay-stale".into(),
        fields: vec![
            (
                "kind".into(),
                CborValue::Text("rename-overlay-stale".into()),
            ),
            ("staleId".into(), CborValue::Text(stale_id.to_string())),
            ("reason".into(), CborValue::Text(reason.to_string())),
        ],
    }
}

/// #156: pair each routine whose signature changed. A routine's stable id is
/// `{object}#{signatureFingerprint}`, so a parameter change looks like a removal
/// plus an addition. A routine only in old and a routine only in new, in the same
/// object with the same name (AL names ignore case), are one routine when that
/// (object, name, ABI status) group holds exactly one of each; unchanged overloads
/// keep their id and do not count. An ambiguous group (two overloads changed at
/// once) stays unpaired, and so does a routine whose ABI status changed with its
/// signature (public `Foo` removed + local `Foo` added stays an ABI removal). Only
/// member-less ids pair: a member trigger's id hashes its member in, so two
/// `OnAction`s share name and signature but are different triggers; distinct
/// member-less ids therefore always differ in signature. Ids the user overlay maps
/// (either end) never pair. Returns `(old_id, new_id)` pairs in old-snapshot order.
pub fn pair_signature_changes(
    old_snap: &CborValue,
    new_snap: &CborValue,
    user: &RenameTable,
) -> Vec<(String, String)> {
    let user_targets: HashSet<&str> = user.values().map(|e| e.new_id.as_str()).collect();
    let normalize = |id: &str| user.get(id).map_or(id, |e| e.new_id.as_str()).to_string();
    let old_rows = routine_rows(old_snap);
    let new_rows = routine_rows(new_snap);
    let old_ids: HashSet<String> = old_rows.keys().map(|id| normalize(id)).collect();

    // (object, lowercased name, ABI) → (old-only candidates, new-only candidates).
    type Group<'a> = (Vec<&'a str>, Vec<&'a str>);
    let mut groups: IndexMap<(&str, String, bool), Group> = IndexMap::new();
    for (id, r) in &old_rows {
        if r.pairable && !user.contains_key(id) && !new_rows.contains_key(id) {
            groups.entry(r.key()).or_default().0.push(id);
        }
    }
    for (id, r) in &new_rows {
        if r.pairable && !user_targets.contains(id.as_str()) && !old_ids.contains(id) {
            groups.entry(r.key()).or_default().1.push(id);
        }
    }
    groups
        .into_values()
        .filter_map(|(olds, news)| match (olds.as_slice(), news.as_slice()) {
            ([o], [n]) => Some((o.to_string(), n.to_string())),
            _ => None,
        })
        .collect()
}

struct RoutineRow {
    object: String,
    name: String,
    abi: bool,
    /// Member-less (`id == {object}#{signatureFingerprint}`) and named.
    pairable: bool,
}

impl RoutineRow {
    fn key(&self) -> (&str, String, bool) {
        (&self.object, self.name.to_lowercase(), self.abi)
    }
}

/// The routine contract facts of one snapshot by stable id, with their display
/// names. A repeated id keeps its last row, as `build_diff_indexes` does.
fn routine_rows(snap: &CborValue) -> IndexMap<String, RoutineRow> {
    let ids = super::indexes::identities_field(snap, "stableIds");
    let names = super::indexes::identities_field(snap, "displayNames");
    let name_of: HashMap<&str, &str> = ids
        .iter()
        .zip(names)
        .filter_map(|(i, n)| match (i, n) {
            (CborValue::Text(i), CborValue::Text(n)) => Some((i.as_str(), n.as_str())),
            _ => None,
        })
        .collect();
    super::snapshot_array(snap, "contractFacts")
        .iter()
        .filter(|f| super::get_str(f, "kind") == Some("routine"))
        .filter_map(|f| {
            let id = super::get_str(f, "stableId")?;
            let sig = super::get_str(f, "signatureFingerprint").unwrap_or("");
            let name = name_of.get(id).copied().unwrap_or("");
            let (object, suffix) = id.rsplit_once('#')?;
            let row = RoutineRow {
                object: object.to_string(),
                name: name.to_string(),
                abi: super::passes::is_abi_surface(f),
                pairable: !name.is_empty() && !sig.is_empty() && suffix == sig,
            };
            Some((id.to_string(), row))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlay(pairs: &[(&str, &str)]) -> RenameOverlay {
        let mut o: RenameOverlay = IndexMap::new();
        for (k, v) in pairs {
            o.insert((*k).to_string(), (*v).to_string());
        }
        o
    }

    fn field_str(d: &DiffDiagnostic, key: &str) -> Option<String> {
        d.fields.iter().find(|(k, _)| k == key).and_then(|(_, v)| {
            if let CborValue::Text(s) = v {
                Some(s.clone())
            } else {
                None
            }
        })
    }

    /// (a) `{A:B, B:C}` → exactly one chain diagnostic (from=A, via=B, to=C).
    #[test]
    fn overlay_chain_diagnostic() {
        let (_table, diags) = build_rename_table(&overlay(&[("A", "B"), ("B", "C")]));
        let chains: Vec<&DiffDiagnostic> = diags
            .iter()
            .filter(|d| d.kind == "rename-overlay-chain")
            .collect();
        assert_eq!(chains.len(), 1, "exactly one chain diag");
        assert_eq!(field_str(chains[0], "from").as_deref(), Some("A"));
        assert_eq!(field_str(chains[0], "via").as_deref(), Some("B"));
        assert_eq!(field_str(chains[0], "to").as_deref(), Some("C"));
    }

    /// (b) `{A:C, B:C}` → exactly one overlap diagnostic, targets `[A,B,C]` in
    /// insertion order (the two olds in overlay order, then the shared new).
    #[test]
    fn overlay_overlap_diagnostic_targets_insertion_order() {
        let (_table, diags) = build_rename_table(&overlay(&[("A", "C"), ("B", "C")]));
        let overlaps: Vec<&DiffDiagnostic> = diags
            .iter()
            .filter(|d| d.kind == "rename-overlay-overlap")
            .collect();
        assert_eq!(overlaps.len(), 1, "exactly one overlap diag");
        let targets = overlaps[0]
            .fields
            .iter()
            .find(|(k, _)| k == "targets")
            .map(|(_, v)| v)
            .expect("targets field");
        let CborValue::Array(items) = targets else {
            panic!("targets must be an array");
        };
        let names: Vec<String> = items
            .iter()
            .filter_map(|v| {
                if let CborValue::Text(s) = v {
                    Some(s.clone())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(names, vec!["A", "B", "C"], "targets in insertion order");
    }

    /// (c) An overlay stableId absent from both old+new → one stale diagnostic.
    #[test]
    fn overlay_stale_diagnostic() {
        let (table, _diags) = build_rename_table(&overlay(&[("GONE", "ALSO_GONE")]));
        let mut old_ids: HashSet<String> = HashSet::new();
        old_ids.insert("present-old".into());
        let mut new_ids: HashSet<String> = HashSet::new();
        new_ids.insert("present-new".into());
        let stale_diags = validate_overlay_against_snapshots(&table, &old_ids, &new_ids);
        // Both the old (not-in-old) and the new (not-in-new) id are stale.
        assert_eq!(stale_diags.len(), 2);
        assert_eq!(stale_diags[0].kind, "rename-overlay-stale");
        assert_eq!(
            field_str(&stale_diags[0], "staleId").as_deref(),
            Some("GONE")
        );
        assert_eq!(
            field_str(&stale_diags[0], "reason").as_deref(),
            Some("not-in-old")
        );
        assert_eq!(
            field_str(&stale_diags[1], "staleId").as_deref(),
            Some("ALSO_GONE")
        );
        assert_eq!(
            field_str(&stale_diags[1], "reason").as_deref(),
            Some("not-in-new")
        );
    }

    /// Identity mappings (`A:A`) are dropped — no diagnostics, empty table.
    #[test]
    fn identity_mapping_dropped() {
        let (table, diags) = build_rename_table(&overlay(&[("A", "A")]));
        assert!(table.is_empty());
        assert!(diags.is_empty());
    }
}
