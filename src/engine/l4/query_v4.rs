//! The query 4.0.0 document (output validation spec 2026-11-06, §7.1, §7.2,
//! §8.3): the 3.x document with each repeated thing stated once.
//!
//! - **Routines** (§7.2, F40): every routine identity a row names (`origin`'s
//!   routine, `firstHop`, a caller's `routine`) is one entry of
//!   `payload.routines` (`r:<n>` → display, signature, object, definition
//!   anchor, stableId, selector), and the row carries `"r:<n>"`. An
//!   operation's own place stays on its row: `origin` is `{routine: "r:<n>",
//!   file, line, column}` (ConTinAI reads `origin.line`, no join needed).
//! - **`via`** (§7.2): its most common value is `payload.defaults.via`; a row
//!   carries `via` only when it differs.
//! - **Touch witnesses** (§7.1, F38): a caller row of `up.callers[]` refers to
//!   its first touch in `payload.witnesses` (`w:<n>`), so a witness shared by
//!   952 callers is written once.
//! - **Words** (§8.3: the renames of §8.1 ship in the major that changes the
//!   document): `tempState` is `temporary` / `not temporary` /
//!   `caller-dependent (Rec)` / `caller-dependent (parameter N)` (1-based) /
//!   `not resolved`; a `tier` is `via-dependency` / `via-hub` /
//!   `via-configuration` (`local` and `code` unchanged); an operation on a
//!   table not resolved has no `tableId` and carries `tableNotResolved` (its
//!   receiver kind, `RecordRef` or `Record`), and `isUnknownBucket` is gone.
//!
//! [`from_v3`] builds it from the 3.x payload the renderers write (their
//! summaries and result index read the rows before this step), and
//! [`to_v3`] is its inverse: the semantic gate of §12.1 (shared evidence
//! gives back each row's operation location and routine) checks
//! `to_v3(from_v3(p)) == p` on every query golden, and `--schema 3` writes
//! the 3.x document itself for one release (owner decision 5's pattern).

use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// The schema version of the default document.
pub const VERSION: &str = "4.0.0";

/// The keys of a row's routine that the routine table replaces by its id.
const FULL: &str = "routine";
/// `firstHop`: a projection of the routine (display, objectDisplay,
/// stableId), restored as that projection.
const HOP: &str = "firstHop";
const HOP_KEYS: &[&str] = &["display", "objectDisplay", "stableId"];

/// The 4.0.0 payload of a 3.x payload (module doc).
#[must_use]
pub fn from_v3(mut payload: Value) -> Value {
    words(&mut payload, true);
    let routines = intern_routines(&mut payload);
    flatten_origins(&mut payload);
    let via = factor_via(&mut payload);
    let witnesses = intern_witnesses(&mut payload);
    if let Value::Object(m) = &mut payload {
        if !routines.is_empty() {
            m.insert("routines".into(), Value::Object(routines));
        }
        if let Some(via) = via {
            m.insert("defaults".into(), json!({ "via": via }));
        }
        if !witnesses.is_empty() {
            m.insert("witnesses".into(), Value::Object(witnesses));
        }
    }
    payload
}

/// The 3.x payload of a 4.0.0 one: [`from_v3`]'s inverse.
#[must_use]
pub fn to_v3(mut payload: Value) -> Value {
    let Value::Object(m) = &mut payload else {
        return payload;
    };
    let routines = m.remove("routines");
    let via = m.remove("defaults").and_then(|d| d.get("via").cloned());
    let witnesses = m.remove("witnesses");
    if let Some(Value::Object(w)) = &witnesses {
        expand_witnesses(&mut payload, w);
    }
    if let Some(via) = &via {
        restore_via(&mut payload, via);
    }
    restore_origins(&mut payload);
    if let Some(Value::Object(r)) = &routines {
        expand_routines(&mut payload, r);
    }
    words(&mut payload, false);
    payload
}

/// A whole query document (`schemaVersion`, `payload`, …) in 4.0.0.
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

/// A 4.0.0 document back in its 3.x payload shape (the version field is
/// left as it is: the caller compares payloads).
#[must_use]
pub fn document_to_v3(mut doc: Value) -> Value {
    if let Value::Object(m) = &mut doc
        && let Some(p) = m.remove("payload")
    {
        m.insert("payload".into(), to_v3(p));
    }
    doc
}

// ---------------------------------------------------------------------------
// Words (§8.3)
// ---------------------------------------------------------------------------

fn temp_word(legacy: &str) -> Option<String> {
    Some(match legacy {
        "known(true)" => "temporary".into(),
        "known(false)" => "not temporary".into(),
        "parameter-dependent(rec)" => "caller-dependent (Rec)".into(),
        "unknown" => "not resolved".into(),
        s => {
            let i: u64 = s
                .strip_prefix("parameter-dependent(")?
                .strip_suffix(')')?
                .parse()
                .ok()?;
            format!("caller-dependent (parameter {})", i + 1)
        }
    })
}

fn temp_legacy(word: &str) -> Option<String> {
    Some(match word {
        "temporary" => "known(true)".into(),
        "not temporary" => "known(false)".into(),
        "caller-dependent (Rec)" => "parameter-dependent(rec)".into(),
        "not resolved" => "unknown".into(),
        s => {
            let n: u64 = s
                .strip_prefix("caller-dependent (parameter ")?
                .strip_suffix(')')?
                .parse()
                .ok()?;
            format!("parameter-dependent({})", n.checked_sub(1)?)
        }
    })
}

fn tier_word(legacy: &str) -> Option<&'static str> {
    match legacy {
        "throughDependency" => Some("via-dependency"),
        "onlyOpen" => Some("via-hub"),
        "configured" => Some("via-configuration"),
        _ => None,
    }
}

fn tier_legacy(word: &str) -> Option<&'static str> {
    match word {
        "via-dependency" => Some("throughDependency"),
        "via-hub" => Some("onlyOpen"),
        "via-configuration" => Some("configured"),
        _ => None,
    }
}

/// `forward`: the §8.1 words; else the 3.x values back.
fn words(v: &mut Value, forward: bool) {
    match v {
        Value::Object(m) => {
            // Only an operation row carries the legacy temp-state words (a
            // field-mode site already says `temporary` / `persistent`).
            let op_row = m.contains_key("operationId");
            if let Some(Value::String(t)) = m.get_mut("tempState").filter(|_| op_row) {
                let to = if forward {
                    temp_word(t)
                } else {
                    temp_legacy(t)
                };
                if let Some(to) = to {
                    *t = to;
                }
            }
            if let Some(Value::String(t)) = m.get_mut("tier") {
                let to = if forward {
                    tier_word(t)
                } else {
                    tier_legacy(t)
                };
                if let Some(to) = to {
                    *t = to.to_string();
                }
            }
            // An operation row's table (§8.1 target resolution); only rows
            // carry `isUnknownBucket`, so only rows are rewritten.
            let row = m.contains_key("operationId");
            if forward && row {
                match m.get("isUnknownBucket") {
                    Some(Value::Bool(true)) => {
                        m.remove("isUnknownBucket");
                        m.remove("tableId");
                        m.remove("tableName");
                        let receiver = m
                            .get("receiver")
                            .and_then(Value::as_str)
                            .unwrap_or("Record")
                            .to_string();
                        m.insert("tableNotResolved".into(), receiver.into());
                    }
                    Some(Value::Bool(false)) => {
                        m.remove("isUnknownBucket");
                    }
                    _ => {}
                }
            } else if !forward && row {
                if m.remove("tableNotResolved").is_some() {
                    m.insert(
                        "tableId".into(),
                        crate::engine::l4::effect_query::UNKNOWN_TABLE_ID.into(),
                    );
                    m.insert("isUnknownBucket".into(), true.into());
                } else if m.contains_key("tableId") {
                    m.insert("isUnknownBucket".into(), false.into());
                }
            }
            m.values_mut().for_each(|x| words(x, forward));
        }
        Value::Array(a) => a.iter_mut().for_each(|x| words(x, forward)),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Routines (§7.2)
// ---------------------------------------------------------------------------

fn stable_id(v: &Value) -> Option<&str> {
    v.get("stableId").and_then(Value::as_str)
}

/// Every object under `key` with a string `stableId`, in document order.
fn visit<'v>(v: &'v Value, key: &str, out: &mut Vec<&'v Value>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                if k == key && x.is_object() && stable_id(x).is_some() {
                    out.push(x);
                }
                visit(x, key, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| visit(x, key, out)),
        _ => {}
    }
}

/// `true` when `part`'s every field is `whole`'s, with the same value.
fn projection_of(part: &Value, whole: &Value) -> bool {
    part.as_object()
        .is_some_and(|p| p.iter().all(|(k, v)| whole.get(k) == Some(v)))
}

fn project(entry: &Value, keys: &[&str]) -> Value {
    let mut m = Map::new();
    for k in keys {
        if let Some(v) = entry.get(*k) {
            m.insert((*k).to_string(), v.clone());
        }
    }
    Value::Object(m)
}

/// The routine table, with every interned occurrence replaced by its id.
/// A full routine (`routine`) is interned only when every occurrence of its
/// stable id is the same object; a `firstHop` only when it is exactly the
/// projection of its entry (so [`to_v3`] restores both exactly).
fn intern_routines(payload: &mut Value) -> Map<String, Value> {
    let mut full: BTreeMap<String, Value> = BTreeMap::new();
    let mut clash: std::collections::BTreeSet<String> = Default::default();
    let mut order: Vec<String> = Vec::new();
    let mut seen = Vec::new();
    visit(payload, FULL, &mut seen);
    for r in seen {
        let id = stable_id(r).expect("visited").to_string();
        match full.get(&id) {
            Some(prev) if prev != r => {
                clash.insert(id);
            }
            Some(_) => {}
            None => {
                full.insert(id.clone(), r.clone());
                order.push(id);
            }
        }
    }
    let mut hops = Vec::new();
    visit(payload, HOP, &mut hops);
    let mut thin: BTreeMap<String, Value> = BTreeMap::new();
    for h in hops {
        let id = stable_id(h).expect("visited").to_string();
        if full.contains_key(&id) || thin.contains_key(&id) {
            continue;
        }
        thin.insert(id.clone(), h.clone());
        order.push(id);
    }
    let mut ids: BTreeMap<String, String> = BTreeMap::new();
    let mut table = Map::new();
    for id in order {
        if clash.contains(&id) {
            continue;
        }
        let entry = full.get(&id).or_else(|| thin.get(&id)).expect("listed");
        let r = format!("r:{}", ids.len() + 1);
        table.insert(r.clone(), entry.clone());
        ids.insert(id, r);
    }
    replace_routines(payload, &ids, &table);
    table
}

fn replace_routines(v: &mut Value, ids: &BTreeMap<String, String>, table: &Map<String, Value>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m.iter_mut() {
                let key = k.as_str();
                if (key == FULL || key == HOP)
                    && let Some(r) = stable_id(x).and_then(|s| ids.get(s))
                {
                    let entry = &table[r];
                    let exact = if key == FULL {
                        entry == x
                    } else {
                        project(entry, HOP_KEYS) == *x && projection_of(x, entry)
                    };
                    if exact {
                        *x = Value::String(r.clone());
                        continue;
                    }
                }
                replace_routines(x, ids, table);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| replace_routines(x, ids, table)),
        _ => {}
    }
}

fn expand_routines(v: &mut Value, table: &Map<String, Value>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m.iter_mut() {
                if let Value::String(r) = &*x
                    && let Some(entry) = table.get(r.as_str())
                {
                    match k.as_str() {
                        FULL => *x = entry.clone(),
                        HOP => *x = project(entry, HOP_KEYS),
                        _ => {}
                    }
                    continue;
                }
                expand_routines(x, table);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| expand_routines(x, table)),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Origins (§7.2): the operation's place on the row
// ---------------------------------------------------------------------------

/// `origin: {routine, anchor: {file, line, column, role}}` →
/// `origin: {routine, file, line, column, role}`.
fn flatten_origins(v: &mut Value) {
    match v {
        Value::Object(m) => {
            if let Some(Value::Object(o)) = m.get_mut("origin")
                && let Some(Value::Object(a)) = o.get("anchor").cloned()
                && o.keys().all(|k| k == FULL || k == "anchor")
                && a.contains_key("file")
            {
                o.remove("anchor");
                o.extend(a);
            }
            m.values_mut().for_each(flatten_origins);
        }
        Value::Array(a) => a.iter_mut().for_each(flatten_origins),
        _ => {}
    }
}

fn restore_origins(v: &mut Value) {
    match v {
        Value::Object(m) => {
            if let Some(Value::Object(o)) = m.get_mut("origin")
                && o.contains_key("file")
            {
                // Everything but the routine was the anchor's.
                let keys: Vec<String> = o.keys().filter(|k| *k != FULL).cloned().collect();
                let mut a = Map::new();
                for k in keys {
                    if let Some(x) = o.remove(&k) {
                        a.insert(k, x);
                    }
                }
                o.insert("anchor".into(), Value::Object(a));
            }
            m.values_mut().for_each(restore_origins);
        }
        Value::Array(a) => a.iter_mut().for_each(restore_origins),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// `via` (§7.2): a document default
// ---------------------------------------------------------------------------

/// Every operation row (an object with `op` and `operationId`; a summary's
/// `first` names an operation without being a row).
fn rows_mut<'v>(v: &'v mut Value, out: &mut Vec<&'v mut Map<String, Value>>) {
    match v {
        Value::Object(m) => {
            if m.contains_key("operationId") && m.contains_key("op") {
                // A row holds no nested row.
                out.push(m);
                return;
            }
            m.values_mut().for_each(|x| rows_mut(x, out));
        }
        Value::Array(a) => a.iter_mut().for_each(|x| rows_mut(x, out)),
        _ => {}
    }
}

/// The most common `via` (ties: the smallest value), removed from the rows
/// that carry it. `None` (nothing changed) when there is no row, or a row
/// carries no `via` (so a missing `via` could not be told from the default).
fn factor_via(payload: &mut Value) -> Option<String> {
    let mut rows = Vec::new();
    rows_mut(payload, &mut rows);
    let mut count: BTreeMap<String, usize> = BTreeMap::new();
    for r in &rows {
        match r.get("via").and_then(Value::as_str) {
            Some(v) => *count.entry(v.to_string()).or_default() += 1,
            None => return None,
        }
    }
    let via = count
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))?
        .0
        .clone();
    for r in rows {
        if r.get("via").and_then(Value::as_str) == Some(via.as_str()) {
            r.remove("via");
        }
    }
    Some(via)
}

fn restore_via(payload: &mut Value, via: &Value) {
    let mut rows = Vec::new();
    rows_mut(payload, &mut rows);
    for r in rows {
        r.entry("via").or_insert_with(|| via.clone());
    }
}

// ---------------------------------------------------------------------------
// Touch witnesses (§7.1)
// ---------------------------------------------------------------------------

/// `up.callers[].firstWitness` → `"w:<n>"`, one table entry per distinct
/// witness.
fn intern_witnesses(payload: &mut Value) -> Map<String, Value> {
    let mut table = Map::new();
    let mut ids: BTreeMap<String, String> = BTreeMap::new();
    let Some(Value::Array(callers)) = payload.pointer_mut("/up/callers") else {
        return table;
    };
    for c in callers {
        let Some(w) = c.get_mut("firstWitness") else {
            continue;
        };
        if !w.is_object() {
            continue;
        }
        let key = w.to_string();
        let id = match ids.get(&key) {
            Some(id) => id.clone(),
            None => {
                let id = format!("w:{}", ids.len() + 1);
                table.insert(id.clone(), w.clone());
                ids.insert(key, id.clone());
                id
            }
        };
        *w = Value::String(id);
    }
    table
}

fn expand_witnesses(payload: &mut Value, table: &Map<String, Value>) {
    let Some(Value::Array(callers)) = payload.pointer_mut("/up/callers") else {
        return;
    };
    for c in callers {
        if let Some(w) = c.get_mut("firstWitness")
            && let Some(entry) = w.as_str().and_then(|id| table.get(id))
        {
            *w = entry.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn routine(id: &str) -> Value {
        json!({"display": "WriteLog", "objectDisplay": "Codeunit Log", "stableId": id,
               "qualifiedDisplay": "Codeunit Log::WriteLog", "signature": "()",
               "anchor": {"file": "ws:a.al", "line": 3, "column": 4, "role": "definition"}})
    }

    fn row(op: &str, line: u64, via: &str, temp: &str) -> Value {
        json!({"op": op, "operationId": format!("x/{op}{line}"), "via": via, "tempState": temp,
               "tableId": "app/table/1", "tableName": "Log", "isUnknownBucket": false,
               "firstHop": {"display": "WriteLog", "objectDisplay": "Codeunit Log", "stableId": "s1"},
               "origin": {"routine": routine("s1"),
                          "anchor": {"file": "ws:a.al", "line": line, "column": 8, "role": "operation"}}})
    }

    /// The query effects rows of §7.2: the routine prints once, every row
    /// keeps its operation's own place, `via` is a default, the words are
    /// §8.1's, and the 3.x payload comes back exactly.
    #[test]
    fn rows_share_their_routine_and_keep_their_place() {
        let unresolved = json!({"op": "FindSet", "operationId": "x/u", "via": "direct",
            "tempState": "unknown", "tableId": "unknown", "isUnknownBucket": true,
            "receiver": "RecordRef",
            "origin": {"routine": routine("s1"),
                       "anchor": {"file": "ws:a.al", "line": 40, "column": 11, "role": "operation"}}});
        let v3 = json!({"effects": [
            row("Insert", 10, "inherited", "known(false)"),
            row("Modify", 12, "inherited", "parameter-dependent(2)"),
            unresolved,
        ]});
        let v4 = from_v3(v3.clone());
        assert_eq!(v4["routines"]["r:1"]["stableId"], "s1");
        assert_eq!(v4["defaults"]["via"], "inherited");
        let e = &v4["effects"];
        assert_eq!(e[0]["firstHop"], "r:1");
        assert_eq!(
            e[0]["origin"],
            json!({"routine": "r:1", "file": "ws:a.al", "line": 10,
                                           "column": 8, "role": "operation"})
        );
        assert_eq!(e[1]["origin"]["line"], 12);
        assert!(e[0].get("via").is_none());
        assert_eq!(e[2]["via"], "direct");
        assert_eq!(e[1]["tempState"], "caller-dependent (parameter 3)");
        assert_eq!(e[2]["tableNotResolved"], "RecordRef");
        assert!(e[2].get("tableId").is_none() && e[0].get("isUnknownBucket").is_none());
        assert_eq!(to_v3(v4), v3);
    }

    /// The touches callers of §7.1: one witness shared by every caller is
    /// written once, and each caller gets it back.
    #[test]
    fn a_shared_first_touch_is_written_once() {
        let w = json!({"op": "CalcFields", "operationId": "x/c", "via": "inherited",
                       "tempState": "known(false)", "touchClass": "persistent",
                       "origin": {"routine": routine("s9"),
                                  "anchor": {"file": "ws:t.al", "line": 740, "column": 1, "role": "operation"}}});
        let callers: Vec<Value> = (0..5)
            .map(|i| {
                json!({"depth": i, "routine": routine(&format!("c{i}")), "witnessCount": 3,
                             "firstWitness": w})
            })
            .collect();
        let v3 = json!({"up": {"callers": callers}});
        let v4 = from_v3(v3.clone());
        assert_eq!(v4["witnesses"].as_object().unwrap().len(), 1);
        assert_eq!(v4["up"]["callers"][4]["firstWitness"], "w:1");
        let r = v4["up"]["callers"][4]["routine"].as_str().unwrap();
        assert_eq!(v4["routines"][r]["stableId"], "c4");
        assert_eq!(to_v3(v4), v3);
    }

    /// A routine whose occurrences differ is never interned (its inverse
    /// could not tell them apart).
    #[test]
    fn differing_occurrences_stay_inline() {
        let mut other = routine("s1");
        other["signature"] = json!("(Integer)");
        let v3 = json!({"a": {"routine": routine("s1")}, "b": {"routine": other}});
        let v4 = from_v3(v3.clone());
        assert!(v4["a"]["routine"].is_object() && v4.get("routines").is_none());
        assert_eq!(to_v3(v4), v3);
    }
}
