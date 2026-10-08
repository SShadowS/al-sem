//! #34: the temp-record suppression rule lives ONCE, in
//! `al_sem::program::body::features::known_temp_suppresses`. Every other site
//! projects its own carrier to `Option<bool>` and calls it.
//!
//! This scans non-test source for the rule's own spellings and fails on any:
//! - `value == Some(true)` as a whole word (the string-keyed `PTempState` test;
//!   `known_value == Some(true)` -- the definition -- does not match);
//! - `known_value() == Some(true)` (a projection compared by hand);
//! - `Known { value: true }` (the `SnapTempState` test).
//!
//! Skipped: `#[cfg(test)]` modules (constructors and truth tables live there)
//! and `//` comments. Tuple-enum proof/propagation/encoding spellings such as
//! `TempStateKind::Known(true)` are a different decision and do not match.

use std::path::Path;

fn rule_spelling(code: &str) -> Option<&'static str> {
    let bytes = code.as_bytes();
    let mut from = 0;
    while let Some(i) = code[from..].find("value == Some(true)") {
        let at = from + i;
        let word_start =
            at == 0 || !(bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_');
        if word_start {
            return Some("`value == Some(true)`");
        }
        from = at + 1;
    }
    if code.contains("known_value() == Some(true)") {
        return Some("`known_value() == Some(true)`");
    }
    if code.contains("Known { value: true }") {
        return Some("`Known { value: true }`");
    }
    None
}

/// Every offending `file:line: text` under `root`.
fn scan(root: &Path, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            scan(&path, out);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        // Skip `#[cfg(test)]` items by brace depth: once the attribute is seen,
        // the next `{` opens a block skipped until its matching `}`.
        let (mut pending_test, mut skip_depth) = (false, 0i32);
        for (n, raw) in text.lines().enumerate() {
            let code = raw.split("//").next().unwrap_or("");
            if skip_depth > 0 || pending_test {
                for c in code.chars() {
                    if c == '{' {
                        skip_depth += 1;
                        pending_test = false;
                    } else if c == '}' {
                        skip_depth -= 1;
                    }
                }
                continue;
            }
            if code.trim() == "#[cfg(test)]" {
                pending_test = true;
                continue;
            }
            if let Some(what) = rule_spelling(code) {
                out.push(format!(
                    "{}:{}: {what}: {}",
                    path.display(),
                    n + 1,
                    raw.trim()
                ));
            }
        }
    }
}

#[test]
fn temp_rule_lives_once() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut hits = Vec::new();
    scan(&base.join("src"), &mut hits);
    scan(&base.join("crates"), &mut hits);
    assert!(
        hits.is_empty(),
        "the temp-record rule is re-derived outside `known_temp_suppresses` -- \
         project the carrier to Option<bool> and call it instead:\n{}",
        hits.join("\n")
    );
}

/// The detector itself, on hand-stated lines: copies are caught, the
/// definition and the non-decision spellings are not.
#[test]
fn temp_rule_spelling_matcher() {
    for copy in [
        "    && ts.value == Some(true)",
        "if rv.temp_state_known_value() == Some(true) {",
        "matches!(ts, Some(SnapTempState::Known { value: true }))",
    ] {
        assert!(rule_spelling(copy).is_some(), "missed a copy: {copy}");
    }
    for fine in [
        "    known_value == Some(true)",
        "TempStateKind::Known(true) => ParamTemp::Temp,",
        "if cs.under_asserterror == Some(true) {",
    ] {
        assert!(rule_spelling(fine).is_none(), "false alarm: {fine}");
    }
}
