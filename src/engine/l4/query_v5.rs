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

/// A table of distinct values, each named `<prefix><n>` (1-based, in first
/// use order).
struct RefTable {
    prefix: &'static str,
    keys: Vec<Value>,
    index: std::collections::HashMap<String, usize>,
}

impl RefTable {
    fn new(prefix: &'static str) -> RefTable {
        RefTable {
            prefix,
            keys: Vec::new(),
            index: std::collections::HashMap::new(),
        }
    }

    /// The reference of `entry`, added on first use.
    fn reference(&mut self, entry: Value) -> String {
        let key = entry.to_string();
        let n = match self.index.get(&key) {
            Some(n) => *n,
            None => {
                self.keys.push(entry);
                self.index.insert(key, self.keys.len());
                self.keys.len()
            }
        };
        format!("{}{n}", self.prefix)
    }

    fn into_value(self) -> Option<Value> {
        (!self.keys.is_empty()).then(|| {
            Value::Object(
                self.keys
                    .into_iter()
                    .enumerate()
                    .map(|(i, v)| (format!("{}{}", self.prefix, i + 1), v))
                    .collect(),
            )
        })
    }
}

/// Every `origin.file` replaced by its `f:<n>` reference into the files
/// table (G34), which holds the file and its display (G41's `fileDisplay`).
fn fold_files(v: &mut Value, apps: &[Value], files: &mut RefTable) {
    match v {
        Value::Object(m) => {
            for (k, x) in m.iter_mut() {
                if k == "origin"
                    && let Value::Object(o) = x
                    && let Some(f) = o.get("file").and_then(Value::as_str).map(str::to_string)
                {
                    let display = file_display(&f, apps);
                    let r = files.reference(serde_json::json!({"file": f, "display": display}));
                    o.insert("file".into(), r.into());
                    continue;
                }
                fold_files(x, apps, files);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| fold_files(x, apps, files)),
        _ => {}
    }
}

/// [`fold_files`]' inverse: each reference back to its file.
fn unfold_files(v: &mut Value, files: &serde_json::Map<String, Value>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m.iter_mut() {
                if k == "origin"
                    && let Value::Object(o) = x
                    && let Some(f) = o
                        .get("file")
                        .and_then(Value::as_str)
                        .and_then(|r| files.get(r))
                {
                    let f = f["file"].clone();
                    o.insert("file".into(), f);
                    continue;
                }
                unfold_files(x, files);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| unfold_files(x, files)),
        _ => {}
    }
}

/// G34: each effect row's `tableId` and `tableName` become `table: "t:<n>"`
/// into the tables table, and each per-table row of
/// `summary.beyondDependencyCalls[].tables` (exactly `ops`, `table`,
/// `tableId`) folds to `["t:<n>", <op>…]`.
fn fold_tables(payload: &mut Value, tables: &mut RefTable) {
    if let Some(Value::Array(rows)) = payload.get_mut("effects") {
        for r in rows.iter_mut() {
            let Value::Object(row) = r else { continue };
            if row.contains_key("table") {
                continue;
            }
            let Some(id) = row.get("tableId").filter(|v| v.is_string()).cloned() else {
                continue;
            };
            row.remove("tableId");
            let mut entry = serde_json::json!({"id": id});
            if let Some(name) = row.remove("tableName") {
                entry["name"] = name;
            }
            row.insert("table".into(), tables.reference(entry).into());
        }
    }
    let beyond = payload
        .get_mut("summary")
        .and_then(|s| s.get_mut("beyondDependencyCalls"))
        .and_then(Value::as_array_mut);
    for b in beyond.into_iter().flatten() {
        let Some(Value::Array(ts)) = b.get_mut("tables") else {
            continue;
        };
        for t in ts.iter_mut() {
            let Value::Object(row) = t else { continue };
            let foldable = row.len() == 3
                && row["table"].is_string()
                && row["tableId"].is_string()
                && row["ops"]
                    .as_array()
                    .is_some_and(|ops| ops.iter().all(Value::is_string));
            if !foldable {
                continue;
            }
            let entry = serde_json::json!({"id": row["tableId"], "name": row["table"]});
            let mut folded = vec![Value::from(tables.reference(entry))];
            folded.extend(row["ops"].as_array().cloned().unwrap_or_default());
            *t = Value::Array(folded);
        }
    }
}

/// [`fold_tables`]' inverse.
fn unfold_tables(payload: &mut Value, tables: &serde_json::Map<String, Value>) {
    if let Some(Value::Array(rows)) = payload.get_mut("effects") {
        for r in rows.iter_mut() {
            let Value::Object(row) = r else { continue };
            let Some(entry) = row
                .get("table")
                .and_then(Value::as_str)
                .and_then(|t| tables.get(t))
                .cloned()
            else {
                continue;
            };
            row.remove("table");
            row.insert("tableId".into(), entry["id"].clone());
            if let Some(name) = entry.get("name") {
                row.insert("tableName".into(), name.clone());
            }
        }
    }
    let beyond = payload
        .get_mut("summary")
        .and_then(|s| s.get_mut("beyondDependencyCalls"))
        .and_then(Value::as_array_mut);
    for b in beyond.into_iter().flatten() {
        let Some(Value::Array(ts)) = b.get_mut("tables") else {
            continue;
        };
        for t in ts.iter_mut() {
            let Value::Array(folded) = t else { continue };
            let Some(entry) = folded
                .first()
                .and_then(Value::as_str)
                .and_then(|r| tables.get(r))
            else {
                continue;
            };
            *t = serde_json::json!({
                "ops": folded[1..].to_vec(),
                "table": entry["name"],
                "tableId": entry["id"],
            });
        }
    }
}

/// A routine row's `qualifiedDisplay` as G34 derives it: its `selector`
/// when that is not its stable id (a workspace routine's selector is its
/// qualified display), else `objectDisplay`, the object number when the
/// routine is in a dependency (its anchor file is `dep:…`), `::` and its
/// `display`.
fn derived_qualified(r: &Value) -> Option<String> {
    let selector = r["selector"].as_str();
    if let Some(s) = selector.filter(|s| Some(*s) != r["stableId"].as_str()) {
        return Some(s.to_string());
    }
    let object = r["objectDisplay"].as_str()?;
    let display = r["display"].as_str()?;
    let dep = r["anchor"]["file"]
        .as_str()
        .is_some_and(|f| f.starts_with("dep:"));
    let number = r["stableId"]
        .as_str()
        .and_then(|id| id.split('#').next())
        .and_then(|id| id.rsplit(':').next())
        .filter(|n| dep && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    Some(match number {
        Some(n) => format!("{object} ({n})::{display}"),
        None => format!("{object}::{display}"),
    })
}

/// G34: a routine row's `qualifiedDisplay` is left out when
/// [`derived_qualified`] gives it, and is `""` when the 4.x row had none.
fn fold_qualified(payload: &mut Value) {
    let Some(Value::Object(rs)) = payload.get_mut("routines") else {
        return;
    };
    for r in rs.values_mut() {
        let derived = derived_qualified(r);
        let Value::Object(row) = r else { continue };
        match row.get("qualifiedDisplay").and_then(Value::as_str) {
            Some(q) if Some(q) == derived.as_deref() => {
                row.remove("qualifiedDisplay");
            }
            Some(_) => {}
            None if row.contains_key("qualifiedDisplay") => {}
            None => {
                row.insert("qualifiedDisplay".into(), "".into());
            }
        }
    }
}

/// [`fold_qualified`]'s inverse.
fn unfold_qualified(payload: &mut Value) {
    let Some(Value::Object(rs)) = payload.get_mut("routines") else {
        return;
    };
    for r in rs.values_mut() {
        let derived = derived_qualified(r);
        let Value::Object(row) = r else { continue };
        match row.get("qualifiedDisplay").and_then(Value::as_str) {
            Some("") => {
                row.remove("qualifiedDisplay");
            }
            Some(_) => {}
            None if row.contains_key("qualifiedDisplay") => {}
            None => {
                if let Some(q) = derived {
                    row.insert("qualifiedDisplay".into(), q.into());
                }
            }
        }
    }
}

/// G38: the one-routine-deep families' constant sentence (in the schema
/// doc, not in every document).
fn family_sentence(family: &str) -> Option<String> {
    let what = match family {
        "reads" => "reads",
        "locks" => "locks",
        "validate" => "validations",
        "writes" => "writes",
        _ => return None,
    };
    Some(format!(
        "dependency code deeper than one routine is not inventoried for {what}"
    ))
}

/// G38: each `families` object's four constant sentences dropped (`drop`),
/// or put back on a one-routine-deep family with none (`!drop`).
fn family_reasons(v: &mut Value, drop: bool) {
    match v {
        Value::Object(m) => {
            for (k, x) in m.iter_mut() {
                if k == UNRENAMED
                    && let Value::Object(fams) = x
                {
                    for (f, fam) in fams.iter_mut() {
                        let Some(sentence) = family_sentence(f) else {
                            continue;
                        };
                        let Value::Object(o) = fam else { continue };
                        if drop {
                            if o.get("reasons") == Some(&serde_json::json!([sentence])) {
                                o.remove("reasons");
                            }
                        } else if !o.contains_key("reasons")
                            && o.get("depth") == Some(&"oneDependencyRoutine".into())
                        {
                            o.insert("reasons".into(), serde_json::json!([sentence]));
                        }
                    }
                    continue;
                }
                family_reasons(x, drop);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| family_reasons(x, drop)),
        _ => {}
    }
}

/// G38: the dependency apps' count and a digest of their identities
/// (`apps-v1:<blake3 of each "appId@version", sorted>`), in place of the
/// full list.
pub(crate) fn dependency_closure(apps: &[Value]) -> String {
    let mut ids: Vec<String> = apps
        .iter()
        .map(|a| {
            format!(
                "{}@{}",
                a["appId"].as_str().unwrap_or("").to_ascii_lowercase(),
                a["version"].as_str().unwrap_or("")
            )
        })
        .collect();
    ids.sort();
    let hash = blake3::hash(ids.join("\n").as_bytes()).to_hex();
    format!("apps-v1:{}", &hash[..16])
}

/// G38: `dependencies.apps` leaves in 5.0.0; `dependencyCount` and
/// `dependencyClosure` take its place (an empty `dependencies` object
/// leaves too).
pub(crate) fn fold_dependencies(payload: &mut Value) {
    let Value::Object(m) = payload else { return };
    let Some(Value::Object(deps)) = m.get_mut("dependencies") else {
        return;
    };
    let Some(Value::Array(apps)) = deps.remove("apps") else {
        return;
    };
    let empty = deps.is_empty();
    if empty {
        m.remove("dependencies");
    }
    m.insert("dependencyCount".into(), apps.len().into());
    m.insert("dependencyClosure".into(), dependency_closure(&apps).into());
}

/// [`fold_dependencies`]' partial inverse: the count and digest leave;
/// the app list cannot come back (§4.6: G38 has no reconstruction test;
/// `--schema 4` writes the 4.0.0 document itself).
pub(crate) fn unfold_dependencies(payload: &mut Value) {
    if let Value::Object(m) = payload {
        m.remove("dependencyCount");
        m.remove("dependencyClosure");
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
    let mut files = RefTable::new("f:");
    fold_files(&mut payload, &apps, &mut files);
    let mut tables = RefTable::new("t:");
    fold_tables(&mut payload, &mut tables);
    fold_qualified(&mut payload);
    family_reasons(&mut payload, true);
    fold_dependencies(&mut payload);
    if let Value::Object(m) = &mut payload {
        if let Some(f) = files.into_value() {
            m.insert("files".into(), f);
        }
        if let Some(t) = tables.into_value() {
            m.insert("tables".into(), t);
        }
    }
    payload
}

/// The 4.0.0 payload of a 5.0.0 one: [`from_v4`]'s inverse, but for
/// `dependencies.apps`, which 5.0.0 does not carry.
#[must_use]
pub fn to_v4(mut payload: Value) -> Value {
    let (files, tables) = match &mut payload {
        Value::Object(m) => (
            m.remove("files").and_then(|v| v.as_object().cloned()),
            m.remove("tables").and_then(|v| v.as_object().cloned()),
        ),
        _ => (None, None),
    };
    unfold_dependencies(&mut payload);
    family_reasons(&mut payload, false);
    unfold_qualified(&mut payload);
    if let Some(t) = &tables {
        unfold_tables(&mut payload, t);
    }
    if let Some(f) = &files {
        unfold_files(&mut payload, f);
    }
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

    const DEP_FILE: &str = "dep:437dbf0e-84ff-417a-965d-ed2bb9650972:src/A.al";

    /// Hand-stated 4.0.0 rows of every shape G34 and G38 fold: two rows on
    /// one table (one reference), a row with no table name, a row with no
    /// table; one file twice; a workspace routine (qualified = selector), a
    /// dependency routine (qualified with its object number), one whose
    /// qualified display is neither, one with none; the four constant
    /// family sentences and one that differs; foldable and unfoldable
    /// per-table summary rows.
    fn v4_payload() -> Value {
        json!({
            "dependencies": {"apps": [{"appId": "437DBF0E-84FF-417A-965D-ED2BB9650972", "name": "Base Application", "version": "28.1"}]},
            "effects": [
                {"op": "Modify", "tableId": "a/table/1", "tableName": "T One", "origin": {"routine": "r:1", "file": DEP_FILE, "line": 3, "column": 1}},
                {"op": "Insert", "tableId": "a/table/1", "tableName": "T One", "origin": {"routine": "r:2", "file": "ws:src/B.al", "line": 4, "column": 2}},
                {"op": "Get", "tableId": "a/table/2", "origin": {"routine": "r:2", "file": DEP_FILE, "line": 9, "column": 2}},
                {"op": "Next", "tableNotResolved": "RecordRef", "origin": {"routine": "r:2", "file": "ws:src/B.al", "line": 5, "column": 2}}
            ],
            "routines": {
                "r:1": {"anchor": {"file": DEP_FILE, "line": 1}, "display": "Get", "objectDisplay": "Table \"T One\"",
                        "qualifiedDisplay": "Table \"T One\" (1062)::Get", "selector": "a:Table:1062#ab", "stableId": "a:Table:1062#ab"},
                "r:2": {"anchor": {"file": "ws:src/B.al", "line": 1}, "display": "Run", "objectDisplay": "Codeunit B",
                        "qualifiedDisplay": "Codeunit B::Run(Integer)", "selector": "Codeunit B::Run(Integer)", "stableId": "w:Codeunit:5#cd"},
                "r:3": {"anchor": {"file": "ws:src/B.al", "line": 7}, "display": "Odd", "objectDisplay": "Codeunit B",
                        "qualifiedDisplay": "something else", "selector": "w:Codeunit:5#ef", "stableId": "w:Codeunit:5#ef"},
                "r:4": {"display": "Bare", "objectDisplay": "Codeunit B", "selector": "Codeunit B::Bare", "stableId": "w:Codeunit:5#01"}
            },
            "result": {"scope": {"families": {
                "beyondDependencyCalls": {"depth": "wholeCone", "reasons": ["calls not resolved"]},
                "reads": {"depth": "oneDependencyRoutine", "reasons": ["dependency code deeper than one routine is not inventoried for reads"]},
                "locks": {"depth": "oneDependencyRoutine", "reasons": ["dependency code deeper than one routine is not inventoried for locks"]},
                "validate": {"depth": "oneDependencyRoutine", "reasons": ["dependency code deeper than one routine is not inventoried for validations"]},
                "writes": {"depth": "oneDependencyRoutine", "reasons": ["a different sentence"]}
            }}},
            "summary": {
                "beyondEntryPoints": [{"entryPoint": "Codeunit X (1)::Y", "tables": [
                    {"ops": ["DB_INSERT", "DB_MODIFY"], "table": "T One", "tableId": "a/table/1"},
                    {"ops": ["DB_DELETE"], "table": "T Three", "tableId": "a/table/3"},
                    {"ops": ["DB_DELETE"], "table": "T Four", "tableId": "a/table/4", "extra": 1}
                ]}],
                "beyondWrites": {"entryPoints": 1, "tables": 1},
                "byTable": [{"beyond": [{"entryPoint": "Codeunit X (1)::Y"}]}]
            }
        })
    }

    /// The new names replace the old ones everywhere but `families`; tables
    /// and files are references into their tables (G34), a derivable
    /// qualified display is left out, the four constant sentences and the
    /// app list leave (G38).
    #[test]
    fn five_names_dependency_calls_and_folds_tables_and_files() {
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
        assert_eq!(v5["effects"][0]["table"], "t:1");
        assert_eq!(v5["effects"][1]["table"], "t:1");
        assert_eq!(
            v5["tables"]["t:1"],
            json!({"id": "a/table/1", "name": "T One"})
        );
        assert_eq!(v5["tables"]["t:2"], json!({"id": "a/table/2"}));
        assert!(v5["effects"][3].get("table").is_none());
        assert_eq!(v5["effects"][0]["origin"]["file"], "f:1");
        assert_eq!(v5["effects"][2]["origin"]["file"], "f:1");
        assert_eq!(
            v5["files"]["f:1"],
            json!({"file": DEP_FILE, "display": "dep:Base Application:src/A.al"})
        );
        assert_eq!(v5["files"]["f:2"]["display"], "ws:src/B.al");
        let r = &v5["routines"];
        assert!(r["r:1"].get("qualifiedDisplay").is_none(), "{r}");
        assert!(r["r:2"].get("qualifiedDisplay").is_none(), "{r}");
        assert_eq!(r["r:3"]["qualifiedDisplay"], "something else");
        assert_eq!(r["r:4"]["qualifiedDisplay"], "");
        let b = &v5["summary"]["beyondDependencyCalls"][0]["tables"];
        assert_eq!(b[0], json!(["t:1", "DB_INSERT", "DB_MODIFY"]));
        assert_eq!(b[1], json!(["t:3", "DB_DELETE"]));
        assert!(b[2].is_object());
        let f = &v5["result"]["scope"]["families"];
        assert!(f["reads"].get("reasons").is_none());
        assert_eq!(f["writes"]["reasons"], json!(["a different sentence"]));
        assert_eq!(v5["dependencyCount"], 1);
        assert!(
            v5["dependencyClosure"]
                .as_str()
                .is_some_and(|c| c.starts_with("apps-v1:"))
        );
        assert!(v5.get("dependencies").is_none());
    }

    /// The previous schema's reconstruction rule (§4.6): `to_v4(from_v4(p))
    /// == p` but for `dependencies.apps` (G38: no reconstruction; `--schema
    /// 4` writes the 4.0.0 document itself), every table, file and
    /// qualified display rebuilt, `families` included (its
    /// `beyondDependencyCalls` is never renamed back).
    #[test]
    fn four_is_rebuilt_from_five() {
        let p = v4_payload();
        let mut want = p.clone();
        want.as_object_mut().unwrap().remove("dependencies");
        assert_eq!(to_v4(from_v4(p)), want);
    }
}
