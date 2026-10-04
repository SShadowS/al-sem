//! `textDocument/codeLens` on the engine-backed `LspSnapshot` (T3 Task 12) —
//! the replacement for `src/handlers.rs`'s `code_lens`, cut over at Task 15.
//!
//! One lens per declaration in the requested file (procedures, triggers, and
//! anything else `decls_by_file` carries — legacy's `get_definitions_in_file`
//! was likewise unfiltered by kind, see `src/graph.rs`'s `get_definitions_in_file`),
//! showing a reference count plus complexity/line-count/parameter-count
//! threshold indicators. Complexity and parameter count are read from the
//! SAME owned-IR walker the `--analyze` CLI path uses
//! ([`crate::analysis::routine_complexity_ir`]) — never re-implemented here.
//!
//! Reference counts use [`effective_incoming_count`], the ONE place this
//! arc generalizes legacy's `CallGraph::get_incoming_call_count` (direct
//! calls + event-subscription count) onto the engine's edge model; see that
//! function's doc and `diagnostics.rs`'s module doc for the full reasoning
//! (both modules share this helper so a routine's "used" evidence is
//! identical whether shown as a codeLens reference count or gating the
//! unused-procedure diagnostic).

use al_syntax::ir::{AlFile, RoutineDecl, RoutineKind};
use lsp_types::{CodeLens, Command};

use crate::config::DiagnosticConfig;
use crate::lsp::encoding::PositionEncoding;
use crate::lsp::handlers::{object_name_for, origin_to_range, resolve_virtual_path};
use crate::lsp::snapshot::LspSnapshot;
use crate::program::RoutineNodeId;

/// `textDocument/codeLens`. Returns an empty `Vec` for an unparsable/
/// non-workspace `uri` (fail-closed, mirrors [`crate::lsp::handlers::prepare`]'s
/// `resolve_virtual_path` use) or a file with no declarations.
#[must_use]
pub fn code_lenses(
    snap: &LspSnapshot,
    enc: PositionEncoding,
    uri: &str,
    cfg: &DiagnosticConfig,
) -> Vec<CodeLens> {
    let Some(virtual_path) = resolve_virtual_path(snap, uri) else {
        return Vec::new();
    };
    let Some(entry) = snap.parsed.get(&virtual_path) else {
        return Vec::new();
    };
    let Some(decls) = snap.decls_by_file.get(&virtual_path) else {
        return Vec::new();
    };
    let table = entry.line_table();

    let mut out = Vec::with_capacity(decls.len());
    for decl in decls.iter() {
        let Some(routine) = find_routine_by_origin(&entry.file, decl.origin.byte.start) else {
            // Structurally shouldn't happen — every `DeclEntry` is built FROM
            // one of `entry.file`'s routines (see `recompute_file`) — but
            // fail closed by skipping rather than guessing at metrics.
            continue;
        };
        let complexity = crate::analysis::routine_complexity_ir(&entry.file.ir, routine);
        let parameter_count = parameter_count_of(routine);
        let line_count = decl.origin.end.row.saturating_sub(decl.origin.start.row) + 1;
        let ref_count = effective_incoming_count(snap, &decl.id);

        let title = format_lens_title(ref_count, complexity, line_count, parameter_count, cfg);
        let object_name = object_name_for(&snap.graph, &decl.id.object).unwrap_or("Unknown");

        out.push(CodeLens {
            range: origin_to_range(&decl.origin, table, enc),
            command: Some(Command {
                title,
                command: "al-call-hierarchy.showReferences".to_string(),
                arguments: Some(vec![serde_json::json!({
                    "object": object_name,
                    "procedure": decl.name,
                    "uri": uri,
                })]),
            }),
            data: None,
        });
    }
    out
}

/// Legacy title format, byte-for-byte (`src/handlers.rs`'s `code_lens`):
/// `"{ref_text} | {complexity_text}, {lines_text}, {params_text}"`.
fn format_lens_title(
    ref_count: usize,
    complexity: u32,
    line_count: u32,
    parameter_count: u32,
    cfg: &DiagnosticConfig,
) -> String {
    let ref_text = match ref_count {
        0 => "0 references".to_string(),
        1 => "1 reference".to_string(),
        n => format!("{n} references"),
    };

    let complexity_text = if complexity >= cfg.complexity_critical {
        format!(
            "complexity: {complexity} \u{26a0}\u{fe0f} (>{})",
            cfg.complexity_critical
        )
    } else if complexity >= cfg.complexity_warning {
        format!("complexity: {complexity} (>{})", cfg.complexity_warning)
    } else {
        format!("complexity: {complexity}")
    };

    let lines_text = if line_count > cfg.length_critical {
        format!(
            "lines: {line_count} \u{26a0}\u{fe0f} (>{})",
            cfg.length_critical
        )
    } else {
        format!("lines: {line_count}")
    };

    let params_text = if parameter_count >= cfg.params_critical {
        format!(
            "params: {parameter_count} \u{26a0}\u{fe0f} (>{})",
            cfg.params_critical
        )
    } else if parameter_count >= cfg.params_warning {
        format!("params: {parameter_count} (>{})", cfg.params_warning)
    } else {
        format!("params: {parameter_count}")
    };

    format!("{ref_text} | {complexity_text}, {lines_text}, {params_text}")
}

/// Legacy hardcoded 0 parameters for triggers (a T3-deleted `parser.rs`
/// behavior, kept for continuity) — mirrored here from the raw `RoutineDecl`.
pub(crate) fn parameter_count_of(routine: &RoutineDecl) -> u32 {
    match routine.kind {
        RoutineKind::Trigger => 0,
        RoutineKind::Procedure => routine.params.len() as u32,
    }
}

/// Find the `RoutineDecl` a `DeclEntry` was built from, by matching its whole-
/// declaration span's start byte offset — `DeclEntry.origin` is copied
/// byte-for-byte from `RoutineDecl.origin` in `recompute_file`, and two
/// routines in the same file can never share a span start, so this is an
/// exact, unambiguous correlation. Shared with `diagnostics.rs`, which needs
/// the same routine (for its `kind`/`attributes`) to apply the unused-
/// procedure exclusion rules.
pub(crate) fn find_routine_by_origin(
    file: &AlFile,
    origin_byte_start: usize,
) -> Option<&RoutineDecl> {
    file.objects
        .iter()
        .flat_map(|o| o.routines.iter())
        .find(|r| r.origin.byte.start == origin_byte_start)
}

/// Generalizes legacy's `CallGraph::get_incoming_call_count` (direct calls +
/// event-subscription count, `src/graph.rs:865-886`) onto the engine's edge
/// model:
///
/// - **direct**: `snap.incoming[id]` — every `Call`/`Run`/`ImplicitTrigger`
///   edge targeting `id`, PLUS every `EventFlow` edge targeting `id` (i.e.
///   `id` is a SUBSCRIBER of some publisher — see `LspSnapshot::incoming`'s
///   doc). This is the mechanism that makes an `[EventSubscriber]` routine
///   "used" without any attribute-based special-casing (see `diagnostics.rs`'s
///   module doc, rule R2).
/// - **as-publisher fan-out**: `snap.publisher_fanout[id]` — the REAL
///   resolved-subscriber count when `id` is a PUBLISHER (precomputed once per
///   snapshot rebuild — see [`crate::lsp::snapshot::LspSnapshot::publisher_fanout`]'s
///   doc). O(1): a t3 whole-branch review found the ORIGINAL version of this
///   function scanned ALL of `event_edges` on every single call (`O(event_edges)`
///   per query), and `compute_all` calls it once per declaration on every
///   diagnostics recompute — itself run on every snapshot swap, INCLUDING a
///   rung-1 single-file body edit — making a full diagnostics pass
///   `O(decls × event_edges)`: quadratic in workspace size, measured 3.4x
///   slower for a 2x larger event-bearing corpus. `publisher_fanout` already
///   sums ROUTES (never mere edge presence — `emit_event_flow_edges` emits
///   one `ClassifiedEdge` per publisher UNCONDITIONALLY, even with zero
///   subscribers (the LSP snapshot drops route-less ones), so a publisher's
///   own entry existing is never itself
///   evidence of usage), mirroring legacy's own
///   `event_subscriptions.get(qname).len()` term (rule R5's "subscribed" half).
#[must_use]
pub(crate) fn effective_incoming_count(snap: &LspSnapshot, id: &RoutineNodeId) -> usize {
    let direct = snap.incoming.get(id).map(Vec::len).unwrap_or(0);
    let as_publisher_fan_out = snap.publisher_fanout.get(id).copied().unwrap_or(0);
    direct + as_publisher_fan_out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::path_to_uri;

    const ALPHA_SRC: &str = r#"codeunit 50100 "LensAlpha"
{
    procedure Caller1()
    begin
        CalledProc();
    end;

    procedure Caller2()
    begin
        CalledProc();
    end;

    procedure CalledProc()
    begin
    end;

    procedure Branchy(X: Integer; Y: Integer)
    begin
        if X > 0 then begin
            if Y > 0 then begin
            end;
        end;
    end;

    trigger OnRun()
    begin
    end;
}
"#;

    fn write_fixture(dir: &std::path::Path) {
        std::fs::write(
            dir.join("app.json"),
            r#"{
    "id": "77777777-0000-0000-0000-000000000012",
    "name": "Task12 Lens Fixture",
    "publisher": "probe",
    "version": "1.0.0.0"
}"#,
        )
        .expect("write app.json");
        std::fs::write(dir.join("Alpha.al"), ALPHA_SRC).expect("write Alpha.al");
    }

    fn fixture_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        write_fixture(dir.path());
        dir
    }

    fn uri_string(dir: &std::path::Path, file: &str) -> String {
        path_to_uri(&dir.join(file)).as_str().to_string()
    }

    fn lens_for<'a>(lenses: &'a [CodeLens], proc_name: &str) -> &'a CodeLens {
        lenses
            .iter()
            .find(|l| {
                l.command.as_ref().is_some_and(|c| {
                    c.arguments
                        .as_ref()
                        .is_some_and(|a| a[0]["procedure"].as_str() == Some(proc_name))
                })
            })
            .unwrap_or_else(|| panic!("no lens for {proc_name}; got {lenses:#?}"))
    }

    // ── one lens per declaration, unfiltered by kind (procs + trigger) ─────

    #[test]
    fn code_lenses_one_per_declaration_including_triggers() {
        let dir = fixture_dir();
        let snap = LspSnapshot::build_full(dir.path()).expect("build_full");
        let uri = uri_string(dir.path(), "Alpha.al");
        let cfg = DiagnosticConfig::default();

        let lenses = code_lenses(&snap, PositionEncoding::Utf16, &uri, &cfg);
        assert_eq!(
            lenses.len(),
            5,
            "Caller1, Caller2, CalledProc, Branchy, OnRun — got {lenses:#?}"
        );
        // The trigger must ALSO get a lens (legacy's get_definitions_in_file
        // was unfiltered by DefinitionKind).
        assert!(
            lenses.iter().any(
                |l| l.command.as_ref().unwrap().title.contains("complexity:")
                    && l.command.as_ref().unwrap().arguments.as_ref().unwrap()[0]["procedure"]
                        .as_str()
                        == Some("OnRun")
            ),
            "OnRun trigger must have a lens too; got {lenses:#?}"
        );
    }

    // ── ref count matches `incoming` exactly (Task 11 fixture numbers) ─────

    #[test]
    fn code_lenses_ref_count_matches_incoming() {
        let dir = fixture_dir();
        let snap = LspSnapshot::build_full(dir.path()).expect("build_full");
        let uri = uri_string(dir.path(), "Alpha.al");
        let cfg = DiagnosticConfig::default();

        let lenses = code_lenses(&snap, PositionEncoding::Utf16, &uri, &cfg);
        let called = lens_for(&lenses, "CalledProc");
        let title = &called.command.as_ref().unwrap().title;
        assert!(
            title.starts_with("2 references"),
            "CalledProc has 2 callers; got title {title:?}"
        );

        let called_decl = snap.decls_by_file["Alpha.al"]
            .iter()
            .find(|d| d.name == "CalledProc")
            .expect("CalledProc decl");
        assert_eq!(
            snap.incoming
                .get(&called_decl.id)
                .map(Vec::len)
                .unwrap_or(0),
            2,
            "sanity: snap.incoming must agree with the lens's own count"
        );

        let caller1 = lens_for(&lenses, "Caller1");
        assert!(
            caller1
                .command
                .as_ref()
                .unwrap()
                .title
                .starts_with("0 references"),
            "Caller1 has no callers; got {:?}",
            caller1.command
        );

        let one_ref_dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            one_ref_dir.path().join("app.json"),
            r#"{"id":"88888888-0000-0000-0000-000000000012","name":"OneRef","publisher":"p","version":"1.0.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            one_ref_dir.path().join("One.al"),
            r#"codeunit 50200 "One"
{
    procedure Caller()
    begin
        Callee();
    end;

    procedure Callee()
    begin
    end;
}
"#,
        )
        .unwrap();
        let one_snap = LspSnapshot::build_full(one_ref_dir.path()).expect("build_full");
        let one_uri = uri_string(one_ref_dir.path(), "One.al");
        let one_lenses = code_lenses(&one_snap, PositionEncoding::Utf16, &one_uri, &cfg);
        let callee_lens = lens_for(&one_lenses, "Callee");
        assert!(
            callee_lens
                .command
                .as_ref()
                .unwrap()
                .title
                .starts_with("1 reference "),
            "singular form for exactly 1 reference; got {:?}",
            callee_lens.command
        );
    }

    // ── complexity delegates to the canonical IR walker, never re-derived ──

    #[test]
    fn code_lenses_complexity_matches_routine_complexity_ir() {
        let dir = fixture_dir();
        let snap = LspSnapshot::build_full(dir.path()).expect("build_full");
        let uri = uri_string(dir.path(), "Alpha.al");
        let cfg = DiagnosticConfig::default();

        let entry = snap.parsed.get("Alpha.al").expect("Alpha.al parsed");
        let branchy_decl = snap.decls_by_file["Alpha.al"]
            .iter()
            .find(|d| d.name == "Branchy")
            .expect("Branchy decl");
        let routine = find_routine_by_origin(&entry.file, branchy_decl.origin.byte.start)
            .expect("Branchy routine");
        let expected_complexity = crate::analysis::routine_complexity_ir(&entry.file.ir, routine);
        // Sanity: this fixture's nested-if body must have complexity > 1 so
        // the assertion below is non-trivial (base 1 + 2 nested ifs = 3).
        assert_eq!(expected_complexity, 3, "fixture assumption");

        let lenses = code_lenses(&snap, PositionEncoding::Utf16, &uri, &cfg);
        let branchy_lens = lens_for(&lenses, "Branchy");
        let title = &branchy_lens.command.as_ref().unwrap().title;
        assert!(
            title.contains(&format!("complexity: {expected_complexity}")),
            "title must show the SAME complexity routine_complexity_ir computes; got {title:?}"
        );
        // 2 declared parameters.
        assert!(title.contains("params: 2"), "{title:?}");
    }

    // ── threshold indicator (⚠️) appears once a metric crosses `critical` ──

    #[test]
    fn code_lenses_shows_warning_marker_past_critical_threshold() {
        let dir = fixture_dir();
        let snap = LspSnapshot::build_full(dir.path()).expect("build_full");
        let uri = uri_string(dir.path(), "Alpha.al");

        let cfg = DiagnosticConfig {
            complexity_critical: 2, // Branchy's complexity (3) now exceeds it.
            ..DiagnosticConfig::default()
        };

        let lenses = code_lenses(&snap, PositionEncoding::Utf16, &uri, &cfg);
        let branchy_lens = lens_for(&lenses, "Branchy");
        let title = &branchy_lens.command.as_ref().unwrap().title;
        assert!(
            title.contains('\u{26a0}'),
            "complexity past critical threshold must show the warning marker; got {title:?}"
        );
    }

    // ── unknown uri / no declarations → empty, never a panic ───────────────

    #[test]
    fn code_lenses_empty_for_unknown_uri_and_declaration_free_file() {
        let dir = fixture_dir();
        std::fs::write(dir.path().join("Empty.al"), "// no declarations here\n").unwrap();
        let snap = LspSnapshot::build_full(dir.path()).expect("build_full");
        let cfg = DiagnosticConfig::default();

        let bogus_uri = path_to_uri(&dir.path().join("DoesNotExist.al"))
            .as_str()
            .to_string();
        assert!(code_lenses(&snap, PositionEncoding::Utf16, &bogus_uri, &cfg).is_empty());

        let empty_uri = uri_string(dir.path(), "Empty.al");
        assert!(code_lenses(&snap, PositionEncoding::Utf16, &empty_uri, &cfg).is_empty());
    }
}
