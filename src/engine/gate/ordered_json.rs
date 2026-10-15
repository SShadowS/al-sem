//! `ordered_json` — a tiny INSERTION-ORDER JSON value tree + serializer shared by
//! the cli-c hand-built JSON envelopes (events fanout/chains, policy check, policy
//! explain AST).
//!
//! These envelopes must emit keys in TS `JSON.stringify(obj, undefined, 2)`
//! insertion order — NOT the alphabetical order serde_json/`DocumentEnvelope` use.
//! Three near-identical copies of this `Jv`/`write`/`serialize` logic (with the same
//! JSON control-char escape table) previously lived in `events.rs`, `format_policy.rs`
//! and `policy_types.rs`; this is the single source of truth.
//!
//! Escaping matches `JSON.stringify`: `"` `\\` `\n` `\r` `\t` `\b` `\f` are the named
//! escapes; every other C0 control char (< U+0020) becomes `\u00XX` (lowercase hex,
//! 4 digits); all other chars (incl. non-ASCII) pass through verbatim.

/// An insertion-order JSON value. `Obj` preserves push order on serialization.
pub enum Jv {
    Str(String),
    Num(i64),
    Bool(bool),
    /// Insertion-order object — keys emitted in push order.
    Obj(Vec<(String, Jv)>),
    Arr(Vec<Jv>),
    /// A JSON literal written verbatim (`null`, a number serde_json printed).
    Raw(String),
}

impl Jv {
    /// A `serde_json::Value`, keys in its own map order.
    pub fn from_json(v: &serde_json::Value) -> Jv {
        use serde_json::Value;
        match v {
            Value::String(s) => Jv::Str(s.clone()),
            Value::Bool(b) => Jv::Bool(*b),
            Value::Array(a) => Jv::Arr(a.iter().map(Jv::from_json).collect()),
            Value::Object(m) => Jv::Obj(
                m.iter()
                    .map(|(k, v)| (k.clone(), Jv::from_json(v)))
                    .collect(),
            ),
            Value::Null | Value::Number(_) => Jv::Raw(v.to_string()),
        }
    }

    /// String value from a `&str`.
    pub fn s(s: &str) -> Jv {
        Jv::Str(s.to_string())
    }
    /// Numeric value from any unsigned size (events counts are `usize`).
    pub fn n(n: usize) -> Jv {
        Jv::Num(n as i64)
    }
}

/// Append a JSON-escaped string (with surrounding quotes) — `JSON.stringify` rules.
pub fn escape_into(s: &str, buf: &mut String) {
    buf.push('"');
    for c in s.chars() {
        match c {
            '"' => buf.push_str("\\\""),
            '\\' => buf.push_str("\\\\"),
            '\n' => buf.push_str("\\n"),
            '\r' => buf.push_str("\\r"),
            '\t' => buf.push_str("\\t"),
            '\u{0008}' => buf.push_str("\\b"),
            '\u{000C}' => buf.push_str("\\f"),
            // Other C0 control chars (< U+0020) → \u00XX (lowercase hex, 4 digits).
            c if (c as u32) < 0x20 => {
                buf.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => buf.push(c),
        }
    }
    buf.push('"');
}

fn push_spaces(buf: &mut String, n: usize) {
    for _ in 0..n {
        buf.push(' ');
    }
}

/// Write a `Jv` with 2-space indentation at `indent` (matching `JSON.stringify(_, _, 2)`).
pub fn write_jv(v: &Jv, buf: &mut String, indent: usize) {
    match v {
        Jv::Str(s) => escape_into(s, buf),
        Jv::Num(n) => buf.push_str(&n.to_string()),
        Jv::Bool(b) => buf.push_str(if *b { "true" } else { "false" }),
        Jv::Raw(s) => buf.push_str(s),
        Jv::Obj(pairs) => {
            if pairs.is_empty() {
                buf.push_str("{}");
                return;
            }
            buf.push('{');
            let inner = indent + 2;
            for (i, (k, val)) in pairs.iter().enumerate() {
                buf.push('\n');
                push_spaces(buf, inner);
                escape_into(k, buf);
                buf.push_str(": ");
                write_jv(val, buf, inner);
                if i + 1 < pairs.len() {
                    buf.push(',');
                }
            }
            buf.push('\n');
            push_spaces(buf, indent);
            buf.push('}');
        }
        Jv::Arr(items) => {
            if items.is_empty() {
                buf.push_str("[]");
                return;
            }
            buf.push('[');
            let inner = indent + 2;
            for (i, val) in items.iter().enumerate() {
                buf.push('\n');
                push_spaces(buf, inner);
                write_jv(val, buf, inner);
                if i + 1 < items.len() {
                    buf.push(',');
                }
            }
            buf.push('\n');
            push_spaces(buf, indent);
            buf.push(']');
        }
    }
}

/// Serialize a `Jv` to a 2-space-indented JSON string (NO trailing newline).
pub fn serialize_jv(v: &Jv) -> String {
    let mut buf = String::new();
    write_jv(v, &mut buf, 0);
    buf
}
