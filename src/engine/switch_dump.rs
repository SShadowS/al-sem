//! The engine-switch difference harness (S0 of
//! `docs/superpowers/specs/2026-10-06-engine-switch-design.md`).
//!
//! [`dump_lines`] renders what `alsem analyze`'s detectors read and produce for one
//! workspace as named line lists ("files"): the model rows in model order, the
//! call resolution, the detector event graph, every registered detector's findings
//! (opt-in ones too), detector stats, diagnostics and coverage. [`compare`] diffs
//! two such dumps file by file, as ORDERED rows: a multiset difference, plus an
//! order flag when the multisets agree but the order does not.
//!
//! Dumps are written to disk ([`write_dump`] / [`read_dump`]) because the two sides
//! of a comparison are two BINARIES: the previous merged step, and the frozen
//! pre-switch output. A dump from a binary that still had L3 stays comparable
//! after L3 is deleted.
//!
//! **What it measures is production, not a copy.** The model comes from
//! [`build_analysis_model`] and coverage from [`analysis_coverage`] — the functions
//! `run_analyze_with_exit` itself calls — the calls from `calls_for` and the event
//! graph from `build_detector_context`, the functions the detectors read through.
//! Each switch step changes what those build; this module does not change.
//!
//! Rows are `{:?}`. Every model type derives `Debug` and holds no hash map or set,
//! so the text is deterministic (pinned by a double-dump test); the one hash map
//! here, `upgraded_bindings`, is emitted in key order.

use std::collections::BTreeMap;
use std::path::Path;

use crate::engine::gate::run::{analysis_coverage, build_analysis_model};
use crate::engine::gate::workspace_diagnostics::compute_workspace_diagnostics;
use crate::engine::l3::call_resolver::calls_for;
use crate::engine::l3::symbol_table::SymbolTable;
use crate::engine::l5::detector_context::build_detector_context;
use crate::engine::l5::detectors::registered_detectors;
use crate::engine::l5::registry::run_detectors;

/// One dump: file name → rows.
pub type Dump = BTreeMap<String, Vec<String>>;

fn rows<T: std::fmt::Debug>(items: impl IntoIterator<Item = T>) -> Vec<String> {
    items.into_iter().map(|t| format!("{t:?}")).collect()
}

/// Render what `alsem analyze` builds and finds for `ws` (see the module doc).
pub fn dump_lines(ws: &Path) -> Dump {
    let mut d = Dump::new();
    d.insert(
        "diagnostics.workspace".into(),
        rows(compute_workspace_diagnostics(ws)),
    );
    let built = build_analysis_model(ws);
    d.insert(
        "status".into(),
        vec![
            format!("fresh={:?}", built.fresh.as_ref().map(|_| "ok")),
            format!("model={:?}", built.model.as_ref().map(|_| "ok")),
        ],
    );
    let Ok(resolved) = built.model else {
        return d;
    };
    let ws_model = &resolved.workspace;
    d.insert("model.objects".into(), rows(&ws_model.objects));
    d.insert("model.tables".into(), rows(&ws_model.tables));
    d.insert("model.routines".into(), rows(&ws_model.routines));
    d.insert(
        "model.root_classifications".into(),
        rows(&resolved.root_classifications),
    );
    d.insert(
        "model.primary_app".into(),
        rows(resolved.primary_app.iter()),
    );
    d.insert(
        "model.infra_diagnostics".into(),
        rows(&resolved.infra_diagnostics),
    );

    {
        let symbols = SymbolTable::build(&ws_model.objects, &ws_model.tables, &ws_model.routines);
        let calls = calls_for(&resolved, &symbols);
        d.insert("calls.edges".into(), rows(&calls.edges));
        let bindings: BTreeMap<_, _> = calls.upgraded_bindings.iter().collect();
        d.insert(
            "calls.upgraded_bindings".into(),
            bindings
                .iter()
                .map(|(k, v)| format!("{k}\t{v:?}"))
                .collect(),
        );
        d.insert("calls.diagnostics".into(), rows(&calls.diagnostics));
    }
    {
        // demanded = 0: the event graph is built unconditionally; no substrate.
        let ctx = build_detector_context(&resolved, 0);
        d.insert("events.symbols".into(), rows(&ctx.event_graph.events));
        d.insert("events.edges".into(), rows(&ctx.event_graph.edges));
    }

    let run = run_detectors(&resolved, &registered_detectors());
    d.insert("findings".into(), rows(&run.findings));
    d.insert("detector_stats".into(), rows(&run.detector_stats));
    d.insert("diagnostics.detect".into(), rows(&run.diagnostics));
    d.insert(
        "diagnostics.summarize".into(),
        rows(&run.summarize_diagnostics),
    );

    let coverage = analysis_coverage(&resolved, ws, &built.fresh);
    d.insert(
        "coverage".into(),
        vec![serde_json::to_string(&coverage).expect("coverage serializes")],
    );
    d
}

/// Write a dump as one `<name>.txt` per file under `dir` (created).
pub fn write_dump(dump: &Dump, dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for (name, lines) in dump {
        // No rows → empty file (a lone "\n" would read back as one empty row).
        let mut text = lines.join("\n");
        if !lines.is_empty() {
            text.push('\n');
        }
        std::fs::write(dir.join(format!("{name}.txt")), text)?;
    }
    Ok(())
}

/// Read back every `<name>.txt` under `dir`.
pub fn read_dump(dir: &Path) -> std::io::Result<Dump> {
    let mut d = Dump::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let Some(name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".txt"))
        else {
            continue;
        };
        let text = std::fs::read_to_string(&path)?;
        d.insert(name.to_string(), text.lines().map(str::to_string).collect());
    }
    Ok(d)
}

/// How one file differs between dump A and dump B.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub name: String,
    /// `None` when the file is absent on that side.
    pub a_rows: Option<usize>,
    pub b_rows: Option<usize>,
    /// Rows in A beyond their count in B (multiset difference), in A order.
    pub only_a: Vec<String>,
    /// Rows in B beyond their count in A, in B order.
    pub only_b: Vec<String>,
    /// Same multiset, different order.
    pub reordered: bool,
}

/// Every file that differs; empty when the dumps are identical.
pub fn compare(a: &Dump, b: &Dump) -> Vec<FileDiff> {
    let names: std::collections::BTreeSet<&String> = a.keys().chain(b.keys()).collect();
    let mut out = Vec::new();
    for name in names {
        let (ra, rb) = (a.get(name), b.get(name));
        if ra == rb {
            continue;
        }
        let empty = Vec::new();
        let (la, lb) = (ra.unwrap_or(&empty), rb.unwrap_or(&empty));
        let only_a = multiset_minus(la, lb);
        let only_b = multiset_minus(lb, la);
        let reordered = only_a.is_empty() && only_b.is_empty();
        out.push(FileDiff {
            name: name.clone(),
            a_rows: ra.map(Vec::len),
            b_rows: rb.map(Vec::len),
            only_a,
            only_b,
            reordered,
        });
    }
    out
}

/// Rows of `x` not matched by an equal row of `y`, counting duplicates, in `x` order.
fn multiset_minus(x: &[String], y: &[String]) -> Vec<String> {
    let mut avail: BTreeMap<&str, usize> = BTreeMap::new();
    for r in y {
        *avail.entry(r.as_str()).or_default() += 1;
    }
    x.iter()
        .filter(|r| match avail.get_mut(r.as_str()) {
            Some(n) if *n > 0 => {
                *n -= 1;
                false
            }
            _ => true,
        })
        .cloned()
        .collect()
}

/// Human-readable report: one block per differing file, `sample` rows per side,
/// each row cut to `width` characters.
pub fn render(diffs: &[FileDiff], sample: usize, width: usize) -> String {
    if diffs.is_empty() {
        return "identical\n".to_string();
    }
    let cut = |s: &str| -> String {
        if s.chars().count() > width {
            format!("{}…", s.chars().take(width).collect::<String>())
        } else {
            s.to_string()
        }
    };
    let mut s = String::new();
    for d in diffs {
        s.push_str(&format!(
            "## {}  A={:?} B={:?}  only-A={} only-B={}{}\n",
            d.name,
            d.a_rows,
            d.b_rows,
            d.only_a.len(),
            d.only_b.len(),
            if d.reordered { "  REORDERED" } else { "" }
        ));
        for r in d.only_a.iter().take(sample) {
            s.push_str(&format!("  - {}\n", cut(r)));
        }
        for r in d.only_b.iter().take(sample) {
            s.push_str(&format!("  + {}\n", cut(r)));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dump(pairs: &[(&str, &[&str])]) -> Dump {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.iter().map(|s| s.to_string()).collect()))
            .collect()
    }

    #[test]
    fn identical_dumps_compare_empty() {
        let a = dump(&[("f", &["1", "2"])]);
        assert!(compare(&a, &a.clone()).is_empty());
        assert_eq!(render(&[], 3, 80), "identical\n");
    }

    #[test]
    fn a_changed_row_is_reported_on_both_sides() {
        let a = dump(&[("f", &["1", "2", "3"])]);
        let b = dump(&[("f", &["1", "X", "3"])]);
        let d = compare(&a, &b);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].only_a, vec!["2"]);
        assert_eq!(d[0].only_b, vec!["X"]);
        assert!(!d[0].reordered);
    }

    #[test]
    fn duplicates_count_and_order_is_observed() {
        // A lost duplicate is a difference even though the SET is unchanged.
        let a = dump(&[("f", &["1", "1"])]);
        let b = dump(&[("f", &["1"])]);
        assert_eq!(compare(&a, &b)[0].only_a, vec!["1"]);
        // Same rows, new order: flagged, nothing on either side.
        let c = dump(&[("f", &["2", "1"])]);
        let e = dump(&[("f", &["1", "2"])]);
        let d = compare(&c, &e);
        assert!(d[0].reordered && d[0].only_a.is_empty() && d[0].only_b.is_empty());
    }

    #[test]
    fn a_file_missing_on_one_side_is_a_difference() {
        let a = dump(&[("f", &["1"]), ("g", &[])]);
        let b = dump(&[("f", &["1"])]);
        let d = compare(&a, &b);
        assert_eq!((d[0].name.as_str(), d[0].b_rows), ("g", None));
    }

    fn corpus(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("r0-corpus")
            .join(name)
    }

    /// A real dump is populated and deterministic: two dumps of one workspace are
    /// identical. A hash-ordered field anywhere in a dumped type fails this.
    #[test]
    fn real_dump_is_populated_and_deterministic() {
        let ws = corpus("ws-txn-d47-event-pos");
        let a = dump_lines(&ws);
        for file in [
            "model.routines",
            "model.objects",
            "events.symbols",
            "events.edges",
            "findings",
        ] {
            assert!(
                a.get(file).is_some_and(|r| !r.is_empty()),
                "{file} is empty: the dump no longer reaches it"
            );
        }
        assert_eq!(a["status"], vec!["fresh=Ok(\"ok\")", "model=Ok(\"ok\")"]);
        let b = dump_lines(&ws);
        assert!(
            compare(&a, &b).is_empty(),
            "{}",
            render(&compare(&a, &b), 3, 300)
        );
    }

    /// The harness must measure production: `run_analyze_with_exit` builds its model
    /// and coverage ONLY through the two functions the dump calls. Inlining either
    /// back into the analyze path (or building a second model there) fails this.
    #[test]
    fn analyze_builds_through_the_dumped_functions() {
        let src = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("src/engine/gate/run.rs"),
        )
        .unwrap();
        let start = src.find("pub fn run_analyze_with_exit(").unwrap();
        let body = &src[start..];
        let body = &body[..body.find("\n}\n").unwrap()];
        assert_eq!(body.matches("build_analysis_model(").count(), 1);
        assert_eq!(body.matches("analysis_coverage(").count(), 1);
        for banned in [
            "assemble_and_resolve_workspace",
            "attach_program_calls",
            "build_program_with_coverage",
            "project_coverage_disk",
        ] {
            assert!(
                !body.contains(banned),
                "run_analyze_with_exit calls {banned} directly; build through \
                 build_analysis_model / analysis_coverage so the switch harness sees it"
            );
        }
    }

    /// S2a: the analyze model is projected from the program engine's parse, and the
    /// file set is L3's app-scoped one. Hand-stated precondition: a root app with a
    /// nested app inside — the program walks the nested file, L3 must not. Fails if
    /// selection falls back to disk (`Missing`), takes the nested file, or drops the
    /// root one.
    #[test]
    fn analyze_model_selects_app_scoped_files_from_the_program_parse() {
        use crate::engine::l3::l3_workspace::{ProgramFiles, select_program_files};
        let root = std::env::temp_dir().join(format!("switch-s2a-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let app = |dir: &Path, id: &str| {
            std::fs::create_dir_all(dir.join("src")).unwrap();
            std::fs::write(
                dir.join("app.json"),
                format!(
                    r#"{{"id":"{id}","name":"N","publisher":"P","version":"1.0.0.0","dependencies":[]}}"#
                ),
            )
            .unwrap();
        };
        app(&root, "11111111-0000-0000-0000-00000000a2a0");
        app(&root.join("nested"), "11111111-0000-0000-0000-00000000a2a1");
        std::fs::write(
            root.join("src/Root.al"),
            "codeunit 50100 Root { procedure A() begin end; }",
        )
        .unwrap();
        std::fs::write(
            root.join("nested/src/Inner.al"),
            "codeunit 50101 Inner { procedure B() begin end; }",
        )
        .unwrap();

        let (ctx, _report, _) =
            crate::program::resolve::full::build_program_with_coverage(&root).unwrap();
        let program_paths: Vec<&str> = ctx
            .parsed()
            .iter()
            .flat_map(|u| u.files.iter().map(|f| f.virtual_path.as_str()))
            .collect();
        assert!(
            program_paths.contains(&"nested/src/Inner.al"),
            "precondition: the program parse walks the nested app, got {program_paths:?}"
        );
        let selected: Vec<String> = match select_program_files(&root, &ctx) {
            Some(ProgramFiles::Selected { files, .. }) => {
                files.iter().map(|(p, _)| p.to_string()).collect()
            }
            Some(ProgramFiles::Missing(p)) => panic!("fell back to disk: {p} missing"),
            None => panic!("failed closed"),
        };
        std::fs::remove_dir_all(&root).ok();
        assert_eq!(selected, vec!["src/Root.al".to_string()]);
    }

    /// `build_analysis_model` must build through the program-backed entry (S2a);
    /// the disk entry may appear only on its failure path.
    #[test]
    fn analysis_model_uses_the_program_parse() {
        let src = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("src/engine/gate/run.rs"),
        )
        .unwrap();
        let start = src.find("pub fn build_analysis_model(").unwrap();
        let body = &src[start..];
        let body = &body[..body.find("\n}\n").unwrap()];
        assert_eq!(
            body.matches("assemble_and_resolve_workspace_from_program(")
                .count(),
            1
        );
        assert!(!body.contains("assemble_and_resolve_workspace("));
    }

    #[test]
    fn write_then_read_round_trips() {
        let dir = std::env::temp_dir().join(format!("switch-dump-rt-{}", std::process::id()));
        let a = dump(&[("f", &["a\tb", ""]), ("g", &["x"]), ("h", &[])]);
        write_dump(&a, &dir).unwrap();
        let b = read_dump(&dir).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        assert!(compare(&a, &b).is_empty(), "{:?}", compare(&a, &b));
    }
}
