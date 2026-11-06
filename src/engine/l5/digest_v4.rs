//! The digest 4.0.0 document (output validation spec 2026-11-06, §7.3,
//! F41): the 3.x document with what several roots share written once.
//!
//! A digest of many roots (round 1's c25: 20 roots of one file) repeated
//! each shared effect, crossing, path and unresolved call site under every
//! root. In 4.0.0:
//!
//! - **Effects**: the intrinsic facts of an effect row (its type, site,
//!   table, boundary, resources, class, events, crossing) are one entry of
//!   `payload.effects` (`e:<n>`). Each root lists its effects as
//!   **references with annotations**, `{effect: "e:<n>", …}`, and what can
//!   differ by root stays on the reference: literal liveness (`status`),
//!   the temp state in that root's frame (`detail.tempState`,
//!   `detail.resourceCount`), conditionality, provenance, the paths and
//!   their counts, and the ordering and refutation evidence (`guarantees`,
//!   `scopedGuarantees` with `validForRefutation`, `refutableGuarantees`)
//!   with the row's own `factId`. A root's liveness and temp state come from
//!   its reference, never from the shared row.
//! - **Crossings**: a dependency call's boundary, sites and targets are one
//!   entry of `payload.crossings`; each root keeps its own path to it
//!   (`prefix`, or `prefixOmitted`) under `entries[].crossings`.
//! - **Paths**: a run of hops several paths end with is one entry of
//!   `payload.pathTails` (`t:<n>`); a path's `hops` end with `{"tail":
//!   "t:<n>"}` (a tail may end with another tail).
//! - **Unresolved call sites**: `entries[].unresolved` holds `u:<n>` ids
//!   into `payload.unresolvedSites`.
//! - **Defaults**: `payload.defaults.effect` holds the most common
//!   `conditionality` and `provenance`; a reference carries them only when
//!   they differ. (Liveness and temp state are never defaulted.)
//!
//! A root's view is the shared facts plus its own annotations, and equals
//! the 3.x entry for the same input: [`to_v3`] is [`from_v3`]'s inverse, and
//! the semantic gate of §12.1 checks `to_v3(from_v3(p)) == p` on every
//! digest golden and on a fixture site live from one root and ruled out
//! from another. `--schema 3` writes the 3.x document for one release.

use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap};

/// The schema version of the default document.
pub const VERSION: &str = "4.0.0";

/// An effect row's fields that can differ by root (module doc).
const ANNOTATIONS: &[&str] = &[
    "factId",
    "status",
    "conditionality",
    "provenance",
    "guarantees",
    "scopedGuarantees",
    "refutableGuarantees",
    "paths",
    "pathsKept",
    "pathsOmitted",
    "pathsProducerTruncated",
    "pathUnavailable",
    "viaPathsTruncated",
];
/// The `detail` keys that can differ by root.
const DETAIL_ANNOTATIONS: &[&str] = &["tempState", "resourceCount"];
/// A crossing's fields that are the root's own (its path to the crossing).
const CROSSING_ANNOTATIONS: &[&str] = &["prefix", "prefixOmitted"];
/// The annotation fields a document default may carry.
const DEFAULTED: &[&str] = &["conditionality", "provenance"];
/// The reference's key naming the shared effect.
const EFFECT: &str = "effect";
/// A tail reference inside `hops`.
const TAIL: &str = "tail";
/// The shortest tail worth sharing.
const MIN_TAIL: usize = 2;

/// The 4.0.0 payload of a 3.x payload (module doc).
#[must_use]
pub fn from_v3(mut payload: Value) -> Value {
    let Some(Value::Array(entries)) = payload.get_mut("entries") else {
        return payload;
    };
    let effects = share_effects(entries);
    let crossings = share_crossings(entries);
    let tails = share_tails(entries);
    let sites = share_unresolved(entries);
    let defaults = factor_defaults(entries);
    let m = payload.as_object_mut().expect("a payload object");
    for (key, table) in [
        ("effects", effects),
        ("crossings", crossings),
        ("pathTails", tails),
        ("unresolvedSites", sites),
    ] {
        if !table.is_empty() {
            m.insert(key.into(), Value::Object(table));
        }
    }
    if !defaults.is_empty() {
        m.insert("defaults".into(), json!({ "effect": defaults }));
    }
    payload
}

/// The 3.x payload of a 4.0.0 one: [`from_v3`]'s inverse.
#[must_use]
pub fn to_v3(mut payload: Value) -> Value {
    let Value::Object(m) = &mut payload else {
        return payload;
    };
    let table = |m: &mut Map<String, Value>, k: &str| match m.remove(k) {
        Some(Value::Object(t)) => t,
        _ => Map::new(),
    };
    let effects = table(m, "effects");
    let crossings = table(m, "crossings");
    let tails = table(m, "pathTails");
    let sites = table(m, "unresolvedSites");
    let defaults = m
        .remove("defaults")
        .and_then(|d| d.get("effect").cloned())
        .and_then(|d| d.as_object().cloned())
        .unwrap_or_default();
    let Some(Value::Array(entries)) = m.get_mut("entries") else {
        return payload;
    };
    for e in entries.iter_mut() {
        if let Some(Value::Array(rows)) = e.get_mut("effects") {
            for r in rows.iter_mut() {
                let Value::Object(row) = r else { continue };
                for (k, v) in &defaults {
                    row.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
        }
        if let Some(Value::Array(us)) = e.get_mut("unresolved") {
            for u in us.iter_mut() {
                if let Some(site) = u.as_str().and_then(|id| sites.get(id)) {
                    *u = site.clone();
                }
            }
        }
        expand_tails(e, &tails);
        if let Some(Value::Object(cs)) = e.get_mut("crossings") {
            for (id, c) in cs.iter_mut() {
                if let (Value::Object(own), Some(Value::Object(shared))) = (c, crossings.get(id)) {
                    for (k, v) in shared {
                        own.entry(k.clone()).or_insert_with(|| v.clone());
                    }
                }
            }
        }
        if let Some(Value::Array(rows)) = e.get_mut("effects") {
            for r in rows.iter_mut() {
                let Value::Object(row) = r else { continue };
                let Some(Value::Object(shared)) = row
                    .remove(EFFECT)
                    .and_then(|id| id.as_str().and_then(|id| effects.get(id)).cloned())
                else {
                    continue;
                };
                merge_effect(row, shared);
            }
        }
    }
    payload
}

/// A whole digest document in 4.0.0.
#[must_use]
pub fn document_from_v3(mut doc: Value) -> Value {
    if let Value::Object(m) = &mut doc {
        if let Some(p) = m.remove("payload") {
            m.insert("payload".into(), from_v3(p));
        }
        m.insert("schemaVersion".into(), VERSION.into());
    }
    doc
}

/// A 4.0.0 document with its payload back in the 3.x shape (the version
/// field is left as it is: in-repo readers and the gate read the payload).
#[must_use]
pub fn document_to_v3(mut doc: Value) -> Value {
    if let Value::Object(m) = &mut doc
        && let Some(p) = m.remove("payload")
    {
        m.insert("payload".into(), to_v3(p));
    }
    doc
}

/// The shared row and the annotations into the 3.x row.
fn merge_effect(row: &mut Map<String, Value>, shared: Map<String, Value>) {
    for (k, v) in shared {
        match (k.as_str(), row.get_mut(&k), v) {
            ("detail", Some(Value::Object(own)), Value::Object(s)) => {
                for (dk, dv) in s {
                    own.entry(dk).or_insert(dv);
                }
            }
            (_, Some(_), _) => {}
            (_, None, v) => {
                row.insert(k, v);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Effects
// ---------------------------------------------------------------------------

fn share_effects(entries: &mut [Value]) -> Map<String, Value> {
    let mut table = Map::new();
    let mut ids: HashMap<String, String> = HashMap::new();
    for e in entries.iter_mut() {
        let Some(Value::Array(rows)) = e.get_mut("effects") else {
            continue;
        };
        for r in rows.iter_mut() {
            let Value::Object(row) = std::mem::take(r) else {
                continue;
            };
            let mut shared = Map::new();
            let mut own = Map::new();
            for (k, v) in row {
                if ANNOTATIONS.contains(&k.as_str()) {
                    own.insert(k, v);
                } else if k == "detail"
                    && let Value::Object(d) = v
                {
                    let (mine, theirs): (Map<String, Value>, Map<String, Value>) = d
                        .into_iter()
                        .partition(|(dk, _)| DETAIL_ANNOTATIONS.contains(&dk.as_str()));
                    if !mine.is_empty() {
                        own.insert("detail".into(), Value::Object(mine));
                    }
                    shared.insert("detail".into(), Value::Object(theirs));
                } else {
                    shared.insert(k, v);
                }
            }
            let shared = Value::Object(shared);
            let key = shared.to_string();
            let id = match ids.get(&key) {
                Some(id) => id.clone(),
                None => {
                    let id = format!("e:{}", ids.len() + 1);
                    table.insert(id.clone(), shared);
                    ids.insert(key, id.clone());
                    id
                }
            };
            own.insert(EFFECT.into(), Value::String(id));
            *r = Value::Object(own);
        }
    }
    table
}

// ---------------------------------------------------------------------------
// Crossings
// ---------------------------------------------------------------------------

fn share_crossings(entries: &mut [Value]) -> Map<String, Value> {
    // A crossing id is shared only when every root's intrinsic part agrees.
    let mut seen: BTreeMap<String, Option<Map<String, Value>>> = BTreeMap::new();
    let split = |c: &Map<String, Value>| -> Map<String, Value> {
        c.iter()
            .filter(|(k, _)| !CROSSING_ANNOTATIONS.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    };
    for e in entries.iter() {
        if let Some(Value::Object(cs)) = e.get("crossings") {
            for (id, c) in cs {
                let Value::Object(c) = c else { continue };
                let mine = split(c);
                match seen.get(id) {
                    None => {
                        seen.insert(id.clone(), Some(mine));
                    }
                    Some(Some(prev)) if *prev != mine => {
                        seen.insert(id.clone(), None);
                    }
                    Some(_) => {}
                }
            }
        }
    }
    let mut table = Map::new();
    for e in entries.iter_mut() {
        if let Some(Value::Object(cs)) = e.get_mut("crossings") {
            for (id, c) in cs.iter_mut() {
                let (Value::Object(own), Some(Some(shared))) = (c, seen.get(id)) else {
                    continue;
                };
                own.retain(|k, _| CROSSING_ANNOTATIONS.contains(&k.as_str()));
                table
                    .entry(id.clone())
                    .or_insert_with(|| Value::Object(shared.clone()));
            }
        }
    }
    table
}

// ---------------------------------------------------------------------------
// Path tails
// ---------------------------------------------------------------------------

/// Every `hops` array of the entries' effect paths and crossing prefixes.
fn hops_arrays(e: &mut Value) -> Vec<&mut Vec<Value>> {
    let mut out = Vec::new();
    let (effects, crossings) = match e {
        Value::Object(m) => {
            let mut effects = None;
            let mut crossings = None;
            for (k, v) in m.iter_mut() {
                match k.as_str() {
                    "effects" => effects = Some(v),
                    "crossings" => crossings = Some(v),
                    _ => {}
                }
            }
            (effects, crossings)
        }
        _ => return out,
    };
    if let Some(Value::Array(rows)) = effects {
        for r in rows {
            if let Some(Value::Array(paths)) = r.get_mut("paths") {
                for p in paths {
                    if let Some(Value::Array(h)) = p.get_mut("hops") {
                        out.push(h);
                    }
                }
            }
        }
    }
    if let Some(Value::Object(cs)) = crossings {
        for c in cs.values_mut() {
            if let Some(Value::Array(h)) = c.get_mut("prefix").and_then(|p| p.get_mut("hops")) {
                out.push(h);
            }
        }
    }
    out
}

/// Share every suffix of two or more hops that ends two or more paths: the
/// longest shared suffix of a path becomes a tail; a tail ends with the
/// longest shared suffix shorter than itself, if any.
fn share_tails(entries: &mut [Value]) -> Map<String, Value> {
    // Suffixes as hop-key sequences, counted once per path.
    let key = |h: &Value| h.to_string();
    let mut count: HashMap<Vec<String>, usize> = HashMap::new();
    for e in entries.iter_mut() {
        for h in hops_arrays(e) {
            let keys: Vec<String> = h.iter().map(key).collect();
            for k in 0..keys.len() {
                if keys.len() - k >= MIN_TAIL {
                    *count.entry(keys[k..].to_vec()).or_default() += 1;
                }
            }
        }
    }
    let shared = |s: &[String]| s.len() >= MIN_TAIL && count.get(s).is_some_and(|n| *n >= 2);
    let count_of = |s: &[String]| count.get(s).copied().unwrap_or(0);
    let mut table = Map::new();
    let mut ids: HashMap<Vec<String>, String> = HashMap::new();
    let mut hop_of: HashMap<String, Value> = HashMap::new();
    for e in entries.iter_mut() {
        for h in hops_arrays(e) {
            for x in h.iter() {
                hop_of.entry(key(x)).or_insert_with(|| x.clone());
            }
            let keys: Vec<String> = h.iter().map(key).collect();
            let Some(k) = (0..keys.len()).find(|&k| shared(&keys[k..])) else {
                continue;
            };
            let id = tail_id(
                &keys[k..],
                &shared,
                &count_of,
                &mut ids,
                &mut table,
                &hop_of,
            );
            h.truncate(k);
            h.push(json!({ TAIL: id }));
        }
    }
    table
}

fn tail_id(
    s: &[String],
    shared: &dyn Fn(&[String]) -> bool,
    count: &dyn Fn(&[String]) -> usize,
    ids: &mut HashMap<Vec<String>, String>,
    table: &mut Map<String, Value>,
    hop_of: &HashMap<String, Value>,
) -> String {
    if let Some(id) = ids.get(s) {
        return id.clone();
    }
    let id = format!("t:{}", ids.len() + 1);
    ids.insert(s.to_vec(), id.clone());
    // A tail ends with a shorter tail only when more paths end with that
    // one (else it is used only through this tail and adds nothing).
    let mine = count(s);
    let rest = (1..s.len()).find(|&k| shared(&s[k..]) && count(&s[k..]) > mine);
    let mut hops: Vec<Value> = s[..rest.unwrap_or(s.len())]
        .iter()
        .map(|k| hop_of[k].clone())
        .collect();
    if let Some(k) = rest {
        let inner = tail_id(&s[k..], shared, count, ids, table, hop_of);
        hops.push(json!({ TAIL: inner }));
    }
    table.insert(id.clone(), Value::Array(hops));
    id
}

fn expand_tails(e: &mut Value, tails: &Map<String, Value>) {
    for h in hops_arrays(e) {
        while let Some(id) = h
            .last()
            .and_then(|x| x.get(TAIL))
            .and_then(Value::as_str)
            .map(str::to_string)
        {
            h.pop();
            match tails.get(&id) {
                Some(Value::Array(t)) => h.extend(t.iter().cloned()),
                _ => break,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Unresolved call sites
// ---------------------------------------------------------------------------

fn share_unresolved(entries: &mut [Value]) -> Map<String, Value> {
    let mut table = Map::new();
    let mut ids: HashMap<String, String> = HashMap::new();
    for e in entries.iter_mut() {
        let Some(Value::Array(us)) = e.get_mut("unresolved") else {
            continue;
        };
        for u in us.iter_mut() {
            if !u.is_object() {
                continue;
            }
            let key = u.to_string();
            let id = match ids.get(&key) {
                Some(id) => id.clone(),
                None => {
                    let id = format!("u:{}", ids.len() + 1);
                    table.insert(id.clone(), u.clone());
                    ids.insert(key, id.clone());
                    id
                }
            };
            *u = Value::String(id);
        }
    }
    table
}

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

/// For each [`DEFAULTED`] field carried by every reference: its most common
/// value (ties: the smallest), when more than half carry it; removed from
/// the references that do. A field some reference lacks is not defaulted
/// (its absence could not be told from the default).
fn factor_defaults(entries: &mut [Value]) -> Map<String, Value> {
    let mut defaults = Map::new();
    for field in DEFAULTED {
        let mut count: BTreeMap<String, (usize, Value)> = BTreeMap::new();
        let mut rows = 0;
        let mut missing = false;
        for e in entries.iter() {
            for r in e
                .get("effects")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                rows += 1;
                match r.get(*field) {
                    Some(v) => count.entry(v.to_string()).or_insert((0, v.clone())).0 += 1,
                    None => missing = true,
                }
            }
        }
        if missing || rows == 0 {
            continue;
        }
        let Some((_, (n, v))) = count
            .iter()
            .max_by(|a, b| a.1.0.cmp(&b.1.0).then(b.0.cmp(a.0)))
        else {
            continue;
        };
        if n * 2 <= rows {
            continue;
        }
        let v = v.clone();
        for e in entries.iter_mut() {
            if let Some(Value::Array(rs)) = e.get_mut("effects") {
                for r in rs.iter_mut() {
                    if let Value::Object(row) = r
                        && row.get(*field) == Some(&v)
                    {
                        row.remove(*field);
                    }
                }
            }
        }
        defaults.insert((*field).to_string(), v);
    }
    defaults
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hop(line: u64, to: &str) -> Value {
        json!({"edge": "direct-call", "file": "ws:a.al", "line": line, "to": to})
    }

    fn effect(fact: &str, status: &str, temp: &str, hops: Vec<Value>) -> Value {
        json!({"factId": fact, "type": "DB_INSERT", "status": status, "provenance": "transitive",
               "conditionality": "unknown", "confidence": "static",
               "evidence": {"file": "ws:b.al", "line": 9, "column": 4},
               "detail": {"table": "T", "tempState": temp},
               "guarantees": ["COMMIT_REACHABLE"],
               "paths": [{"hops": hops, "terminal": {"file": "ws:b.al", "line": 9}}]})
    }

    /// §7.3's test: one site, live from one root and ruled out (and
    /// temporary) from another, keeps both answers after sharing; the
    /// intrinsic facts are written once, and every root's entry comes back
    /// equal to its 3.x entry. Discrimination (2026-11-06): dropping the
    /// per-root `status` annotation in `share_effects` fails here (and the
    /// CLI gate `output_shared_evidence`); restored, it passes.
    #[test]
    fn a_site_live_from_one_root_and_ruled_out_from_another_keeps_both() {
        let tail = vec![hop(10, "r:b"), hop(20, "r:c"), hop(30, "r:d")];
        let mut a = vec![hop(1, "r:a")];
        a.extend(tail.clone());
        let mut b = vec![hop(2, "r:x")];
        b.extend(tail);
        let v3 = json!({"entries": [
            {"routine": {"stableId": "A"}, "effects": [effect("f1", "live", "known(false)", a)],
             "unresolved": [{"calleeDisplay": "X.Run"}],
             "crossings": {"c:1": {"boundary": "B", "sites": ["s"], "prefix": {"hops": [hop(5, "r:q")]}}}},
            {"routine": {"stableId": "B"}, "effects": [effect("f2", "dead", "known(true)", b)],
             "unresolved": [{"calleeDisplay": "X.Run"}],
             "crossings": {"c:1": {"boundary": "B", "sites": ["s"], "prefixOmitted": true}}},
        ]});
        let v4 = from_v3(v3.clone());
        assert_eq!(v4["effects"].as_object().unwrap().len(), 1, "{v4:#}");
        let e = |i: usize| &v4["entries"][i]["effects"][0];
        assert_eq!(e(0)["effect"], e(1)["effect"]);
        assert_eq!(
            (e(0)["status"].as_str(), e(1)["status"].as_str()),
            (Some("live"), Some("dead"))
        );
        assert_eq!(e(1)["detail"]["tempState"], "known(true)");
        assert!(v4["effects"]["e:1"]["detail"].get("tempState").is_none());
        assert!(v4["effects"]["e:1"].get("status").is_none());
        assert_eq!(v4["pathTails"].as_object().unwrap().len(), 1);
        assert_eq!(v4["crossings"]["c:1"]["boundary"], "B");
        assert_eq!(v4["entries"][1]["unresolved"][0], "u:1");
        assert_eq!(v4["defaults"]["effect"]["provenance"], "transitive");
        assert_eq!(to_v3(v4), v3);
    }
}
