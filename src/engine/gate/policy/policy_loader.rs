//! Policy loader — port of al-sem `src/policy/policy-loader.ts`.
//!
//! Parses a policy YAML string into a validated [`PolicyDoc`]. Reproduces al-sem's
//! OWN validation error strings verbatim (`policy-loader.ts:56-158`). The LIBRARY
//! `yaml:`-prefixed parse errors are NOT byte-matchable (no differential feeds
//! malformed YAML), so a `serde_yaml` parse failure maps to a single
//! `policy root must be a map` / generic message — never asserted by a golden.
//!
//! Validation contract (mirrors loadPolicyFromString):
//!   - `version` must be the integer **1**.
//!   - all predicate values are string-only (enforced by the compiler).
//!   - duplicate keys → error (serde_yaml rejects duplicate mapping keys by default).
//!   - anchors/aliases are rejected (al-sem `maxAliasCount: 0`); serde_yaml expands
//!     aliases — but no differential exercises anchors, so this is not byte-checked.

use serde_yaml::Value;

use crate::engine::gate::policy::policy_types::{
    ALLOWED_COVERAGE, ALLOWED_FACTS, ALLOWED_SEVERITIES, ALLOWED_UNKNOWN, PolicyDefaults,
    PolicyDoc, Rule,
};
use crate::engine::gate::policy::predicate_compiler::compile_predicate;

/// Result of a load attempt. al-sem `LoadResult`.
pub enum LoadResult {
    Ok {
        policy: PolicyDoc,
        #[allow(dead_code)]
        warnings: Vec<String>,
    },
    Err {
        errors: Vec<String>,
        #[allow(dead_code)]
        warnings: Vec<String>,
    },
}

/// Rule id regex: `^[a-z][a-z0-9-]{2,80}$`. al-sem `RULE_ID_RE`.
fn rule_id_valid(id: &str) -> bool {
    let bytes = id.as_bytes();
    // First char a-z; total length 3..=81 (1 + {2,80}); rest [a-z0-9-].
    if bytes.is_empty() {
        return false;
    }
    if !(bytes[0].is_ascii_lowercase()) {
        return false;
    }
    let rest = &bytes[1..];
    if rest.len() < 2 || rest.len() > 80 {
        return false;
    }
    rest.iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

const ALLOWED_TOP_FIELDS: &[&str] = &["version", "description", "defaults", "rules"];
const ALLOWED_RULE_FIELDS: &[&str] = &[
    "id",
    "title",
    "description",
    "message",
    "severity",
    "when",
    "except",
    "requireCoverage",
    "onUnknown",
    "facts",
];

fn mapping_keys(m: &serde_yaml::Mapping) -> Vec<String> {
    m.iter()
        .filter_map(|(k, _)| match k {
            Value::String(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

fn get<'a>(m: &'a serde_yaml::Mapping, key: &str) -> Option<&'a Value> {
    m.get(Value::String(key.to_string()))
}

fn as_str(v: &Value) -> Option<&str> {
    match v {
        Value::String(s) => Some(s.as_str()),
        _ => None,
    }
}

/// Render the `version` value the way al-sem's `String(top.version)` would for the
/// error message.
fn version_display(v: Option<&Value>) -> String {
    match v {
        None => "undefined".to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Null) => "null".to_string(),
        Some(Value::Sequence(_)) | Some(Value::Mapping(_)) => "[object Object]".to_string(),
        Some(Value::Tagged(t)) => version_display(Some(&t.value)),
    }
}

/// al-sem `loadPolicyFromString`.
pub fn load_policy_from_string(yaml: &str) -> LoadResult {
    let mut errors: Vec<String> = Vec::new();
    let warnings: Vec<String> = Vec::new();

    // Parse. A library parse failure is NOT byte-matched (the differential never
    // feeds malformed YAML). serde_yaml rejects duplicate keys by default — matching
    // al-sem's `uniqueKeys: true`.
    let parsed: Result<Value, _> = serde_yaml::from_str(yaml);
    let root = match parsed {
        Ok(v) => v,
        Err(e) => {
            return LoadResult::Err {
                errors: vec![format!("yaml: {e}")],
                warnings,
            };
        }
    };

    let Value::Mapping(top) = root else {
        return LoadResult::Err {
            errors: vec!["policy root must be a map".to_string()],
            warnings,
        };
    };

    for k in mapping_keys(&top) {
        if !ALLOWED_TOP_FIELDS.contains(&k.as_str()) {
            errors.push(format!("unknown top-level field '{k}'"));
        }
    }

    // version must coerce to the JS number 1. al-sem checks `top.version !== 1`
    // where `top.version` is `doc.toJS()` — so a YAML scalar that the parser yields
    // as the number 1 passes. eemeli yaml v2 (YAML 1.2 core) yields a *number* for
    // both `1` and `1.0`; serde_yaml agrees (`Number(1)` / `Number(1.0)`), so we
    // accept any Number whose value == 1.0 (covers `1`, `1.0`, `0x1`). A quoted
    // `"1"`/`"1.0"` is a string in both → `"1" !== 1` errors (kept). The leading-zero
    // `01` is a STRING in YAML 1.2 core (serde_yaml agrees) → errors in both; it is
    // corpus-invisible (the only `version` value in the differential is the literal
    // `1`), so this is not byte-checked.
    let version_ok =
        matches!(get(&top, "version"), Some(Value::Number(n)) if n.as_f64() == Some(1.0));
    if !version_ok {
        errors.push(format!(
            "policy version must be 1 (got {})",
            version_display(get(&top, "version"))
        ));
    }

    // defaults block.
    let mut defaults = PolicyDefaults::default();
    if let Some(d_val) = get(&top, "defaults") {
        match d_val {
            Value::Mapping(d) => {
                if let Some(ou) = get(d, "onUnknown") {
                    match as_str(ou) {
                        Some(s) if ALLOWED_UNKNOWN.contains(&s) => {
                            defaults.on_unknown = Some(s.to_string());
                        }
                        _ => errors.push(format!(
                            "defaults.onUnknown: must be one of {}",
                            ALLOWED_UNKNOWN.join(", ")
                        )),
                    }
                }
                if let Some(rc) = get(d, "requireCoverage") {
                    match as_str(rc) {
                        Some(s) if ALLOWED_COVERAGE.contains(&s) => {
                            defaults.require_coverage = Some(s.to_string());
                        }
                        _ => errors.push(format!(
                            "defaults.requireCoverage: must be one of {}",
                            ALLOWED_COVERAGE.join(", ")
                        )),
                    }
                }
            }
            _ => errors.push("defaults must be a map".to_string()),
        }
    }

    // rules must be an array.
    let rules_val = get(&top, "rules");
    let Some(Value::Sequence(rules_seq)) = rules_val else {
        errors.push("rules must be an array".to_string());
        return LoadResult::Err { errors, warnings };
    };

    let mut rules: Vec<Rule> = Vec::new();
    let mut seen_ids: Vec<String> = Vec::new();

    for (i, raw) in rules_seq.iter().enumerate() {
        let path = format!("rules[{i}]");
        let Value::Mapping(r) = raw else {
            errors.push(format!("{path}: must be a map"));
            continue;
        };

        for k in mapping_keys(r) {
            if !ALLOWED_RULE_FIELDS.contains(&k.as_str()) {
                errors.push(format!("{path}.{k}: unknown rule field"));
            }
        }

        let id = get(r, "id").and_then(as_str).unwrap_or("").to_string();
        if !rule_id_valid(&id) {
            errors.push(format!(
                "{path}.id: must match ^[a-z][a-z0-9-]{{2,80}}$ (got '{id}')"
            ));
            continue;
        }
        if seen_ids.iter().any(|s| s == &id) {
            errors.push(format!("{path}: duplicate rule id '{id}'"));
            continue;
        }
        seen_ids.push(id.clone());

        let severity = get(r, "severity").and_then(as_str);
        let severity = match severity {
            Some(s) if ALLOWED_SEVERITIES.contains(&s) => s.to_string(),
            _ => {
                errors.push(format!(
                    "{path}.severity: must be one of {}",
                    ALLOWED_SEVERITIES.join(", ")
                ));
                continue;
            }
        };

        let Some(when_raw) = get(r, "when") else {
            errors.push(format!("{path}.when: required"));
            continue;
        };
        let when = match compile_predicate(when_raw, &format!("{path}.when")) {
            Ok(p) => p,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };

        let except = match get(r, "except") {
            None => None,
            Some(ex_raw) => match compile_predicate(ex_raw, &format!("{path}.except")) {
                Ok(p) => Some(p),
                Err(e) => {
                    errors.push(e);
                    continue;
                }
            },
        };

        let require_coverage = match get(r, "requireCoverage") {
            None => None,
            Some(v) => match as_str(v) {
                Some(s) if ALLOWED_COVERAGE.contains(&s) => Some(s.to_string()),
                _ => {
                    errors.push(format!("{path}.requireCoverage: invalid value"));
                    continue;
                }
            },
        };

        let on_unknown = match get(r, "onUnknown") {
            None => None,
            Some(v) => match as_str(v) {
                Some(s) if ALLOWED_UNKNOWN.contains(&s) => Some(s.to_string()),
                _ => {
                    errors.push(format!("{path}.onUnknown: invalid value"));
                    continue;
                }
            },
        };

        let facts = match get(r, "facts") {
            None => None,
            Some(v) => match as_str(v) {
                Some(s) if ALLOWED_FACTS.contains(&s) => Some(s.to_string()),
                _ => {
                    errors.push(format!("{path}.facts: invalid value"));
                    continue;
                }
            },
        };

        rules.push(Rule {
            id,
            title: get(r, "title").and_then(as_str).map(|s| s.to_string()),
            description: get(r, "description")
                .and_then(as_str)
                .map(|s| s.to_string()),
            message: get(r, "message").and_then(as_str).map(|s| s.to_string()),
            severity,
            when,
            except,
            require_coverage,
            on_unknown,
            facts,
        });
    }

    if !errors.is_empty() {
        return LoadResult::Err { errors, warnings };
    }

    let has_defaults = defaults.on_unknown.is_some() || defaults.require_coverage.is_some();
    let policy = PolicyDoc {
        version: 1,
        description: get(&top, "description")
            .and_then(as_str)
            .map(|s| s.to_string()),
        defaults: if has_defaults { Some(defaults) } else { None },
        rules,
    };
    LoadResult::Ok { policy, warnings }
}

/// Load a policy from a file path. al-sem `loadPolicyFromFile`.
pub fn load_policy_from_file(path: &std::path::Path) -> LoadResult {
    load_policy_from_input(&crate::program::inputs::FileInput {
        path: path.to_path_buf(),
        bytes: std::fs::read(path),
    })
}

/// [`load_policy_from_file`] over a file already read (the command's
/// `WorkspaceInputs`, #160). Reads `text()`, so a read error and a non-UTF-8 file both
/// keep the `failed to read <path>: <e>` text.
pub fn load_policy_from_input(file: &crate::program::inputs::FileInput) -> LoadResult {
    match file.text() {
        Ok(yaml) => load_policy_from_string(yaml),
        Err(e) => LoadResult::Err {
            errors: vec![format!("failed to read {}: {}", file.path.display(), e)],
            warnings: Vec::new(),
        },
    }
}

/// The bundled default policy, vendored from al-sem at build time. al-sem reads
/// `src/policy/policy-default.yaml`; we embed the byte-identical vendored copy.
pub const BUNDLED_DEFAULT_POLICY_YAML: &str = include_str!("policy-default.yaml");
