//! `--show-source` (MCP output wave 2, W2-7, plan item 2g): the cited line
//! ±2 under each location a human output prints.
//!
//! The text is the one `aldump --dump-dependency-source` writes: a `ws:` file
//! from the workspace folder, a `dep:<app guid>:<path>` file from the AL the
//! snapshot reads inside that dependency's `.app` (embedded source, or the AL
//! synthesized for a runtime package). It runs on the human text BEFORE
//! [`crate::program::resolve::full::readable_locations`] rewrites the guids,
//! so a location's file is never ambiguous.

use std::collections::HashMap;
use std::path::Path;

/// The source files a human output's locations may name.
pub struct SourceTexts {
    ws: std::path::PathBuf,
    /// `<guid lower-case>` → virtual path → text.
    deps: HashMap<String, HashMap<String, String>>,
}

impl SourceTexts {
    /// Over the workspace at `ws` (its dependencies' sources read as the
    /// dump command reads them). A snapshot that cannot be built leaves the
    /// dependency texts empty: their locations get no excerpt.
    #[must_use]
    pub fn read(ws: &Path) -> SourceTexts {
        let mut deps: HashMap<String, HashMap<String, String>> = HashMap::new();
        if let Ok(snapshot) = (crate::snapshot::SnapshotBuilder {
            workspace_root: ws.to_path_buf(),
            local_providers: Vec::new(),
        })
        .build()
        {
            for app in snapshot.apps.iter().skip(1) {
                let Some(root) = &app.source else {
                    continue;
                };
                let files = deps.entry(app.id.guid.to_ascii_lowercase()).or_default();
                for f in root.files.iter() {
                    files.insert(f.virtual_path.clone(), f.text.to_string());
                }
            }
        }
        SourceTexts {
            ws: ws.to_path_buf(),
            deps,
        }
    }

    /// The text of a location's file.
    fn text(&self, unit: &str) -> Option<String> {
        if let Some(rel) = unit.strip_prefix("ws:") {
            return std::fs::read_to_string(self.ws.join(rel)).ok();
        }
        let rest = unit.strip_prefix("dep:")?;
        let (guid, path) = rest.split_once(':')?;
        self.deps
            .get(&guid.to_ascii_lowercase())?
            .get(path)
            .cloned()
    }
}

/// Every `ws:<path>:<line>` / `dep:<guid>:<path>:<line>` location on `line`:
/// (file, 1-based line).
fn locations(line: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    let b = line.as_bytes();
    let mut i = 0;
    while i < line.len() {
        let rest = &line[i..];
        let boundary = i == 0 || !b[i - 1].is_ascii_alphanumeric();
        let head = if rest.starts_with("ws:") {
            3
        } else if rest.starts_with("dep:") && rest.len() > 41 && rest.as_bytes()[40] == b':' {
            41
        } else {
            0
        };
        if head == 0 || !boundary {
            i += rest.chars().next().map_or(1, char::len_utf8);
            continue;
        }
        let end = rest
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | ']' | '"' | ',' | ';'))
            .unwrap_or(rest.len());
        let tok = &rest[..end];
        // `<file>:<line>` or `<file>:<line>:<column>`.
        let mut parts: Vec<&str> = tok.rsplitn(3, ':').collect();
        parts.reverse();
        let found = match parts.as_slice() {
            [f, l, c] if l.parse::<usize>().is_ok() && c.parse::<usize>().is_ok() => {
                Some((f.to_string(), l.parse().unwrap()))
            }
            [f, l, last] => last.parse().ok().map(|n| (format!("{f}:{l}"), n)),
            [f, l] => l.parse().ok().map(|n| (f.to_string(), n)),
            _ => None,
        };
        if let Some((f, n)) = found
            && f.len() > head
        {
            out.push((f, n));
        }
        i += end.max(1);
    }
    out
}

/// `text` with the cited line ±2 under every line that prints a location:
/// `| N | source` lines, the cited one marked `>`. A location whose file is
/// not readable gets none.
#[must_use]
pub fn with_excerpts(text: &str, sources: &SourceTexts) -> String {
    let mut cache: HashMap<String, Option<Vec<String>>> = HashMap::new();
    let mut out = String::with_capacity(text.len() * 2);
    for line in text.lines() {
        out.push_str(line);
        out.push('\n');
        let indent = line.len() - line.trim_start().len() + 2;
        for (file, n) in locations(line) {
            let lines = cache
                .entry(file.clone())
                .or_insert_with(|| {
                    sources
                        .text(&file)
                        .map(|t| t.replace('\r', "").lines().map(String::from).collect())
                })
                .as_ref();
            let Some(lines) = lines.filter(|l| n >= 1 && n <= l.len()) else {
                continue;
            };
            // Output validation V2 (§9.1, F48): a location on a declaration
            // (a procedure, trigger, action or field header) shows the
            // routine's first statement instead, the excerpt starting there.
            let (n, range) = match first_statement(lines, n) {
                Some(s) => {
                    // Round-2 G26: the excerpt says which line it shows
                    // when it is not the cited one.
                    out.push_str(&format!("{:indent$}first statement at :{s}\n", ""));
                    (s, s..=(s + 4).min(lines.len()))
                }
                None => (n, n.saturating_sub(2).max(1)..=(n + 2).min(lines.len())),
            };
            for k in range {
                out.push_str(&format!(
                    "{:indent$}{} {k:>5} | {}\n",
                    "",
                    if k == n { ">" } else { " " },
                    lines[k - 1]
                ));
            }
        }
    }
    out
}

/// When 1-based line `n` of `lines` is a declaration header, the 1-based
/// line of the first statement of the body it opens (the first non-blank
/// line after its `begin`), within 60 lines; `None` for any other line.
fn first_statement(lines: &[String], n: usize) -> Option<usize> {
    let t = lines[n - 1].trim_start().to_ascii_lowercase();
    let header = [
        "trigger ",
        "procedure ",
        "local procedure ",
        "internal procedure ",
    ]
    .iter()
    .any(|p| t.starts_with(p))
        || t.starts_with("action(")
        || t.starts_with("field(");
    if !header {
        return None;
    }
    let end = (n + 60).min(lines.len());
    let begin = (n..end).find(|&i| lines[i].trim().eq_ignore_ascii_case("begin"))?;
    ((begin + 1)..lines.len())
        .find(|&i| !lines[i].trim().is_empty())
        .filter(|&i| !lines[i].trim().to_ascii_lowercase().starts_with("end"))
        .map(|i| i + 1)
}

#[cfg(test)]
mod tests {
    use super::locations;

    #[test]
    fn locations_read_ws_and_guid_dep_forms() {
        let l = "  Insert at ws:src/A.al:12 and (COMMIT at \
                 dep:d0d00001-0000-4000-8000-0000000000b1:src/B.al:46) ws:src/C.al:3:9";
        assert_eq!(
            locations(l),
            vec![
                ("ws:src/A.al".to_string(), 12),
                (
                    "dep:d0d00001-0000-4000-8000-0000000000b1:src/B.al".to_string(),
                    46
                ),
                ("ws:src/C.al".to_string(), 3),
            ]
        );
        assert!(locations("news:x:1 at dep:Base:x").is_empty());
    }
}
