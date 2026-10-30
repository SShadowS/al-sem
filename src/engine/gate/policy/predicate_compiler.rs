//! Predicate compiler — port of al-sem `src/policy/predicate-compiler.ts`.
//!
//! Compiles a raw YAML predicate node (a `serde_yaml::Value`) into the typed
//! [`Predicate`] AST. 4 node kinds (field / all / any / not). The operator is
//! DERIVED from the field's `value_shape` (the YAML never names an operator):
//!   enum/enum-list → `in`; glob/glob-list → `glob`/`glob-in`;
//!   string-exact/string-list → `==`/`in`.
//!
//! Errors reproduce al-sem's verbatim message strings.

use serde_yaml::Value;

use crate::engine::gate::policy::policy_types::{Predicate, PredicateOperator, PredicateValue};
use crate::engine::gate::policy::predicate_fields::{
    FieldScope, FieldValueShape, PredicateFieldDef, get_field_def,
};

pub type CompileResult = Result<Predicate, String>;

/// Collect the keys of a YAML mapping in insertion order (mirrors JS
/// `Object.keys`). Non-string keys are stringified defensively.
fn mapping_keys(m: &serde_yaml::Mapping) -> Vec<String> {
    m.iter()
        .map(|(k, _)| match k {
            Value::String(s) => s.clone(),
            other => yaml_scalar_to_string(other),
        })
        .collect()
}

fn yaml_scalar_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Null => "null".to_string(),
        _ => String::new(),
    }
}

/// Look up a key in a mapping by its string name.
fn get<'a>(m: &'a serde_yaml::Mapping, key: &str) -> Option<&'a Value> {
    m.get(Value::String(key.to_string()))
}

/// al-sem `compilePredicate(raw, path)`.
pub fn compile_predicate(raw: &Value, path: &str) -> CompileResult {
    let Value::Mapping(obj) = raw else {
        let p = if path.is_empty() { "<root>" } else { path };
        return Err(format!("{p}: expected an object"));
    };
    let keys = mapping_keys(obj);

    // Single-key boolean wrappers.
    if keys.len() == 1 {
        let k = &keys[0];
        if k == "all" || k == "any" {
            let items = get(obj, k).unwrap();
            let Value::Sequence(seq) = items else {
                return Err(format!("{path}.{k}: expected array"));
            };
            if seq.is_empty() {
                return Err(format!("{path}.{k}: empty array not allowed"));
            }
            let mut children: Vec<Predicate> = Vec::with_capacity(seq.len());
            for (i, item) in seq.iter().enumerate() {
                let child = compile_predicate(item, &format!("{path}.{k}[{i}]"))?;
                children.push(child);
            }
            if k == "any" {
                let scopes = collect_scopes(&children);
                if scopes.len() > 1 {
                    return Err(format!(
                        "{path}.any: mixed {} scope predicates — move routine filters into a parent all, or split into separate rules",
                        scopes_join(&scopes)
                    ));
                }
            }
            return Ok(if k == "all" {
                Predicate::All { children }
            } else {
                Predicate::Any { children }
            });
        }
        if k == "not" {
            let inner = get(obj, "not").unwrap();
            let is_empty_map = matches!(inner, Value::Mapping(m) if m.is_empty());
            if !matches!(inner, Value::Mapping(_)) || is_empty_map {
                return Err(format!("{path}.not: empty or missing predicate"));
            }
            let child = compile_predicate(inner, &format!("{path}.not"))?;
            let scopes = collect_scopes(std::slice::from_ref(&child));
            // Disallow `not` over a routine-only predicate.
            if scopes.len() == 1 && scopes.contains(&FieldScope::Routine) {
                return Err(format!(
                    "{path}.not wraps a routine-scope predicate — use except: for routine-scope carve-outs"
                ));
            }
            return Ok(Predicate::Not {
                child: Box::new(child),
            });
        }
    }

    // Multi-key map (or single-key field) → compile each key; wrap in implicit all if >1.
    let mut children: Vec<Predicate> = Vec::new();
    for k in &keys {
        if k == "all" || k == "any" || k == "not" {
            // Re-wrap the single boolean key into its own mapping and recurse.
            let mut sub = serde_yaml::Mapping::new();
            sub.insert(Value::String(k.clone()), get(obj, k).unwrap().clone());
            let p = compile_predicate(&Value::Mapping(sub), path)?;
            children.push(p);
            continue;
        }
        // Field predicate.
        let Some(def) = get_field_def(k) else {
            return Err(format!("{path}.{k}: unknown predicate field"));
        };
        let field_p = compile_field_predicate(def, get(obj, k).unwrap(), &format!("{path}.{k}"))?;
        children.push(field_p);
    }
    if children.is_empty() {
        let p = if path.is_empty() { "<root>" } else { path };
        return Err(format!("{p}: empty predicate"));
    }
    if children.len() == 1 {
        return Ok(children.into_iter().next().unwrap());
    }
    Ok(Predicate::All { children })
}

/// al-sem `compileFieldPredicate`. Derives the operator from the field's value shape.
fn compile_field_predicate(def: &PredicateFieldDef, raw: &Value, path: &str) -> CompileResult {
    let is_list = matches!(raw, Value::Sequence(_));
    // Build the raw item list (JS `isList ? raw : [raw]`).
    let items: Vec<&Value> = match raw {
        Value::Sequence(seq) => seq.iter().collect(),
        single => vec![single],
    };

    // Extract string values — al-sem REQUIRES every value be a string (the compiler
    // rejects non-strings). `serde_yaml::Value::String` is the only accepted scalar.
    // (YAML 1.2 core: yes/no/on/off parse as strings, so they arrive here as
    // `Value::String` — matching eemeli yaml v2.)
    let str_of = |v: &Value| -> Option<String> {
        match v {
            Value::String(s) => Some(s.clone()),
            _ => None,
        }
    };

    match def.value_shape {
        FieldValueShape::Enum | FieldValueShape::EnumList => {
            if let Some(allowed) = def.enum_values {
                for v in &items {
                    let ok = match v {
                        Value::String(s) => allowed.contains(&s.as_str()),
                        _ => false,
                    };
                    if !ok {
                        return Err(format!(
                            "{path}: invalid enum value '{}' (allowed: {})",
                            yaml_value_display(v),
                            allowed.join(", ")
                        ));
                    }
                }
            }
            if def.value_shape == FieldValueShape::Enum && is_list && items.len() > 1 {
                return Err(format!("{path}: expected single value, got list"));
            }
            // operator is always `in`; value is the LIST form (al-sem: `value: list`).
            let list: Vec<String> = items.iter().map(|v| yaml_value_display(v)).collect();
            Ok(Predicate::Field {
                field: def.name.to_string(),
                operator: PredicateOperator::In,
                value: PredicateValue::List(list),
            })
        }
        FieldValueShape::Glob | FieldValueShape::GlobList => {
            let mut strs: Vec<String> = Vec::with_capacity(items.len());
            for v in &items {
                match str_of(v) {
                    Some(s) => strs.push(s),
                    None => return Err(format!("{path}: glob value must be a string")),
                }
            }
            if is_list {
                Ok(Predicate::Field {
                    field: def.name.to_string(),
                    operator: PredicateOperator::GlobIn,
                    value: PredicateValue::List(strs),
                })
            } else {
                Ok(Predicate::Field {
                    field: def.name.to_string(),
                    operator: PredicateOperator::Glob,
                    value: PredicateValue::Str(strs.into_iter().next().unwrap()),
                })
            }
        }
        FieldValueShape::StringExact | FieldValueShape::StringList => {
            let mut strs: Vec<String> = Vec::with_capacity(items.len());
            for v in &items {
                match str_of(v) {
                    Some(s) => strs.push(s),
                    None => return Err(format!("{path}: value must be a string")),
                }
            }
            if is_list {
                Ok(Predicate::Field {
                    field: def.name.to_string(),
                    operator: PredicateOperator::In,
                    value: PredicateValue::List(strs),
                })
            } else {
                Ok(Predicate::Field {
                    field: def.name.to_string(),
                    operator: PredicateOperator::Eq,
                    value: PredicateValue::Str(strs.into_iter().next().unwrap()),
                })
            }
        }
    }
}

/// Render a YAML scalar the way al-sem's `String(v)` would for the enum error
/// message (and for the enum-list `value` payload — which always holds strings,
/// but a non-string would have been rejected before reaching the payload step).
fn yaml_value_display(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Null => "null".to_string(),
        Value::Sequence(_) => "[object Object]".to_string(),
        Value::Mapping(_) => "[object Object]".to_string(),
        Value::Tagged(t) => yaml_value_display(&t.value),
    }
}

/// Collect the field scopes referenced by a predicate set. al-sem `collectScopes`.
/// The loader reads it too, to check a pin's shape (policy version 2).
pub(crate) fn collect_scopes(preds: &[Predicate]) -> Vec<FieldScope> {
    let mut out: Vec<FieldScope> = Vec::new();
    for p in preds {
        collect_scopes_one(p, &mut out);
    }
    out
}

fn collect_scopes_one(p: &Predicate, out: &mut Vec<FieldScope>) {
    match p {
        Predicate::Field { field, .. } => {
            if let Some(def) = get_field_def(field)
                && !out.contains(&def.scope)
            {
                out.push(def.scope);
            }
        }
        Predicate::Not { child } => collect_scopes_one(child, out),
        Predicate::All { children } | Predicate::Any { children } => {
            for c in children {
                collect_scopes_one(c, out);
            }
        }
    }
}

/// Join scope names with `/` for the mixed-scope error (al-sem `[...scopes].join("/")`).
fn scopes_join(scopes: &[FieldScope]) -> String {
    scopes
        .iter()
        .map(|s| match s {
            FieldScope::Routine => "routine",
            FieldScope::Fact => "fact",
            FieldScope::Model => "model",
            FieldScope::Write => "write",
            FieldScope::Rule => "rule",
        })
        .collect::<Vec<_>>()
        .join("/")
}
