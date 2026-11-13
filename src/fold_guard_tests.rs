//! The one-fold guard (TLS-43). AL identifiers compare and key with one fold,
//! `al_syntax::fold_identifier` / `eq_fold_identifier`. Three review rounds of TLS-43
//! each found a new seam where one side of a compare used another fold
//! (`to_ascii_lowercase`, `eq_ignore_ascii_case`, `to_lowercase`), so this test
//! counts those calls per file in `src/engine` and `src/program` against
//! `tests/fold-allowlist.txt`, which says which kinds of site may keep them.
//!
//! Discrimination (2026-11-13): one added `.to_lowercase()` in
//! `src/engine/l5/detectors/d19.rs` fails it, naming the file and both counts;
//! removed, it passes.

use std::collections::BTreeMap;
use std::path::Path;

const CALLS: [&str; 3] = [
    ".to_lowercase()",
    ".to_ascii_lowercase()",
    "eq_ignore_ascii_case(",
];

fn count_in(dir: &Path, root: &Path, out: &mut BTreeMap<String, usize>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .flatten()
        .map(|e| e.path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            count_in(&p, root, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            let text = std::fs::read_to_string(&p).expect("a source file");
            let n: usize = CALLS.iter().map(|c| text.matches(c).count()).sum();
            if n > 0 {
                let rel = p.strip_prefix(root).expect("under the root");
                out.insert(rel.to_string_lossy().replace('\\', "/"), n);
            }
        }
    }
}

#[test]
fn identifier_folds_stay_on_the_allowlist() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut found = BTreeMap::new();
    for d in ["src/engine", "src/program"] {
        count_in(&root.join(d), root, &mut found);
    }
    let list = std::fs::read_to_string(root.join("tests/fold-allowlist.txt"))
        .expect("tests/fold-allowlist.txt");
    let allowed: BTreeMap<String, usize> = list
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            let (n, p) = l.split_once(' ').expect("`<count> <path>`");
            (p.trim().to_string(), n.parse().expect("a count"))
        })
        .collect();
    let mut diffs = Vec::new();
    for (path, &n) in &found {
        match allowed.get(path) {
            Some(&a) if a == n => {}
            Some(&a) => diffs.push(format!("{path}: {n} calls, allowlist says {a}")),
            None => diffs.push(format!("{path}: {n} calls, not on the allowlist")),
        }
    }
    for (path, &a) in &allowed {
        if !found.contains_key(path) {
            diffs.push(format!("{path}: 0 calls, allowlist says {a}"));
        }
    }
    assert!(
        diffs.is_empty(),
        "case-fold calls differ from tests/fold-allowlist.txt (read its header: an AL \
         identifier uses fold_identifier / eq_fold_identifier):\n{}",
        diffs.join("\n")
    );
}
