//! The query 5.0.0 document (round-2 output validation spec 2026-11-07,
//! §4.5 G41 and G43, §4.6): the 4.0.0 document with the dependency boundary
//! named in the §8.1 words and each operation's file readable.
//!
//! - **Dependency calls** (G43): the keys that named a dependency boundary
//!   "entry point" carry its §8.1 name: `entryPoints` → `dependencyCalls`,
//!   `beyondEntryPoints` → `beyondDependencyCalls`, `entryPoint` →
//!   `dependencyCall`, `throughEntryPoint` → `throughDependencyCall`,
//!   wherever they occur in the payload (`summary.beyondEntryPoints`,
//!   `summary.beyondWrites.entryPoints`, `down.beyond[].entryPoint`, …).
//!   `result.scope.families` already used the new words in 4.0.0 and is left
//!   as it is.
//! - **File display** (G41): every `origin` gains `fileDisplay`, its `file`
//!   with a dependency app's guid replaced by the app's name
//!   (`dep:Base Application:src/…`, from `payload.dependencies.apps`);
//!   `origin.file` keeps its guid form and is unchanged.
//!
//! [`to_v4`] is [`from_v4`]'s inverse; `--schema 4` writes the 4.0.0
//! document itself for one release (r1 §8.3).

use serde_json::Value;

/// The schema version of the default document.
pub const VERSION: &str = "5.0.0";

/// The 4.x keys that become their §8.1 names in 5.0.0 (G43).
pub(crate) const RENAMES: &[(&str, &str)] = &[
    ("entryPoints", "dependencyCalls"),
    ("beyondEntryPoints", "beyondDependencyCalls"),
    ("entryPoint", "dependencyCall"),
    ("throughEntryPoint", "throughDependencyCall"),
];

/// The object whose keys are left as they are: 4.0.0 already named its
/// family `beyondDependencyCalls`.
pub(crate) const UNRENAMED: &str = "families";

/// Renames every key of `v` by `pairs` (`(from, to)`), keeping key order
/// out of it (the serializer sorts), never inside an [`UNRENAMED`] object.
pub(crate) fn rename_keys(v: &mut Value, pairs: &[(&str, &str)]) {
    match v {
        Value::Object(m) => {
            let keys: Vec<String> = m.keys().cloned().collect();
            for k in keys {
                if let Some((_, to)) = pairs.iter().find(|(from, _)| *from == k)
                    && let Some(x) = m.remove(&k)
                {
                    debug_assert!(!m.contains_key(*to), "both {k} and {to} in one object");
                    m.insert((*to).to_string(), x);
                }
            }
            for (k, x) in m.iter_mut() {
                if k != UNRENAMED {
                    rename_keys(x, pairs);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| rename_keys(x, pairs)),
        _ => {}
    }
}

/// `pairs` the other way round.
pub(crate) fn inverse(pairs: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
    pairs.iter().map(|(a, b)| (*b, *a)).collect()
}

/// `dep:<guid>:<path>` with the app's name for its guid; any other file
/// as it is.
pub(crate) fn file_display(file: &str, apps: &[Value]) -> String {
    let Some(rest) = file.strip_prefix("dep:") else {
        return file.to_string();
    };
    let Some((guid, path)) = rest.split_once(':') else {
        return file.to_string();
    };
    let name = apps
        .iter()
        .find(|a| {
            a["appId"]
                .as_str()
                .is_some_and(|g| g.eq_ignore_ascii_case(guid))
        })
        .and_then(|a| a["name"].as_str());
    match name {
        Some(n) => format!("dep:{n}:{path}"),
        None => file.to_string(),
    }
}

fn add_file_display(v: &mut Value, apps: &[Value]) {
    match v {
        Value::Object(m) => {
            for (k, x) in m.iter_mut() {
                if k == "origin"
                    && let Value::Object(o) = x
                    && let Some(f) = o.get("file").and_then(Value::as_str)
                {
                    let d = file_display(f, apps);
                    o.insert("fileDisplay".into(), d.into());
                }
                add_file_display(x, apps);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| add_file_display(x, apps)),
        _ => {}
    }
}

fn drop_file_display(v: &mut Value) {
    match v {
        Value::Object(m) => {
            for (k, x) in m.iter_mut() {
                if k == "origin"
                    && let Value::Object(o) = x
                {
                    o.remove("fileDisplay");
                }
                drop_file_display(x);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(drop_file_display),
        _ => {}
    }
}

/// The 5.0.0 payload of a 4.0.0 one (module doc).
#[must_use]
pub fn from_v4(mut payload: Value) -> Value {
    rename_keys(&mut payload, RENAMES);
    let apps: Vec<Value> = payload
        .pointer("/dependencies/apps")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    add_file_display(&mut payload, &apps);
    payload
}

/// The 4.0.0 payload of a 5.0.0 one: [`from_v4`]'s inverse.
#[must_use]
pub fn to_v4(mut payload: Value) -> Value {
    drop_file_display(&mut payload);
    rename_keys(&mut payload, &inverse(RENAMES));
    payload
}

/// A whole 4.0.0 query document in 5.0.0.
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
        m.insert("schemaVersion".into(), super::query_v4::VERSION.into());
    }
    doc
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn v4_payload() -> Value {
        json!({
            "dependencies": {"apps": [{"appId": "437DBF0E-84FF-417A-965D-ED2BB9650972", "name": "Base Application"}]},
            "effects": [
                {"op": "Modify", "origin": {"routine": "r:1", "file": "dep:437dbf0e-84ff-417a-965d-ed2bb9650972:src/A.al", "line": 3, "column": 1}},
                {"op": "Insert", "origin": {"routine": "r:2", "file": "ws:src/B.al", "line": 4, "column": 2}}
            ],
            "result": {"scope": {"families": {"beyondDependencyCalls": {"depth": "wholeCone"}}}},
            "summary": {
                "beyondEntryPoints": [{"entryPoint": "Codeunit X (1)::Y", "tables": ["T"]}],
                "beyondWrites": {"entryPoints": 1, "tables": 1},
                "byTable": [{"beyond": [{"entryPoint": "Codeunit X (1)::Y"}]}]
            }
        })
    }

    /// The new names replace the old ones everywhere but `families`, and
    /// each origin names its app.
    #[test]
    fn five_names_dependency_calls_and_files() {
        let v5 = from_v4(v4_payload());
        let s = v5.to_string();
        for (old, _) in RENAMES {
            assert!(!s.contains(&format!("\"{old}\"")), "{old} left: {s}");
        }
        assert_eq!(v5["summary"]["beyondWrites"]["dependencyCalls"], 1);
        assert_eq!(
            v5["summary"]["beyondDependencyCalls"][0]["dependencyCall"],
            "Codeunit X (1)::Y"
        );
        assert!(v5["result"]["scope"]["families"]["beyondDependencyCalls"].is_object());
        assert_eq!(
            v5["effects"][0]["origin"]["fileDisplay"],
            "dep:Base Application:src/A.al"
        );
        assert_eq!(
            v5["effects"][0]["origin"]["file"],
            "dep:437dbf0e-84ff-417a-965d-ed2bb9650972:src/A.al"
        );
        assert_eq!(v5["effects"][1]["origin"]["fileDisplay"], "ws:src/B.al");
    }

    /// The previous schema's reconstruction rule: `to_v4(from_v4(p)) == p`,
    /// `families` included (its `beyondDependencyCalls` is never renamed
    /// back).
    #[test]
    fn four_is_rebuilt_from_five() {
        let p = v4_payload();
        assert_eq!(to_v4(from_v4(p.clone())), p);
    }
}
