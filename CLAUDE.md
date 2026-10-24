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
cargo build --release          # Optimized release (full LTO, cu=1 — ~4 min for alsem; SLOW). Unwinds on panic,
                               # so the gates' release build and `perf_bounds` share artifacts; the SHIPPED
                               # binaries are `--profile release-ship` (= release + panic = "abort")
cargo build --profile release-fast  # Thin LTO — USE THIS for measurement/triage loops (alsem/aldump on DO);
                                    # full --release only for the SHA-pinned north-star measure, perf_bounds, benches
cargo build --profile cdo --bin aldump  # No LTO, incremental — for CORRECTNESS loops only (the CDO
                                    # unknown count, scripts/cdo-gate). ~1 min rebuild after an edit vs
                                    # ~2 min for release-fast. Never quote timings or memory from it.
cargo test                     # Run tests
cargo test -p al-sem --lib <filter>   # Package is al-sem (HYPHEN); al_sem fails. Renamed from
                                      # al-call-hierarchy 2026-08-07 — the BINARY keeps that name
rustfmt path/to/file.rs        # Format a file (NEVER `cargo fmt` — whole-crate churn)
scripts/ci-steps clippy        # Lint — CI's EXACT bar (release + -D warnings)
scripts/check-goldens          # Run ALL byte-compared golden families at once (see Testing Philosophy & Goldens)
scripts/ci-steps task          # The per-task gate (see Test tiers below)
scripts/ci-steps task-affected # `task` with only the tests the changed paths reach (agent inner loop)
cargo bench -p al-sem-benches --bench lsp_pipeline  # Benches are their own package (benches/Cargo.toml)
scripts/sweep-builds           # Delete build output unused for 7 days (main + al-sem-issue-* worktrees;
                               # needs `cargo install cargo-sweep`). Run after an issue, before a release,
                               # or when `df -h /u` is low: U: has filled up three times
```

`[profile.dev]` keeps only line-table debug info (dependencies none), which cut the dev/test
`.pdb` files that had grown to 19 GB. For a debugger session that needs local variables:
`CARGO_PROFILE_DEV_DEBUG=full cargo test …`.

### Test tiers

Every test still runs before a merge; the tiers decide HOW OFTEN each one runs.

| Tier | When | Command | What it runs |
|------|------|---------|--------------|
| 1. Inner loop | while editing | `cargo test -p al-sem --lib <module>`, `cargo test --test <umbrella> <member>::`; per agent task `scripts/ci-steps task-affected` | only what you touched (`task-affected`: fmt, dev clippy, gen-syntax, the lib tests and the test binaries the changed paths reach; an unmapped path runs the whole suite) |
| 2. Task gate | before every commit / per task | `scripts/ci-steps task` | `fmt`, clippy in the dev profile (`-D warnings`), `gen-syntax`, the WHOLE suite once — which already includes every `check-goldens` target |
| 3. Branch gate | once, before a merge | `scripts/ci-steps all` + `scripts/cdo-gate <CDO_WS>` | CI's exact bar (release clippy, release build, `perf_bounds`) plus the CDO suite |

- **The debug suites never run the CDO tests.** `ci-steps test`, `check-goldens` and
  the pre-commit hook unset `CDO_WS`, so the CDO-gated tests skip there whatever your
  environment says. In a debug build they cost ~5 minutes per run
  (`abi_ingestion_integrity_cdo_gate` alone took 243 s). `scripts/cdo-gate` is the one
  runner for them, built with the `cdo` profile (warm, after an edit: 236 s, against
  ~375 s on `release-fast`; measured 2026-10-07). **Do not run `cdo-gate` per commit**:
  while iterating, the CDO number you are moving comes straight from
  `target/cdo/aldump.exe --program-call-graph-stats` / `--dependency-bodies-stats`
  (~1 min build + seconds to run); `cdo-gate` is the branch gate. **Do not set `CDO_WS` globally** — it was
  set as a user environment variable until 2026-10-04 and made every `cargo test` a
  CDO run. The pinned path is `U:/Git/DO-cdo-baseline/Cloud`; pass it to `cdo-gate`.
- **The pre-commit hook trusts a green task gate.** A green `ci-steps test` writes a
  stamp of the tested tree (`scripts/tree-stamp`, stored at
  `git rev-parse --git-path gate-stamp`). The hook skips its golden run only when the
  commit's tree is IDENTICAL to that stamp; partial staging or any later edit runs the
  goldens as before. `fmt` and the coverage check never skip.
- Measured 2026-10-04 (dev machine, warm caches, no source edit between runs). Before,
  every task ran `ci-steps all` (636 s) + `check-goldens` (299 s) + `cdo-gate` (384 s)
  = **~22 min**. Now the task gate is **34 s** (65 s while `gen-syntax` still rewrote
  its files and forced a full recompile — fixed the same day), and the branch gate is
  `ci-steps all` (422 s) + `cdo-gate` (269 s) = **~11.5 min, once per branch**. A source
  edit adds its debug rebuild on top; that part was not measured.
- **`ci-steps test` and `check-goldens` run on cargo-nextest; `cdo-gate` does not.**
  Measured 2026-10-18 (warm, same test counts): the whole suite 46–50 s on nextest
  against 81–104 s on `cargo test`, the golden targets 27–31 s against 64–80 s. On
  `cdo-gate` nextest was slower (97–112 s against 72–84 s): it runs one process per
  test, and `program_resolve_harness` rebuilds its shared CDO context in each. Install
  it once with `cargo install cargo-nextest --locked`; the scripts refuse to run without it.
- **`post-merge` usually builds nothing.** When the merge is provably the tree the gates
  passed (spec `docs/superpowers/specs/2026-10-18-gate-speedups-design.md`), it closes
  on the gate-run records instead of rebuilding; `post-merge --full` forces the gates.

**Do not pipe a long-running gate/test through `| tail`** (e.g. `bash scripts/cdo-gate … | tail`): the pipeline's exit code is `tail`'s, not the command's, so a FAILURE reads as success. Redirect to a log file and `grep` it, or run it with `run_in_background` and read the output file.

## Running

There is no `--no-lsp` flag — `clap` rejects unknown flags outright. LSP server mode is
the default whenever `--project` is *not* given; passing `--project` alone switches to
CLI mode (index-and-report, or `--analyze` for the quality-diagnostics report). `--lsp`
forces LSP server mode at top precedence (it overrides `--project`, always starting the
server). `--analyze` without `--project` is a hard error. Other flags: `--no-watcher` (disable the file
watcher; rely on LSP change notifications instead), `--no-telemetry` (see `docs/telemetry.md`).
See `src/main.rs`'s `Args` (clap derive) for the authoritative flag list.

## Prerequisites

- Rust 1.75+
- tree-sitter-al grammar (currently **v4.4.1 + `main` at `c89c2ecb`**, see the Grammar
  section — the package version did not move for 463 commits, many of them shape changes): a plain, gitignored clone of
  `SShadowS/tree-sitter-al` at `tree-sitter-al/`, on its `main` branch. **It is NOT pinned
  and NOT a submodule** (it was one until 2026-10-03). Local builds and CI both build
  against `main`, so they always use the same grammar.
  - Set it up once: `git clone https://github.com/SShadowS/tree-sitter-al` at the repo root.
    Update it with `git -C tree-sitter-al pull`.
  - **When `main` moves, the build stops on purpose, locally and in CI alike**:
    `crates/al-syntax/build.rs` compares `node-types.json` with the committed
    `node-types.sha256` and panics on a mismatch. Do the steps in the `grammar-upgrade`
    skill (it sat 3 releases behind for 4 days once and left CI red the whole time).
  - **Git worktrees have no grammar clone** (it is gitignored). Set `TREE_SITTER_AL_PATH`
    to the main checkout's `tree-sitter-al/` — the `al-syntax` build script falls back to
    `../../tree-sitter-al` (relative to `crates/al-syntax`) when the env var is unset,
    which resolves only in the main checkout.
  - Removing a worktree: worktrees created while the grammar was a submodule make
    `git worktree remove` FAIL ("working trees containing submodules cannot be moved or
    removed") — verify the tree is clean/merged, then
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

### In a kraken tentacle (server1)

`.kraken/` runs al-sem remotely; `/work/src` holds the grammar and the CDO baseline
(`/work/src/cdo-baseline`). BC there is a **BC 28.4 on Linux sidecar**, reached as
`http://bc` (instance `BC`) through `bc-dev` in `.mcp.json`.
- **It is for checks and probes.** An engine fact recorded as "measured on BC 28" is still
  confirmed on a Windows Cronus: the image's `KNOWN-LIMITATIONS.md` lists real differences
  (RDLC, ICU collation, the hub test runner's resets).
- **Start it** with `kraken siphon start bc --no-wait`, then `kraken siphon status bc` until
  ready. A cold BC takes about 3 minutes, longer than the Bash tool's 120 s default. It
  stops itself after a quiet spell.

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
Every module above lives in `src/lsp/` (a library module) so
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
or consumed by `src/engine/gate` (the `analyze` CLI).
**`alsem analyze`'s detectors (`src/engine/l4`/`l5`) read the program engine's call
resolution** (B3 Phase A, `docs/superpowers/specs/2026-10-04-compact-graph-core-design.md`
§7): `src/program/model/program_calls.rs` (`attach_program_calls`) converts the resolved
edges into the model's call shape and returns the model (`Model`) with them in
`calls`, and since engine-switch S4 also the event graph in `events` (from the program
engine's subscription inventory, `program::model::events`). Since S9.6 both are
mandatory: the assembly entries return `ModelRows`, which only `attach_program_calls`
turns into a model, so no consumer can see a model without calls. The model (body facts, objects,
routines) is assembled from the program engine's parse (S2a/S2b). The r4/r4f detector
goldens use the same path (`assemble_and_resolve_workspace_with_program_calls`).
**Since engine-switch S6 every single-app consumer builds the same program-backed model**
(`program_calls::assemble_and_resolve_workspace_program`): `alsem prove`, `digest`,
`fingerprint`, `diff`/snapshot, `events fanout`/`chains`, `policy check`, `query`, the
HTML report, `compute_analyzer_diagnostics`, and `aldump`'s `--r3a1/2/3`,
`--r4-findings`, `--r4f-*` modes. **Since S7 the cross-app path is the program engine
too**: `program_calls::assemble_and_resolve_cross_app_program` builds ONE model of the
workspace plus the dependencies it requires (`FULL` profile; each dependency body
resolved from its own app, `ProgramContext::resolve_dependency_bodies`), and
`capability_cone::build_cross_app_base`, the R3a-4 artifacts
(`dep_artifacts_from_model`), `project_r3a5_cross_app`, `project_r4_findings_cross_app`
and `aldump --r3a4/--r3a5/--r4-findings-cross-app` all read it. With no dependency,
cross-app mode reports exactly what single-app mode does (a contract test).
**Since S8 `alsem analyze` is cross-app by default** (`--single-app` keeps the
workspace-only model). The cross-app model holds only the dependency code the
workspace demands (`program::resolve::demand`), one detector-context builder serves
both modes, and the cone follows every resolved call that dependency code makes
(object runs and calls between dependency apps included). The combined graph (solver,
detector traversals) still takes workspace callers only. **S9 deletes L3**: S9.6
removed every test, golden and `aldump` mode that measured L3 itself (`--l3-*`,
`--b3`, `--r2-5a-merged-index`, the `tests/l3` suite), then the engine. Spec:
`docs/superpowers/specs/2026-10-06-engine-switch-design.md`; plan:
`docs/superpowers/plans/2026-10-07-engine-switch-s9.md`.

**Rules that live in modules** (find the files by name; the code is the tour):
- `src/state_paths.rs` is the one place that knows both the current and pre-rename
  per-user state locations. **Read it before touching any `~/.al-sem/` path** — the
  read-fallback is what keeps an install that predates the 2026-08-07 rename working.
  `config.rs` reads `~/.al-sem/config.json` and `.al-sem.json`, falling back to the
  pre-rename `.al-call-hierarchy` spellings.
- `src/engine/l4/effect_query.rs` (`DbEffectQuery`): **read its module doc BEFORE building
  anything on it** — it states which question this substrate answers and which one
  `cone_derived.rs` already answers better (physical-table WRITES), so a second answer to
  the same question is not built.
- `src/program/impact_graph/` is the may-execute call graph behind `alsem graph` (JSONL,
  schema `al-sem.callgraph`), read by bc-dev-mcp's test selection. Its contract (hubs,
  `hiddenCallers`, pruning) is in `docs/superpowers/specs/2026-10-13-callgraph-export-design.md`;
  change the schema only together with bc-dev-mcp's parser.
- `src/program/model/` (`Model`, `ModelRoutine`, …) is built only by
  `program_calls::attach_program_calls`. Every graph build states a `BuildProfile`
  (`src/program/profile.rs`); there is no default.
- `src/lsp/custom.rs` holds the `al-preview://` URI parser (`parse_al_preview_uri`), also
  used by `lsp::handlers`'s ABI-symbol URI minting.
- `src/lsp/lens.rs`/`diagnostics.rs` share `routine_complexity_ir` and
  `is_framework_invocation_attribute` with `analysis.rs` (the `--analyze` metrics).
- `src/language.rs` holds legacy tree-sitter S-expression queries
  (`DEFINITIONS`/`CALLS`/`EVENT_SUBSCRIBERS`/`VARIABLES`) that are dead code (zero call
  sites). Keep them: `tree-sitter-al/queries/highlights.scm` and `tags.scm` (editor syntax
  highlighting, a separate consumer) still reference the same grammar node names.

**`crates/al-syntax`** is the **only** crate that touches tree-sitter: FFI grammar
binding (`language.rs`), the generated raw vocabulary + typed CST (`raw/generated/`),
the CST→IR lowerer (`lower/mod.rs`), and the owned AL syntax IR (`ir/`) every consumer
above builds on. See the Grammar section below.

**Testing:** Integration tests are consolidated into **umbrella crates** — one
`tests/<dir>/main.rs` per domain (`tests/{gap,cli,l2_ir,temp_state,r25_abi,r3,r4,lsp}/`),
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

Measured by `cargo bench -p al-sem-benches --bench lsp_pipeline` (Criterion; `benches/lsp_pipeline.rs`,
rewritten for the engine-backed LSP surface at T3 Task 16) against a deterministic
synthetic corpus (`tests/perf_support/`) — 1000 codeunits with real cross-file call
fan-in/fan-out for the query rows, exercising `LspSnapshot`/`lsp::handlers`/
`lsp::updater` (no legacy `Indexer` involved — that pipeline is deleted, see
Architecture above). A release-only CI gate (`tests/perf_bounds.rs`) asserts every
operation stays within 3x its target on every PR (rung 1/rung 2 additionally carry a
corpus-relative bound — see that file's own doc), so an order-of-magnitude regression
fails loudly even though the day-to-day numbers have wide headroom. Re-run the bench for
current numbers; do not quote old ones.

| Operation | Target |
|-----------|--------|
| `build_full` (100 files) | < 500ms |
| `build_full` (1000 files) | < 2s |
| `prepare` (prepareCallHierarchy) | < 1ms |
| `incoming` (1000-file graph, 999-way real fan-in) | ~25ms — see note below |
| `outgoing` (1000-file graph) | < 1ms |
| `compute_all` (diagnostics recompute, 1000-file event-bearing graph) | 50ms (architectural — see footnote below) |
| Incremental update, rung 1 (body-only edit, 1000-file graph) | 100ms (real-CDO-workspace re-measurement) |
| Incremental update, rung 2 (signature-change edit, 1000-file graph) | ~1.5s (real-CDO-workspace re-measurement) |

**Every rung-1/rung-2 save additionally pays `compute_all`'s own cost** — `server.rs`'s
`on_swap` runs the full diagnostics recompute after EVERY snapshot swap, including a
single-file rung-1 body edit, so the real per-save latency a user experiences is
rung-N's own number PLUS `compute_all`'s row above. `compute_all` used to carry a real O(decls × event_edges)
quadratic cost (found and fixed in a t3 whole-branch review pass — see
`src/lsp/snapshot.rs`'s `LspSnapshot::publisher_fanout` doc); the numbers
reflect the fix, measured on an event-bearing corpus (`tests/perf_support/`'s
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

## Grammar (tree-sitter-al v4.4.1 + main `c89c2ecb`)

**Current reality:** the grammar is **v4.4.1** by `tree-sitter-al/package.json`, but the
committed `node-types.sha256` matches `main` at **`c89c2ecb`** (2026-10-22), hundreds of
commits past the v4.4.1 tag commit (`7819df5`) with the package version unchanged. **The package
version is NOT a grammar identity any more** — name the commit. What each bump moved is in
CHANGELOG.md.

**Measure a grammar bump on BOTH sides — neither outcome is a rule.** A bump can move zero
test outcomes or several goldens. Run the full suite on the old grammar and the new one and
diff the per-test outcomes; that is the only way to attribute a moved golden to the grammar
rather than to whatever else is in your diff.
Nothing pins the grammar commit: local builds and CI (`.github/workflows/ci.yml`) both use
`SShadowS/tree-sitter-al` `main`, so a grammar change surfaces on the next build rather than
silently drifting. `crates/al-syntax` is the **only** crate
that links tree-sitter or walks its raw CST — every other consumer (`src/lsp/snapshot.rs`,
`src/program/body` and everything layered on it, `src/program/resolve`) reads the owned
AL syntax IR that `al-syntax`'s lowerer (`crates/al-syntax/src/lower/mod.rs`) produces.

Upgrading the grammar, and the notes for touching the lowerer: see the `grammar-upgrade` skill.

## Adding New AL Constructs

See the `add-al-construct` skill.

## Resolution Coverage

The honest taxonomy, as emitted by `aldump
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
(workspace-only edges) variants are emitted, each with `realUnknownRate = unknown / total`.
Re-measure with `scripts/cdo-gate <CDO_WS>`; never quote an old number as current.

## Project Direction & The Moat

The product's moat is **precise whole-program call-graph resolution** for AL. The
north-star metric is the **real-`unknown` edge rate** on real BC apps (measure with
`aldump --program-call-graph-stats <workspace>`, the FRESH clean-room resolver — no L3
oracle): drive it toward zero, where the residual is provably dynamic. See Resolution
Coverage above for the full honest taxonomy (`resolved`/`builtin`/`dynamic`/`external`/
`unknown`) and the call-graph resolution redesign spec
under `docs/superpowers/specs/`.

**The unknown rate has two definitions — do not conflate them.** (There used to be a
second, engine axis: `aldump --l3-call-graph-stats` reported the legacy L3 engine's
advisory `legacyL3UnknownRate`. Engine-switch S9.6 removed it with L3; only the fresh
resolver's numbers exist now.)
`realUnknownRate` is the current, authoritative definition: `ambiguousResolved` is a
closed candidate set, not a hole, so it is excluded from `unknown`.
`realUnknownRateLegacyIncludingAmbiguous` is the PRE-reclassification definition, which
counted `ambiguousResolved` as `unknown` too. Both are reported side by side, so a
metric-definition change is never stat-juked.

## Testing Philosophy & Goldens

- **Build profiles (spec 2026-10-04 §4).** Every graph build states a `BuildProfile`; there
  is no default. A memory saving is a profile choice, never a deletion: `FULL` keeps every
  fact. A tool that reads something must declare it.
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
  move any of the **23 golden directories**, which map to **8 test targets**:

  | Golden dir(s) | Test target |
  |------------|-------------|
  | `tests/r4-goldens/`, `tests/r4f-goldens/` | `--test r4` |
  | `tests/ir-l2-goldens/` (the `l2_features.snapshot`) | `--test l2_ir` |
  | `tests/cli-{a,b,c,c-events,c-policy,query}-goldens/`, `tests/gate-goldens/`, `tests/al2dump-smoke-goldens/` | `--test cli` |
  | `tests/r{0,1a,2a,2c,2d}-goldens/` (r2c is the event-graph family, projected from the program engine's event graph) | `--test differential` |
  | `tests/r3a{1,2,3,4,5}-goldens/` | `--test r3` |
  | `tests/l4-summary-baseline/` | `--test l4_summary_differential` |
  | `tests/goldens/semantic-edges/` | `--test program_resolve_harness` |

  The eighth target, `--test r25_abi`, owns no golden directory: it runs the
  `tests/r2-5a-vectors/` ABI ingestion oracles. Engine-switch S9.6 deleted the
  goldens that measured the legacy L3 engine (`r2b`, `r2-5a`, `r2-5b-*`,
  `aldump-smoke`, the `docs/b3-triage/` tables) and the `--test l3` suite with it.

  Canonical loop: **`scripts/check-goldens --regen`** (regens all eight targets),
  inspect the diff, then **`scripts/check-goldens`** (no regen) to confirm green.
  The script is the source of truth for this table — keep the two in sync.
  Regenerating only the family you were looking at is the trap that shipped two red
  commits in the BCQuality wave (the `l2_features.snapshot` went stale) and again in
  the l3-substrate arc (`tests/r3a3-goldens/` moved while `check-goldens` did not
  cover `--test r3` at all). A **pre-commit hook** (`scripts/git-hooks/pre-commit`,
  enabled via `git config core.hooksPath scripts/git-hooks`) blocks a commit
  touching any of those paths unless `check-goldens` passes; enable it once per
  clone. It costs ~23s warm-cache when it fires, plus any debug rebuild, and it
  skips the golden run when the exact tree already passed `scripts/ci-steps test`
  (see Test tiers).
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
- **Record every hole in Microsoft's `altool graph` in
  `docs/al-compiler-callgraph-holes.md`, in the same change that finds it.** The owner
  competes with Microsoft's AL tooling (alc, `altool graph`, the official AL LSP and MCP);
  each hole is a row in his feature matrix. A compiler/engine disagreement explained by an
  altool limit (a new `Verdict` in `compiler_golden.rs`, an "oracle cannot see X" limit, a
  bc-dev-mcp altool workaround) gets an entry with a repro, versions and how al-sem compares.
  Always name which side a tool is on.
- **New advisory L5 detector = triage on a real workspace BEFORE shipping DEFAULT.**
  Run it on DO/CDO, triage every finding against real source (the `triage-findings`
  skill; or `/triage-wave` to fan out one subagent per detector). `alsem analyze` is
  cross-app by default; `--single-app` leaves d43/d44/d45 with no findings on CDO
  (0, against 195 at `--scope primary` and 1380 at `--scope all`, measured 2026-10-13 on
  `U:/Git/DO-cdo-baseline/Cloud`). Measure event detectors in the default mode, at both
  `--scope primary` and `--scope all`. A detector with
  **> 30% false positives on its sample ships OPT-IN, not default.** When a detector
  is systematically wrong, **fix the root cause — do not demote-and-ship-broken**
  (the BCQuality wave found d56/d60/d63 broken on real code and fixed all three; only
  d56's residual key-remap shape kept it opt-in). Measure the population before
  building taxonomy for it.
- **Detector substrate gotcha — "is there a branch in this loop?" uses
  `statement_tree`, NOT `condition_references`.** `ModelRoutine.condition_references`
  (built by `ir_walk::collect_cond_idents`) records unquoted IDENTIFIER names only: a
  bare `X` or a member's `.Name`, looking through parentheses, calls and operators.
  It never records a QUOTED name, bare (`if "My Var"`) or as a member
  (`case Rec."E-Mail" of`), and real BC code quotes field names constantly. It also
  carries identifiers, not structure: it cannot tell you a branch exists. Walk the
  control-flow tree `ModelRoutine.statement_tree` (`PCFNNode`; `if`/`case` kind nodes
  carry `source_range`) for a STRUCTURAL, shape-independent branch check — see d60's
  `tree_has_branch_within`. This bit d60 (a quoted-field scrutinee survived the
  identifier-only guard) before the switch. `ir_walk`'s `condition_references_shapes`
  test pins exactly which shapes are recorded (#24).
- **al-sem retirement is COMPLETE.** `U:\Git\al-sem` was archived to
  `al-sem-OBOLETE`; nothing in this repo reads from it or writes into it at test
  time, and zero tests point at it any more. Every differential/golden is
  Rust-owned and regenerable via `REGEN_TEMP_GOLDENS=1 cargo test` (see above).
- **The CDO baseline is a PINNED, reproducible workspace — keep it that way.**
  `CDO_WS` must point at `U:/Git/DO-cdo-baseline/Cloud`: a git worktree of the DO
  repo parked DETACHED at `bc3ccb18`, with a clean tree and its own copy of
  `.alpackages`. It is deliberately separate from the day-to-day `U:/Git/DO/Cloud`
  checkout, which moves as work happens.
  The previous baseline was minted from a tree that was later copied out of its git
  repo, losing `.git`; its recorded mint SHA (`64643a2f`) then existed in no checkout
  at all, so nobody could tell "the engine regressed" from "the input changed". The
  concrete cost: the semantic audit paired ZERO sites against it and still reported a
  pass for two months. **A baseline you cannot reconstruct is not a baseline.**
  Every golden stamps `workspace_git_sha`, `workspace_dirty` AND
  `dependency_closure_sha256`. That third field exists because git state covers only
  TRACKED files — the symbol closure is gitignored, so a workspace can report
  `dirty: false` while the dependencies the resolver reads were swapped wholesale.
  Since #29 it is computed from the SAME discovery the loader uses
  (`dependencies::discover_app_files`): every `.app` in the workspace's own AND each
  ancestor `.alpackages` up to the git boundary, keyed by `../`-relative path,
  length-prefixed, tagged `closure-v2:<hex>` (`closure-v2:empty` for no packages).
  Under `ENFORCE_CDO_WS=1` any drift in these is a HARD FAILURE, not a warning —
  including a golden with no closure stamp, or a probe that cannot read a cache;
  ungated developer runs still only warn. Advancing the pin is deliberate: move the
  worktree, re-mint, re-triage. A digest-SCHEME change on an unmoved baseline is a
  re-stamp, not a re-mint: `mint-goldens --restamp` rewrites only the closure stamp
  and refuses if any golden's git stamp differs from the workspace.
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
- **Edit files with the built-in Edit / Write tools, never with a script.** No
  `python - <<EOF ... s.replace(...)`, `sed -i`, `perl -pi` or heredoc rewrites of
  source, tests, docs or CHANGELOG. A scripted edit is invisible to the user's
  review, bypasses the Read-before-Edit check, and silently does nothing when its
  pattern drifts (a `replace` that matches zero times still exits 0). This holds for
  discrimination proofs too: make the break with Edit, run the test, revert it with
  Edit. Bulk mechanical changes across many files are still one Edit per site (use
  `replace_all` within a file). Files a repo script generates (`gen-syntax`, golden
  regen, `rustfmt`) are the only exception.
- **Autonomous-flow exception to the merge rule.** The issue orchestrator
  (`/orchestrate` → `/issue`, executor `scripts/agentflow/`) may write to `master`
  in exactly two cases without a per-change request: (1) the gated squash-merge of
  an issue branch, only when `scripts/ci-steps all`, `scripts/check-goldens
  --verify-coverage`, `scripts/check-goldens`, `scripts/cdo-gate` (non-docs diffs),
  the findings register (both reviewers `accepted` every entry), and GitHub CI are
  all green and the attested base, head, and issue-body hash still match at merge
  time; (2) the validated revert of a commit the flow itself merged, after the
  post-merge check fails. A third, non-`master` exception: after a re-rebase,
  `--force-with-lease` on the flow's own issue branch is the only permitted force
  form. The flow never touches
  `.github/`, `scripts/`, `.claude/`, this file, `.gitignore`, `Cargo.toml` version
  fields, or the `tree-sitter-al` path; a diff that does is blocked. `master`
  carries no branch protection; adding it is a spec change for the flow. Kill
  switch: `.agent/HALT`.
