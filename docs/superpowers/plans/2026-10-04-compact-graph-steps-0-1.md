# Compact graph core, steps 0 and 1: measure the server, parse without the spike

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Measure the LSP server's true idle memory (step 0), then stop the first workspace root from holding every dependency's syntax tree at once (step 1), without losing any fact the `FULL` profile keeps.

**Architecture:** A `BuildProfile` struct says what a build must keep. Its first field, `dependency_bodies`, is `Summary` (the LSP server, `alsem`'s preflight count) or `Keep` (`alsem`, `aldump`, tools). Every dependency file is parsed, immediately summarized into the pack format's `PackedFile` (nodes, `RoutineMeta`, parse status), and its syntax tree dropped; `Keep` additionally retains the trees in the shared dependency tier. The dependency tier (`DepNodes`) now owns `dep_meta` and the recovered-file list, so every consumer reads dependency metadata from one place.

**Tech Stack:** Rust 2024, rayon (`big_stack_pool`), the repo's `DepCache`/`DepNodes`/`DeclSurface`/`PackedFile` types.

**Spec:** `docs/superpowers/specs/2026-10-04-compact-graph-core-design.md` (§3, §4, §5, §9, §11). Read §4 and §5 before any task.

## Global Constraints

- Plain English in docs and comments (CLAUDE.md). Short sentences.
- Format each touched file with `rustfmt <file>`; never `cargo fmt`. Stage only intended paths; never `git add -A`. Never stage `src/engine/deps/mod.rs` (a line-ending-only local change).
- Edit files with the Edit/Write tools only — no inline python or sed edits.
- `FULL` output must be byte-identical to today's `master` (goldens, CDO). No golden may move in this plan.
- Real-unknown rate stays 0 (`scripts/cdo-gate`).
- Per task: `scripts/ci-steps task` must pass before the commit. Before the final review: `scripts/ci-steps all` and `scripts/cdo-gate U:/Git/DO-cdo-baseline/Cloud`. Never pipe a gate through `| tail`; redirect to a log and grep it.
- Every new or changed test gets a discrimination proof: break the code it guards, watch it fail, revert, watch it pass, and record both outcomes in the commit message. Assert that a scripted break actually applied.
- `BuildProfile` has NO default and NO `Default` impl. Every call site states its profile.
- Never delete a fact the `FULL` profile keeps today (spec §2).
- Git Bash with Windows paths; never `2>nul`.

## Review Focus

1. **A dependency file whose parse is `Recovered`** must still appear in `ProgramReport::recovered_files` and `FreshCoverage::recovered_files` under BOTH profiles (CDO's Base Application has 5 such files on older grammars; the ratchet depends on this). Test in Task 6.
2. **Two roots with the same dependencies but different profiles in one `DepCache`** (LSP builds `LIGHT`, a tool builds `FULL` in the same process; and the reverse order; and concurrently) must each get exactly what they asked for — a `FULL` build must get dependency bodies even when a `LIGHT` tier is live. Test in Task 6.
3. **A sibling app that is both workspace multi-app source and an embedded dependency** (duplicate units, the case `dedup_routines_preserving_genuine_overloads` exists for) must produce the same deduped nodes and the same last-write-wins `dep_meta` from summaries as from today's path. Test in Task 4.
4. **A rung-3 rebuild** (dependency change) in the running server must keep working under `LIGHT`: it goes through `build_full_with_parsed_with_cache`. Covered by the existing updater tests, which must stay green unchanged (Task 6 checks them by name).
5. **`dependency_source = symbols`** (no dependency source at all): summaries are empty, ABI ingestion is unchanged, `dep_meta` is empty, and nothing panics. Test in Task 6.

---

## File structure

| File | Responsibility | Task |
|---|---|---|
| `src/census_hook.rs` (new) | No-op phase marks for the byte-census probe | 1 |
| `tools/census-probe/` (new, own crate, not a workspace member) | The census probe, kept in the repo so every step can re-run it | 2 |
| `docs/2026-10-04-step0-server-census.md` (new) | Step 0 measurement report | 2 |
| `src/program/profile.rs` (new) | `BuildProfile`, `DependencyBodies` | 3 |
| `src/program/dep_summary.rs` (new) | `summarize_file`, `DepUnitSummary`, the profiled parse | 4, 6 |
| `src/program/resolve/decl_surface.rs` | Shared per-file `RoutineMeta` helper (one code path) | 4 |
| `src/program/dep_cache.rs` | `DepNodes` owns `dep_meta`, `recovered`, optional `bodies`; `DepKey` carries the profile | 3, 5, 6 |
| `src/program/build.rs` | Dependency nodes from summaries; dedup moves instead of clones | 5, 6 |
| `src/program/resolve/full.rs` | `ProgramContext` (workspace-only `parsed`, `dep_bodies()`), profiled entry points, two-tier surface | 3, 5, 6 |
| `src/lsp/snapshot.rs` | `dep_texts` from the snapshot; `LIGHT`; no dependency arena drop | 5, 6 |
| `src/program/resolve/differential.rs`, `semantic_golden.rs` | Use the context's surface helper | 5 |
| `CLAUDE.md`, `CHANGELOG.md`, spec §11 | Docs | 2, 7 |

---

## Task 1: Phase-mark hook on master

The census probe marks build phases through a hook. It exists only on the scratch branch `census/graph-bytes` (commit `37beb3f1`). Move it into `master` as a permanent no-op so the probe works on every later step.

**Files:**
- Create: `src/census_hook.rs`
- Modify: `src/lib.rs` (add `pub mod census_hook;` after `pub mod capped_io;`)
- Modify: `src/program/resolve/full.rs` (4 marks), `src/lsp/snapshot.rs` (5 marks)

**Interfaces:**
- Produces: `al_sem::census_hook::HOOK: OnceLock<fn(&'static str)>`, `al_sem::census_hook::mark(name: &'static str)`. Phase names, in order: `"1.snapshot"`, `"2.parse"`, `"3.dep_layer"`, `"4.assemble_graph"`, `"5.index+surface+dep_meta+dep_texts"`, `"6.resolve_workspace_files"`, `"7.event_edges"`, `"8.incoming+decl_by_id"`, `"9.publish_snapshot"`. These strings are the probe's contract — keep them exact.

- [ ] **Step 1: Write the hook module**

```rust
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
```

- [ ] **Step 2: Add the marks** exactly where commit `37beb3f1` put them (`git show 37beb3f1 -- src/program/resolve/full.rs src/lsp/snapshot.rs` shows the 9 lines and their anchors): in `build_context_with` after the snapshot is built (`"1.snapshot"`); in `build_context_from_snapshot_cached` after the parse (`"2.parse"`), after `drop(shared_tier)` (`"3.dep_layer"`), after `assemble_program_graph` (`"4.assemble_graph"`); in `LspSnapshot::from_context` after `dep_texts = ...` (`"5.index+surface+dep_meta+dep_texts"`), after the per-file resolve loop (`"6.resolve_workspace_files"`), after `event_edges` is built (`"7.event_edges"`), after `build_decl_by_id` (`"8.incoming+decl_by_id"`), and just before the `dep-arena-drop` spawn (`"9.publish_snapshot"`).

- [ ] **Step 3: Write the test** (in `src/census_hook.rs`). The hook is process-global and other tests build snapshots concurrently, so the recorder keeps only marks from the test's own thread (all 9 marks run on the calling thread).

```rust
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
                "5.index+surface+dep_meta+dep_texts",
                "6.resolve_workspace_files",
                "7.event_edges",
                "8.incoming+decl_by_id",
                "9.publish_snapshot",
            ]
        );
    }
}
```

- [ ] **Step 4: Run it, then prove discrimination**

Run: `cargo test -p al-sem --lib census_hook` — Expected: PASS.
Then delete the `"7.event_edges"` mark line, rerun (Expected: FAIL, the list lacks `7.event_edges`), restore it, rerun (PASS). Record both outcomes.

- [ ] **Step 5: Gate and commit**

Run `scripts/ci-steps task > <scratch>/t1.log 2>&1; echo rc=$?` — Expected rc=0.
```bash
git add src/census_hook.rs src/lib.rs src/program/resolve/full.rs src/lsp/snapshot.rs
git commit -m "feat: phase-mark hook for the byte-census probe (no-op unless registered)"
```

---

## Task 2: Probe in the repo, and the step 0 measurement

The probe lives in the session scratchpad and would be lost. Move it into the repo, add an "updaters running" mode, and measure the running server (spec §9 step 0).

**Files:**
- Create: `tools/census-probe/Cargo.toml`, `tools/census-probe/src/main.rs` (copied from `C:/Users/SShadowS/AppData/Local/Temp/claude/U--Git-al-call-hierarchy/e9b369c8-ac7c-4abe-91bb-e823ec61a546/scratchpad/census-probe/`), `tools/census-probe/README.md`
- Create: `docs/2026-10-04-step0-server-census.md`
- Modify: `docs/superpowers/specs/2026-10-04-compact-graph-core-design.md` §11 (write in the measured retained target)

**Interfaces:**
- Consumes: `al_sem::census_hook` (Task 1); `al_sem::lsp::updater::{SharedSnapshot, spawn_updater, ChangeEvent}`; `LspSnapshot::build_full_with_parsed_with_cache`.

- [ ] **Step 1: Copy the probe.** In `tools/census-probe/Cargo.toml` keep the path dependency on the repo (`al-sem = { path = "../.." }`) and add an empty `[workspace]` table so the probe is its own workspace (it must not join the repo's workspace or CI). Build it with `CARGO_TARGET_DIR=C:/lpt`, never the repo's `target/`. `README.md`: three lines — what it measures, the build command, the corpora (CG harness, CDO), and that its accounting conventions are `.claude/skills/byte-census/SKILL.md`'s.

- [ ] **Step 2: Add `--with-updaters`.** After building each root exactly as today, start that root's updater as `src/server.rs:597-629` does, with a no-op `on_swap`:

```rust
let shared = std::sync::Arc::new(al_sem::lsp::updater::SharedSnapshot::new(
    std::sync::Arc::new(snapshot),
));
let (tx, rx) = std::sync::mpsc::channel::<al_sem::lsp::updater::ChangeEvent>();
let handle = al_sem::lsp::updater::spawn_updater(
    std::sync::Arc::clone(&shared),
    rx,
    root.to_path_buf(),
    workspace_unit,
    source,
    std::sync::Arc::clone(&cache),
    |_, _| {},
);
roots.push((shared, tx, handle)); // keep all alive
```

Wait until the heap stops moving (the probe's existing 0.9 s settle rule), then record "retained, all updaters idle". Report per-root deltas and the total. Afterwards drop every `tx` (each updater returns when its channel closes) and join the handles. If `spawn_updater`'s signature differs from the one above, follow the code, not this plan, and note it in the report.

- [ ] **Step 3: Measure.** Corpora: the CG harness 7-root set the census used (see `census-report.md`'s corpus table) and CDO (`U:/Git/DO-cdo-baseline/Cloud`), both `embedded` and `symbols`. For each: build peak, retained without updaters (must match the census within noise — a sanity check), retained with updaters idle, and the per-root updater share split into `ResolveIndex`, `DeclSurface` and the object map if the probe can reach them (they are `pub(crate)`; if not, report the per-root delta and say so). Also one process RSS reading per run, labelled as context.

- [ ] **Step 4: Write `docs/2026-10-04-step0-server-census.md`**: conventions header (heap vs RSS, MiB, probe commit), tables, and one paragraph: the true idle baseline and what the shared updater indexes (spec §6) would remove. Then fill spec §11's retained target with the measured number ("snapshots' ~260 MiB plus one shared copy of the dependency updater indexes" becomes a number).

- [ ] **Step 5: Audit and commit.** Dispatch the `measurement-auditor` agent on the report before committing. Then:
```bash
git add tools/census-probe docs/2026-10-04-step0-server-census.md docs/superpowers/specs/2026-10-04-compact-graph-core-design.md
git commit -m "measure: step 0 — the running server's idle memory, census probe kept in tools/"
```

---

## Task 3: `BuildProfile`, plumbed everywhere, no behaviour change

**Files:**
- Create: `src/program/profile.rs`; Modify: `src/program/mod.rs` (`pub mod profile;`)
- Modify: `src/program/resolve/full.rs`, `src/program/dep_cache.rs`, `src/program/build.rs`, `src/lsp/snapshot.rs`, every caller listed in Step 3

**Interfaces:**
- Produces:
  - `al_sem::program::profile::{BuildProfile, DependencyBodies}` with `BuildProfile::FULL`, `BuildProfile::LIGHT`.
  - `build_context_with(workspace_root: &Path, dependency_source: DependencySource, profile: BuildProfile, dep_cache: &DepCache) -> Option<ProgramContext>`
  - `build_context_from_snapshot_cached(snap: AppSetSnapshot, profile: BuildProfile, dep_cache: &DepCache) -> Result<ProgramContext, String>`
  - `build_context_from_snapshot(snap: AppSetSnapshot, profile: BuildProfile) -> Result<ProgramContext, String>`
  - `build_context(workspace_root)` and `build_context_res(workspace_root)` keep their signatures and mean `FULL` (they are the CLI/tool entry points).
  - `DepKey::of(snap: &AppSetSnapshot, profile: BuildProfile) -> DepKey`
  - `ProgramContext::profile() -> BuildProfile`

- [ ] **Step 1: Write the type**

```rust
//! What a graph build must keep (spec §4).
//!
//! Each tool states what it needs; nothing is chosen by default. `FULL` keeps
//! every fact the engine can produce and is today's behaviour exactly. Smaller
//! profiles may skip a fact, never change one.

/// Dependency syntax trees (the bodies of dependency routines).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DependencyBodies {
    /// Keep a per-file summary (nodes, `RoutineMeta`, parse status) and drop
    /// each tree as soon as it is summarized.
    Summary,
    /// Keep the summaries AND every tree, shared in the dependency tier, so an
    /// analysis can walk dependency code.
    Keep,
}

/// What a build must keep. Deliberately no `Default`: every call site states it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BuildProfile {
    pub dependency_bodies: DependencyBodies,
}

impl BuildProfile {
    /// Everything. `alsem`, `aldump` and every tool unless it declares less.
    pub const FULL: BuildProfile = BuildProfile {
        dependency_bodies: DependencyBodies::Keep,
    };
    /// The LSP server's needs.
    pub const LIGHT: BuildProfile = BuildProfile {
        dependency_bodies: DependencyBodies::Summary,
    };
}
```

- [ ] **Step 2: Thread it through.** Add the `profile` parameter to the functions in Interfaces; store it in `ProgramContext` (new field `pub(crate) profile: BuildProfile` plus the `profile()` accessor). Add `keep_bodies: bool` to `DepKey` (set from `profile.dependency_bodies == DependencyBodies::Keep`) and pass the profile into `DepKey::of`. `build_dep_layer_cached` gains a `profile: BuildProfile` parameter, placed before `dep_cache` — `build_dep_layer_cached(snap, abi_cache, parsed, profile, dep_cache)` — and uses it for the key (Task 6 replaces `parsed` with `DepInput` in the same position); `build_dep_layer` (the cache-less public wrapper) passes `BuildProfile::FULL`. In this task the profile changes nothing else.

- [ ] **Step 3: Choose each caller's profile explicitly**
  - `LspSnapshot::build_full_with_cache` and `build_full_with_parsed_with_cache` → `BuildProfile::LIGHT` (the LSP server, `main.rs`'s CLI index mode and the updater's rung-3 rebuild all go through these).
  - `fresh_coverage` → `BuildProfile { dependency_bodies: DependencyBodies::Summary }` written out in full with a comment: it reads no dependency body; it DOES read edge details, which a later step will declare (spec §4).
  - `build_context`, `build_context_res`, `build_context_from_snapshot` callers in `abi_check.rs`, `differential.rs`, `semantic_golden.rs`, `full.rs` (`resolve_full_program*`), tests and benches → `BuildProfile::FULL`.
  - `dep_cache.rs` tests: use the profile the test is about; default to `LIGHT` where the test exercises the LSP path.

- [ ] **Step 4: Test that profiles split the cache**

```rust
#[test]
fn different_profiles_do_not_share_a_dep_tier() {
    let fx = two_roots_one_alpackages();
    let cache = DepCache::default();
    let light = crate::program::resolve::full::build_context_with(
        &fx.root_a, DependencySource::Embedded, BuildProfile::LIGHT, &cache,
    ).expect("light");
    let full = crate::program::resolve::full::build_context_with(
        &fx.root_b, DependencySource::Embedded, BuildProfile::FULL, &cache,
    ).expect("full");
    assert!(!Arc::ptr_eq(&light.dep_layer.dep_nodes, &full.dep_layer.dep_nodes));
}
```

Discrimination: drop `keep_bodies` from `DepKey`'s fields → the two tiers become one → FAIL. Restore → PASS.

- [ ] **Step 5: Gate and commit.** `scripts/ci-steps task` rc=0, then `scripts/check-goldens` shows no golden moved.
```bash
git add src/program/profile.rs src/program/mod.rs src/program/resolve/full.rs src/program/dep_cache.rs src/program/build.rs src/lsp/snapshot.rs
git status --short   # then `git add` each caller file Step 3 changed, by name (never -A)
git commit -m "feat: BuildProfile, stated at every graph build; no behaviour change"
```

---

## Task 4: The per-file summary, from one code path

**Files:**
- Create: `src/program/dep_summary.rs`; Modify: `src/program/mod.rs` (`pub mod dep_summary;`)
- Modify: `src/program/resolve/decl_surface.rs`

**Interfaces:**
- Produces:
  - `decl_surface::file_routine_meta(app: AppRef, file: &AlFile, virtual_path: &str) -> Vec<(RoutineNodeId, RoutineMeta)>` — `pub(crate)`.
  - `dep_summary::summarize_file(app: AppRef, tier: TrustTier, virtual_path: &str, file: &AlFile) -> PackedFile`
  - `dep_summary::DepUnitSummary { pub app: AppId, pub files: Vec<PackedFile> }`

- [ ] **Step 1: Extract the helper** in `decl_surface.rs`, and make `DeclSurface::build` and `build_split` call it (so today's two loops and the summary share ONE body):

```rust
/// Every routine declared in `file`, keyed as `DeclSurface` keys it. The ONE
/// place that turns a file's declarations into `RoutineMeta`: `build`,
/// `build_split` and the dependency summaries all call it, so they cannot drift.
pub(crate) fn file_routine_meta(
    app: AppRef,
    file: &al_syntax::ir::AlFile,
    virtual_path: &str,
) -> Vec<(RoutineNodeId, RoutineMeta)> {
    let mut out = Vec::new();
    for obj in &file.objects {
        let key = match obj.id {
            Some(n) => ObjKey::Id(n),
            None => ObjKey::Name(obj.name.fold_identifier()),
        };
        let obj_id = ObjectNodeId { app, kind: obj.kind, key };
        for routine in &obj.routines {
            out.push((
                source_routine_node_id(obj_id.clone(), routine),
                RoutineMeta::from_decl(routine, virtual_path),
            ));
        }
    }
    out
}
```

In `build`: replace the inner `for obj ... for routine ...` loop with `for (r_id, meta) in file_routine_meta(app_ref, &pf.file, &pf.virtual_path) { local.insert(r_id, meta); }`. In `build_split`: the same, inserting into `local` or `dep` by `is_primary`. Insertion order is unchanged, so last-write-wins is unchanged.

- [ ] **Step 2: Write `summarize_file`**

```rust
//! Per-file dependency summaries (spec §5): what the engine needs from a
//! dependency file, produced right after parsing so the syntax tree can go.
//! The format is the pack spec's `PackedFile`; nothing here persists it.

use al_syntax::ir::{AlFile, ParseStatus};

use crate::program::node::AppRef;
use crate::program::node_extract::extract_nodes;
use crate::program::pack::PackedFile;
use crate::program::resolve::decl_surface::file_routine_meta;
use crate::snapshot::{AppId, TrustTier};

/// One dependency app's summaries, in the app's file order.
pub struct DepUnitSummary {
    pub app: AppId,
    pub files: Vec<PackedFile>,
}

/// Summarize one parsed dependency file. Uses the SAME extraction
/// (`extract_nodes`) and the SAME `RoutineMeta` helper as the tree-based path,
/// so a summary carries exactly what today's build reads from the tree.
#[must_use]
pub fn summarize_file(app: AppRef, tier: TrustTier, virtual_path: &str, file: &AlFile) -> PackedFile {
    let mut objects = Vec::new();
    let mut routines = Vec::new();
    extract_nodes(app, file, tier, &mut objects, &mut routines);
    PackedFile {
        virtual_path: virtual_path.to_string(),
        parse_status_recovered: file.parse_status == ParseStatus::Recovered,
        objects,
        routines,
        routine_meta: file_routine_meta(app, file, virtual_path),
    }
}
```

(`recovered_file_paths` today tests `== ParseStatus::Recovered`; keep that exact predicate.)

- [ ] **Step 3: Equality tests** in `dep_summary.rs`. Build a snapshot with one dependency app holding three files (one with an overload pair, one with a `Recovered` parse — reuse `RECOVERED_SRC`'s unbalanced `#if` shape — one plain), parse it with `parse_snapshot`, and assert for each file: `summarize_file(..).objects/routines` equal what `extract_nodes` puts in fresh vectors for the same file, `routine_meta` equals `file_routine_meta(..)`, and `parse_status_recovered` is true exactly for the broken file. Add Review Focus 3's test here: the multi-app sibling fixture from `build.rs`'s layer-split tests (an app present as both workspace multi-app source and an embedded dependency) — summarize every dependency file, concatenate in unit/file order, run Step 4's sort + `dedup_routines_preserving_genuine_overloads`, and assert the result equals `build_dep_layer(..).dep_routines`; build `dep_meta` by inserting every summary's `routine_meta` in order and assert it equals `DeclSurface::build_split(..)`'s frozen map.

Discrimination: make `summarize_file` skip `routine_meta` for the last routine of each file → the meta assertion FAILS; restore → PASS. Make it read `ParseStatus::Clean` inverted → the recovered assertion FAILS; restore → PASS.

- [ ] **Step 4: Gate and commit.** `scripts/ci-steps task` rc=0; no golden moved.
```bash
git add src/program/dep_summary.rs src/program/mod.rs src/program/resolve/decl_surface.rs
git commit -m "feat: per-file dependency summary (PackedFile) from the same extraction path"
```

---

## Task 5: The dependency tier owns `dep_meta` and the recovered list

Behaviour-preserving restructure: still parse everything, but every consumer reads dependency metadata from `DepNodes`, and `dep_texts` comes from the snapshot. After this task nothing outside the dependency-tier build reads dependency `ParsedUnit`s.

**Files:**
- Modify: `src/program/dep_cache.rs`, `src/program/build.rs`, `src/program/resolve/full.rs`, `src/lsp/snapshot.rs`, `src/program/resolve/differential.rs`, `src/program/resolve/semantic_golden.rs` (only if it reads dependency units — check), `src/snapshot/parse.rs`

**Interfaces:**
- Produces:
  - `DepNodes { objects, routines, abi_ingest_errors, pub dep_meta: Arc<DepMetaMap>, pub recovered: Vec<String>, pub bodies: Option<Arc<Vec<ParsedUnit>>>, lsp }` — `bodies` is always `None` in this task (Task 6 fills it).
  - `DepLspTier { pub dep_texts: Arc<DepTexts> }` (no `dep_meta` any more).
  - `ProgramContext::decl_surface(&self) -> DeclSurface` = `DeclSurface::build(&self.graph, workspace units).with_frozen(Arc::clone(&self.dep_layer.dep_nodes.dep_meta))`.
  - `ProgramContext::recovered_files(&self) -> Vec<String>`: dependency tier's `recovered` plus the workspace unit's recovered paths, sorted (same format as today: `"<app name>::<virtual path>"`).
  - `lsp::snapshot::build_dep_texts(snap: &AppSetSnapshot, apps: &AppRegistry, primary: AppRef) -> DepTexts`.

- [ ] **Step 1: Build `dep_meta` and `recovered` with the nodes.** In `build_dep_nodes`, while iterating the non-primary units, summarize each file with `summarize_file` (Task 4), extend `objects`/`routines` from the summary, insert its `routine_meta` into a `DepMetaMap` in order, and push `"<unit.app.name>::<virtual_path>"` to `recovered` when flagged. Sort `recovered` at the end. Keep Steps 2b and 4 unchanged.

- [ ] **Step 2: Read the frozen tier everywhere.**
  - `resolve_full_program_from_parts`: take a `&DeclSurface` built by the caller instead of building `DeclSurface::build(graph, parsed)`; `resolve_full_program_with` and `resolve_full_program_for_export` pass `ctx.decl_surface()`.
  - `resolve_full_program_with`: `recovered_files: ctx.recovered_files()`.
  - `differential.rs::project_fresh_event_rows_on`: `let surface = ctx.decl_surface();`.
  - `LspSnapshot::from_context`: one branch only — `let surface = DeclSurface::build(&graph, ws_units).with_frozen(Arc::clone(&dep_layer.dep_nodes.dep_meta));` and `let tier = dep_layer.dep_nodes.lsp.get_or_init(|| Arc::new(DepLspTier { dep_texts: Arc::new(build_dep_texts(&snap, &graph.apps, primary_app_ref)) }));`. `dep_meta` for the snapshot is `Arc::clone(&dep_layer.dep_nodes.dep_meta)`.
- [ ] **Step 3: `dep_texts` from the snapshot**

```rust
/// Dependency file texts by `(app, virtual path)`, read from the snapshot's
/// source files (the same `Arc<str>`s, never copies). Needs no parse.
pub(crate) fn build_dep_texts(snap: &AppSetSnapshot, apps: &AppRegistry, primary: AppRef) -> DepTexts {
    let mut dep_texts = DepTexts::new();
    for unit in &snap.apps {
        let (Some(app_ref), Some(source)) = (apps.find(&unit.id), unit.source.as_ref()) else {
            continue;
        };
        if app_ref == primary {
            continue;
        }
        for f in source.files.iter() {
            dep_texts
                .entry((app_ref, f.virtual_path.clone()))
                .or_insert_with(|| Arc::clone(&f.text));
        }
    }
    dep_texts
}
```

Before replacing the old `build_dep_texts(&graph, &parsed, primary)`, read its body: if it used `insert` (last wins) instead of `entry().or_insert` (first wins), use the same rule here. Byte-identical is the bar.

- [ ] **Step 4: `DepCache::get` no longer needs the LSP tier.** A live entry now always carries `dep_meta` and `recovered`, and `dep_texts` can be built from any snapshot, so `get` returns any live entry: remove the `.filter(|nodes| nodes.lsp.get().is_some())` and update its doc. Update the `dep_cache.rs` tests whose names assert the old rule (`a_shared_tier_hit_parses_only_the_workspace`, `dep_lsp_tier_after_the_first_root_is_dropped_is_fresh_and_correct`, `roots_share_dep_meta_and_dep_texts`) to the new rule, keeping what each test is FOR: a hit parses only the workspace; a fresh tier after the last root drops is correct; roots share `dep_meta` (now via `dep_nodes.dep_meta`) and `dep_texts`.

- [ ] **Step 5: Equality and gates.** `scripts/ci-steps task` rc=0; `scripts/check-goldens`: no golden moved; `scripts/cdo-gate U:/Git/DO-cdo-baseline/Cloud` PASS (this task changes how every resolution reads dependency metadata, so run CDO now, not only at the end). Add one test: on the multi-app fixture, `ctx.decl_surface()` resolves every dependency routine id to the same `RoutineMeta` that `DeclSurface::build(&ctx.graph, <all units parsed>)` did before (build the old surface from a fresh `parse_snapshot` in the test). Discrimination: build `dep_meta` from all units but the last → FAIL; restore → PASS.

- [ ] **Step 6: Commit**
```bash
git add src/program/dep_cache.rs src/program/build.rs src/program/resolve/full.rs src/lsp/snapshot.rs src/program/resolve/differential.rs src/snapshot/parse.rs
git commit -m "refactor: the dependency tier owns dep_meta and recovered files; dep_texts from the snapshot"
```

---

## Task 6: Parse, summarize, drop — `Summary` and `Keep`

**Files:**
- Modify: `src/program/dep_summary.rs`, `src/program/build.rs`, `src/program/resolve/full.rs`, `src/lsp/snapshot.rs`, `src/program/dep_cache.rs`, `src/snapshot/parse.rs`

**Interfaces:**
- Consumes: Tasks 3-5.
- Produces:
  - `dep_summary::BuildParse { pub workspace: Option<ParsedUnit>, pub dep_summaries: Vec<DepUnitSummary>, pub dep_bodies: Option<Vec<ParsedUnit>> }`
  - `dep_summary::parse_for_build(snap: &AppSetSnapshot, profile: BuildProfile, skip_dependencies: bool) -> BuildParse`
  - `build_dep_layer_cached(snap, abi_cache, input: DepInput<'_>, profile, dep_cache)` where `pub enum DepInput<'a> { Parsed(&'a [ParsedUnit]), Built(BuildParse) }` — `Parsed` keeps the old public entry points (`build_dep_layer`, `build_program_graph_from_parsed`) working for benches and tests by summarizing from the given trees.
  - `ProgramContext.parsed` holds ONLY the workspace unit, always. New `ProgramContext::dep_bodies(&self) -> Option<&[ParsedUnit]>` (`Some` exactly when the profile is `Keep`).
  - `snapshot::parse::parse_file(unit: &AppUnit, f: &SourceFile) -> ParsedFile` — `pub(crate)`, factored out of `parse_unit_in_pool` (one parse code path).

- [ ] **Step 1: The profiled parse**

```rust
/// What a build parse produced (spec §5).
pub struct BuildParse {
    pub workspace: Option<ParsedUnit>,
    pub dep_summaries: Vec<DepUnitSummary>,
    /// `Some` only under `DependencyBodies::Keep`.
    pub dep_bodies: Option<Vec<ParsedUnit>>,
}

/// Parse `snap` for a graph build. Each dependency file is parsed and
/// summarized in the same parallel task; under `Summary` its tree is dropped
/// right there, so no more trees are alive than there are worker threads.
/// `skip_dependencies` (a shared-tier hit) parses only the workspace.
#[must_use]
pub fn parse_for_build(snap: &AppSetSnapshot, profile: BuildProfile, skip_dependencies: bool) -> BuildParse {
    parse_for_build_observed(snap, profile, skip_dependencies, None)
}

/// Counts dependency trees alive at once, for the bound test. Production
/// passes `None`; the test passes a counter, so it watches the real code path.
pub(crate) struct LiveTrees {
    pub(crate) live: std::sync::atomic::AtomicUsize,
    pub(crate) peak: std::sync::atomic::AtomicUsize,
}

pub(crate) fn parse_for_build_observed(
    snap: &AppSetSnapshot,
    profile: BuildProfile,
    skip_dependencies: bool,
    observe: Option<&LiveTrees>,
) -> BuildParse {
    use rayon::prelude::*;
    use std::sync::atomic::Ordering::SeqCst;

    let keep = profile.dependency_bodies == DependencyBodies::Keep;
    let mut apps = AppRegistry::default();
    let refs: Vec<AppRef> = snap.apps.iter().map(|u| apps.intern(&u.id)).collect();

    crate::big_stack::big_stack_pool().install(|| {
        let mut workspace = None;
        let mut dep_summaries = Vec::new();
        let mut dep_bodies = keep.then(Vec::new);
        for (unit, &app_ref) in snap.apps.iter().zip(&refs) {
            let Some(source) = unit.source.as_ref() else { continue };
            if unit.id == snap.workspace_app {
                workspace = crate::snapshot::parse::parse_unit(unit);
                continue;
            }
            if skip_dependencies {
                continue;
            }
            let per_file: Vec<(PackedFile, Option<ParsedFile>)> = source
                .files
                .par_iter()
                .map(|f| {
                    let pf = crate::snapshot::parse::parse_file(unit, f);
                    if let Some(o) = observe {
                        let now = o.live.fetch_add(1, SeqCst) + 1;
                        o.peak.fetch_max(now, SeqCst);
                    }
                    let summary = summarize_file(app_ref, pf.provenance.tier, &pf.virtual_path, &pf.file);
                    let kept = if keep { Some(pf) } else { drop(pf); None };
                    if let Some(o) = observe {
                        o.live.fetch_sub(1, SeqCst);
                    }
                    (summary, kept)
                })
                .collect();
            let (files, trees): (Vec<_>, Vec<_>) = per_file.into_iter().unzip();
            dep_summaries.push(DepUnitSummary { app: unit.id.clone(), files });
            if let Some(bodies) = dep_bodies.as_mut() {
                bodies.push(ParsedUnit { app: unit.id.clone(), files: trees.into_iter().flatten().collect() });
            }
        }
        BuildParse { workspace, dep_summaries, dep_bodies }
    })
}
```

Notes for the implementer: `extract_nodes` takes the tier from the unit's provenance today (`pf.provenance.tier` in `build_dep_nodes`) — keep exactly that. Under `Keep` the "live" counter does not drop (trees are retained); the bound test runs `Summary` only.

- [ ] **Step 2: The dependency layer from summaries.** `build_dep_nodes` takes `DepInput`. For `Built(parse)`: consume `parse.dep_summaries` in order — `objects.extend(file.objects)`, `routines.extend(file.routines)` (moves), insert `routine_meta` pairs into `dep_meta` in order, collect `recovered`; store `parse.dep_bodies.map(Arc::new)` in `DepNodes.bodies`. For `Parsed(units)`: summarize each non-primary file from the given tree (Task 5's code), `bodies: None`. The `DepNodes` built under `Keep` are cached under a `DepKey` with `keep_bodies = true` (Task 3), so a `Keep` hit always carries bodies.
  Also change `dedup_routines_preserving_genuine_overloads` to MOVE survivors instead of cloning (`for r in routines.drain(..)` over the run, deciding by the precomputed counts) — same survivors, same markers, same order. Its existing tests must pass unchanged.

- [ ] **Step 3: The context.** `build_context_from_snapshot_cached(snap, profile, dep_cache)`: `let hit = dep_cache.get(&DepKey::of(&snap, profile));` then `let parse = parse_for_build(&snap, profile, hit.is_some());`, take `parse.workspace` out for `parsed` (a one-element `Vec`, or empty), pass the rest as `DepInput::Built(parse)` to `build_dep_layer_cached`. Keep the existing `Arc::ptr_eq` assertion. `ProgramContext::dep_bodies()` returns `self.dep_layer.dep_nodes.bodies.as_deref().map(Vec::as_slice)`.

- [ ] **Step 4: The LSP snapshot.** `parsed` now holds only the workspace unit: delete the `dep-arena-drop` thread and its comment block (nothing large is left to drop; dependency trees under `LIGHT` died during the parse). Keep the `"9.publish_snapshot"` mark.

- [ ] **Step 5: Tests (one per spec check and Review Focus item)**
  1. **Bound:** a 64-file dependency fixture under `Summary`, observed: `peak <= rayon::current_num_threads()` of the big-stack pool (read it inside `install`). Discrimination: collect all `ParsedFile`s first and summarize after (the old shape) → peak = 64 → FAIL; restore → PASS.
  2. **Profiles agree:** for the CDO-free fixtures used by `full.rs` tests and the multi-app fixture, `resolve_full_program_with` under `FULL` and under `LIGHT` give identical `edges`, `histogram`, `primary_histogram` and `recovered_files`.
  3. **Recovered dependency file (Review Focus 1):** a dependency app whose embedded source has `RECOVERED_SRC`; `ProgramReport::recovered_files` contains `"<dep name>::Broken.al"` under both profiles, and `fresh_coverage(..).recovered_files == 1`. Discrimination: drop the flag in `summarize_file` → FAIL.
  4. **Cross-profile cache (Review Focus 2):** one `DepCache`; build `LIGHT` then `FULL` (FULL's `dep_bodies()` is `Some` and non-empty); `FULL` then `LIGHT` (LIGHT's is `None`); `FULL` twice (second shares the first's tier AND has bodies); and two threads building `FULL` concurrently (both get bodies). Discrimination: let a `Keep` request hit a `Summary` tier (ignore `keep_bodies` in the key) → the LIGHT-then-FULL case FAILS.
  5. **Symbols mode (Review Focus 5):** `DependencySource::Symbols` build: `dep_summaries` empty, `dep_meta` empty, nodes equal today's (compare against a `FULL` build).
  6. **Rung 3 (Review Focus 4):** run `cargo test -p al-sem --lib updater` and the `lsp` umbrella; they must pass unchanged — list the rung-3 tests by name in the commit message.

- [ ] **Step 6: Gates.** `scripts/ci-steps task` rc=0; `scripts/check-goldens`: no golden moved; `scripts/cdo-gate U:/Git/DO-cdo-baseline/Cloud` PASS.

- [ ] **Step 7: Commit**
```bash
git add src/program/dep_summary.rs src/program/build.rs src/program/resolve/full.rs src/lsp/snapshot.rs src/program/dep_cache.rs src/snapshot/parse.rs
git commit -m "perf: summarize each dependency file at parse time; LIGHT drops its tree, FULL keeps it shared"
```

---

## Task 7: Docs, the measured result, and the branch gate

**Files:**
- Modify: `CLAUDE.md`, `CHANGELOG.md`, `docs/2026-10-04-step0-server-census.md` (append "after step 1")

- [ ] **Step 1: CLAUDE.md.** (a) Architecture, "Consumer 2": replace the claim that `src/engine/l4`/`l5` consume the program engine with the truth: `alsem analyze` runs the program engine only for its preflight count (`fresh_coverage`) and runs the detectors on the separate L3 model until spec step 3 (B3). (b) Add under Testing Philosophy, one bullet: "**Build profiles (spec 2026-10-04 §4).** Every graph build states a `BuildProfile`; there is no default. A memory saving is a profile choice, never a deletion: `FULL` keeps every fact. A tool that reads something must declare it." (c) Key Modules: add `src/program/profile.rs` and `src/program/dep_summary.rs`, one line each.

- [ ] **Step 2: Re-run the probe** (`tools/census-probe`, same corpora and modes as Task 2, with and without updaters). Append the after-step-1 tables to the step 0 report. Expected direction: the first root's build peak loses the dependency syntax trees (804 MiB CG / 915 MiB CDO in embedded mode); `fresh_coverage`'s peak falls similarly; retained memory is unchanged within noise. Report what is measured, not the expectation. Run the `measurement-auditor` agent on the appended section.

- [ ] **Step 3: CHANGELOG** under `## [Unreleased]`: `### Changed` — the summary-at-parse change with the measured peaks (before/after, corpus, mode, heap-not-RSS), and `BuildProfile`; `### Added` — `tools/census-probe` and the phase hook. Plain English.

- [ ] **Step 4: Branch gate.** `scripts/ci-steps all` rc=0 and `scripts/cdo-gate U:/Git/DO-cdo-baseline/Cloud` PASS, each to a log file, grepped.

- [ ] **Step 5: Commit**
```bash
git add CLAUDE.md CHANGELOG.md docs/2026-10-04-step0-server-census.md
git commit -m "docs: build profiles, the true alsem/L3 state, and step 1's measured peaks"
```

Container acceptance (spec §11: the CentralGauge harness gate) happens after a release, with the harness sessions, and is not part of this plan.
