//! **The before/after delta** (agent-tasks design §10.3-10.4, astra a11):
//! per routine of the change, what its behavior gained and lost between the
//! comparison base and the working tree, from the two sides' summaries
//! ([`super::keys`]).
//!
//! What is reused from `alsem diff` (`gate/diff`), not re-implemented:
//!
//! - **pairing**: a routine whose id is on both sides pairs by id; a routine
//!   only one side has pairs through `renames::pair_signature_changes`
//!   (signature pairing with its member and ABI safeguards), run over both
//!   sides' complete routine inventories. Two overloads changed at once stay
//!   unpaired: one deleted, one added, with the reason.
//! - **preflight, schema and contract findings**: `run_diff_engine` over the
//!   two sides' contract snapshots (`snapshot_full::contract_snapshot`):
//!   its preflight (schema version, universe, app identity), its schema
//!   passes (fields, enums, and the table key passes) and its ABI pass.
//! - **the loss wording**: `policy::loss_kind`, the loose coverage policy's
//!   rule. A key only the base side has is "removed" when the new side's
//!   coverage is complete, and "no longer observed" when it is not (an
//!   unresolved call in the cone, a depth-bounded search, no body), or when
//!   the new side writes the same table with the same operation under an
//!   undecided class.
//!
//! Deleted and added are decided from the complete inventories only: a
//! routine is deleted when the base has it and the working tree has no
//! routine of that id and none paired with it; added the other way round.

use al_syntax::IdentifierFoldExt;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde_json::{Value, json};

use super::keys::{RootSummary, Site};
use crate::engine::gate::cbor::CborValue;
use crate::engine::gate::diff::fingerprint::{DiffCategory, DiffKind};
use crate::engine::gate::diff::{
    CoveragePolicy, DiffEngineOptions, policy, renames, run_diff_engine,
};

/// How a routine of the change pairs across the two sides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pairing {
    /// The same stable id on both sides.
    Id,
    /// `renames.rs` paired it: the base routine's id differs.
    Signature { base: String },
    /// Only the working tree has it (complete inventories).
    Added { reason: Option<String> },
}

/// One side of a before/after answer as the main part printed it with
/// `--summaries`.
pub struct Side<'a> {
    pub doc: &'a Value,
    pub summaries: HashMap<String, RootSummary>,
    pub contract: CborValue,
    /// The routine ids of its complete inventory (`contractFacts`).
    pub inventory: HashSet<String>,
}

impl<'a> Side<'a> {
    /// The side of a main part's answer, or why it has none.
    pub fn of(doc: &'a Value) -> Result<Side<'a>, String> {
        let summaries: HashMap<String, RootSummary> = match doc.get("summaries") {
            Some(v) if v.is_object() => serde_json::from_value(v.clone())
                .map_err(|e| format!("the summaries do not read: {e}"))?,
            _ => return Err("the part printed no summaries".into()),
        };
        let contract_json = doc
            .get("contract")
            .filter(|c| c.is_object())
            .ok_or("the part printed no contract snapshot")?;
        let contract = crate::engine::gate::snapshot_deserialize::json_to_cbor(contract_json);
        let inventory = contract_json["contractFacts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|f| f["kind"] == "routine")
            .filter_map(|f| f["stableId"].as_str().map(str::to_string))
            .collect();
        Ok(Side {
            doc,
            summaries,
            contract,
            inventory,
        })
    }

    fn roots(&self) -> Vec<String> {
        self.doc["roots"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| r["stableId"].as_str().map(str::to_string))
            .collect()
    }
}

/// A key's change on one routine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyChange {
    pub key: String,
    pub label: String,
    pub base: usize,
    pub new: usize,
    /// `added`, `removed`, `no-longer-observed`, `count`.
    pub wording: &'static str,
    /// Why "no longer observed".
    pub reason: Option<String>,
    /// The sites only the working tree has (`ws:file:line`) and only the base
    /// has; `ambiguous` when the operation identities cannot say which.
    pub added_sites: Vec<String>,
    pub removed_sites: Vec<String>,
    pub ambiguous: bool,
    /// Its target belongs to a dependency app.
    pub dependency: bool,
}

impl KeyChange {
    fn json(&self) -> Value {
        let mut v = json!({
            "key": self.key, "label": self.label, "base": self.base, "new": self.new,
            "change": self.wording,
        });
        if let Some(r) = &self.reason {
            v["reason"] = json!(r);
        }
        if !self.added_sites.is_empty() {
            v["addedSites"] = json!(self.added_sites);
        }
        if !self.removed_sites.is_empty() {
            v["removedSites"] = json!(self.removed_sites);
        }
        if self.ambiguous {
            v["sites"] = json!("ambiguous");
        }
        if self.dependency {
            v["dependencyReaching"] = json!(true);
        }
        v
    }

    /// The human clause (`+ write T Modify (physical)`, `Modify on T: 2 → 1
    /// sites`, `- commit: no longer observed (…)`).
    #[must_use]
    pub fn human(&self) -> String {
        match self.wording {
            "added" => format!("+ {}", self.label),
            "removed" => format!("- {}", self.label),
            "no-longer-observed" => format!(
                "- {}: no longer observed ({})",
                self.label,
                self.reason
                    .as_deref()
                    .unwrap_or("the new side is incomplete")
            ),
            _ => {
                let which = if self.ambiguous {
                    " (which site: ambiguous)".to_string()
                } else if !self.added_sites.is_empty() {
                    format!(" (added at {})", self.added_sites.join(", "))
                } else if !self.removed_sites.is_empty() {
                    format!(" (removed: base {})", self.removed_sites.join(", "))
                } else {
                    String::new()
                };
                format!("{}: {} → {} sites{which}", self.label, self.base, self.new)
            }
        }
    }
}

/// The table or resource guid a key names, when it is an app object.
fn key_guid(key: &str) -> Option<&str> {
    let mut parts = key.split('|');
    let fam = parts.next()?;
    let target = match fam {
        "write" => parts.next()?,
        _ if fam.chars().all(|c| c.is_ascii_uppercase() || c == '_') => parts.next()?,
        "callee" | "event" => parts.next()?,
        _ => return None,
    };
    let guid = target.split(['/', ':']).next()?;
    (guid.len() == 36).then_some(guid)
}

/// Pair the sites of one key by operation identity, in source order: the
/// sites only the new side has, only the base has, and whether that split
/// is ambiguous (one identity on both sides with differing counts above
/// one).
fn pair_sites(base: &[Site], new: &[Site]) -> (Vec<String>, Vec<String>, bool) {
    let mut by_ident: BTreeMap<&str, (Vec<&Site>, Vec<&Site>)> = BTreeMap::new();
    for s in base {
        by_ident.entry(s.ident.as_str()).or_default().0.push(s);
    }
    for s in new {
        by_ident.entry(s.ident.as_str()).or_default().1.push(s);
    }
    let (mut added, mut removed, mut ambiguous) = (Vec::new(), Vec::new(), false);
    let at = |s: &Site| format!("{}:{}", s.file, s.line);
    for (b, n) in by_ident.values() {
        if b.len() == n.len() {
            continue;
        }
        if !b.is_empty() && !n.is_empty() && (b.len() > 1 || n.len() > 1) {
            ambiguous = true;
            continue;
        }
        if b.len() < n.len() {
            added.extend(n[b.len()..].iter().map(|s| at(s)));
        } else {
            removed.extend(b[n.len()..].iter().map(|s| at(s)));
        }
    }
    (added, removed, ambiguous)
}

/// The cone key an own key's effect shows as (a cone change it explains is
/// not printed twice).
fn cone_twin(own: &str) -> Option<String> {
    let p: Vec<&str> = own.split('|').collect();
    Some(match p.as_slice() {
        ["write", t, op, _] => {
            let ty = match op.to_ascii_lowercase().as_str() {
                "insert" => "DB_INSERT",
                "modify" | "modifyall" | "rename" => "DB_MODIFY",
                "delete" | "deleteall" => "DB_DELETE",
                _ => return None,
            };
            format!("{ty}|{t}")
        }
        ["commit"] => "COMMIT|".into(),
        // ENG-09: a checked run's commit (`keys`: `commit|checked-run`).
        ["commit", r] => format!("COMMIT|{r}"),
        ["error", "error-throw", _] => "ERROR_THROW|".into(),
        // ENG-22: a checked run's own error (`keys`: `error|error-implicit|checked-run`).
        ["error", "error-implicit", r] if !r.is_empty() => format!("ERROR_IMPLICIT|{r}"),
        ["error", _, _] => "ERROR_IMPLICIT|".into(),
        ["ui", op] => format!("{}|", op.to_ascii_uppercase().replace('-', "_")),
        ["http", _] => "HTTP|".into(),
        ["file", _] => "FILE|".into(),
        ["background", _] => "BACKGROUND_TASK|".into(),
        _ => return None,
    })
}

/// The loss wording of a key only (or less often) on the base side, through
/// the diff policy's rule.
fn loss(new: &RootSummary, key: &str) -> (&'static str, Option<String>) {
    // A call in the routine's own body is read off the body itself: when the
    // body was analyzed, a call it no longer makes is removed, whatever the
    // cone beyond it could not see.
    if (key.starts_with("callee|") || key.starts_with("event|")) && !new.coverage.no_body {
        return ("removed", None);
    }
    let mut why = new.coverage.why();
    if let ["write", t, op, _] = key.split('|').collect::<Vec<_>>().as_slice() {
        let undecided = new.own.keys().any(|k| {
            let q: Vec<&str> = k.split('|').collect();
            q.len() == 4
                && q[1] == *t
                && q[2] == *op
                && (q[3] == "unknown" || q[3] == "caller-dependent")
        });
        if undecided {
            why.push("the new side writes it under an undecided class".into());
        }
    }
    match policy::loss_kind(!why.is_empty()) {
        DiffKind::CapabilityLostUnderCoverage => ("no-longer-observed", Some(why.join("; "))),
        _ => ("removed", None),
    }
}

/// The delta of one paired routine.
#[must_use]
pub fn root_delta(base: &RootSummary, new: &RootSummary) -> (Vec<KeyChange>, Vec<KeyChange>) {
    root_delta_with(base, new, &HashMap::new())
}

/// [`root_delta`], with the base's callee ids that paired by signature
/// (`renamed`: base id → working-tree id) read as their working-tree ids: a
/// call to a routine whose parameter list changed is the same call.
#[must_use]
pub fn root_delta_with(
    base: &RootSummary,
    new: &RootSummary,
    renamed: &HashMap<String, String>,
) -> (Vec<KeyChange>, Vec<KeyChange>) {
    let translated;
    let base = if renamed.is_empty() {
        base
    } else {
        let mut b = base.clone();
        b.own = base
            .own
            .iter()
            .map(|(k, v)| {
                let k = match k.split_once('|') {
                    Some((fam @ ("callee" | "event"), id)) => match renamed.get(id) {
                        Some(n) => {
                            if let Some(l) = base.labels.get(k) {
                                b.labels.insert(format!("{fam}|{n}"), l.clone());
                            }
                            format!("{fam}|{n}")
                        }
                        None => k.clone(),
                    },
                    _ => k.clone(),
                };
                (k, v.clone())
            })
            .fold(
                BTreeMap::new(),
                |mut m: BTreeMap<String, Vec<Site>>, (k, v)| {
                    m.entry(k).or_default().extend(v);
                    m
                },
            );
        translated = b;
        &translated
    };
    let label = |k: &str| {
        new.labels
            .get(k)
            .or_else(|| base.labels.get(k))
            .cloned()
            .unwrap_or_else(|| k.to_string())
    };
    let empty: Vec<Site> = Vec::new();
    let keys: BTreeSet<&String> = base.own.keys().chain(new.own.keys()).collect();
    let mut own = Vec::new();
    for k in keys {
        let b = base.own.get(k).unwrap_or(&empty);
        let n = new.own.get(k).unwrap_or(&empty);
        if b.len() == n.len() {
            continue;
        }
        let (added_sites, removed_sites, ambiguous) = pair_sites(b, n);
        let (wording, reason) = if b.is_empty() {
            ("added", None)
        } else if n.is_empty() {
            loss(new, k)
        } else {
            ("count", None)
        };
        own.push(KeyChange {
            key: k.clone(),
            label: label(k),
            base: b.len(),
            new: n.len(),
            wording,
            reason,
            added_sites,
            removed_sites,
            ambiguous,
            dependency: key_guid(k).is_some_and(|g| !is_primary(g, new, base)),
        });
    }
    // The cone's changes no own change explains.
    let explained: HashSet<String> = own
        .iter()
        .filter(|c| c.base == 0 || c.new == 0)
        .filter_map(|c| cone_twin(&c.key))
        .collect();
    let mut cone = Vec::new();
    for k in new.cone.symmetric_difference(&base.cone) {
        if explained.contains(k) {
            continue;
        }
        let gained = new.cone.contains(k);
        let (wording, reason) = if gained {
            ("added", None)
        } else {
            loss(new, k)
        };
        cone.push(KeyChange {
            key: k.clone(),
            label: label(k),
            base: usize::from(!gained),
            new: usize::from(gained),
            wording,
            reason,
            added_sites: Vec::new(),
            removed_sites: Vec::new(),
            ambiguous: false,
            dependency: key_guid(k).is_some_and(|g| !is_primary(g, new, base)),
        });
    }
    (own, cone)
}

/// Whether `guid` is the app of the routine (its stable id's guid).
fn is_primary(guid: &str, new: &RootSummary, base: &RootSummary) -> bool {
    let of = |s: &RootSummary| s.stable_id.split(':').next().map(str::to_string);
    of(new)
        .or_else(|| of(base))
        .is_some_and(|g| g.eq_ignore_ascii_case(guid))
}

/// The whole delta (al-sem.impact/1's `delta` blocks, `deleted`, `added`,
/// `schemaChanges`), from the base side and the new side. `dependency_note`:
/// the changed dependency declarations, when the change edits `app.json`'s
/// dependencies (every dependency-reaching change is then unreliable).
#[must_use]
pub fn compute(base: &Side<'_>, new: &Side<'_>, dependency_note: Option<&str>) -> Value {
    // Pairing: by id, then renames.rs over the complete inventories.
    let auto = renames::pair_signature_changes(
        &base.contract,
        &new.contract,
        &renames::RenameTable::new(),
    );
    let by_new: HashMap<&str, &str> = auto.iter().map(|(o, n)| (n.as_str(), o.as_str())).collect();
    let by_base: HashMap<&str, &str> = auto.iter().map(|(o, n)| (o.as_str(), n.as_str())).collect();
    let renamed: HashMap<String, String> = auto.iter().cloned().collect();
    let mut deltas = serde_json::Map::new();
    let mut changed = 0usize;
    let mut paired = 0usize;
    for id in new.roots() {
        let (pairing, base_id) = if base.inventory.contains(&id) {
            (Pairing::Id, Some(id.clone()))
        } else if let Some(b) = by_new.get(id.as_str()) {
            (
                Pairing::Signature {
                    base: (*b).to_string(),
                },
                Some((*b).to_string()),
            )
        } else {
            (
                Pairing::Added {
                    reason: unpaired_reason(&id, new, base),
                },
                None,
            )
        };
        let d = match (&pairing, base_id.as_deref()) {
            (Pairing::Added { reason }, _) => {
                json!({"pairing": "added", "reason": reason, "note": "new: its current effects are its whole behavior"})
            }
            (_, Some(b)) => {
                paired += 1;
                match (base.summaries.get(b), new.summaries.get(&id)) {
                    (Some(bs), Some(ns)) => {
                        let (own, cone) = root_delta_with(bs, ns, &renamed);
                        if !own.is_empty() || !cone.is_empty() {
                            changed += 1;
                        }
                        let unreliable = dependency_note.filter(|_| {
                            own.iter().chain(&cone).any(|c| c.dependency)
                                || ns.coverage.gaps_dependency > 0
                        });
                        json!({
                            "pairing": if pairing == Pairing::Id { "id" } else { "signature" },
                            "baseStableId": b,
                            "own": own.iter().map(KeyChange::json).collect::<Vec<_>>(),
                            "cone": cone.iter().map(KeyChange::json).collect::<Vec<_>>(),
                            "coverage": {"base": bs.coverage, "new": ns.coverage},
                            "unreliable": unreliable.map(|n| format!("the dependency declaration changed ({n})")),
                        })
                    }
                    (bs, ns) => json!({
                        "pairing": if pairing == Pairing::Id { "id" } else { "signature" },
                        "baseStableId": b,
                        "notComputed": match (bs.is_some(), ns.is_some()) {
                            (false, _) => "the base side computed no summary for it",
                            _ => "the new side computed no summary for it",
                        },
                    }),
                }
            }
            _ => Value::Null,
        };
        deltas.insert(id, d);
    }
    // Deleted: base roots no working-tree routine has or pairs with.
    let mut deleted = Vec::new();
    for id in base.roots() {
        if new.inventory.contains(&id) || by_base.contains_key(id.as_str()) {
            continue;
        }
        let s = base.summaries.get(&id);
        deleted.push(json!({
            "stableId": id,
            "display": s.map(|s| s.display.clone()),
            "handle": s.map(|s| s.handle.clone()),
            "anchor": s.map(|s| json!({"file": s.file, "line": s.line})),
            "baseCallers": s.map(|s| &s.callers),
            "reason": unpaired_reason(&id, base, new),
        }));
    }
    // Schema and contract findings: the diff engine over the two contracts.
    let result = run_diff_engine(
        &base.contract,
        &new.contract,
        &DiffEngineOptions {
            coverage_policy: CoveragePolicy::Loose,
            deterministic: true,
            rename_overlay: None,
        },
    );
    let findings: Vec<Value> = result
        .findings
        .iter()
        .filter(|f| matches!(f.category, DiffCategory::Schema | DiffCategory::Abi))
        .map(|f| cbor_json(&crate::engine::gate::diff::format::finding_to_cbor(f)))
        .collect();
    let diagnostics: Vec<Value> = result
        .diagnostics
        .iter()
        .map(|d| cbor_json(&crate::engine::gate::diff::format::diagnostic_to_cbor(d)))
        .collect();
    json!({
        "deltas": deltas,
        "paired": paired,
        "behaviorChanged": changed,
        "deleted": deleted,
        "schemaChanges": {
            "findings": findings,
            "diagnostics": diagnostics,
            "fatal": result.fatal,
            "categories": ["abi", "schema"],
            "notComputed": "events, capabilities and permissions (alsem diff <base> <new> gives them)",
        },
    })
}

/// Why a routine only one side has did not pair (`renames.rs`'s rules), or
/// `None` when no routine of its object and name is on the other side.
fn unpaired_reason(id: &str, this: &Side<'_>, other: &Side<'_>) -> Option<String> {
    let (object, _) = id.rsplit_once('#')?;
    let name_of = |side: &Side<'_>, rid: &str| -> Option<String> {
        let ids = side.contract_ids();
        ids.get(rid).cloned()
    };
    let name = name_of(this, id)?.fold_identifier();
    let others: Vec<String> = other
        .contract_ids()
        .into_iter()
        .filter(|(rid, n)| {
            rid.rsplit_once('#').is_some_and(|(o, _)| o == object)
                && n.fold_identifier() == name
                && !this.inventory.contains(rid)
        })
        .map(|(rid, _)| rid)
        .collect();
    if others.is_empty() {
        return None;
    }
    Some(format!(
        "{} of the same object and name only on the other side (renames.rs pairs one-to-one, same ABI status, member-less routines only)",
        crate::engine::l5::routine_rollup::plural(others.len(), "routine", "routines")
    ))
}

impl Side<'_> {
    /// Routine id → display name, from the contract snapshot's identities.
    fn contract_ids(&self) -> HashMap<String, String> {
        let c = &self.doc["contract"]["identities"];
        c["stableIds"]
            .as_array()
            .into_iter()
            .flatten()
            .zip(c["displayNames"].as_array().into_iter().flatten())
            .filter_map(|(i, n)| Some((i.as_str()?.to_string(), n.as_str()?.to_string())))
            .filter(|(i, _)| self.inventory.contains(i))
            .collect()
    }
}

/// A `CborValue` as JSON (keys in insertion order).
pub(crate) fn cbor_json(v: &CborValue) -> Value {
    match v {
        CborValue::Null | CborValue::Undefined => Value::Null,
        CborValue::Bool(b) => json!(b),
        CborValue::Int(n) => json!(n),
        CborValue::Float(f) => json!(f),
        CborValue::Text(s) => json!(s),
        CborValue::Array(a) => Value::Array(a.iter().map(cbor_json).collect()),
        CborValue::Map(m) => {
            Value::Object(m.iter().map(|(k, v)| (k.clone(), cbor_json(v))).collect())
        }
    }
}

#[cfg(test)]
#[path = "delta_tests.rs"]
mod tests;
