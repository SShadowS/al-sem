//! Writes into the repository tree during a test run must be atomic.
//!
//! `cargo-nextest` runs every test in its own process, all at once. Under
//! `REGEN_TEMP_GOLDENS=1` some tests rewrite committed goldens and fixtures while
//! other tests read them. `std::fs::write` truncates a file and then fills it, so a
//! reader can see it empty or half written. It happened: `output_fixture`'s regen
//! rewrote the MCP-output dependency `.app` in place, a concurrent `explain` loaded
//! the dependency as symbol-only, and a wrong `explain.issuedoc.json` golden was
//! written. The one allowed writer is `regen::write_golden` (no write when the bytes
//! are equal; otherwise a temp file in the same directory, renamed over the target).
//!
//! Two guards:
//! - [`write_golden_never_exposes_a_partial_file`] and
//!   [`write_golden_skips_equal_bytes`] pin the helper itself;
//! - [`regen_paths_write_only_through_write_golden`] pins the USE: it scans every
//!   test source and fails when a function that consults the regen switch writes a
//!   file any other way, directly or through a same-file helper.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::regen;

/// A reader that loops while a writer swaps a file between two contents of
/// different lengths must only ever see one of them, whole. With an in-place
/// `std::fs::write` the reader sees a truncated or empty file within a few swaps.
#[test]
fn write_golden_never_exposes_a_partial_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dep.app");
    let a: Arc<Vec<u8>> = Arc::new(vec![b'a'; 4 << 20]);
    let b: Arc<Vec<u8>> = Arc::new(vec![b'b'; 3 << 20]);
    regen::write_golden(&path, a.as_slice()).unwrap();

    let done = Arc::new(AtomicBool::new(false));
    let reads = Arc::new(AtomicUsize::new(0));
    let reader = {
        let (path, a, b, done, reads) = (
            path.clone(),
            Arc::clone(&a),
            Arc::clone(&b),
            Arc::clone(&done),
            Arc::clone(&reads),
        );
        std::thread::spawn(move || -> Result<(), String> {
            while !done.load(Ordering::Relaxed) {
                let got = std::fs::read(&path).map_err(|e| format!("read failed: {e}"))?;
                if got != *a && got != *b {
                    return Err(format!(
                        "a reader saw a partial file: {} bytes (whole files are {} and {})",
                        got.len(),
                        a.len(),
                        b.len()
                    ));
                }
                reads.fetch_add(1, Ordering::Relaxed);
            }
            Ok(())
        })
    };
    for i in 0..200 {
        let next = if i % 2 == 0 { &b } else { &a };
        regen::write_golden(&path, next.as_slice()).unwrap();
    }
    done.store(true, Ordering::Relaxed);
    let outcome = reader.join().expect("reader thread");
    assert!(outcome.is_ok(), "{}", outcome.unwrap_err());
    assert!(
        reads.load(Ordering::Relaxed) > 0,
        "precondition: the reader read while the writer wrote"
    );
}

/// A regen that changes nothing writes nothing: the file keeps its modification
/// time, and no temp file is left behind.
#[test]
fn write_golden_skips_equal_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("golden.json");
    std::fs::write(&path, b"{}\n").unwrap();
    let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(old)
        .unwrap();
    regen::write_golden(&path, b"{}\n").unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        old,
        "equal bytes must not be rewritten"
    );
    regen::write_golden(&path, b"{\"a\": 1}\n").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"{\"a\": 1}\n");
    let names: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["golden.json"], "no temp file is left behind");
}

/// Calls that write a file in place (or clear a directory a reader may be in).
const RAW_WRITES: &[&str] = &[
    "fs::write(",
    "File::create(",
    "fs::copy(",
    "remove_dir_all(",
    "write_source_app(",
    "write_symbol_app(",
    "write_dependency(",
    "write_dependency_lib(",
];

/// What marks a function as one that writes into the tree on request.
const REGEN_SWITCHES: &[&str] = &["regen_mode()", "regen()", "REMINT_", "REGEN_TEMP_GOLDENS"];

/// Every function in every test source that consults the regen switch writes
/// files only through `regen::write_golden`: no raw write in its body, and no call
/// to a same-file function that does one (followed transitively).
#[test]
fn regen_paths_write_only_through_write_golden() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut files = Vec::new();
    collect_rs(&root, &mut files);
    assert!(
        files.len() > 100,
        "precondition: the test sources were found"
    );
    let mut violations = Vec::new();
    let mut regen_fns = 0;
    for file in &files {
        let rel = file
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        if rel.replace('\\', "/") == "common/regen.rs" {
            continue;
        }
        let code = strip_comments_and_strings(&std::fs::read_to_string(file).unwrap());
        // The switch tokens are compared on the stripped code, so a doc comment
        // or a message that names them does not count.
        let raw = std::fs::read_to_string(file).unwrap();
        let fns = functions(&code, &raw);
        let writers = writer_fns(&fns);
        for (name, body) in &fns {
            if !REGEN_SWITCHES.iter().any(|s| body.raw.contains(s)) {
                continue;
            }
            regen_fns += 1;
            for w in RAW_WRITES {
                if body.code.contains(w) {
                    violations.push(format!("{rel}: fn {name} calls {w}…)"));
                }
            }
            for w in &writers {
                if w != name && calls(&body.code, w) {
                    violations.push(format!(
                        "{rel}: fn {name} calls {w}(), which writes in place"
                    ));
                }
            }
        }
    }
    assert!(
        regen_fns >= 40,
        "precondition: the regen functions were found ({regen_fns})"
    );
    assert!(
        violations.is_empty(),
        "a regen path writes a file in place; use regen::write_golden (atomic, \
         skips equal bytes) instead:\n{}",
        violations.join("\n")
    );
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

struct Body {
    /// The body with comments and string literals blanked out.
    code: String,
    /// The same byte range of the original text (string literals kept, so an
    /// env-var name in `std::env::var("…")` is seen).
    raw: String,
}

/// `fn name` → its body (from the first `{` after the name to its matching `}`).
/// Two functions of one name in one file (in different modules) are merged.
fn functions(code: &str, raw: &str) -> BTreeMap<String, Body> {
    let bytes = code.as_bytes();
    let mut out: BTreeMap<String, Body> = BTreeMap::new();
    let mut i = 0;
    while let Some(at) = code[i..].find("fn ") {
        let start = i + at;
        i = start + 3;
        if start > 0 && is_ident(bytes[start - 1]) {
            continue;
        }
        let name: String = code[i..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if name.is_empty() {
            continue;
        }
        // The body opens at the first `{` not inside the signature's parens or
        // generics; a `;` first means a declaration without a body.
        let mut j = i + name.len();
        let mut depth = 0i32;
        let open = loop {
            if j >= bytes.len() {
                break None;
            }
            match bytes[j] {
                b'(' | b'<' | b'[' => depth += 1,
                b')' | b'>' | b']' => depth -= 1,
                b'{' if depth <= 0 => break Some(j),
                b';' if depth <= 0 => break None,
                _ => {}
            }
            j += 1;
        };
        let Some(open) = open else { continue };
        let mut depth = 0i32;
        let mut end = open;
        for (k, &c) in bytes.iter().enumerate().skip(open) {
            match c {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = k;
                        break;
                    }
                }
                _ => {}
            }
        }
        let body = out.entry(name).or_insert(Body {
            code: String::new(),
            raw: String::new(),
        });
        body.code.push_str(&code[open..=end]);
        body.raw.push_str(&raw[open..=end]);
    }
    out
}

/// The functions that write in place: a raw write in the body, or a call to
/// another such function of the same file (to a fixed point).
fn writer_fns(fns: &BTreeMap<String, Body>) -> BTreeSet<String> {
    let mut writers: BTreeSet<String> = fns
        .iter()
        .filter(|(_, b)| RAW_WRITES.iter().any(|w| b.code.contains(w)))
        .map(|(n, _)| n.clone())
        .collect();
    loop {
        let more: Vec<String> = fns
            .iter()
            .filter(|(n, _)| !writers.contains(*n))
            .filter(|(_, b)| writers.iter().any(|w| calls(&b.code, w)))
            .map(|(n, _)| n.clone())
            .collect();
        if more.is_empty() {
            return writers;
        }
        writers.extend(more);
    }
}

/// `code` calls `name(` as a free function (not a method `.name(` nor a path
/// `x::name(`, which names another module's function).
fn calls(code: &str, name: &str) -> bool {
    let needle = format!("{name}(");
    let bytes = code.as_bytes();
    code.match_indices(&needle).any(|(at, _)| {
        at == 0 || {
            let before = bytes[at - 1];
            !is_ident(before) && before != b'.' && before != b':'
        }
    })
}

fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// `src` with comments and the contents of string and char literals replaced by
/// spaces (newlines kept), so braces and tokens in them are not seen. Byte
/// offsets are preserved, so a range of the result is the same range of `src`.
fn strip_comments_and_strings(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = b.to_vec();
    let blank = |out: &mut Vec<u8>, from: usize, to: usize| {
        for c in &mut out[from..to.min(b.len())] {
            if *c != b'\n' {
                *c = b' ';
            }
        }
    };
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                let end = src[i..].find('\n').map_or(b.len(), |e| i + e);
                blank(&mut out, i, end);
                i = end;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let end = src[i + 2..].find("*/").map_or(b.len(), |e| i + 2 + e + 2);
                blank(&mut out, i, end);
                i = end;
            }
            b'r' if (b.get(i + 1) == Some(&b'"') || b.get(i + 1) == Some(&b'#'))
                && (i == 0 || !is_ident(b[i - 1]) || b[i - 1] == b'b') =>
            {
                let hashes = b[i + 1..].iter().take_while(|&&c| c == b'#').count();
                if b.get(i + 1 + hashes) != Some(&b'"') {
                    i += 1;
                    continue;
                }
                let close = format!("\"{}", "#".repeat(hashes));
                let body = i + 2 + hashes;
                let end = src[body..]
                    .find(&close)
                    .map_or(b.len(), |e| body + e + close.len());
                blank(&mut out, body, end - close.len());
                i = end;
            }
            b'"' => {
                let mut j = i + 1;
                while j < b.len() && b[j] != b'"' {
                    j += if b[j] == b'\\' { 2 } else { 1 };
                }
                blank(&mut out, i + 1, j);
                i = j + 1;
            }
            b'\'' => {
                // A char literal (`'x'`, `'\n'`, `'\u{7b}'`); otherwise a lifetime.
                let end = if b.get(i + 1) == Some(&b'\\') {
                    src[i + 2..].find('\'').map(|e| i + 2 + e)
                } else {
                    let len = src[i + 1..].chars().next().map_or(1, char::len_utf8);
                    (b.get(i + 1 + len) == Some(&b'\'')).then_some(i + 1 + len)
                };
                match end {
                    Some(end) => {
                        blank(&mut out, i + 1, end);
                        i = end + 1;
                    }
                    None => i += 1,
                }
            }
            _ => i += 1,
        }
    }
    String::from_utf8(out)
        .expect("blanking keeps UTF-8 (multi-byte chars only inside blanked spans or untouched)")
}
