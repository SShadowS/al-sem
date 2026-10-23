# #221 (epic #160 step 13a): remove salsa, fix watcher blindness — plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps
> use checkbox (`- [ ]`) syntax for tracking.

**Goal:** (1) delete the unused `salsa` dependency, byte-neutral; (2) make each LSP root's
watcher see every input the LSP build and the server read (`.al` renames and moves, folder
renames and moves, replacements, the root `app.json`, ancestor `.alpackages`, the workspace
`.al-sem.json`), from before the first read, and stop full rebuilds for files nothing reads.

**Architecture** (ledger `## Design`):
- The updater (`src/lsp/updater.rs`) gets ONE disk reconciliation: `PathChanged`,
  `FileSaved` and `FileRemoved` all go through `reconcile(path)`, whose plan depends only on
  the disk and on per-file stamps, never on the event kind or on coalescing.
- The watcher (`src/watcher.rs`) becomes a pure `map_event` plus a `WatchState` that owns the
  registrations (root recursive, walked ancestors and cache folders non-recursive) and the one
  per-event step `handle_raw`. The cache-folder walk has ONE implementation,
  `dependencies::package_cache_scope`, shared with the loader.
- The server (`src/server.rs`) shares one `ConfigHandle` per root, starts the watch state
  before the build reads, and reloads the workspace config on `ConfigChanged`.

**Tech stack:** Rust; notify 8.2; `scripts/ci-steps`, `scripts/check-goldens`.

**Spec:** ledger `.agent/issue-221/ledger.md`, section `## Design` (converged after 4 spec
rounds; register `.agent/issue-221/findings.json`, 41 entries). "Design" below means that
section. The red tests and their stubs: ledger `## Acceptance tests (red)`. Probes P2, P2b,
P8-P11, P14: ledger `## Assumption probes`, raw output under `/work/bench/160-13/`.

Three tasks, one commit each, in this order. **No golden moves in any task**: a moved golden
is a defect, never a regen.

| Task | Title | Red tests it turns green (and commits) |
|---|---|---|
| 1 | Salsa removal | none (grep + lock delta + lib outcome diff) |
| 2 | Updater reconciliation | T1-T9 (incl. T5's renamed test and T7(c)'s tightened one), T13(a-e), T15, T15(b) |
| 3 | Watcher and server wiring | T10 (16 rows + rewritten `test_watcher_detects_al_file`), T11(a-i), T12(a-d), T14 |

## Global Constraints

- **Editing.** Edit/Write tools only. No `sed -i`, `perl -pi`, heredoc, `python` or script
  rewrites of any repo file, discrimination breaks included (break with Edit, run, revert with
  Edit; re-read the edited line before running to confirm the break applied). Generated files
  (`Cargo.lock` by cargo, `rustfmt`) are the only exception.
- **Formatting.** `rustfmt <file>` per touched `.rs` file; never `cargo fmt`.
- **No silencing.** Never add or move a line matching `#[allow(` / `#![allow(`; no `#[ignore`.
  Dead code is deleted. `AlFileWatcher::try_recv` (`watcher.rs`, no caller) is deleted with its
  `#[allow(dead_code)]`, never moved; `ServerState.updater_handle`'s `#[allow(dead_code)]`
  (`server.rs:126`) stays exactly where it is.
- **Cargo.toml.** Never ADD a line matching `version = "`. Task 1 only deletes lines.
- **Protected paths, never edited:** `.github/`, `scripts/`, `.claude/`, `CLAUDE.md`,
  `.gitignore`, `tree-sitter-al`, `.agent/` (except the ledger's `## Discrimination proofs`,
  `## Gate results`, `## Discoveries` rows and Timeline lines).
- **Environment.** `export TREE_SITTER_AL_PATH=/work/alsem/tree-sitter-al` in every shell.
- **Per-task gate, before the commit.**
  1. `scripts/ci-steps task-affected > /work/bench/160-13/gate-T<n>.log 2>&1; echo $?` = 0,
     then `grep -nE 'FAILED|^error|^warning|test result' /work/bench/160-13/gate-T<n>.log`
     (never `| tail`).
  2. `scripts/check-goldens > /work/bench/160-13/goldens-T<n>.log 2>&1; echo $?` = 0, and
     `git status --short tests/*goldens* tests/goldens` is empty.
  3. The pre-commit hook passes unmodified (never `--no-verify`).
- **After each commit:** `python scripts/agentflow check-diff --base master --head HEAD
  --issue 221 --cwd /work/al-sem-issue-221-a2` prints `reasons: []`; anything else is fixed in
  a new commit, never amended.
- **Staging.** `git add <paths>` of the task's files only; never `git add -A`. Commit messages
  name `#221` and end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- **Discrimination proof** for every test a task turns green: the break the Design's test
  table states (copied per task below), the failing line, the revert, the pass. One row per
  break in the ledger's `## Discrimination proofs` (test, mutation, fail output, pass output,
  commit). Every break mutates the PRODUCTION call site, not a test helper.
- **Test commands.** Updater: `cargo test -p al-sem --lib lsp::updater::tests::<name>`.
  Config: `cargo test -p al-sem --lib config::tests::<name>`. Watcher/server (binary-only
  modules): `cargo test -p al-sem --bin al-call-hierarchy <name>`.
- **CHANGELOG.** Each task adds its own line(s) under `## [Unreleased]` in the same commit.

## The red tests: where they come from

The acceptance tests and stubs are uncommitted in the worktree (5 files: `src/lsp/updater.rs`,
`src/dependencies.rs`, `src/config.rs`, `src/watcher.rs`, `src/server.rs`).

### Task 0 (setup, no commit)

- [ ] Save reference copies outside the repo: `mkdir -p /work/bench/160-13/red && git diff >
  /work/bench/160-13/red/red.patch`, and `cp --parents` the five files into
  `/work/bench/160-13/red/tree/`.
- [ ] Park the edits: `git stash push -m "221 red acceptance tests"` (`.agent/` stays
  untracked and in place). Keep the stash until Task 3 is committed. `git status --short`
  shows only `.agent/` and this plan's commit is HEAD.
- [ ] Each task brings in its tests and stubs by reading `/work/bench/160-13/red/tree/<path>`
  and adding the text with Edit. No `unimplemented!("#221")` body is ever committed; stub
  signatures may change together with the tests that call them.

Carried from the ledger (do not "fix"):
- **T7(d)** runs only on a case-insensitive file system; on Linux it returns early (green,
  vacuous). Its proof is recorded as "not runnable here" unless a Windows run is available.
- **T10's Windows row** (`t10_windows_root_in_another_case_is_still_under_the_root`) is
  `#[cfg(windows)]`; same treatment.
- **T2's second half** (an absent unknown `.a.al.tmp` → no generation) passes today; it guards
  the new "absent and unknown → nothing" rule. Its break exists only after Task 2.
- **T15's hook** is the test itself: it replaces `src/` between `build` and `spawn_updater`.
  No production seam.

## Review Focus

- **A `.al` file saved by the editor with unchanged text** must still take rung 1 (tests and
  the perf gate rely on it, probe "No-op save semantics"): the event's own file is ALWAYS read,
  even when its stamp is unchanged. Pinned by the existing
  `spawn_updater_coalesces_five_rapid_saves_into_one_apply` and T7(c); Task 2 must keep both.
- **`git status` churn** (VS Code runs it on every save; it rewrites `.git/index.lock`): no
  rebuild, no reconcile. Pinned by T10 `t10_git_paths_are_dropped`; Task 3 also asserts
  `handle_raw` sends nothing for it (add to T11 as `t11j_git_lock_churn_sends_nothing`:
  `Create(File) ws/.git/index.lock` → empty log).
- **A dependency `.app` dropped into an ancestor cache that did not exist at startup** must be
  seen (T11(b), T11(e)).
- **A failed rebuild** (`app.json` broken mid-edit) must never lose the last good snapshot nor a
  pending config change (T9(c), T15(b), existing `build_failure_leaves_prev_snapshot_...`).
- **A Windows root** (lowercased by `normalize_path`) must still contain raw event paths in the
  disk's case (T10 Windows row; `rel_under_root` compares normalized forms).

---

## Task 1 — Salsa removal (byte-neutral)

**Files:** `Cargo.toml` (delete `:125-129`, the comment and `salsa = "=0.28.5"`), `Cargo.lock`
(regenerated by cargo), `src/engine/l4/summary.rs` (7 derives), `src/engine/l4/scc.rs` (2),
`src/engine/l4/combined_graph.rs` (9, incl. the multi-line one at `:116`),
`src/engine/l4/capability_cone.rs` (6, plus the last sentence of the doc at `:129-132`,
"salsa::SalsaValue still derives ... under a revision"), `docs/OUTSTANDING.md:712-720` (tick
the `salsa` item `[x]` with "done in #221"), `CHANGELOG.md`.

This commit holds the salsa removal ONLY: none of the red tests, none of the stubs.

- [ ] **Before.** On the clean tree: `cargo test -p al-sem --lib 2>&1 | grep -E '^test .* (ok|FAILED|ignored)$' | sort > /work/bench/160-13/lib-before.txt`.
- [ ] **Remove.** Delete the 24 `salsa::SalsaValue` derive entries (keep each `#[derive(...)]`'s
  other traits; drop the attribute only if salsa was its sole entry), the doc sentence, and the
  `Cargo.toml` block. `cargo build` regenerates `Cargo.lock`.
- [ ] **Grep.** `grep -rn "salsa\|SalsaValue" src crates tests benches Cargo.toml` prints nothing.
- [ ] **Lock delta.** `git diff Cargo.lock | grep -E '^[-+]name = ' | sort > /work/bench/160-13/lock-delta-T1.txt`;
  `diff /work/bench/160-13/lock-delta-T1.txt <(sort /work/bench/160-13/p2/lock-delta.txt)` is
  empty: exactly the 15 `-name` lines (`allocator-api2 boxcar crossbeam-queue hashlink
  intrusive-collections inventory parking_lot parking_lot_core redox_syscall rustc-hash salsa
  salsa-macro-rules salsa-macros thin-vec typeid`), no `+` line.
- [ ] **Feature delta.** `cargo tree -p al-sem -e normal -f '{p} [{f}]'` (host and
  `--target all`) diffed against `/work/bench/160-13/p2/pkgfeat-after{,-all}.txt`: equal (P2b:
  only `hashbrown v0.17.1` and `portable-atomic` lose features).
- [ ] **After.** The same lib outcome list into `lib-after.txt`; `diff lib-before.txt
  lib-after.txt` is empty.
- [ ] **CHANGELOG** `### Removed`: "The unused `salsa` dependency and its 24 `SalsaValue`
  derives in `src/engine/l4` (#221). Byte-neutral; `Cargo.lock` drops 15 packages."
- [ ] Gate (also `scripts/ci-steps task`, the full task gate, once), commit (`Cargo.toml`,
  `Cargo.lock`, the four `l4` files, `docs/OUTSTANDING.md`, `CHANGELOG.md`), `check-diff`.

No discrimination proof: the derives implement a trait nothing bounds on, so there is no
behaviour to pin (Design, "Salsa removal").

## Task 2 — Updater reconciliation

**Files:** `src/lsp/updater.rs` only (code, tests, docs), `CHANGELOG.md`.

**Interfaces (produces):**
```rust
pub enum ChangeEvent { FileSaved(PathBuf), FileRemoved(PathBuf), PathChanged(PathBuf),
                       ConfigChanged, DepsChanged, Overflow }   // Clone, Debug, PartialEq, Eq
pub fn rel_under_root(root: &Path, p: &Path) -> Option<PathBuf>;  // used by Task 3's map_event
```
`rel_under_root` compares `protocol::normalize_path(root)` with `normalize_path(p)` component
by component and returns `p`'s OWN trailing components (disk case). `p == root` → `Some("")`.

Internal (exact names free, behaviour fixed by Design "Updater: one disk reconciliation",
"Stamps", "Staged stamps", "`ConfigChanged` → full republish"):
- `Updater` fields: `stamps: HashMap<String, FileStamp>` (empty after every full build,
  including the initial one; never seeded); `canonicalize: fn(&Path) -> io::Result<PathBuf>`
  (default `std::fs::canonicalize`, the T7(e) seam); `#[cfg(test)] reads: AtomicUsize`
  counting every source read `reconcile` makes.
- `FileStamp`: `(len, modified)` plus `(dev, ino)` under `#[cfg(unix)]`.
- `classify` sends `PathChanged`/`FileSaved`/`FileRemoved` through `reconcile`, rules 1-7 of the
  Design in order; `Decision` carries the batch's staged stamps; they are committed on a rung-1
  or rung-2 apply or a no-op batch, discarded on rung 3.
- `PathClass::NotWorkspaceSource` for skip dirs becomes `Ignored` (no rung 3).
- `spawn_updater`'s inner loop breaks with `(decision, config_changed)`; the four arms of the
  Design's `ConfigChanged` paragraph (no-op → `on_swap(&cur, &Full)` same generation; rung 1 →
  `Full` instead of the delta; rung 3 failure → `on_swap(&cur, &Full)`).
- Delete the H-10 early `return` in `case_mismatched_no_op_save_does_not_duplicate_incoming_edges`
  (already done in the red copy).

- [ ] **Red.** Bring in from the red copy of `updater.rs`: the two `ChangeEvent` variants and
  derive, `rel_under_root`, the `canonicalize`/`reads` fields, the stub arms in `classify` and
  `coalesce_batch`, the T7(c) and T5 test edits, and the whole `#221: one disk reconciliation`
  test block. Run `cargo test -p al-sem --lib lsp::updater::tests::` : 27 fail, each with the
  ledger's red line (T7(d) passes, vacuous).
- [ ] **Green.** Implement reconcile, stamps, staged stamps, `Ignored`, the `ConfigChanged` flag.
  The `.al` stub arm and `ConfigChanged => {}` disappear. All updater tests pass (40, 1 ignored).
- [ ] **Docs (O9).** Rewrite the module doc's rung-3 bullet (`updater.rs:105-108`: no more
  "under `.alpackages/`"), the `ChangeEvent` doc (`:179-186`: paths are reconciled from disk;
  `FileRemoved` is not trusted), `PathClass` and `classify_path`'s docs.
- [ ] **Discrimination proofs** (Design test table):
  - T1: treat "absent" as keep → `Alpha.al`'s decls remain.
  - T2: absent → keep → vp remains; drop "absent and unknown → nothing" → `Rung::Two`.
  - T3: drop the prefix part of `affected` → `src/*` remain.
  - T4: drop the directory walk → `pkg/Pkg.al` missing.
  - T5 (both tests): `Ignored` → rung 3 → a generation.
  - T6(a): decide absence by read error `NotFound` only → `src/*` remain; T6(b)+(c): decide
    file-vs-folder by extension → fail; T6(d): skip known files without comparing stamps →
    old content.
  - T7(a): apply the fallback whenever a case match exists → lost/duplicated vp; T7(b): drop the
    "different file" check → `target.al` missing; T7(c): judge presence from `p` not
    `root.join(K)` → `Target.al` removed; T7(e): treat a failed canonicalize as "different
    file" → `Rung::Two`. T7(d): "not runnable here (case-sensitive FS)".
  - T8: trust `FileRemoved` → `Gamma.al` dropped.
  - T9(a)/(b)/(c): drop that arm → no `Full` recorded (three rows).
  - T13(a): read known files without the stamp check → second count > 3; (b) skip unknown
    files → `src/New.al` missing; (c) limit `affected` to direct children → `src/sub/A.al`
    remains; (d) carry a kind flag that coalescing overwrites → old content; (e) no stamp
    compare → old content.
  - T15: seed stamps from a post-build stat in `spawn_updater` → old content.
  - T15(b): commit stamps during classify → the read is skipped, old text stays.
- [ ] **Callers.** `src/server.rs` still sends `FileSaved` from `didSave` and the old watcher
  mapping; both now reconcile from disk. `cargo test -p al-sem --bin al-call-hierarchy
  server::tests::dispatch_prepare_and_outgoing_then_didsave` passes unchanged.
- [ ] **CHANGELOG** `### Fixed`: "LSP updater: a `.al` file renamed, moved or replaced, a
  folder renamed, moved in or out, or replaced, and a `FileRemoved` for a file still on disk
  are reconciled from disk instead of leaving stale or missing declarations; a `.al` under
  `.alpackages`, `.snapshots` or `node_modules` no longer triggers a full rebuild (#221)."
- [ ] Gate, commit (`src/lsp/updater.rs`, `CHANGELOG.md`), `check-diff`.

## Task 3 — Watcher and server wiring

**Files:** `src/dependencies.rs`, `src/config.rs`, `src/watcher.rs`, `src/server.rs`,
`CHANGELOG.md`.

**Interfaces:**
- Consumes: Task 2's `ChangeEvent::{PathChanged, ConfigChanged}` and `rel_under_root`.
- Produces:
```rust
// src/dependencies.rs
pub struct CacheScope { pub candidates: Vec<PathBuf>, pub walked_dirs: Vec<PathBuf> }
pub enum Role { CacheFolder, AppFile }
pub fn package_cache_scope(project_root: &Path) -> CacheScope;   // the ONLY folder walk
pub fn is_app_file(name: &OsStr) -> bool;                        // case-sensitive `.app`
pub fn cache_scope_role(scope: &CacheScope, path: &Path) -> Option<Role>;
// src/config.rs
pub struct ConfigHandle;  // RwLock<(DiagnosticConfig, reloaded: bool)> + root + no_diagnostics
impl ConfigHandle { fn new(PathBuf, bool) -> Self; fn get(&self) -> DiagnosticConfig;
  fn install_initial(&self, DiagnosticConfig); fn reload(&self, DiagnosticConfig);
  fn reload_from_disk(&self); }
// src/watcher.rs
pub trait WatchBackend: Send { fn watch(&mut self, &Path, RecursiveMode) -> notify::Result<()>;
                               fn unwatch(&mut self, &Path) -> notify::Result<()>; }
pub type BackendFactory = Arc<dyn Fn(Sender<notify::Result<Event>>)
    -> notify::Result<Box<dyn WatchBackend>> + Send + Sync>;
pub fn recommended_backend() -> BackendFactory;
pub struct WatchScope { pub root: PathBuf, pub cache: CacheScope }  // WatchScope::compute(root)
pub fn watch_plan(&WatchScope) -> BTreeMap<PathBuf, RecursiveMode>;  // Recursive wins a tie
pub fn map_event(&WatchScope, &Event) -> Vec<ChangeEvent>;           // rules 1-9, pure
pub struct WatchState { scope, registered, cache_seen, failed }
impl WatchState { fn new(WatchScope) -> Self; fn start(&mut self, &mut dyn WatchBackend);
  fn handle_raw(&mut self, &mut dyn WatchBackend, notify::Result<Event>, &ConfigHandle,
                &Sender<ChangeEvent>);
  fn resync(&mut self, &mut dyn WatchBackend) -> bool;
  fn retry_failed(&mut self, &mut dyn WatchBackend, &Sender<ChangeEvent>); }
```
`map_event` returns, for an outside-root `CacheFolder` path, nothing itself: `handle_raw`
handles rule 4's resync request (the stub signature leaves this to the implementer; keep
`map_event` pure).

Behaviour: Design "One enumerator, one predicate", "Watch plan and watch state", "Event
kinds", "Event mapping", "Config sharing and reload", "Startup order".

- [ ] **Red.** Bring in from the red copies: `dependencies.rs` stubs, `config.rs`
  `ConfigHandle` + T14, `watcher.rs` stubs + `mod fake` + the T10/T11 tests + the rewritten
  `test_watcher_detects_al_file` (and the `Watcher::watch(&mut watcher, ..)` call fix),
  `server.rs` `RootBuild.{watch_backend, build_hook}` + T12 tests. Add the Review Focus test
  `t11j_git_lock_churn_sends_nothing`. Runs: bin target 30+1 fail (`not implemented: #221`,
  T12's lines), `config::tests::t14_*` fails `T14: reload then install_initial must keep the
  reloaded config`.
- [ ] **Green, dependencies.** `package_cache_scope` (Design rules: own `<root>/.alpackages`;
  unless `<root>/.git` exists, each ancestor's `.alpackages`, stop after an ancestor with `.git`
  or at 8 levels; no `is_dir`); `find_all_alpackages_folders` = candidates filtered by `is_dir`
  + `paths_equal` dedup; `discover_app_files` uses `is_app_file`. `cargo test -p al-sem --lib
  dependencies::` stays 34/34 and `scripts/check-goldens` stays green (F12: any move means the
  refactor is wrong).
- [ ] **Green, watcher.** Implement the stubs; delete `FileChange`, `is_relevant_path` (and its
  test, replaced by T10), `AlFileWatcher` with `try_recv` and its `#[allow(dead_code)]`. The
  production loop: create the backend from the factory, `WatchState::start`, then loop on the
  raw receiver with a 100 ms timeout calling `handle_raw`, and `retry_failed` every 5 s.
- [ ] **Green, server.** `RootState::state`: create the channel and the `ConfigHandle`, start
  the watch state (registration on the calling thread), call `build_hook`, then
  `install_initial(DiagnosticConfig::load(..))` and build. `build_server_state` takes the
  sender/receiver and the handle (update its three test callers). `on_swap` reads
  `handle.get()`; with every detector off it publishes the diff against an empty map instead
  of returning early (`server.rs:606-608`), and sends nothing when nothing was published.
  Delete `to_change_event`, `path_is_dependency_source`, `start_file_watcher`.
  `all_detectors_disabled_publishes_no_diagnostics` passes unchanged.
- [ ] **Docs (O9).** `server.rs` module doc (`:70`), the watcher-thread docs (`:785-830`), the
  `didSave` doc (`:1220-1230`); the `watcher.rs` module doc.
- [ ] **Discrimination proofs** (Design test table):
  - T10: per row, delete that rule → that row fails (16 rows; the positive `.app` row: delete
    the `AppFile` arm). Windows row: "not runnable here".
  - `test_watcher_detects_al_file`: drop rule 9 → empty log.
  - T11: (a) register candidates before parents; (b) send before resync; (c) send on every
    `CacheFolder` event; (d) existence diff only, no forced re-watch; (e) send without resync
    after a retry; (f) delete the `AppFile` arm of `map_event`; (g) last-planned mode wins;
    (h) a name-only test in `cache_scope_role`; (i) apply the forced re-watch to every
    candidate; (j) drop rule 8 → a `PathChanged` sent.
  - T12: (a) start the watcher after the build; (b) drop the `PathChanged` forwarding in
    `handle_raw`; (c) drop the reload call in `handle_raw`; (d) restore the early return at
    `server.rs:606-608`.
  - T14: make `install_initial` a plain store → the first order keeps `old`.
- [ ] **Whole-suite check.** `scripts/ci-steps task` (full) once: green.
- [ ] **CHANGELOG** `### Fixed`: "LSP watcher: each root now watches its root `app.json`, its
  workspace `.al-sem.json` (reloaded live), and the ancestor `.alpackages` folders the loader
  reads, registered before the first build; folder renames and moves, and files replaced
  without their own event, are reconciled. Stated limits: an edit to the global
  `~/.al-sem/config.json` and a `.git` created or removed at the root or an ancestor still
  need a server restart (#221)."
- [ ] Gate, commit (the four `src` files, `CHANGELOG.md`), `check-diff`. Then confirm every
  test of the ledger's red table is committed by name (`git grep -n 'fn <name>'`) and drop
  nothing: the Task 0 stash stays until the operator says otherwise.

## Acceptance map (Design "Acceptance matrix")

| Item | Task |
|---|---|
| No `salsa` dependency or derive; lock delta = P2; feature delta = P2b | 1 |
| Build and whole suite green | each task's gate; `ci-steps all` + `cdo-gate` at the branch gate (conductor) |
| No golden moves | every task (`check-goldens`, no regen) |
| A test per missed path shape, each with a discrimination proof | 2 (T1-T9, T13, T15), 3 (T10-T12, T14) |
| Non-speed, no bench | — |
