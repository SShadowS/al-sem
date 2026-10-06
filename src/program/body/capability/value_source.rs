//! Port of al-sem `src/index/capability/value-source.ts` — classify an
//! `ExpressionInfo` (the serialized tree-sitter expression summary) into a
//! [`ValueSource`]. Used by every IO / dispatch / background family extractor to
//! capture resource arguments (HTTP URL, storage key, dispatch target, telemetry
//! event id, ...).
//!
//! Chases `constant-var` chains via the variable index's captured `initializer`,
//! capped at `MAX_CHASE_DEPTH = 3`. NEVER panics — any internal failure returns
//! `ValueSource::Unknown` (engine-never-throws).
//!
//! L2 strip: `table-field` `ValueSource`s carry NO `tableId` (it is L3-resolved /
//! `"unknown"` at L2). Because [`ValueSource::TableField`] has no `tableId` field,
//! the strip is structural — both here and when parsing a stored initializer JSON.

use super::super::features::PExpressionInfo;
use super::{ExtractionContext, ValueSource};

const MAX_CHASE_DEPTH: u32 = 3;

/// Classify a `PExpressionInfo` as a [`ValueSource`]. `None` → `Unknown`.
pub fn classify_value_source(
    info: Option<&PExpressionInfo>,
    ctx: &ExtractionContext,
) -> ValueSource {
    classify_at_depth(info, ctx, 0)
}

fn classify_at_depth(
    info: Option<&PExpressionInfo>,
    ctx: &ExtractionContext,
    depth: u32,
) -> ValueSource {
    let Some(info) = info else {
        return ValueSource::Unknown;
    };

    match info.kind.as_str() {
        // ── Literal forms ──────────────────────────────────────────────────
        "string_literal" => ValueSource::Literal {
            value: info
                .value
                .clone()
                .unwrap_or_else(|| strip_single_quotes(&info.text)),
        },
        "integer" | "decimal" | "boolean" => ValueSource::Literal {
            value: info
                .value
                .clone()
                .unwrap_or_else(|| info.text.trim().to_string()),
        },

        // ── Enum / database reference ──────────────────────────────────────
        "qualified_enum_value" | "database_reference" => {
            let enum_name = info
                .qualifier
                .as_deref()
                .map(strip_double_quotes)
                .unwrap_or_default();
            let member = info
                .member
                .clone()
                .or_else(|| info.value.clone())
                .unwrap_or_default();
            ValueSource::Enum { enum_name, member }
        }

        // ── Identifier — parameter / constant-var / chase ──────────────────
        "identifier" | "quoted_identifier" => {
            let name = info
                .value
                .clone()
                .unwrap_or_else(|| info.text.clone())
                .to_lowercase();
            classify_identifier(&name, ctx, depth)
        }

        // ── Member expression — potential table-field ──────────────────────
        "member_expression" => classify_member_expression(&info.text, ctx),

        // ── Unary expression (e.g. -3) — literal when operand numeric ──────
        "unary_expression" => match &info.value {
            Some(v) => ValueSource::Literal { value: v.clone() },
            None => ValueSource::Expression,
        },

        // ── Anything else → expression ─────────────────────────────────────
        _ => ValueSource::Expression,
    }
}

fn classify_identifier(name_lc: &str, ctx: &ExtractionContext, depth: u32) -> ValueSource {
    let Some(sym) = ctx.variables.get(name_lc) else {
        // Not in scope — opaque expression reference.
        return ValueSource::Expression;
    };

    if sym.is_parameter {
        return ValueSource::Parameter {
            index: sym.parameter_index.unwrap_or(0),
            var_name: name_lc.to_string(),
        };
    }

    // Local variable — try to chase the captured initializer.
    let init = sym.initializer.as_ref().map(parse_initializer);

    match init {
        None => ValueSource::ConstantVar {
            var_name: name_lc.to_string(),
            initializer: Box::new(ValueSource::Unknown),
        },
        Some(init) => {
            // No initializer captured or already opaque → emit constant-var.
            if matches!(init, ValueSource::Unknown | ValueSource::Expression) {
                return ValueSource::ConstantVar {
                    var_name: name_lc.to_string(),
                    initializer: Box::new(init),
                };
            }

            if depth >= MAX_CHASE_DEPTH {
                return ValueSource::ConstantVar {
                    var_name: name_lc.to_string(),
                    initializer: Box::new(init),
                };
            }

            // Chase one hop deeper for `constant-var` (var-to-var alias).
            if let ValueSource::ConstantVar {
                var_name: inner_name,
                ..
            } = &init
            {
                let deeper = classify_identifier(inner_name, ctx, depth + 1);
                return match deeper {
                    ValueSource::Literal { .. }
                    | ValueSource::Enum { .. }
                    | ValueSource::Parameter { .. } => deeper,
                    _ => ValueSource::ConstantVar {
                        var_name: name_lc.to_string(),
                        initializer: Box::new(deeper),
                    },
                };
            }

            // Initializer already resolved (literal / enum / parameter / table-field).
            init
        }
    }
}

/// Parse a stored variable-initializer JSON (one hop) into a [`ValueSource`],
/// STRIPPING `tableId` from any `table-field`. Unrecognized shapes → `Unknown`.
fn parse_initializer(value: &serde_json::Value) -> ValueSource {
    let Some(kind) = value.get("kind").and_then(|k| k.as_str()) else {
        return ValueSource::Unknown;
    };
    match kind {
        "literal" => ValueSource::Literal {
            value: value
                .get("value")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        "enum" => ValueSource::Enum {
            enum_name: value
                .get("enumName")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            member: value
                .get("member")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        "parameter" => ValueSource::Parameter {
            index: value.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            var_name: value
                .get("varName")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        "constant-var" => ValueSource::ConstantVar {
            var_name: value
                .get("varName")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            initializer: Box::new(
                value
                    .get("initializer")
                    .map(parse_initializer)
                    .unwrap_or(ValueSource::Unknown),
            ),
        },
        // `tableId` STRIPPED — only `fieldName` survives.
        "table-field" => ValueSource::TableField {
            field_name: value
                .get("fieldName")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        "expression" => ValueSource::Expression,
        _ => ValueSource::Unknown,
    }
}

/// Parse a `member_expression` text `Receiver.Field` and classify it as a
/// `table-field` when the receiver resolves to a record-typed variable; else
/// `expression`. The receiver's `tableId` is L3 (absent at L2) → STRIPPED.
fn classify_member_expression(text: &str, ctx: &ExtractionContext) -> ValueSource {
    let Some(dot_idx) = text.find('.') else {
        return ValueSource::Expression;
    };

    let receiver_raw = text[..dot_idx].trim();
    let field_raw = text[dot_idx + 1..].trim();
    let receiver_lc = receiver_raw.to_lowercase();

    let Some(sym) = ctx.variables.get(&receiver_lc) else {
        return ValueSource::Expression;
    };

    let decl_type = sym.declared_type.to_lowercase();
    let is_record = decl_type.starts_with("record ")
        || decl_type == "record"
        || decl_type.starts_with("recordref");
    if !is_record {
        // Member call on a non-record (e.g. HttpClient.Get) — expression.
        return ValueSource::Expression;
    }

    ValueSource::TableField {
        field_name: strip_double_quotes(field_raw).to_string(),
    }
}

// ─── Quote helpers ───────────────────────────────────────────────────────────

fn strip_single_quotes(s: &str) -> String {
    let t = s.trim();
    let bytes = t.as_bytes();
    if bytes.len() >= 2 && bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\'' {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

fn strip_double_quotes(s: &str) -> String {
    let t = s.trim();
    let bytes = t.as_bytes();
    if bytes.len() >= 2 && bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"' {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}
