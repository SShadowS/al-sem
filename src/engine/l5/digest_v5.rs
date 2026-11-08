//! The digest 5.0.0 document (round-2 output validation spec 2026-11-07,
//! §4.5 G40, G42 and G43, §4.6): the 4.0.0 document with one shape per
//! field and the dependency boundary in the §8.1 words.
//!
//! - **Dependency calls** (G43): `entryPoints` → `dependencyCalls`,
//!   `beyondEntryPoints` → `beyondDependencyCalls`, `entryPoint` →
//!   `dependencyCall`, `throughEntryPoint` → `throughDependencyCall`, as in
//!   query 5.0.0 ([`crate::engine::l4::query_v5::RENAMES`]).
//! - **Table answers** (G42): `entries[].tableAnswers[]` names its table
//!   once, in prove's shape: `tableRef` (`{id, name, number, app, appId}`,
//!   `null` for a name no table has, added in 4.x) and `asked` (the input
//!   as given); the 4.x `table` (the asked string) leaves.
//! - **An ambiguous selector** (G40): a `selector-ambiguous` diagnostic
//!   lists its candidates once, `candidates: [{selector, display,
//!   signature, stableId, definedAt}]` in source order, in place of the
//!   three 4.x string lists (`candidates`, `candidateRoutines`,
//!   `candidateSelectors`) and the 4.x `candidateRows` they are rebuilt
//!   from.
//! - **Dependency apps** (G38, r3 H6): `dependencies.apps` leaves;
//!   `dependencyCount` and `dependencyFingerprint` take its place, as in
//!   query 5.0.0 ([`crate::engine::l4::query_v5::dependency_fingerprint`]
//!   states its inputs and canonical form; it identifies an app set, not a
//!   retrievable manifest, not proof of unchanged package contents).
//! - **Shared table answers** (F top-9, c15): an `entries[].tableAnswers`
//!   list two entries or more carry alike (every verdict, blocker and table)
//!   is kept once in `sharedTableAnswers` (`ta:<n>`), and those entries name
//!   its key in place of the list; a list one entry alone has stays inline.
//! - **Temporary state** (r3 §5.2): an effect's `detail.tempState` in the
//!   §8.1 words, as query 4.0.0 names them (`known(false)` → `not
//!   temporary`, `known(true)` → `temporary`, `parameter-dependent(i)` →
//!   `caller-dependent (parameter i+1)`, `parameter-dependent(rec)` →
//!   `caller-dependent (Rec)`, `unknown` → `not resolved`).
//!
//! [`to_v4`] is [`from_v4`]'s inverse; `--schema 4` writes the 4.0.0
//! document itself for one release (r1 §8.3).

use crate::engine::l4::query_v5::{RENAMES, inverse, rename_keys};
use serde_json::{Value, json};

/// The schema version of the default document.
pub const VERSION: &str = "5.0.0";

/// The 4.x key of a structured candidate list (additive in 4.x).
const ROWS: &str = "candidateRows";

fn table_answers(payload: &mut Value, from: &str, to: &str) {
    let Some(Value::Array(entries)) = payload.get_mut("entries") else {
        return;
    };
    for e in entries {
        let Some(Value::Array(answers)) = e.get_mut("tableAnswers") else {
            continue;
        };
        for a in answers {
            if let Value::Object(m) = a
                && let Some(x) = m.remove(from)
            {
                m.insert(to.to_string(), x);
            }
        }
    }
}

fn diagnostics(payload: &mut Value) -> impl Iterator<Item = &mut serde_json::Map<String, Value>> {
    payload
        .get_mut("changedRootsDiagnostics")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object_mut)
        .filter(|m| m.get("kind").and_then(Value::as_str) == Some("selector-ambiguous"))
}

/// G35: one hop as an array: `{edge, line, to}` → `[edge, line, to]`,
/// with `file` → `[edge, line, to, file]`; a tail reference `{tail: "t:n"}`
/// → `"t:n"`. Any other hop shape stays an object.
fn hop_to_array(h: &Value) -> Value {
    let Value::Object(m) = h else {
        return h.clone();
    };
    if m.len() == 1
        && let Some(t) = m.get("tail").filter(|t| t.is_string())
    {
        return t.clone();
    }
    let core = ["edge", "line", "to"].iter().all(|k| m.contains_key(*k));
    let extra = m.len().saturating_sub(3);
    if core && (extra == 0 || (extra == 1 && m.contains_key("file"))) {
        let mut a = vec![m["edge"].clone(), m["line"].clone(), m["to"].clone()];
        if let Some(f) = m.get("file") {
            a.push(f.clone());
        }
        return Value::Array(a);
    }
    h.clone()
}

/// [`hop_to_array`]'s inverse.
fn hop_from_array(h: &Value) -> Value {
    match h {
        Value::String(_) => json!({"tail": h}),
        Value::Array(a) if a.len() == 3 || a.len() == 4 => {
            let mut m = serde_json::Map::new();
            m.insert("edge".into(), a[0].clone());
            m.insert("line".into(), a[1].clone());
            m.insert("to".into(), a[2].clone());
            if let Some(f) = a.get(3) {
                m.insert("file".into(), f.clone());
            }
            Value::Object(m)
        }
        _ => h.clone(),
    }
}

/// Every hop list (`hops` anywhere, each `pathTails` entry) mapped by `f`.
fn map_hops(v: &mut Value, f: fn(&Value) -> Value) {
    match v {
        Value::Object(m) => {
            for (k, x) in m.iter_mut() {
                match (k.as_str(), &mut *x) {
                    ("hops", Value::Array(hs)) => {
                        for h in hs.iter_mut() {
                            *h = f(h);
                        }
                    }
                    ("pathTails", Value::Object(ts)) => {
                        for hs in ts.values_mut() {
                            if let Value::Array(hs) = hs {
                                for h in hs.iter_mut() {
                                    *h = f(h);
                                }
                            }
                        }
                    }
                    _ => map_hops(x, f),
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| map_hops(x, f)),
        _ => {}
    }
}

/// r3 §5.2: an effect's `detail.tempState` in the §8.1 words (`forward`:
/// `known(false)` → `not temporary`, as query 4.0.0 does), else the 4.x
/// value back. A value with no word is left as it is.
fn temp_states(v: &mut Value, forward: bool) {
    match v {
        Value::Object(m) => {
            if let Some(Value::Object(d)) = m.get_mut("detail")
                && let Some(Value::String(t)) = d.get_mut("tempState")
            {
                let mapped = if forward {
                    crate::engine::l4::query_v4::temp_word(t)
                } else {
                    crate::engine::l4::query_v4::temp_legacy(t)
                };
                if let Some(w) = mapped {
                    *t = w;
                }
            }
            for x in m.values_mut() {
                temp_states(x, forward);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| temp_states(x, forward)),
        _ => {}
    }
}

/// The payload key of the table answers two entries or more give alike
/// (F top-9, c15).
const SHARED_ANSWERS: &str = "sharedTableAnswers";

/// F top-9 (c15): `forward`, an `entries[].tableAnswers` list that two
/// entries or more carry alike (every verdict, blocker and table, compared
/// whole) is kept once under [`SHARED_ANSWERS`] (`ta:<n>`, in entry order)
/// and each entry names its key in its place; a list one entry alone has
/// stays inline. Else every key is replaced by its list again. Exact both
/// ways: an entry's own answers are never merged with another's.
fn shared_table_answers(payload: &mut Value, forward: bool) {
    let Some(m) = payload.as_object_mut() else {
        return;
    };
    if forward {
        let lists: Vec<Value> = m
            .get("entries")
            .and_then(Value::as_array)
            .map(|es| {
                es.iter()
                    .filter_map(|e| e.get("tableAnswers"))
                    .filter(|a| a.as_array().is_some_and(|a| !a.is_empty()))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let mut keys: Vec<(Value, String)> = Vec::new();
        for l in &lists {
            let n = lists.iter().filter(|x| *x == l).count();
            if n > 1 && !keys.iter().any(|(v, _)| v == l) {
                keys.push((l.clone(), format!("ta:{}", keys.len() + 1)));
            }
        }
        if keys.is_empty() {
            return;
        }
        if let Some(Value::Array(es)) = m.get_mut("entries") {
            for e in es {
                if let Some(a) = e.get_mut("tableAnswers")
                    && let Some((_, k)) = keys.iter().find(|(v, _)| v == a)
                {
                    *a = Value::from(k.as_str());
                }
            }
        }
        let shared: serde_json::Map<String, Value> =
            keys.into_iter().map(|(v, k)| (k, v)).collect();
        m.insert(SHARED_ANSWERS.into(), Value::Object(shared));
    } else {
        let Some(Value::Object(shared)) = m.remove(SHARED_ANSWERS) else {
            return;
        };
        if let Some(Value::Array(es)) = m.get_mut("entries") {
            for e in es {
                if let Some(a) = e.get_mut("tableAnswers")
                    && let Some(list) = a.as_str().and_then(|k| shared.get(k))
                {
                    *a = list.clone();
                }
            }
        }
    }
}

/// The 5.0.0 payload of a 4.0.0 one (module doc).
#[must_use]
pub fn from_v4(mut payload: Value) -> Value {
    rename_keys(&mut payload, RENAMES);
    temp_states(&mut payload, true);
    map_hops(&mut payload, hop_to_array);
    crate::engine::l4::query_v5::fold_dependencies(&mut payload);
    table_answers(&mut payload, "table", "asked");
    shared_table_answers(&mut payload, true);
    for d in diagnostics(&mut payload) {
        if let Some(rows) = d.remove(ROWS) {
            d.remove("candidateRoutines");
            d.remove("candidateSelectors");
            d.insert("candidates".into(), rows);
        }
    }
    payload
}

/// The key of r3 §7 R1's records (additive in 5.x; no earlier schema has it).
const ASSUMPTION_RECORDS: &str = "assumptionRecords";

/// r3 §7 R1: `v` without any `assumptionRecords` key, at any depth (an
/// entry's own, its summary's `commits.literalDead`): what a 3.x or 4.x
/// document leaves out. The rows' `assumes` strings stay.
pub fn strip_assumption_records(v: &mut Value) {
    match v {
        Value::Object(m) => {
            m.remove(ASSUMPTION_RECORDS);
            for x in m.values_mut() {
                strip_assumption_records(x);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(strip_assumption_records),
        _ => {}
    }
}

/// The 4.0.0 payload of a 5.0.0 one: [`from_v4`]'s inverse, but for r3 §7
/// R1's `assumptionRecords`, which 4.0.0 does not carry.
#[must_use]
pub fn to_v4(mut payload: Value) -> Value {
    strip_assumption_records(&mut payload);
    for d in diagnostics(&mut payload) {
        let rows = match d.get("candidates") {
            Some(Value::Array(rows)) if rows.iter().all(Value::is_object) && !rows.is_empty() => {
                rows.clone()
            }
            _ => continue,
        };
        let s = |r: &Value, k: &str| r[k].as_str().unwrap_or("").to_string();
        d.insert(
            "candidates".into(),
            rows.iter().map(|r| json!(s(r, "display"))).collect(),
        );
        d.insert(
            "candidateRoutines".into(),
            rows.iter()
                .map(|r| {
                    json!(format!(
                        "{}{} at {} (stable id {})",
                        s(r, "display"),
                        s(r, "signature"),
                        s(r, "definedAt"),
                        s(r, "stableId")
                    ))
                })
                .collect(),
        );
        d.insert(
            "candidateSelectors".into(),
            rows.iter().map(|r| json!(s(r, "selector"))).collect(),
        );
        d.insert(ROWS.into(), Value::Array(rows));
    }
    shared_table_answers(&mut payload, false);
    table_answers(&mut payload, "asked", "table");
    crate::engine::l4::query_v5::unfold_dependencies(&mut payload);
    map_hops(&mut payload, hop_from_array);
    temp_states(&mut payload, false);
    rename_keys(&mut payload, &inverse(RENAMES));
    payload
}

/// A whole 4.0.0 digest document in 5.0.0.
#[must_use]
pub fn document_from_v4(mut doc: Value) -> Value {
    if let Value::Object(m) = &mut doc {
        if let Some(p) = m.remove("payload") {
            m.insert("payload".into(), from_v4(p));
        }
        m.insert("schemaVersion".into(), VERSION.into());
    }
    doc
}

/// A 5.0.0 document back in its 4.0.0 shape (version included).
#[must_use]
pub fn document_to_v4(mut doc: Value) -> Value {
    if let Value::Object(m) = &mut doc {
        if let Some(p) = m.remove("payload") {
            m.insert("payload".into(), to_v4(p));
        }
        m.insert("schemaVersion".into(), super::digest_v4::VERSION.into());
    }
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4_payload() -> Value {
        json!({
            "changedRootsDiagnostics": [{
                "kind": "selector-ambiguous",
                "selector": "Codeunit X::P",
                "candidates": ["Codeunit X::P", "Codeunit X::P"],
                "candidateRoutines": [
                    "Codeunit X::P(Integer) at ws:src/x.al:3 (stable id a#1)",
                    "Codeunit X::P(Integer, Boolean) at ws:src/x.al:9 (stable id a#2)"
                ],
                "candidateSelectors": ["Codeunit X::P(Integer)", "Codeunit X::P(Integer, Boolean)"],
                "candidateRows": [
                    {"selector": "Codeunit X::P(Integer)", "display": "Codeunit X::P",
                     "signature": "(Integer)", "stableId": "a#1", "definedAt": "ws:src/x.al:3"},
                    {"selector": "Codeunit X::P(Integer, Boolean)", "display": "Codeunit X::P",
                     "signature": "(Integer, Boolean)", "stableId": "a#2", "definedAt": "ws:src/x.al:9"}
                ]
            }, {"kind": "selector-unmatched", "selector": "Q", "candidates": []}],
            "dependencies": {"apps": [{"appId": "g", "name": "A", "version": "1.0"}]},
            "pathTails": {"t:1": [
                {"edge": "direct-call", "line": 3, "to": "r:a"},
                {"edge": "direct-call", "file": "ws:src/x.al", "line": 4, "to": "r:b"},
                {"tail": "t:2"}
            ], "t:2": [{"edge": "event", "line": 5, "to": "r:c", "via": "odd"}]},
            "entries": [{
                "summary": {"entryPoints": {"count": 1}, "commits": {"entryPoints": 0}},
                "effects": [{"paths": [{"throughEntryPoint": "Codeunit Y (2)::Z",
                    "hops": [{"edge": "direct-call", "line": 9, "to": "r:d"}, {"tail": "t:1"}]}]},
                    {"detail": {"tempState": "known(false)"}},
                    {"detail": {"tempState": "known(true)", "resourceCount": "2"}},
                    {"detail": {"tempState": "parameter-dependent(2)"}},
                    {"detail": {"tempState": "parameter-dependent(rec)"}},
                    {"detail": {"tempState": "unknown"}},
                    {"detail": {"tempState": "something new"}}],
                "tableAnswers": [{"table": "CDO File", "answer": "yes",
                                  "tableRef": {"id": "g/table/1", "name": "CDO File", "number": 1, "app": "A", "appId": "g"}}]
            }]
        })
    }

    /// One candidate array, one table shape, the §8.1 boundary words.
    #[test]
    fn five_has_one_shape_per_field() {
        let v5 = from_v4(v4_payload());
        let d = &v5["changedRootsDiagnostics"][0];
        assert!(d.get("candidateRoutines").is_none() && d.get("candidateSelectors").is_none());
        assert_eq!(d["candidates"][1]["definedAt"], "ws:src/x.al:9");
        assert!(d.get(ROWS).is_none());
        // An unmatched selector's `candidates` is not an ambiguity's.
        assert_eq!(v5["changedRootsDiagnostics"][1]["candidates"], json!([]));
        let a = &v5["entries"][0]["tableAnswers"][0];
        assert_eq!(a["asked"], "CDO File");
        assert!(a.get("table").is_none());
        assert_eq!(v5["entries"][0]["summary"]["dependencyCalls"]["count"], 1);
        assert_eq!(
            v5["entries"][0]["effects"][0]["paths"][0]["throughDependencyCall"],
            "Codeunit Y (2)::Z"
        );
        // G35: hops as arrays, a tail reference as its id; another shape
        // stays an object.
        assert_eq!(
            v5["entries"][0]["effects"][0]["paths"][0]["hops"],
            json!([["direct-call", 9, "r:d"], "t:1"])
        );
        assert_eq!(
            v5["pathTails"]["t:1"],
            json!([
                ["direct-call", 3, "r:a"],
                ["direct-call", 4, "r:b", "ws:src/x.al"],
                "t:2"
            ])
        );
        assert!(v5["pathTails"]["t:2"][0].is_object());
        // r3 §5.2: the temp states in the §8.1 words; a value with no word
        // is left as it is.
        let temp = |i: usize| v5["entries"][0]["effects"][i]["detail"]["tempState"].clone();
        assert_eq!(temp(1), "not temporary");
        assert_eq!(temp(2), "temporary");
        assert_eq!(temp(3), "caller-dependent (parameter 3)");
        assert_eq!(temp(4), "caller-dependent (Rec)");
        assert_eq!(temp(5), "not resolved");
        assert_eq!(temp(6), "something new");
        // G38: the app list leaves; its count and digest stay.
        assert_eq!(v5["dependencyCount"], 1);
        assert!(v5.get("dependencies").is_none());
    }

    /// F top-9 (c15): table answers two entries give alike are kept once,
    /// each entry naming them; an entry whose answers differ (one blocker
    /// more) keeps its own inline; 4.0.0 is rebuilt exactly. Hand-stated:
    /// three entries, the first and third alike.
    ///
    /// DISCRIMINATION (2026-11-08): sharing by the table asked alone (the
    /// lists compared by `[0]["asked"]` instead of whole) fails at "its own
    /// answers": the second entry, whose blockers differ, is folded into
    /// `ta:1`; restored, passes.
    #[test]
    fn alike_table_answers_are_kept_once_and_rebuilt() {
        let answers = |blockers: usize| {
            json!([{"table": "CDO File", "answer": "unknown",
                    "blockedBy": (0..blockers).map(|i| json!({"kind": "open-world-dispatch", "detail": format!("d{i}")})).collect::<Vec<_>>(),
                    "tableRef": {"id": "g/table/1", "name": "CDO File"}}])
        };
        let p = json!({"entries": [
            {"routine": {"stableId": "a"}, "tableAnswers": answers(2)},
            {"routine": {"stableId": "b"}, "tableAnswers": answers(3)},
            {"routine": {"stableId": "c"}, "tableAnswers": answers(2)},
        ]});
        let v5 = from_v4(p.clone());
        let e = &v5["entries"];
        assert_eq!(e[0]["tableAnswers"], "ta:1", "{v5:#}");
        assert_eq!(e[2]["tableAnswers"], "ta:1", "{v5:#}");
        assert!(e[1]["tableAnswers"].is_array(), "its own answers: {v5:#}");
        assert_eq!(
            e[1]["tableAnswers"][0]["blockedBy"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        assert_eq!(v5[SHARED_ANSWERS]["ta:1"][0]["asked"], "CDO File");
        assert_eq!(to_v4(v5), p);
    }

    /// The previous schema's reconstruction rule: every 4.x candidate
    /// string, table answer, hop and boundary key is rebuilt exactly; the
    /// app list is not (G38, §4.6: no reconstruction test).
    #[test]
    fn four_is_rebuilt_from_five() {
        let p = v4_payload();
        let mut want = p.clone();
        want.as_object_mut().unwrap().remove("dependencies");
        assert_eq!(to_v4(from_v4(p)), want);
    }
}
