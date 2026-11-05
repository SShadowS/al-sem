//! The MCP-output fixture (plan `docs/superpowers/plans/2026-11-04-mcp-output-fixes.md`,
//! wave 0, item 0b) and its golden family `tests/cli-output-goldens/`.
//!
//! Every explain/query golden before this one was single-app, so the shapes an agent
//! meets on a real workspace never appeared in a golden: dependency boundary groups
//! (they exist only in a cross-app model whose dependency ships bodies), temporary
//! and physical writes on one table, `RecordRef` loops, gaps. The fixture at
//! `tests/fixtures/mcp-output/` produces each of them:
//!
//! - `Out Issue Document::IssueDoc` runs the dependency codeunit `Base Poster`, whose
//!   `OnRun` deletes `Base Doc Line` and whose callee `PostDocument` (not the entry
//!   routine) inserts into twelve tables, modifies `Base Register`, raises the
//!   dependency event `OnAfterPostDocument` and commits: one boundary group per
//!   effect type, with a commit and more than ten resources beyond the first hop;
//! - it also runs `Base Calc`, a second dependency `OnRun`, so two boundaries share a
//!   routine name;
//! - `Out Log Mgt::WriteLog` deletes a TEMPORARY `Out Log` and inserts a physical one;
//! - a `RecordRef.Next()` loop over a table decided at run time, a dynamic
//!   `Codeunit.Run(<variable>)` and an interface call (gaps, prove obligations);
//! - an HTTP call, a background task, an own event with a subscriber, a page action
//!   caller and one `[Test]` procedure that reaches `WriteLog`.
//!
//! The dependency `.app` under `ws/.alpackages/` is built from `dep/` by
//! `tests/common/symbol_app.rs::source_app_bytes`; [`fixture_app_matches_its_sources`]
//! checks the committed archive holds exactly those sources (and rewrites it under
//! `REGEN_TEMP_GOLDENS=1`).
//!
//! Goldens: `tests/cli-output-goldens/<slug>.{json,human.txt}`, Rust-owned; refresh
//! with `scripts/check-goldens --regen` and READ the diff.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::regen;

pub(crate) const DEP_GUID: &str = "d0d00001-0000-4000-8000-0000000000b1";
const DEP_APP_FILE: &str = "probe_Out Base_1.0.0.0.app";

pub(crate) fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mcp-output")
}

/// The workspace directory (`app.json`, `src/`, `.alpackages/`).
pub(crate) fn workspace() -> PathBuf {
    fixture_root().join("ws")
}

fn goldens_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/cli-output-goldens")
}

/// The dependency's `(zip entry, text)` pairs, read from `dep/src/`, sorted.
fn dep_sources() -> Vec<(String, String)> {
    let src = fixture_root().join("dep/src");
    let mut out: Vec<(String, String)> = std::fs::read_dir(&src)
        .unwrap_or_else(|e| panic!("read {}: {e}", src.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "al"))
        .map(|p| {
            let name = format!("src/{}", p.file_name().unwrap().to_string_lossy());
            let text = std::fs::read_to_string(&p).unwrap().replace('\r', "");
            (name, text)
        })
        .collect();
    out.sort();
    out
}

fn dep_symbols() -> String {
    std::fs::read_to_string(fixture_root().join("dep/SymbolReference.json"))
        .unwrap()
        .replace('\r', "")
}

/// The dependency `.app`'s bytes, built from `dep/` (a pure function of those files:
/// the zip carries a fixed timestamp).
fn dep_app_bytes() -> Vec<u8> {
    let sources = dep_sources();
    let pairs: Vec<(&str, &str)> = sources
        .iter()
        .map(|(n, t)| (n.as_str(), t.as_str()))
        .collect();
    crate::symbol_app::source_app_bytes(DEP_GUID, "Out Base", "1.0.0.0", &dep_symbols(), &pairs, "")
}

/// Every entry of a `.app` (40-byte NAVX header, then a zip), sorted by name.
fn app_entries(path: &Path) -> Vec<(String, String)> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    app_bytes_entries(&bytes)
}

fn app_bytes_entries(bytes: &[u8]) -> Vec<(String, String)> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&bytes[40..])).unwrap();
    let mut out = Vec::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).unwrap();
        let mut text = String::new();
        f.read_to_string(&mut text).unwrap();
        out.push((f.name().unwrap().to_string(), text));
    }
    out.sort();
    out
}

/// The committed dependency `.app` holds exactly the files under `dep/` (a fresh build
/// from them has the same entries); `REGEN_TEMP_GOLDENS=1` rewrites it.
///
/// Every other test of this fixture reads that `.app` WHILE this one runs, so the
/// regen goes through [`regen::write_golden`]: no write when the bytes are equal, and
/// otherwise a temp file renamed over it, never a truncate-and-fill in place (which
/// once let a concurrent `explain` load the dependency as symbol-only and mint a
/// wrong golden).
#[test]
fn fixture_app_matches_its_sources() {
    let committed = workspace().join(".alpackages").join(DEP_APP_FILE);
    if regen::regen_mode() {
        regen::write_golden(&committed, dep_app_bytes())
            .unwrap_or_else(|e| panic!("regen write {}: {e}", committed.display()));
        return;
    }
    assert_eq!(
        app_entries(&committed),
        app_bytes_entries(&dep_app_bytes()),
        "{} is stale: run REGEN_TEMP_GOLDENS=1 cargo test --test cli output_fixture::",
        committed.display()
    );
}

/// `alsem <args...>`, plus `--deterministic` for the commands that take it (every one
/// but `explain`, whose output has no timestamp): `(stdout, stderr, exit code)`.
pub(crate) fn run_alsem(args: &[&str]) -> (String, String, i32) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_alsem"));
    cmd.args(args);
    if args.first() != Some(&"explain") {
        cmd.arg("--deterministic");
    }
    let out = cmd
        .env("RUST_LOG", "off")
        .env("ALCH_DRIVER_VERSION_OVERRIDE", "cli-output-v1")
        .output()
        .expect("spawn alsem");
    (
        String::from_utf8_lossy(&out.stdout).replace('\r', ""),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

/// One golden: a slug and the argv after `alsem` (the workspace is `{ws}`).
pub(crate) struct Case {
    pub slug: &'static str,
    pub argv: Vec<String>,
    pub exit_code: i32,
}

fn case(slug: &'static str, argv: &[&str], exit_code: i32) -> Case {
    let ws = workspace().to_string_lossy().into_owned();
    Case {
        slug,
        argv: argv
            .iter()
            .map(|a| {
                if *a == "{ws}" {
                    ws.clone()
                } else {
                    a.to_string()
                }
            })
            .collect(),
        exit_code,
    }
}

/// The fixture's goldens keep the spellings they were minted with:
/// `<owner>::<routine>` for `explain`, `prove` and `digest`, `Object.Routine`
/// for `query`. Since wave 3's item 3b every tool takes both
/// (`output_invariants_w3::one_selector_grammar_for_every_tool`).
pub(crate) const ISSUE_DOC: &str = "Out Issue Document::IssueDoc";
pub(crate) const WRITE_LOG: &str = "Out Log Mgt::WriteLog";
pub(crate) const ISSUE_DOC_Q: &str = "Out Issue Document.IssueDoc";

/// Every table the fixture's routines write, physical or temporary, workspace or
/// dependency: `prove writes-table` runs for each.
pub(crate) const TABLES: &[&str] = &[
    "Out Doc",
    "Out Log",
    "Base Doc Line",
    "Base Ledger 01",
    "Base Ledger 12",
    "Base Register",
    "Base Price",
];

/// The golden cases. Each runs in both `--format json` and `--format human`.
pub(crate) fn cases() -> Vec<Case> {
    let mut v = vec![
        case(
            "explain.issuedoc",
            &["explain", "{ws}", "--routine", ISSUE_DOC],
            0,
        ),
        case(
            "explain.writelog",
            &["explain", "{ws}", "--routine", WRITE_LOG],
            0,
        ),
        case(
            "prove.issuedoc.may-commit",
            &["prove", "{ws}", ISSUE_DOC, "may-commit"],
            0,
        ),
        case(
            "prove.writelog.may-commit",
            &["prove", "{ws}", WRITE_LOG, "may-commit"],
            0,
        ),
        case(
            "digest.issuedoc",
            &["digest", "{ws}", "--routine", ISSUE_DOC],
            0,
        ),
        case(
            "digest.issuedoc.detail",
            &[
                "digest",
                "{ws}",
                "--routine",
                ISSUE_DOC,
                "--include-reads",
                "--tests",
            ],
            0,
        ),
        case(
            "digest.writelog",
            &["digest", "{ws}", "--routine", WRITE_LOG],
            0,
        ),
        case(
            "query.issuedoc.effects",
            &["query", "effects", "{ws}", "--routine", ISSUE_DOC_Q],
            0,
        ),
        case(
            "query.issuedoc.out-log.touches",
            &[
                "query",
                "touches",
                "{ws}",
                "--table",
                "Out Log",
                "--from",
                ISSUE_DOC_Q,
            ],
            0,
        ),
        case(
            "query.issuedoc.base-register.touches",
            &[
                "query",
                "touches",
                "{ws}",
                "--table",
                "Base Register",
                "--from",
                ISSUE_DOC_Q,
            ],
            0,
        ),
    ];
    for t in TABLES {
        let slug: &'static str = Box::leak(
            format!(
                "prove.issuedoc.writes-table-{}",
                t.to_lowercase().replace(' ', "-")
            )
            .into_boxed_str(),
        );
        let q: &'static str = Box::leak(format!("writes-table:{t}").into_boxed_str());
        v.push(case(slug, &["prove", "{ws}", ISSUE_DOC, q], 0));
    }
    v
}

fn check_case(c: &Case) -> Option<String> {
    let mut failures = String::new();
    for (format, ext) in [("json", "json"), ("human", "human.txt")] {
        let mut args: Vec<&str> = c.argv.iter().map(String::as_str).collect();
        args.extend(["--format", format]);
        let (got, stderr, code) = run_alsem(&args);
        if code != c.exit_code {
            failures.push_str(&format!(
                "[{}/{format}] exit {code}, want {} — stderr:\n{stderr}\n",
                c.slug, c.exit_code
            ));
            continue;
        }
        let golden = goldens_dir().join(format!("{}.{ext}", c.slug));
        if regen::regen_mode() {
            std::fs::create_dir_all(goldens_dir()).unwrap();
            regen::write_golden(&golden, &got).unwrap();
            continue;
        }
        match std::fs::read_to_string(&golden) {
            Ok(want) if want.replace('\r', "") == got => {}
            Ok(_) => failures.push_str(&format!(
                "[{}/{format}] diverged from {}\n",
                c.slug,
                golden.display()
            )),
            Err(e) => failures.push_str(&format!(
                "[{}/{format}] read {}: {e} — run scripts/check-goldens --regen\n",
                c.slug,
                golden.display()
            )),
        }
    }
    (!failures.is_empty()).then_some(failures)
}

/// Every case, both formats, byte-compared with its golden.
#[test]
fn output_fixture_goldens() {
    let failures: Vec<String> = cases().iter().filter_map(check_case).collect();
    assert!(failures.is_empty(), "{}", failures.concat());
}
