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

/// The 5.0.0 payload of a 4.0.0 one (module doc).
#[must_use]
pub fn from_v4(mut payload: Value) -> Value {
    rename_keys(&mut payload, RENAMES);
    map_hops(&mut payload, hop_to_array);
    crate::engine::l4::query_v5::fold_dependencies(&mut payload);
    table_answers(&mut payload, "table", "asked");
    for d in diagnostics(&mut payload) {
        if let Some(rows) = d.remove(ROWS) {
            d.remove("candidateRoutines");
            d.remove("candidateSelectors");
            d.insert("candidates".into(), rows);
        }
    }
    payload
}

/// The 4.0.0 payload of a 5.0.0 one: [`from_v4`]'s inverse.
#[must_use]
pub fn to_v4(mut payload: Value) -> Value {
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
    table_answers(&mut payload, "asked", "table");
    crate::engine::l4::query_v5::unfold_dependencies(&mut payload);
    map_hops(&mut payload, hop_from_array);
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
                    "hops": [{"edge": "direct-call", "line": 9, "to": "r:d"}, {"tail": "t:1"}]}]}],
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
        // G38: the app list leaves; its count and digest stay.
        assert_eq!(v5["dependencyCount"], 1);
        assert!(v5.get("dependencies").is_none());
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
