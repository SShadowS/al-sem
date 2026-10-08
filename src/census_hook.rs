//! Phase marks for the out-of-tree byte-census probe (`tools/census-probe`).
//!
//! A no-op until a probe registers [`HOOK`]: one atomic load per mark. The
//! probe records live heap at each mark to attribute memory to build phases
//! (see `.claude/skills/byte-census/SKILL.md`). The phase names are the
//! probe's contract; renaming one silently breaks its report.

use std::sync::OnceLock;

/// Set once by a probe. Never set in production.
pub static HOOK: OnceLock<fn(&'static str)> = OnceLock::new();

/// Report reaching build phase `name` to the probe, if one is registered.
#[inline]
pub fn mark(name: &'static str) {
    if let Some(f) = HOOK.get() {
        f(name);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    static SEEN: Mutex<Vec<(std::thread::ThreadId, &'static str)>> = Mutex::new(Vec::new());

    fn record(name: &'static str) {
        SEEN.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((std::thread::current().id(), name));
    }

    /// Pins the USE: the marks fire, in order, during a real LSP build. Deleting
    /// any `mark(..)` call site in `full.rs`/`snapshot.rs` fails this test.
    #[test]
    fn a_real_build_reports_every_phase_in_order() {
        let _ = super::HOOK.set(record);
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("app.json"),
            r#"{"id":"11111111-1111-1111-1111-111111111111","name":"Ws","publisher":"T","version":"1.0.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("Cu.al"),
            "codeunit 50000 Cu\n{\n    procedure Foo()\n    begin\n    end;\n}\n",
        )
        .unwrap();
        crate::lsp::snapshot::LspSnapshot::build_full(dir.path()).expect("build");
        let me = std::thread::current().id();
        let mine: Vec<&str> = SEEN
            .lock()
            .unwrap()
            .iter()
            .filter(|(t, _)| *t == me)
            .map(|(_, n)| *n)
            .collect();
        assert_eq!(
            mine,
            [
                "1.snapshot",
                "2.parse",
                "3.dep_layer",
                "4.assemble_graph",
                "5.index+surface+dep_meta+dep_lines",
                "6.resolve_workspace_files",
                "7.event_edges",
                "8.incoming+decl_by_id",
                "9.publish_snapshot",
            ]
        );
    }
}
