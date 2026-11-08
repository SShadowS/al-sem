//! The complete compact result index (output validation spec 2026-11-06
//! §5.2, F42): everything a caller asked for by name — each requested
//! root's identity and verdicts, each selected test — goes, always, into a
//! JSON file beside a file answer (`<answer>.index.json`, schema
//! [`SCHEMA`]), so nothing requested is only in the full document. The
//! envelope's `summary` (at most 8 KiB) carries as many rows as fit, then
//! `index: {path, rows, sha256}` and, when it cut rows,
//! `summaryTruncated: {rowsListed, rowsTotal, dropped}`. `summaryTruncated`
//! is about the transport; a selection's own completeness
//! (`selectionComplete`) is reported apart. `countsOnly` is gone.
//!
//! The renderer that builds the summary builds the rows ([`with_index`]);
//! the front end that places the answer file writes them (`mcp::pool`), so
//! it costs one more file write, not another pass.

use serde_json::{Value, json};

use crate::engine::batch::output::SUMMARY_CAP_BYTES;

/// The index file's schema.
pub const SCHEMA: &str = "al-sem.result-index/1";

/// The key a summary carries its index rows under until the front end moves
/// them to the file (never sent to the client).
pub const ROWS_KEY: &str = "indexRows";

/// The key a summary carries fields for the index file's top level under
/// (r3 §5.2: what every row would repeat, said once per index, such as the
/// tests hand-off), moved to the file with the rows (never sent).
pub const SHARED_KEY: &str = "indexShared";

/// What the summary keeps free for the `index` object the front end adds.
pub const INDEX_RESERVE_BYTES: usize = 512;

/// `summary` bounded for the envelope with its complete `rows`: the rows go
/// under [`ROWS_KEY`] whole; `summary[list_key]` (the rows as the summary
/// shows them) is cut from the end until the summary, `index` reserved,
/// fits [`SUMMARY_CAP_BYTES`]. A cut adds `summaryTruncated` with the
/// dropped rows' `id_key` values (or their row range in the index, when the
/// ids do not fit either).
#[must_use]
pub fn with_index(mut summary: Value, list_key: &str, id_key: &str, rows: Vec<Value>) -> Value {
    let size = |v: &Value| v.to_string().len() + INDEX_RESERVE_BYTES;
    let total = summary[list_key].as_array().map_or(0, Vec::len);
    let mut dropped: Vec<Value> = Vec::new();
    while size(&summary) > SUMMARY_CAP_BYTES {
        let Some(row) = summary[list_key].as_array_mut().and_then(Vec::pop) else {
            break;
        };
        dropped.push(row.get(id_key).cloned().unwrap_or(Value::Null));
        // Room for the truncation record itself.
        let listed = total - dropped.len();
        summary["summaryTruncated"] = json!({
            "rowsListed": listed,
            "rowsTotal": total,
            "dropped": [format!("index rows {}-{}", listed + 1, total)],
        });
    }
    if !dropped.is_empty() {
        dropped.reverse();
        let mut t = summary.clone();
        t["summaryTruncated"]["dropped"] = Value::Array(dropped);
        if size(&t) <= SUMMARY_CAP_BYTES {
            summary = t;
        }
    }
    summary[ROWS_KEY] = Value::Array(rows);
    summary
}

/// The index file's bytes for `rows` of `tool`'s answer at `answer_path`,
/// with `shared` ([`SHARED_KEY`]'s object) at the top level.
#[must_use]
pub fn file_bytes(
    tool: &str,
    answer_path: &str,
    rows: &[Value],
    shared: Option<&Value>,
) -> Vec<u8> {
    let mut doc = json!({
        "schema": SCHEMA,
        "tool": tool,
        "answer": answer_path,
        "rows": rows,
    });
    if let (Some(Value::Object(s)), Value::Object(d)) = (shared, &mut doc) {
        for (k, v) in s {
            d.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    let mut b = serde_json::to_vec_pretty(&doc).expect("the index serializes");
    b.push(b'\n');
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_stay_whole_and_the_listed_ones_are_cut() {
        let rows: Vec<Value> = (0..200)
            .map(|i| json!({"stableId": format!("id{i}"), "pad": "x".repeat(60)}))
            .collect();
        let s = with_index(
            json!({"kind": "t", "routines": rows.clone()}),
            "routines",
            "stableId",
            rows.clone(),
        );
        let mut without = s.clone();
        without.as_object_mut().unwrap().remove(ROWS_KEY);
        assert!(without.to_string().len() + INDEX_RESERVE_BYTES <= SUMMARY_CAP_BYTES);
        assert_eq!(s[ROWS_KEY].as_array().unwrap().len(), 200);
        let t = &s["summaryTruncated"];
        assert_eq!(t["rowsTotal"], 200);
        assert_eq!(
            t["rowsListed"].as_u64().unwrap() as usize,
            s["routines"].as_array().unwrap().len()
        );
    }
}
