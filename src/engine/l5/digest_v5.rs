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

/// The 5.0.0 payload of a 4.0.0 one (module doc).
#[must_use]
pub fn from_v4(mut payload: Value) -> Value {
    rename_keys(&mut payload, RENAMES);
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
            "entries": [{
                "summary": {"entryPoints": {"count": 1}, "commits": {"entryPoints": 0}},
                "effects": [{"paths": [{"throughEntryPoint": "Codeunit Y (2)::Z"}]}],
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
    }

    /// The previous schema's reconstruction rule: every 4.x candidate
    /// string, table answer and boundary key is rebuilt exactly.
    #[test]
    fn four_is_rebuilt_from_five() {
        let p = v4_payload();
        assert_eq!(to_v4(from_v4(p.clone())), p);
    }
}
