---
name: add-al-construct
description: Use when adding parse/lower/resolve support for a new AL construct: grammar → lowerer → IR consumers → fixture + goldens → measure.
---

# Adding New AL Constructs

1. **Grammar first.** If the construct isn't already parseable, add/fix the rule in
   `tree-sitter-al/grammar.js` (a separate repo we own — `SShadowS/tree-sitter-al`) and
   regenerate (`tree-sitter generate` from `tree-sitter-al/`). Verify the real shape with
   `tree-sitter parse <file.al>` — never assume from reading `grammar.js` alone (see
   the Grammar section in CLAUDE.md and the `grammar-upgrade` skill for why).
2. **Lower it.** Teach `crates/al-syntax/src/lower/mod.rs` to turn the new CST shape into
   the owned IR (`crates/al-syntax/src/ir/`) — extend `ObjectKind`/`RoutineKind`/`StmtKind`/
   `ExprKind` etc. as needed. This is the ONLY place that reads raw tree-sitter nodes for
   the new construct; get it right here and every consumer below sees a clean IR node.
3. **Wire the IR consumers that need it**, as applicable:
   - LSP surface: nothing to wire separately — `src/lsp/snapshot.rs`'s `LspSnapshot`
     projects directly off the program engine's resolved graph (below), so a new
     construct that resolves correctly there is automatically visible to
     prepare/incoming/outgoing/codeLens/diagnostics with no LSP-specific step.
   - Program engine (the moat): `src/program/resolve/extract.rs` (obligation extraction)
     and/or `src/program/resolve/resolver.rs` (dispatch) if it's a new call/edge shape
   - Detector model (body facts the detectors read): the body walk under
     `src/program/body/` and the projection in `src/program/model/workspace.rs`
4. **Add a fixture** under `tests/fixtures/` (or the plan/task-specific golden family) and
   **regenerate goldens**: `REGEN_TEMP_GOLDENS=1 cargo test` rewrites Rust-owned goldens —
   inspect the diff before committing; it is a measurement, never an auto-bless (see
   Testing Philosophy & Goldens in CLAUDE.md). `REGEN_TEMP_GOLDENS` is value-tested (`=1`
   specifically), not presence-tested — `REGEN_TEMP_GOLDENS=0` does NOT trigger a regen.
5. Format touched files with `rustfmt <file>` (never `cargo fmt`), run
   `cargo clippy --all-targets --all-features`, and — if the change could move the
   call-graph resolution needle — re-measure with `aldump --program-call-graph-stats`
   (see Project Direction & The Moat in CLAUDE.md).
