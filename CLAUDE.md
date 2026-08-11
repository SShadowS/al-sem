# CLAUDE.md

Write in plain English.
Use common words and short sentences. Avoid jargon when a simpler term exists. If a technical term is necessary, define it the first time you use it.
Assume I am intelligent but unfamiliar with the terminology. Be concise, but do not remove details needed for correctness.

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**al-sem** is a whole-program semantic analysis engine for AL (Microsoft Dynamics 365
Business Central). Its core is a precise call-graph resolver; on top of that sit three
products — a call-hierarchy LSP server, the `alsem` code-quality analyzer and impact-query
CLI, and the `aldump` engine-inspection tool.

**The crate was renamed from `al-call-hierarchy` to `al-sem` on 2026-08-07**, because the
call hierarchy is one consumer of the engine rather than the whole of it. Four things
deliberately KEEP the old name, and each will look like an oversight to a future tidier:

| Still `al-call-hierarchy` | Why it must stay |
|---------------------------|------------------|
| the LSP server **binary** (explicit `[[bin]]` in Cargo.toml) | `.github/workflows/build-and-deploy.yml` copies it into three `claude-code-lsps/al-language-server-go*` packages, and `al-language-server-python` spawns it |
| diagnostic `source`, the `showReferences` command id, `serverInfo.name`, the `al-call-hierarchy/*` custom LSP methods | wire strings an editor extension keys on |
| the App Insights service name (`src/telemetry/exporter.rs`) | the join key for existing dashboards and saved queries |
| `ANON_SALT` in `src/program/resolve/anon.rs` | frozen bytes; every committed anonymized golden was minted with them |

Per-user state moved to `~/.al-sem/` (joining the analyzer's existing `cache/`) with a
read-fallback to `~/.al-call-hierarchy/` — see `src/state_paths.rs`. The local checkout
directory is still `U:/Git/al-call-hierarchy`; renaming the GitHub repo does not move it.

## Build Commands

```bash
cargo build                    # Debug build
cargo build --release          # Optimized release (full LTO, cu=1 — ~4 min for alsem; SLOW)
cargo build --profile release-fast  # Thin LTO — USE THIS for measurement/triage loops (alsem/aldump on DO);
                                    # full --release only for the SHA-pinned north-star measure, perf_bounds, benches
cargo test                     # Run tests
cargo test -p al-sem --lib <filter>   # Package is al-sem (HYPHEN); al_sem fails. Renamed from
                                      # al-call-hierarchy 2026-08-07 — the BINARY keeps that name
rustfmt path/to/file.rs        # Format a file (NEVER `cargo fmt` — whole-crate churn)
scripts/ci-steps clippy        # Lint — CI's EXACT bar (release + -D warnings)
scripts/check-goldens          # Run ALL byte-compared golden families at once (see Testing Philosophy & Goldens)
```

**Do not pipe a long-running gate/test through `| tail`** (e.g. `bash scripts/cdo-gate … | tail`): the pipeline's exit code is `tail`'s, not the command's, so a FAILURE reads as success. Redirect to a log file and `grep` it, or run it with `run_in_background` and read the output file.

## Running

```bash
# LSP mode (default) - communicates via stdio
cargo run

# CLI mode: index a project and report definition/call-site counts
cargo run -- --project /path/to/al-project

# CLI mode: code quality analysis (complexity, params, line count, fan-in, unused procs)
cargo run -- --project /path/to/al-project --analyze --format text   # or json | csv

# With verbose logging
cargo run -- --project /path/to/al-project --verbose
```

There is no `--no-lsp` flag — `clap` rejects unknown flags outright. LSP server mode is
the default whenever `--project` is *not* given; passing `--project` alone switches to
CLI mode (index-and-report, or `--analyze` for the quality-diagnostics report). `--lsp`
forces LSP server mode at top precedence (it overrides `--project`, always starting the
server). `--analyze` without `--project` is a hard error. Other flags: `--no-watcher` (disable the file
watcher; rely on LSP change notifications instead), `--no-telemetry` (see `docs/telemetry.md`).
See `src/main.rs`'s `Args` (clap derive) for the authoritative flag list.

## Prerequisites

- Rust 1.75+
- tree-sitter-al **v4.0.0** grammar (included as a git submodule at `tree-sitter-al/`,
  pinned in the superproject's index; CI instead checks out the grammar repo's `main`
  branch unpinned — see the Grammar section below)
  - Clone with `git clone --recurse-submodules`, or run `git submodule update --init` after clone
  - **Git worktrees do not get their own submodule checkout.** From a worktree, either
    run `git submodule update --init` there too, or set `TREE_SITTER_AL_PATH` to point
    at an already-checked-out `tree-sitter-al/` (e.g. the main checkout's copy) — the
    `al-syntax` build script falls back to `../../tree-sitter-al` (relative to
    `crates/al-syntax`) when the env var is unset, which resolves correctly only for a
    normal (non-worktree) checkout.
  - Removing a worktree: `git worktree remove` FAILS here ("working trees containing
    submodules cannot be moved or removed") — verify the tree is clean/merged, then
    `rm -rf <worktree-dir> && git worktree prune`.
  - `.claude/` is gitignored EXCEPT `.claude/commands/` (project slash commands are
    versioned — they encode project doctrine, e.g. `/triage-wave`). Skills still exist
    only in the MAIN checkout — from a worktree, reference skill scripts by absolute
    main-checkout path (e.g. `U:/Git/al-call-hierarchy/.claude/skills/...`).
  - `git config core.hooksPath scripts/git-hooks` (see "Testing Philosophy & Goldens"
    below) resolves to an ABSOLUTE path into the MAIN checkout, shared by every
    worktree — a commit made FROM a worktree therefore runs the MAIN checkout's
    copy of the hook, not the worktree's own. A hook edit only takes effect once it
    lands there, so testing an edit from the worktree that authored it does not
    exercise the copy that will actually run.

## Architecture

This repo is **one engine, two consumers** — not two independent pipelines. The
whole-program semantic graph (`src/snapshot/`, `src/program/`) is the ONLY
resolution engine; the LSP surface below is a direct CONSUMER of its resolved
output, not a second, tree-sitter-only pipeline running in parallel. (It used to
be two: a legacy `graph.rs`/`indexer.rs`/`parser.rs`/`handlers.rs` pipeline built
its own naive call graph independently of the program engine. That pipeline was
deleted at the T3 LSP-migration arc's capstone task, licensed by a differential
harness that proved parity-or-improvement against real BC source — see
CHANGELOG's `Removed` entry for the deletion evidence and the harness's CDO
results.)

**Consumer 1 — the LSP surface** (call hierarchy / code lens / diagnostics the editor sees):
```
AL Source Files + .alpackages → snapshot::snapshot_workspace (AppSetSnapshot)
    → al_syntax::parse per file → program::build + program::resolve::full::
    resolve_full_program → lsp::snapshot::LspSnapshot (decls_by_file / edges_by_file /
    event_edges — the O(1) query surface, built directly from the resolved graph)
    → LSP Server (server.rs) ← lsp::updater::spawn_updater (background incremental
      rebuild, fed by the File Watcher (watcher.rs) + didSave)
    → Request Handlers (lsp::handlers: prepare/incoming/outgoing; lsp::lens: codeLens;
      lsp::diagnostics: compute_all; lsp::custom: dependencyDocumentSymbol /
      eventPublishersInFile / eventReferenceAtPosition / fieldProperties / actionProperties)
    → LSP Responses
```
Every module above lives in `src/lsp/` (a library module, see Key Modules below) so
benches/tests can build a snapshot and query it in-process; `main.rs` re-exports `lsp`
(alongside `config`/`telemetry`/`analysis`) so the binary-only modules (`server.rs`,
`watcher.rs`) keep using it unchanged.

**Consumer 2 — the CLI / `aldump`** (whole-program call-graph resolution, the moat):
```
Workspace + .alpackages → snapshot::snapshot_workspace (AppSetSnapshot, identity-verified
    source roots per app) → al_syntax::parse per file → program::build (ProgramGraph:
    nodes + app-qualified identity + topology index) → program::resolve::full::
    resolve_full_program (the fresh, clean-room edge resolver — no L3 oracle) → Histogram
    (taxonomy'd edge counts) + per-edge Route report
```
Driven via `aldump --program-call-graph-stats <workspace>` (the north-star metric command)
or consumed programmatically by `src/engine/l4`/`l5` (effect summaries, detectors) and
`src/engine/gate` (the `analyze` CLI's SARIF/JSON/HTML report path).

**Key Modules — LSP surface (`src/lsp/`, `server.rs`, `watcher.rs`):**
- `main.rs` - CLI entry point (clap), dispatches to LSP server / CLI index / `--analyze`
- `server.rs` - LSP server initialization, dispatch, and the diagnostics-publish/watcher wiring
- `src/lsp/snapshot.rs` - `LspSnapshot`: the immutable, `Arc`-shareable query surface
  (`decls_by_file`/`edges_by_file`/`event_edges`/`dep_decl_by_id`), built by
  [`LspSnapshot::build_full`] directly from the program engine's resolved graph
- `src/lsp/updater.rs` - The incremental updater: `ChangeEvent` batching, the two-rung
  (plus degenerate rung-3) soundness ladder, atomic `SharedSnapshot` swap
- `src/lsp/def_surface.rs` - Per-file definition-surface fingerprinting (the rung-1/rung-2 gate)
- `src/lsp/encoding.rs` - UTF-8/UTF-16 `PositionEncoding` negotiation + `LineTable` conversion
- `src/lsp/handlers.rs` - `prepare`/`incoming`/`outgoing` (prepareCallHierarchy / incomingCalls
  / outgoingCalls)
- `src/lsp/lens.rs` - `code_lenses` (reference counts + quality-metric thresholds)
- `src/lsp/diagnostics.rs` - `compute_all`/`DiagnosticsState` (unused-procedure + code-quality
  diagnostics, diffed and republished on every snapshot swap)
- `src/lsp/custom.rs` - `dependencyDocumentSymbol` / `eventPublishersInFile` /
  `eventReferenceAtPosition` (engine-backed custom requests, reading dependency ABI/workspace
  IR directly) plus `fieldProperties`/`actionProperties` (graph-independent, pure
  source-read + `al-syntax` facade lookup) and the `al-preview://` URI parser
  (`parse_al_preview_uri`, also used by `lsp::handlers`'s ABI-symbol URI minting)
- `watcher.rs` - File system watcher for incremental updates
- `analysis.rs` - Code quality metrics (cyclomatic complexity, params, line count) for
  `--analyze` — a library module (shares `routine_complexity_ir`/
  `is_framework_invocation_attribute` with `src/lsp/lens.rs`/`diagnostics.rs`)
- `config.rs` - Diagnostic threshold config (global `~/.al-sem/config.json` + per-workspace
  `.al-sem.json`; both fall back to the pre-rename `.al-call-hierarchy` spellings)
- `state_paths.rs` - The one place that knows both the current and pre-rename per-user
  state locations. **Read it before touching any `~/.al-sem/` path** — the read-fallback
  is what keeps an install that predates the 2026-08-07 rename working
- `app_package.rs` - Parser for .app files (extracts SymbolReference.json)
- `dependencies.rs` - Dependency resolution from app.json and .alpackages
- `protocol.rs` - LSP protocol URI/path conversion helpers
- `types.rs` - Core AL object-type enum shared across lib/binary
- `language.rs` - Re-exports `al_syntax::language::language()`; also holds the legacy
  tree-sitter S-expression queries (`DEFINITIONS`/`CALLS`/`EVENT_SUBSCRIBERS`/`VARIABLES`),
  which are dead code — retired by the owned-IR migration, zero call sites repo-wide.
  Kept only because `tree-sitter-al/queries/highlights.scm`/`tree-sitter-al/queries/tags.scm` (editor syntax
  highlighting, a separate consumer) still reference the same grammar node names.

**Key Modules — program engine (`src/engine/`, `src/program/`, `src/snapshot/`):**
- `src/snapshot/` - App-set ingestion: turns a workspace + symbol-only dep tables into
  identity-verified per-app source roots (`snapshot_workspace`, `AppSetSnapshot`)
- `src/program/` - Whole-program semantic graph: `node`/`topology` (app-qualified identity
  + index), `build` (assembly), `sig_fp` (signature fingerprints)
- `src/program/resolve/` - **The fresh call/behaviour-edge resolver** — `full.rs`
  (`resolve_full_program`, the entry point), `resolver.rs`/`receiver.rs`/`arg_dispatch.rs`
  (dispatch), `builtins.rs`/`member_catalog.rs` (platform intrinsic catalogs), `edge.rs`
  (the `Histogram` taxonomy + `ObligationOutcome`)
- `src/program/abi_ingest.rs` - Dependency ABI ingestion (sibling of `resolve/`, not inside it)
- `src/engine/l2/` - Structural body-walk + feature projection over the owned IR
- `src/engine/l3/` - Legacy workspace symbol table + call resolver (the RETIRED al-sem
  port; `--l3-call-graph-stats` and siblings are advisory-only — see Project Direction)
- `src/engine/l4/` - Per-routine effect summaries over the call graph's SCC condensation.
  Its db-effect QUERY surface is `effect_query.rs` (`DbEffectQuery`: down / up-global /
  the ancestor-scoped up-query) over `reverse_index.rs`'s transpose, with
  `effect_query_cli.rs` as the `alsem query` transport + the `RoutineIx`→`L3Routine`
  join. Read `effect_query.rs`'s module doc BEFORE building anything on it — it states
  which question this substrate answers and which one `cone_derived.rs` already answers
  better (physical-table WRITES), so a second answer to the same question is not built.
- `src/engine/l5/` - Detectors + query substrate (findings, event-flow, digests, fingerprints)
- `src/engine/gate/` - The production `analyze` CLI path (SARIF/JSON/HTML/terminal report,
  baseline diffing, inline suppression, policy)
- `src/engine/deps/` - `.app` symbol-reference ingestion (manifest + SymbolReference.json → ABI)
- `src/bin/aldump.rs` - Multi-mode dump CLI: `--program-call-graph-stats` (north-star),
  `--l2`/`--l3-*` (legacy engine layers), `--graphify-export`, etc. — see its `usage()`
- `src/bin/alsem.rs` - The production `analyze`/diagnostics CLI (installed binary name);
  also hosts `alsem query touches|effects` (the db-effect query surface above)
- `src/bin/mint-goldens.rs` - Mints/regenerates committed golden fixtures

**`crates/al-syntax`** is the **only** crate that touches tree-sitter: FFI grammar
binding (`language.rs`), the generated raw vocabulary + typed CST (`raw/generated/`),
the CST→IR lowerer (`lower/mod.rs`), and the owned AL syntax IR (`ir/`) every consumer
above builds on. See the Grammar section below.

**Testing:** Integration tests are consolidated into **umbrella crates** — one
`tests/<dir>/main.rs` per domain (`tests/{gap,cli,l3,l2_ir,temp_state,r25_abi,r3,r4,lsp}/`),
each a single link target whose `main.rs` lists its members as `mod` items;
`program_resolve_harness`, `perf_bounds`, and `differential` remain standalone
top-level `tests/*.rs` crates. Run one member module with
`cargo test --test <umbrella> <member_stem>::` (libtest module filter).
`tests/common/{cdo,regen}.rs` are shared helpers — `cdo.rs` gates CDO-workspace
tests, `regen.rs` gates golden-regeneration paths; each umbrella's `main.rs`
hoists the needed include ONCE (`#[path = "../common/regen.rs"] mod regen;`),
and members reach it via `use crate::regen;`/`use crate::cdo;`. The `cli`
umbrella additionally owns `ENV_LOCK`/`env_guard()`, which serializes the
process-global `std::env` mutation in the `cli_a_*` differentials.
`scripts/cdo-gate` runs the full CDO-gated suite with `ENFORCE_CDO_WS=1` (see
Testing Philosophy & Goldens below).

**Core Patterns:**
- **String interning** (`string-interner`): All symbol names deduplicated for memory efficiency
- **Parallel parsing** (`rayon`): files are parsed concurrently via `par_iter` on a
  dedicated pool (`crate::big_stack::big_stack_pool`, sized for the `al-syntax`
  lowerer's recursion — see `src/snapshot/parse.rs:84-95`). There are **no**
  thread-local parsers: `al_syntax::parse` constructs a fresh
  `tree_sitter::Parser` and calls `set_language` per file
  (`crates/al-syntax/src/parse.rs:11-14`). Reusing a parser per worker thread is
  an open, unmeasured optimisation — see the pack-cache spec §17.5.
- **Incremental updates** (`notify`): Only re-parse changed files

## Performance Targets

Measured by `cargo bench --bench lsp_pipeline` (Criterion; `benches/lsp_pipeline.rs`,
rewritten for the engine-backed LSP surface at T3 Task 16) against a deterministic
synthetic corpus (`tests/perf_support/`) — 1000 codeunits with real cross-file call
fan-in/fan-out for the query rows, exercising `LspSnapshot`/`lsp::handlers`/
`lsp::updater` (no legacy `Indexer` involved — that pipeline is deleted, see
Architecture above). A release-only CI gate (`tests/perf_bounds.rs`) asserts every
operation stays within 3x its target on every PR (rung 1/rung 2 additionally carry a
corpus-relative bound — see that file's own doc), so an order-of-magnitude regression
fails loudly even though the day-to-day numbers below have wide headroom.

| Operation | Target | Measured (2026-07-13, dev machine, Criterion median) |
|-----------|--------|-------------------------------------------------------|
| `build_full` (100 files) | < 500ms | ~8.07ms |
| `build_full` (1000 files) | < 2s | ~74.45ms |
| `prepare` (prepareCallHierarchy) | < 1ms | ~7.88µs |
| `incoming` (1000-file graph, 999-way real fan-in) | ~25ms — see note below | ~16.34ms |
| `outgoing` (1000-file graph) | < 1ms | ~6.60µs |
| `compute_all` (diagnostics recompute, 1000-file event-bearing graph) | 50ms (architectural — see footnote below) | ~7.9ms |
| Incremental update, rung 1 (body-only edit, 1000-file graph) | 100ms (real-CDO-workspace re-measurement) | ~13.28ms |
| Incremental update, rung 2 (signature-change edit, 1000-file graph) | ~1.5s (real-CDO-workspace re-measurement) | ~149.93ms |

**Every rung-1/rung-2 save additionally pays `compute_all`'s own cost** — `server.rs`'s
`on_swap` runs the full diagnostics recompute after EVERY snapshot swap, including a
single-file rung-1 body edit, so the real per-save latency a user experiences is
rung-N's own number PLUS `compute_all`'s row above (~13.28ms + ~7.9ms ≈ 21ms for a
rung-1 save on this corpus). `compute_all` used to carry a real O(decls × event_edges)
quadratic cost (found and fixed in a t3 whole-branch review pass — see
`src/lsp/snapshot.rs`'s `LspSnapshot::publisher_fanout` doc); the row above already
reflects the fix, measured on an event-bearing corpus (`tests/perf_support/`'s
generator now declares 2 publisher/2 subscriber routines per file specifically so this
cost is never accidentally measured against an all-zero `event_edges` population).

**`incoming`'s target moved from the legacy pipeline's sub-microsecond number to
~25ms — this is architectural, not a regression.** The legacy `graph.rs` stored a
byte range per call site at index time and served it directly (O(1) per caller).
`lsp::handlers::incoming` deliberately re-derives every distinct caller's position
LIVE from that caller's OWN current file text (a fresh `LineTable` per caller) —
never serving a stale stored witness span, a correctness rule the live engine holds
that the legacy pipeline did not. For a 999-way fan-in that's 999 real per-file text
scans: genuinely O(distinct callers), a different complexity class than
`prepare`/`outgoing` (whose cost doesn't scale with fan-in). 20ms is still
editor-imperceptible for a "who calls this" panel.

## Key Data Structures

```rust
// src/program/node.rs — overload-aware routine identity (object + name + enclosing
// member + arity + a param-type-sequence fingerprint; total, not name-only)
RoutineNodeId { object: ObjectNodeId, name_lc, enclosing_member_lc, params_count, sig_fp }

// src/lsp/snapshot.rs — LspSnapshot's owned, Arc-shareable query surface
DeclEntry { id: RoutineNodeId, name, origin, name_origin, virtual_path }  // a declaration
EdgeRef { file: String, idx: u32 }  // index into edges_by_file[file] — never a borrow
```

## Grammar (tree-sitter-al v4.0.0)

**Current reality:** the grammar is **v4.0.0** (`tree-sitter-al/package.json`, tag
`v4.0.0`; the pin sits a few commits past the tag on the grammar repo's `main`, matching
what unpinned CI checks out). The submodule pointer in this repo's git index is pinned to a specific
commit (reproducible local/dev builds); CI instead checks out `SShadowS/tree-sitter-al`
`main` **unpinned** (`.github/workflows/ci.yml`) so a breaking grammar change surfaces
on the next PR rather than silently drifting. `crates/al-syntax` is the **only** crate
that links tree-sitter or walks its raw CST — every other consumer (`src/lsp/snapshot.rs`,
`src/engine/l2` and everything layered on it, `src/program/resolve`) reads the owned
AL syntax IR that `al-syntax`'s lowerer (`crates/al-syntax/src/lower/mod.rs`) produces.
Practical effect: the "flat vs. recursive walk" hazard that mattered under the old
tree-sitter-query architecture (see History below) no longer applies to engine code at
all — the IR's `Block`/`Stmt` items are already flattened once, at the lowering
boundary, so nothing downstream ever sees a `statement_block`/`declaration_body`
wrapper node.

**Notes still relevant if you touch the lowerer itself** (`crates/al-syntax/src/lower/mod.rs`,
the one place that still reads raw grammar shapes):
- A scoped member trigger (`Object::Member` triggers) is a single named
  `member_trigger_name` CST node (`object`/`member` fields, no literal `::` token in its
  field set) rather than a plain `(identifier)`/`(quoted_identifier)` — `routine_name_text`
  joins the two fields back into `Object::Member` text. Editor consumers
  (`tree-sitter-al/queries/highlights.scm` + `tree-sitter-al/queries/tags.scm`) capture `member_trigger_name` directly;
  the engine does not use `.scm` queries at all (IR-only).
- A `case` branch's `pattern` field binds one `_single_pattern` value per branch value —
  the `,` separators are never tagged `pattern` — and an `in`-as-case-pattern lowers to
  the named `in_expression` node, never an inline `seq` leaking `left`/`operator`/`right`.
  The lowerer keeps a defensive `is_named()` filter on `children_by_field(Pattern)` as
  belt-and-suspenders (an anonymous `,` token has no `RawKind` and would panic if it ever
  reached `lower_expr`).
- Object/field/action bodies wrap their content in a `declaration_body` node (repeat1 of
  `_body_element`); a code block's statements wrap in `statement_block`; report dataitem
  bodies in `report_body`; `var_section`'s body in `var_body`. `grammar.js`'s own rule
  definitions are the source of truth — grep it rather than trusting this list to stay
  current.
- **Validate a lowerer change with the differential goldens** (`cargo test`): zero
  divergences = behaviour-preserving. Goldens are **Rust-owned baselines** regenerated
  from this engine — see Testing Philosophy & Goldens below.
- Dump a real tree with `tree-sitter parse <file.al>` from `tree-sitter-al/` — always
  verify a grammar-shape claim against real `tree-sitter parse` output, not just a read
  of `grammar.js` (two owned-grammar field-pollution bugs upstream of the lowerer were
  found exactly this way; see CHANGELOG history).
- **v4.0.0 shapes (2026-08-12 upgrade — what moved and what to rely on):**
  - The statement terminator `;` lives OUTSIDE `code_block`, `if_statement` and the
    branch rules — statement/block spans end at their last real token (1 byte earlier
    than v3 when a `;` followed). Body/branch fields (`body`, `then_branch`,
    `else_branch`, …) hold exactly ONE named node, never `[stmt, ';']`.
  - A dangling `else` binds to the INNER `if` (was: outer construct) — the one v4
    change that alters what a program means. `case … else` bodies are one
    `statement_block`.
  - Operator precedence is Pascal-correct: `and`/`or`/`xor` bind TIGHTER than
    comparison, unary minus tighter than `*` (`-a * b` = `(-a) * b`), and `..` is no
    longer an expression operator. Binary IR shapes reflect this.
  - ~70 more keywords became NAMED nodes (`record_keyword`, `field_keyword`, …), plus
    `assignment_operator` and `filter_operator`. They appear as named children
    everywhere — `schema/kind_policy.rs` classifies them `Trivia` and the shared
    `structural_children` filter drops them, so positional reads stay clean. A new
    RawKind variant fails `kind_policy.rs`'s exhaustive match (the loudness gate):
    triage it there, never wildcard it.
  - Keyword nodes have a UNIFORM shape (one anonymous child, canonical lowercase
    spelling) — read a keyword's text from the node itself, never a child. Queries
    naming non-lowercase anonymous spellings (`"IF"`, `"Then"`) no longer compile.
  - Dotted-reference fields no longer include the `.` separator tokens.

**History (V1 → V2 → V3, kept for archaeology — not actionable for engine code today):**
V2 removed V1's wrapper nodes (`procedure name:`/`trigger_declaration name:` held
`identifier`/`quoted_identifier` directly, no `name`/`trigger_name` wrapper), renamed
`parameter_name:` → `name:` and `property:` → `member:`, merged `field_access` into
`member_expression`, unified `named_trigger`/`onrun_trigger` into `trigger_declaration`,
and unified the individual `*_property` nodes into one `property` node. V3 then
reintroduced wrapper nodes for a different reason (structural grouping, not per-field
naming) — `code_block`'s body wraps in `statement_block`, object/field/action bodies in
`declaration_body`, etc. (see above) — which broke direct-child (`named_children`) reads
written against V2's flat shape; the fix at the time was recursive walks or explicit
`child_by_field_name("body")` descent. All of that now lives entirely inside the
`al-syntax` lowerer; nothing else needs to know it happened.

## Adding New AL Constructs

1. **Grammar first.** If the construct isn't already parseable, add/fix the rule in
   `tree-sitter-al/grammar.js` (a separate repo we own — `SShadowS/tree-sitter-al`) and
   regenerate (`tree-sitter generate` from `tree-sitter-al/`). Verify the real shape with
   `tree-sitter parse <file.al>` — never assume from reading `grammar.js` alone (see
   Grammar section above for why).
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
   - Legacy L3 engine (advisory-only; rarely needs touching for new work):
     `src/engine/l2/ir_walk.rs`, `src/engine/l3/`
4. **Add a fixture** under `tests/fixtures/` (or the plan/task-specific golden family) and
   **regenerate goldens**: `REGEN_TEMP_GOLDENS=1 cargo test` rewrites Rust-owned goldens —
   inspect the diff before committing; it is a measurement, never an auto-bless (see
   Testing Philosophy & Goldens below). `REGEN_TEMP_GOLDENS` is value-tested (`=1`
   specifically), not presence-tested — `REGEN_TEMP_GOLDENS=0` does NOT trigger a regen.
5. Format touched files with `rustfmt <file>` (never `cargo fmt`), run
   `cargo clippy --all-targets --all-features`, and — if the change could move the
   call-graph resolution needle — re-measure with `aldump --program-call-graph-stats`
   (see Project Direction & The Moat below).

## Resolution Coverage

The old table here (`Local procedures | Yes`, `Record methods | Partial`, ...) predated
the whole resolution program and is gone — a binary yes/no doesn't describe the fresh
resolver's output. The honest taxonomy, as emitted by `aldump
--program-call-graph-stats <workspace>` (`src/program/resolve/edge.rs`'s `Histogram`;
JSON keys shown):

| Bucket | JSON key | Meaning |
|--------|----------|---------|
| Resolved (source) | `resolvedSource` | Target routine found in first-party/workspace source |
| Resolved (catalog) | `resolvedCatalog` | Platform intrinsic — a cataloged builtin, not a hole |
| Resolved (ABI/external) | `resolvedAbiExternal` | Target routine found via a dependency's ABI |
| Conditionally resolved | `conditionalResolved` | Resolved under a stated precondition (e.g. interface dispatch) |
| Honest dynamic | `honestDynamic` | Provably runtime-typed — no static target exists to find |
| Honest empty | `honestEmpty` | Provably no callee (e.g. an empty event subscriber slot) |
| **Unknown** | `unknown` | **A TRUE resolution failure — the signal to eliminate** |
| Ambiguous, resolved | `ambiguousResolved` | Closed same-object overload-ambiguity candidate set — NOT counted as `unknown` |

Both `wholeProgram` (every edge, including dependency-internal ones) and `primaryScoped`
(workspace-only edges — mirrors `--l3-call-graph-stats-cross-app`'s scoping) variants are
emitted, each with `realUnknownRate = unknown / total`. **Last measured** (CDO,
Continia's real BC workspace — requires `CDO_WS`; not reproducible in this sandbox, see
`scripts/cdo-gate`), immediately after the Tier-1 deep-review-remediation merge (commit
`f171d0f`), JSON SHA-256 `0a3b85bc832ff0a3e77acee118d203edbf62827dc37617c8d9315fe52d5cb7d0`:

| Scope | total | resolvedSource | resolvedCatalog | resolvedAbiExternal | honestDynamic | honestEmpty | conditionalResolved | unknown | ambiguousResolved |
|-------|------:|----------------:|-----------------:|----------------------:|----------------:|-------------:|----------------------:|--------:|--------------------:|
| primaryScoped | 18113 | 8325 | 5783 | 57 | 55 | 3876 | 17 | **0** | 0 |
| wholeProgram | 43375 | 10219 | 5783 | 57 | 55 | 26942 | 319 | **0** | 0 |

`realUnknownRate` is **0.0000%** in both scopes — but treat that as a point-in-time
measurement to re-verify (`scripts/cdo-gate <CDO_WS>`), not an immutable fact: the deep
review that produced Tier 0 found the metric had been structurally unfalsifiable (missed
edges could land in `builtin`/vanish entirely/never get measured/report success on
failure) before Tier 0's instrument-honesty fixes landed. Tier 0 closed those specific
holes; the zero above is the first measurement taken with the hardened instrument.

## Project Direction & The Moat

The product's moat is **precise whole-program call-graph resolution** for AL. The
north-star metric is the **real-`unknown` edge rate** on real BC apps (measure with
`aldump --program-call-graph-stats <workspace>`, the FRESH clean-room resolver — no L3
oracle): drive it toward zero, where the residual is provably dynamic. See Resolution
Coverage above for the full honest taxonomy (`resolved`/`builtin`/`dynamic`/`external`/
`unknown`) and the current CDO numbers, and the call-graph resolution redesign spec
under `docs/superpowers/specs/`.

**Two distinct "legacy" axes exist — do not conflate them:**
1. **Engine axis** (which resolver produced the number): `aldump
   --program-call-graph-stats` (the fresh resolver, above — **authoritative**) vs.
   `aldump --l3-call-graph-stats` and its `-cross-app`/`-unknown-breakdown` siblings (the
   legacy L3 engine, a al-sem-era port — **advisory only**, reported under a DIFFERENT
   key, `legacyL3UnknownRate`; L3 excludes `MemberNotFound`/ambiguous cases the fresh
   engine counts as `Unknown`, so the two numbers are not directly comparable even when
   both are non-zero).
2. **Definition axis** (within the fresh resolver only): `realUnknownRate` (current
   authoritative definition — `ambiguousResolved` is a closed candidate set, not a hole,
   so it is excluded from `unknown`) vs. `realUnknownRateLegacyIncludingAmbiguous`
   (the PRE-reclassification definition, which counted `ambiguousResolved` as `unknown`
   too — reported side-by-side, additively, so a metric-definition change is never
   stat-juked).

## Testing Philosophy & Goldens

- **A test must pin the USE, not just the helper — and you must prove it can fail.**
  The recurring defect in this repo: a test calls a helper directly and asserts its
  behaviour, so the production **call site** can be deleted while the test, the whole
  suite and every golden stay green. The guard is gone and nothing says so. It appears
  when a change makes production code **unable to produce the precondition** the old
  test relied on, and the test gets "rescued" by pointing it at the function instead of
  the scenario — which looks like preserved coverage and is not.
  **Hand-state the precondition instead**: construct the input literally (two rows
  carrying the same id *by assignment*, not by asking `compute_routine_id` for a
  collision it can no longer produce). A test that states its precondition survives any
  schema change, because it never depends on production code to create it.
  **Then prove discrimination**: break the thing (delete the `.then_with`, flip
  first-wins to last-wins, restore the `unwrap_or_default`), watch the test fail, revert,
  watch it pass — and record both outcomes. This is not optional ceremony; in the arc
  that produced this rule, **five** instances were caught in review and **zero** by
  `cargo test`, and every real catch came with a discrimination proof while every miss
  lacked one. Watch too for the over-claim that travels with it — docs asserting guards
  are "each pinned executably" when only some are.
  **A discrimination proof that PASSES is evidence about the TEST, not the code.** Three
  times in the 2026-07-31 substrate arc a break came back green: once because `rustfmt`
  had reflowed the target text so a scripted patch matched nothing at all, once because
  `Vec::dedup` collapses only ADJACENT equals and the fixture carried no consecutive
  duplicate, and once because the contribution broken was redundant with a second code
  path. The first two were test/tooling defects and were fixed; the third was a real
  property of the code and was recorded as a stated LIMIT of that golden. Always assert
  that a scripted break actually applied (`assert s.count(old) == 1`) — an unasserted
  scripted break proves nothing, and its green run reads exactly like a passing test.
- **The al-sem TypeScript reference is RETIRED.** This engine began as a faithful Rust
  port of al-sem (TS), validated by byte-for-byte differential goldens. That era is over.
  The engine is now **Rust-owned**: correctness and resolution precision take priority
  over reproducing al-sem's output.
- **No byte-to-byte parity with al-sem.** Tests assert **Rust-owned baselines** (goldens
  regenerated from THIS engine) and **structural CONTRACTS** (invariants that hold
  regardless of which engine is "right" — e.g. "every `builtin` edge's method is in the
  catalog", "no edge is both `builtin` and `resolved`"). When the Rust engine is
  intentionally MORE accurate than al-sem (resolving record/framework built-ins, removing
  phantom uncertainties, etc.), that divergence is CORRECT — rebaseline the Rust-owned
  golden; do not chase al-sem.
- **We control all downstream consumers.** Every program consuming engine output (CLI
  formats, snapshots, fingerprints, SARIF, digests, prove/diff) is ours to change. Output
  shape may be refactored freely when it improves the product — update the consumers and
  their goldens together.
- **Goldens regen:** `REGEN_TEMP_GOLDENS=1 cargo test` rewrites Rust-owned goldens.
  Inspect the diff is intended before committing. Manifest "matrix" oracles hold
  Rust-owned numbers; update them to the current Rust value when the engine intentionally
  improves.
- **One change moves SEVERAL golden families — regen and run them together, never
  one at a time.** A change anywhere under `src/engine/`, `crates/al-syntax/src/`,
  `tests/r0-corpus/` (fixtures), `tests/fixtures/`, or any golden/vector dir can
  move any of the **30 golden directories**, which map to **9 test targets**:

  | Golden dir(s) | Test target |
  |------------|-------------|
  | `tests/r4-goldens/`, `tests/r4f-goldens/` | `--test r4` |
  | `tests/ir-l2-goldens/` (the `l2_features.snapshot`) | `--test l2_ir` |
  | `tests/cli-{a,b,c,c-events,c-policy,query}-goldens/`, `tests/gate-goldens/`, `tests/{al2,al}dump-smoke-goldens/` | `--test cli` |
  | `tests/r{0,1a,2a,2b,2c,2d}-goldens/` (r2c is the `l3eg` family, byte-compared only by `tests/differential.rs` — `--test l3`'s own `l3eg_oracles` is a *different*, non-golden check) | `--test differential` |
  | `tests/r3a{1,2,3,4,5}-goldens/` | `--test r3` |
  | `tests/r2-5a-goldens/`, `tests/r2-5b-{cg,cov,eg,rt}-goldens/` | `--test r25_abi` |
  | `tests/l4-summary-baseline/` | `--test l4_summary_differential` |
  | `tests/goldens/semantic-edges/` | `--test program_resolve_harness` |

  The ninth target, `--test l3` (l3cg/l3cov/l3eg/l3rt oracle + vectors), owns no
  byte-compared golden directory of its own — despite the name, `l3eg_oracles.rs`
  is explicitly NOT a golden diff. `scripts/check-goldens` still runs it
  `--no-fail-fast` alongside the eight golden-bearing targets above.

  Canonical loop: **`scripts/check-goldens --regen`** (regens all nine targets),
  inspect the diff, then **`scripts/check-goldens`** (no regen) to confirm green.
  The script is the source of truth for this table — keep the two in sync.
  Regenerating only the family you were looking at is the trap that shipped two red
  commits in the BCQuality wave (the `l2_features.snapshot` went stale) and again in
  the l3-substrate arc (`tests/r3a3-goldens/` moved while `check-goldens` did not
  cover `--test r3` at all). A **pre-commit hook** (`scripts/git-hooks/pre-commit`,
  enabled via `git config core.hooksPath scripts/git-hooks`) blocks a commit
  touching any of those paths unless `check-goldens` passes; enable it once per
  clone. It costs ~23s warm-cache when it fires, plus any debug rebuild.
- **A NEW `tests/r0-corpus/` fixture moves THREE golden families, and
  `--test r4` alone will not tell you.** Adding `ws-d2-uncertain` (2026-07-31) needed
  `tests/r4-goldens/*.r4.golden.json`, `tests/r2c-goldens/*.l3eg.golden.json` (the l3eg
  differential walks every corpus dir and flags any non-golden fixture that produces
  events) and `tests/ir-l2-goldens/l2_features.snapshot` (+1 line per routine). `--test
  r4` was GREEN while the repository was red; only `scripts/check-goldens` caught it.
  Also: **the golden-regen env var only REWRITES existing goldens, it cannot mint one.**
  `run_smoke_entry` asserts the golden file exists BEFORE the regen runs, so a new r4
  fixture needs a seed file (the projection shape, `findingCount` 0, empty `findings`)
  committed first, which the regen then fills. The l3eg regen does mint.
- **New advisory L5 detector = triage on a real workspace BEFORE shipping DEFAULT.**
  Run it on DO/CDO, triage every finding against real source (the `triage-findings`
  skill; or `/triage-wave` to fan out one subagent per detector). A detector with
  **> 30% false positives on its sample ships OPT-IN, not default.** When a detector
  is systematically wrong, **fix the root cause — do not demote-and-ship-broken**
  (the BCQuality wave found d56/d60/d63 broken on real code and fixed all three; only
  d56's residual key-remap shape kept it opt-in). Measure the population before
  building taxonomy for it.
- **Detector substrate gotcha — "is there a branch in this loop?" uses
  `statement_tree`, NOT `condition_references`.** `L3Routine.condition_references`
  (built by `ir_walk::collect_cond_idents`) records only PLAIN-identifier condition
  references — it misses a parenthesized condition (`if (X)`) and a quoted-field
  scrutinee (`case Rec."E-Mail" of`), exactly the shapes real BC code uses. Walk the
  control-flow tree `L3Routine.statement_tree` (`PCFNNode`; `if`/`case` kind nodes
  carry `source_range`) for a STRUCTURAL, shape-independent branch check — see d60's
  `tree_has_branch_within`. This bit d60 (two paren/quoted conditions survived the
  identifier-only guard) before the switch.
- **al-sem retirement is COMPLETE.** `U:\Git\al-sem` was archived to
  `al-sem-OBOLETE`; nothing in this repo reads from it or writes into it at test
  time, and zero tests point at it any more. Every differential/golden is
  Rust-owned and regenerable via `REGEN_TEMP_GOLDENS=1 cargo test` (see above).
- **CDO ratchet tests skip silently by default, but can be made to fail loudly.**
  The north-star zero-ratchets (real-unknown rate, unknown count, `ambiguousResolved`
  pin, coverage contract) live in tests gated on the `CDO_WS` env var pointing at a
  real Business Central workspace — a tree that only exists on machines with access
  to it, so CI cannot run them and they no-op (skip) when `CDO_WS` is unset. Setting
  `ENFORCE_CDO_WS=1` alongside makes every one of those gates panic instead of
  skipping when the workspace is missing, so a moved/lost `CDO_WS` fails loudly
  rather than silently passing (`tests/common/cdo.rs`). Run `scripts/cdo-gate
  <path-to-cdo-workspace>` (or `CDO_WS=<path> scripts/cdo-gate`) to run the full
  CDO-gated suite this way — it exports `ENFORCE_CDO_WS=1` itself and exits non-zero
  on any failure. The user schedules this locally (cron / Task Scheduler); it is not
  part of CI.

## Working Principle

**Always pursue the best solution — not the simplest, easiest, or quickest.** Time is not
a constraint and this project is not yet released, so refactoring is always on the table
and all downstream consumers are ours to change. Fix root causes, never symptoms: when a
golden or test disagrees with the code, find out WHY before rebaselining (a wrong golden
or half-finished feature gets fixed properly, not papered over). Prefer correct
architecture over a quick patch even when larger; verify by building/running/measuring,
not by assertion.

## Development Guidelines

- **CHANGELOG.md must be updated** after making any feature additions, bug fixes, or breaking changes
- Follow [Keep a Changelog](https://keepachangelog.com/) format
- Group changes under: Added, Changed, Deprecated, Removed, Fixed, Security
- **Format per-file with `rustfmt <file>`**, never `cargo fmt`. Stage only intended paths;
  never `git add -A`. Never push or merge to `master` without an explicit request.
