//! Content-keyed, fail-closed on-disk cache for the preflight verdict
//! ([`FreshCoverage`]).
//!
//! Spec: `docs/superpowers/specs/2026-08-01-preflight-verdict-cache.md`.
//! Measurement: `docs/2026-07-31-preflight-census.md` — `preflight.fresh_coverage`
//! is **83.4 % of a DO run** (2,642 ms of 3,171 ms) and 10.8 % of BC Base App
//! 8020. It builds, resolves and destroys a whole-program model of ~11,600 files
//! — ~95 % of them dependency source that did not change since the last run — to
//! produce four scalars.
//!
//! # Why memoizing this is not a "silent clean"
//!
//! `fresh_coverage(workspace_root)` is a pure function of (workspace bytes +
//! discovered `.app` bytes + engine binary). It reads no config, no thresholds and
//! no verdict-affecting environment — every `AnalyzeArgs` knob acts DOWNSTREAM of
//! `fresh` (`engine/gate/run.rs`). Replaying its result under a content-complete
//! key is replaying a verification that DID happen, on inputs proven
//! byte-identical — not fabricating one.
//!
//! That argument holds only under three rules, and each is load-bearing:
//!
//! 1. **The key is content-complete** ([`cache_key`]). Every stale-verdict
//!    scenario is a key gap. There is NO payload-level defence: a well-formed
//!    entry under a correct key is indistinguishable from a correct one without
//!    recomputing, so all soundness lives in the key plus entry integrity.
//! 2. **Every abnormal state means recompute, silently** ([`lookup`]). Missing,
//!    unreadable, schema mismatch, version mismatch, self-hash mismatch → miss,
//!    logged at debug, never an error. Modelled on `snapshot::cache`.
//! 3. **`Ok` is cached, `Err` never is** (enforced at the call site in
//!    [`super::full::fresh_coverage`], and restated here because it is the rule
//!    most easily lost in a refactor). `fresh_coverage` returns
//!    `Result<FreshCoverage, String>` whose `Err` is the first-class
//!    *could-not-verify* state, and that state captures TRANSIENT environment —
//!    a locked file, a dying disk. Caching it would laminate a one-time I/O flake
//!    into a persistent verdict. Degraded `Ok`s (`unknown > 0`, recovered files,
//!    opaque apps) ARE cached: they are as deterministic as clean ones, and the
//!    key changes on any edit anyway.
//!
//! # The payload is byte-gated OUTPUT, not just an exit code
//!
//! `run.rs` copies `fresh.opaque_apps` into the coverage block the Json/Terminal/
//! Html formatters render, and `evaluate_preflight` warns on stderr on every
//! degraded run independent of `--require-dependencies`. So a warm run must
//! reproduce the entry EXACTLY, `opaque_apps` included, in its produced order
//! (already deterministically sorted at production — stored as-is, never re-sorted
//! on load). The merge gate therefore gains a new required form: a cold run and a
//! warm run must be byte-identical.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::snapshot::identity::TrustTier;
use crate::snapshot::snapshot::{AppSetSnapshot, AppUnit, World};

use super::full::FreshCoverage;

/// Serialization shape of a cache entry. Bump when the ENTRY's shape changes;
/// old entries then fail the version check and are recomputed.
///
/// This is deliberately NOT the primary defence against a stale verdict — see
/// [`binary_identity`] for why a hand-bumped constant cannot be trusted with
/// that job.
const PREFLIGHT_CACHE_SCHEMA: u32 = 1;

/// Env var pointing the cache at a specific directory. **Required for tests** —
/// without it every test would share one global mutable directory, which is
/// half of the pre-existing defect in `snapshot::cache` (no override, no
/// pruning).
///
/// This module fixes the OVERRIDE half and, as of today, does NOT fix the
/// pruning half: there is no size or age bound and the directory grows without
/// limit. `docs/superpowers/specs/2026-08-01-preflight-verdict-cache.md` §4
/// asked for size/age-bounded pruning and it was never implemented. Known debt,
/// stated here rather than implied away — an earlier revision of this doc
/// claimed the whole defect was "deliberately not copied", which was false for
/// the pruning half.
pub const ENV_CACHE_DIR: &str = "ALSEM_PREFLIGHT_CACHE_DIR";

/// Env var disabling the cache entirely (any non-empty value). `scripts/cdo-gate`
/// exports this so the north-star CDO ratchets can never measure a warm replay of
/// themselves.
pub const ENV_NO_CACHE: &str = "ALSEM_NO_PREFLIGHT_CACHE";

/// Cache root: `$ALSEM_PREFLIGHT_CACHE_DIR` when set, else
/// `<os-cache>/alsem/preflight-v1/`.
///
/// A NEW versioned root on purpose. It must NOT live in `~/.al-sem/cache`: that
/// directory's artifact shape and `cache prune` stdout are al-sem-golden-pinned,
/// and a foreign file shape there lands in the `removed-unreadable` bucket.
fn cache_dir() -> Option<PathBuf> {
    // Test override, consulted first. Tests need an ISOLATED directory, and
    // reaching that through `std::env::set_var` would be both `unsafe` under
    // edition 2024 and racy across parallel tests. This keeps the env var as the
    // production surface while giving tests a safe, non-global-env seam.
    #[cfg(test)]
    if let Some(d) = tests::test_dir_override() {
        return Some(d);
    }
    if std::env::var_os(ENV_NO_CACHE).is_some_and(|v| !v.is_empty()) {
        return None;
    }
    let dir = match std::env::var_os(ENV_CACHE_DIR) {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => dirs::cache_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("alsem")
            .join("preflight-v1"),
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::debug!("preflight cache dir {} unusable: {e}", dir.display());
        return None;
    }
    Some(dir)
}

/// blake3 of the running executable, memoized for the process.
///
/// This is the engine-identity component of the key, and it subsumes — in ONE
/// value that cannot rot — the resolver's behaviour, the builtin catalogs, the
/// grammar (tree-sitter-al is compiled in, not loaded), the lowerer, and the
/// semantics of every cached field.
///
/// **Why not a hand-bumped constant:** this repo has live proof they rot.
/// `engine/gate/cache_prune.rs`'s `CACHE_VERSION_GRAMMAR` read
/// `"tree-sitter-al-v2.5.2-native"` while the pinned grammar had already moved
/// to v3.2.0 — never bumped across two grammar upgrades, and nothing failed,
/// until a dedicated test (`cache_prune::tests::
/// cache_version_grammar_tracks_the_linked_grammar`) was added to assert it
/// against `tree-sitter-al/package.json`, at which point it was corrected. A
/// hand-bumped constant needs its OWN test to catch this kind of rot; a
/// binary hash cannot rot in the first place — there is nothing to bump.
///
/// Accepted cost: rebuilding identical source produces a different binary
/// (embedded timestamps/paths), so a rebuild invalidates the cache. That fails
/// toward RECOMPUTE, which is the correct direction for this instrument.
///
/// Returns `None` when the executable cannot be read — which disables the cache
/// rather than keying on a weaker identity.
fn binary_identity() -> Option<&'static str> {
    static ID: OnceLock<Option<String>> = OnceLock::new();
    ID.get_or_init(|| {
        let exe = std::env::current_exe().ok()?;
        let bytes = std::fs::read(exe).ok()?;
        Some(blake3::hash(&bytes).to_hex().to_string())
    })
    .as_deref()
}

/// Length-prefixed field fold. Length prefixing is not stylistic: without it,
/// `("ab", "c")` and `("a", "bc")` fold identically, which is the same collision
/// class that makes `SourceRoot::content_hash` unusable here (see
/// [`source_identity`]).
fn put(h: &mut blake3::Hasher, field: &[u8]) {
    h.update(&(field.len() as u64).to_le_bytes());
    h.update(field);
}

/// Content identity of one app's source.
///
/// Two cases, separated by `TrustTier` because the two providers produce
/// `SourceRoot::content_hash` values of completely different strength
/// (`snapshot/provider.rs`):
///
/// - [`TrustTier::EmbeddedSource`] — `EmbeddedAppProvider` sets `content_hash` to
///   the blake3 of the WHOLE `.app` file (via `snapshot::cache::cached_source`).
///   That is content-complete and **already paid for** by this run, so it is used
///   directly.
/// - Walked source ([`TrustTier::Workspace`] / `LocalSource*`) — `walk_al_source`
///   folds ONLY `f.text.as_bytes()`, concatenated, with **no `virtual_path` and no
///   length prefix**. It collides on a file RENAME, and — verdict-changingly — on
///   RE-SPLITTING the same bytes across different file boundaries (`"fo"`+`"obar"`
///   vs `"foo"`+`"bar"` hash identically and parse completely differently). So
///   this computes its OWN fold over `(virtual_path, len, text)` and never reuses
///   that field. The underlying collision is a latent snapshot-layer bug tracked
///   separately in `docs/OUTSTANDING.md`; this cache does not depend on it being
///   fixed.
///
/// A symbol-only app has no `source` at all, so its identity comes from the `.app`
/// bytes directly. `None` is returned when that cannot be read — the caller then
/// declines to cache rather than key on an incomplete identity.
fn source_identity(unit: &AppUnit) -> Option<String> {
    match &unit.source {
        Some(src) if src.tier == TrustTier::EmbeddedSource => Some(src.content_hash.clone()),
        Some(src) => {
            let mut h = blake3::Hasher::new();
            for f in src.files.iter() {
                put(&mut h, f.virtual_path.as_bytes());
                put(&mut h, f.text.as_bytes());
            }
            Some(h.finalize().to_hex().to_string())
        }
        None => match &unit.app_path {
            // Symbol-only dependency: identity is the `.app`'s own bytes.
            Some(p) => crate::snapshot::embedded::app_content_hash(p).ok(),
            // No source and no `.app` — e.g. a workspace root with zero `.al`
            // files. Nothing to hash, and nothing that can vary.
            None => Some(String::new()),
        },
    }
}

/// The cache key: `blake3(binary_identity ‖ canonical_fold(snapshot))`, 64 hex.
///
/// Derived FROM the snapshot the preflight already builds, deliberately. A
/// cheaper pre-snapshot key would mean a second discovery implementation that can
/// drift from the real one — which is exactly what
/// `compute_gate_model_instance_id` demonstrates: it hashes `"{guid}@{version}"`
/// plus one `ws:<relPosix>` per discovered file, i.e. file NAMES and never file
/// CONTENT, so a cache keyed on it would serve a stale verdict after ANY edit.
/// The `snapshot_build` cost is therefore an accepted floor, not an oversight.
///
/// Returns `None` when any component cannot be established — the caller then runs
/// uncached. Failing toward recompute is always the safe direction here.
///
/// The whole DISCOVERED app set is folded, not the primary's reachable closure:
/// `load_all_apps` loads every `.app` under ancestor `.alpackages` without
/// app.json filtering, those become graph nodes, and event-subscriber wiring is
/// whole-snapshot-scoped. An unrelated package appearing or vanishing therefore
/// causes a (spurious, harmless) miss.
///
/// The `.dependencies/` law is satisfied by construction: this reads discovered
/// app CONTENT and never folder names.
pub fn cache_key(snap: &AppSetSnapshot) -> Option<String> {
    let mut h = blake3::Hasher::new();
    put(&mut h, b"alsem-preflight-v1");
    put(&mut h, &PREFLIGHT_CACHE_SCHEMA.to_le_bytes());
    put(&mut h, binary_identity()?.as_bytes());

    put(&mut h, snap.workspace_app.guid.as_bytes());
    put(&mut h, snap.workspace_app.name.as_bytes());
    put(&mut h, snap.workspace_app.publisher.as_bytes());
    put(&mut h, snap.workspace_app.version.as_bytes());
    put(
        &mut h,
        match snap.world {
            World::Closed => b"closed",
            World::Open => b"open",
        },
    );

    // Canonical order — `snap.apps` order is an input we do not want to depend on.
    let mut units: Vec<(&AppUnit, String)> = Vec::with_capacity(snap.apps.len());
    for u in &snap.apps {
        units.push((u, source_identity(u)?));
    }
    units.sort_by(|(a, ah), (b, bh)| {
        (&a.id.guid, &a.id.version, ah).cmp(&(&b.id.guid, &b.id.version, bh))
    });

    put(&mut h, &(units.len() as u64).to_le_bytes());
    for (u, src_id) in &units {
        put(&mut h, u.id.guid.as_bytes());
        put(&mut h, u.id.name.as_bytes());
        put(&mut h, u.id.publisher.as_bytes());
        put(&mut h, u.id.version.as_bytes());
        put(&mut h, format!("{:?}", u.provenance.tier).as_bytes());
        put(&mut h, src_id.as_bytes());
        // Verdict-load-bearing: `opaque_dependency_closure` BFSes the declared
        // deps, and resolution visibility is closure-scoped.
        put(&mut h, &(u.declared_deps.len() as u64).to_le_bytes());
        for d in &u.declared_deps {
            put(&mut h, d.app_id.as_bytes());
            put(&mut h, d.name.as_bytes());
            put(&mut h, d.publisher.as_bytes());
            put(&mut h, d.version.as_bytes());
        }
        // Friend-app visibility feeds `internal` member resolution.
        put(&mut h, &(u.internals_visible_to.len() as u64).to_le_bytes());
        for f in &u.internals_visible_to {
            put(&mut h, f.app_id.as_bytes());
            put(&mut h, f.name.as_bytes());
            put(&mut h, f.publisher.as_bytes());
        }
        // Preprocessor symbols + version basis for `#if` evaluation.
        put(&mut h, format!("{:?}", u.compilation).as_bytes());
    }
    Some(h.finalize().to_hex().to_string())
}

/// One cache entry. Self-describing so a stale or foreign file is recognised
/// rather than misread.
#[derive(Serialize, Deserialize)]
struct Entry {
    schema_version: u32,
    /// The key this entry was stored under. Redundant with the filename (which is
    /// the lookup) and kept for auditability + as a second mismatch check.
    key: String,
    /// blake3 of the running binary at store time.
    binary: String,
    payload: FreshCoverage,
    /// blake3 over `schema_version ‖ key ‖ binary ‖ payload-json`. Cheap
    /// belt-and-suspenders against bit-rot and hand edits; also what makes the
    /// poisoned-entry tests expressible.
    self_hash: String,
}

fn compute_self_hash(schema: u32, key: &str, binary: &str, payload: &FreshCoverage) -> String {
    let mut h = blake3::Hasher::new();
    put(&mut h, &schema.to_le_bytes());
    put(&mut h, key.as_bytes());
    put(&mut h, binary.as_bytes());
    put(
        &mut h,
        serde_json::to_string(payload)
            .unwrap_or_default()
            .as_bytes(),
    );
    h.finalize().to_hex().to_string()
}

fn entry_path(dir: &Path, key: &str) -> PathBuf {
    dir.join(format!("{key}.json"))
}

/// Look up a cached verdict. **Every abnormal state returns `None`** (recompute):
/// no directory, no file, unreadable, malformed JSON, schema mismatch, key
/// mismatch, binary mismatch, or self-hash mismatch.
pub fn lookup(key: &str) -> Option<FreshCoverage> {
    let dir = cache_dir()?;
    let path = entry_path(&dir, key);
    let text = std::fs::read_to_string(&path).ok()?;
    let entry: Entry = match serde_json::from_str(&text) {
        Ok(e) => e,
        Err(e) => {
            log::debug!("preflight cache entry {} unreadable: {e}", path.display());
            return None;
        }
    };
    if entry.schema_version != PREFLIGHT_CACHE_SCHEMA || entry.key != key {
        return None;
    }
    if entry.binary != binary_identity()? {
        return None;
    }
    let expect = compute_self_hash(
        entry.schema_version,
        &entry.key,
        &entry.binary,
        &entry.payload,
    );
    if expect != entry.self_hash {
        log::debug!("preflight cache entry {} failed self-hash", path.display());
        return None;
    }
    Some(entry.payload)
}

/// Persist a verdict. Best-effort: any failure is logged at debug and ignored —
/// a cache that cannot be written must never break a run.
///
/// Atomic by tmp+rename, mirroring `snapshot::cache::persist_cache`. Two
/// concurrent writers of the same key write IDENTICAL bytes by construction (same
/// key ⇒ same inputs ⇒ same verdict), so last-wins is safe. On Windows the rename
/// can fail with a sharing violation if a reader holds the destination; that is a
/// debug log and nothing more.
///
/// **Only ever called with an `Ok` verdict** — see this module's rule 3.
pub fn store(key: &str, value: &FreshCoverage) {
    let Some(dir) = cache_dir() else { return };
    let Some(binary) = binary_identity() else {
        return;
    };
    let entry = Entry {
        schema_version: PREFLIGHT_CACHE_SCHEMA,
        key: key.to_string(),
        binary: binary.to_string(),
        self_hash: compute_self_hash(PREFLIGHT_CACHE_SCHEMA, key, binary, value),
        payload: value.clone(),
    };
    let Ok(json) = serde_json::to_string(&entry) else {
        return;
    };
    let tmp = dir.join(format!(".{}.{}.tmp", key, std::process::id()));
    if let Err(e) = std::fs::write(&tmp, json.as_bytes()) {
        log::debug!("preflight cache write failed: {e}");
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, entry_path(&dir, key)) {
        log::debug!("preflight cache rename failed: {e}");
        let _ = std::fs::remove_file(&tmp);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::resolve::full::{FreshCoverage, fresh_coverage};
    use std::sync::{Mutex, MutexGuard, OnceLock};

    // ---------------------------------------------------------------------
    // Harness
    // ---------------------------------------------------------------------

    /// Process-global cache-dir override + the lock serializing it. The override
    /// IS global state (there is one cache dir per process), so every test that
    /// touches it holds the lock for its whole body.
    static OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

    fn lock() -> MutexGuard<'static, ()> {
        static L: OnceLock<Mutex<()>> = OnceLock::new();
        L.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    pub(super) fn test_dir_override() -> Option<PathBuf> {
        OVERRIDE.lock().ok()?.clone()
    }

    fn set_override(d: Option<PathBuf>) {
        *OVERRIDE.lock().unwrap() = d;
    }

    /// The TRACER: a verdict no workspace below can produce (they are tiny,
    /// dependency-free and clean, so their true `unknown` is 0). Written into
    /// the cache BY HAND — never minted by production code.
    ///
    /// This is what makes every test here a TWO-OUTCOME oracle: `unknown == 999`
    /// proves the cache was served, any other value proves a recompute ran. Both
    /// directions are observable, so each test carries its own can-fail proof
    /// rather than needing one bolted on afterwards.
    const TRACER: usize = 999;

    fn tracer_verdict() -> FreshCoverage {
        FreshCoverage {
            unknown: TRACER,
            coverage_holds: true,
            recovered_files: 0,
            opaque_apps: vec![],
        }
    }

    const APP_JSON: &str = concat!(
        "{\"id\":\"11111111-1111-1111-1111-111111111111\",",
        "\"name\":\"T\",\"publisher\":\"P\",\"version\":\"1.0.0.0\"}"
    );

    const CODEUNIT: &str = "codeunit 50000 A { procedure P() begin end; }";
    const CODEUNIT_Q: &str = "codeunit 50000 A { procedure Q() begin end; }";

    /// A minimal but REAL workspace: root `app.json` + one `.al` file per entry.
    fn workspace(dir: &Path, files: &[(&str, &str)]) {
        std::fs::write(dir.join("app.json"), APP_JSON).unwrap();
        for (name, text) in files {
            std::fs::write(dir.join(name), text).unwrap();
        }
    }

    fn key_of(ws: &Path) -> String {
        let snap = crate::program::resolve::full::build_snapshot_res(ws).unwrap();
        cache_key(&snap).expect("a well-formed workspace must yield a key")
    }

    fn key_for_files(files: &[(&str, &str)]) -> String {
        let d = tempfile::tempdir().unwrap();
        workspace(d.path(), files);
        key_of(d.path())
    }

    // ---------------------------------------------------------------------
    // 1. The hit is real, reached through the PRODUCTION entry point
    // ---------------------------------------------------------------------

    #[test]
    fn warm_hit_serves_the_cached_verdict_through_fresh_coverage() {
        let _g = lock();
        let cache = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        workspace(ws.path(), &[("a.al", CODEUNIT)]);
        set_override(Some(cache.path().to_path_buf()));

        let cold = fresh_coverage(ws.path()).unwrap();
        assert_ne!(
            cold.unknown, TRACER,
            "precondition: the real workspace must not produce the tracer value"
        );

        store(&key_of(ws.path()), &tracer_verdict());
        let warm = fresh_coverage(ws.path()).unwrap();
        set_override(None);

        assert_eq!(
            warm.unknown, TRACER,
            "the production entry point must serve the cached entry"
        );
    }

    #[test]
    fn a_cache_dir_without_the_entry_recomputes() {
        let _g = lock();
        let ws = tempfile::tempdir().unwrap();
        workspace(ws.path(), &[("a.al", CODEUNIT)]);

        let cache = tempfile::tempdir().unwrap();
        set_override(Some(cache.path().to_path_buf()));
        store(&key_of(ws.path()), &tracer_verdict());
        assert_eq!(
            fresh_coverage(ws.path()).unwrap().unknown,
            TRACER,
            "precondition: the seeded entry is served"
        );

        let empty = tempfile::tempdir().unwrap();
        set_override(Some(empty.path().to_path_buf()));
        let recomputed = fresh_coverage(ws.path()).unwrap();
        set_override(None);
        assert_ne!(recomputed.unknown, TRACER);
    }

    // ---------------------------------------------------------------------
    // 2. Key completeness — one row per component, each hand-stating its
    //    precondition by CONSTRUCTING both inputs
    // ---------------------------------------------------------------------

    #[test]
    fn key_changes_when_a_file_body_changes() {
        let _g = lock();
        assert_ne!(
            key_for_files(&[("a.al", CODEUNIT)]),
            key_for_files(&[("a.al", CODEUNIT_Q)]),
            "an edited body must change the key"
        );
    }

    #[test]
    fn key_changes_when_a_file_is_added() {
        let _g = lock();
        assert_ne!(
            key_for_files(&[("a.al", CODEUNIT)]),
            key_for_files(&[
                ("a.al", CODEUNIT),
                ("b.al", "codeunit 50001 B { procedure R() begin end; }"),
            ]),
            "an added file must change the key"
        );
    }

    /// A file RENAME with byte-identical contents.
    ///
    /// DISCRIMINATION PROOF, pre-recorded: this row fails if `source_identity`
    /// reuses `SourceRoot::content_hash` for walked source, because
    /// `walk_al_source` folds only `f.text.as_bytes()` and never the
    /// `virtual_path`. It passes only because this module computes its own fold
    /// including the path.
    #[test]
    fn key_changes_when_a_file_is_renamed() {
        let _g = lock();
        assert_ne!(
            key_for_files(&[("a.al", CODEUNIT)]),
            key_for_files(&[("b.al", CODEUNIT)]),
            "a rename must change the key"
        );
    }

    /// The RE-SPLIT collision, hand-stated: two workspaces whose files hold the
    /// SAME concatenated bytes in path order, split at different boundaries.
    ///
    /// This is the verdict-changing case (the two parse completely differently)
    /// and it is exactly what `SourceRoot::content_hash` cannot distinguish: it
    /// folds neither path nor length.
    ///
    /// DISCRIMINATION, measured rather than assumed: this row is carried by the
    /// PATH being folded, NOT by the length prefix. Dropping the length prefix
    /// leaves it GREEN, because `a.al`+head+`b.al`+tail and `a.al`+whole+`b.al`
    /// still differ in where `b.al` lands. The length prefix has its own row
    /// below; do not read this one as covering it.
    #[test]
    fn key_changes_when_the_same_bytes_are_split_across_different_files() {
        let _g = lock();
        let (head, tail) = CODEUNIT.split_at(20);
        // "a.al" < "b.al", so both workspaces concatenate to the same bytes.
        assert_ne!(
            key_for_files(&[("a.al", head), ("b.al", tail)]),
            key_for_files(&[("a.al", CODEUNIT), ("b.al", "")]),
            "re-splitting identical bytes across file boundaries must change the key"
        );
    }

    /// The length prefix in [`put`], isolated.
    ///
    /// Hand-stated collision: without length prefixes the fold is a bare
    /// concatenation of (path, text) pairs, so ONE file named `a.alb.al` with
    /// empty text and TWO empty files `a.al` / `b.al` both fold to the bytes
    /// `a.alb.al`. Length prefixing is the only thing separating them.
    ///
    /// DISCRIMINATION PROOF (recorded): delete the length prefix from `put`
    /// and this row FAILS while every other key row still passes; restore it
    /// and it passes. This is the row that pins the prefix.
    #[test]
    fn key_changes_when_a_path_boundary_shifts_into_the_text() {
        let _g = lock();
        assert_ne!(
            key_for_files(&[("a.alb.al", "")]),
            key_for_files(&[("a.al", ""), ("b.al", "")]),
            "a path/text boundary shift must change the key (length prefixing)"
        );
    }

    #[test]
    fn key_is_stable_for_identical_input() {
        let _g = lock();
        assert_eq!(
            key_for_files(&[("a.al", CODEUNIT)]),
            key_for_files(&[("a.al", CODEUNIT)]),
            "identical content must produce the identical key"
        );
    }

    // ---------------------------------------------------------------------
    // 3. Fail-closed: every abnormal entry state recomputes
    // ---------------------------------------------------------------------

    /// Seed a tracer, PROVE it would have been served, then corrupt the entry and
    /// assert the run recomputes instead. The intermediate assertion is what
    /// makes each corruption row a real discrimination proof rather than a test
    /// that could pass because the entry was never reachable.
    fn assert_corruption_recomputes(name: &str, mutate: impl Fn(&Path)) {
        let _g = lock();
        let cache = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        workspace(ws.path(), &[("a.al", CODEUNIT)]);
        set_override(Some(cache.path().to_path_buf()));

        let key = key_of(ws.path());
        store(&key, &tracer_verdict());
        let path = cache.path().join(format!("{key}.json"));
        assert!(path.exists(), "{name}: precondition — entry was written");
        assert_eq!(
            fresh_coverage(ws.path()).unwrap().unknown,
            TRACER,
            "{name}: precondition — the intact entry IS served"
        );

        mutate(&path);
        let got = fresh_coverage(ws.path()).unwrap();
        set_override(None);
        assert_ne!(
            got.unknown, TRACER,
            "{name}: a corrupt entry must recompute, never be served"
        );
    }

    #[test]
    fn truncated_entry_recomputes() {
        assert_corruption_recomputes("truncated", |p| {
            let t = std::fs::read_to_string(p).unwrap();
            std::fs::write(p, &t[..t.len() / 2]).unwrap();
        });
    }

    #[test]
    fn wrong_schema_version_recomputes() {
        assert_corruption_recomputes("schema", |p| {
            let t = std::fs::read_to_string(p).unwrap();
            let old = format!("\"schema_version\":{PREFLIGHT_CACHE_SCHEMA}");
            assert_eq!(t.matches(&old).count(), 1, "patch must apply exactly once");
            std::fs::write(p, t.replace(&old, "\"schema_version\":424242")).unwrap();
        });
    }

    /// The payload is edited WITHOUT updating `self_hash` — exactly what bit-rot
    /// or a hand-edit looks like. Without the self-hash check this entry is
    /// well-formed and would be served.
    ///
    /// The corruption deliberately leaves `unknown` AT the tracer and flips
    /// `coverage_holds` instead. An earlier version rewrote `unknown` itself,
    /// which made this row pass for the wrong reason: with the self-hash check
    /// disabled the served payload no longer equalled the tracer either, so the
    /// assertion could not tell "recomputed" from "served a corrupted entry".
    /// Its discrimination proof came back GREEN-WHEN-BROKEN and exposed that.
    ///
    /// DISCRIMINATION PROOF (recorded): disable the `expect != entry.self_hash`
    /// branch and this row FAILS; restore it and it passes.
    #[test]
    fn self_hash_mismatch_recomputes() {
        assert_corruption_recomputes("self_hash", |p| {
            let t = std::fs::read_to_string(p).unwrap();
            let old = "\"coverage_holds\":true";
            assert_eq!(t.matches(old).count(), 1, "patch must apply exactly once");
            std::fs::write(p, t.replace(old, "\"coverage_holds\":false")).unwrap();
        });
    }

    #[test]
    fn key_mismatch_inside_the_entry_recomputes() {
        assert_corruption_recomputes("key", |p| {
            let t = std::fs::read_to_string(p).unwrap();
            let v: serde_json::Value = serde_json::from_str(&t).unwrap();
            let mut o = v.as_object().unwrap().clone();
            o.insert("key".into(), serde_json::json!("0".repeat(64)));
            std::fs::write(p, serde_json::to_string(&o).unwrap()).unwrap();
        });
    }

    /// An entry written by a DIFFERENT engine binary, internally CONSISTENT
    /// (its `self_hash` is correctly computed over its own foreign `binary`), so
    /// only the binary check itself can reject it.
    ///
    /// This replaces a weaker version that merely overwrote the `binary` field in
    /// place. That corruption also invalidated the self-hash, so the entry was
    /// rejected by the self-hash branch and the row stayed green even with the
    /// binary check disabled; its discrimination proof caught that. The binary
    /// check's real job is not tamper-detection (the self-hash covers that) but
    /// rejecting a WELL-FORMED entry from another engine version, which is what
    /// this now hand-states.
    ///
    /// DISCRIMINATION PROOF (recorded): disable the
    /// `entry.binary != binary_identity()` branch and this row FAILS; restore it
    /// and it passes.
    #[test]
    fn entry_from_a_different_binary_is_rejected() {
        let _g = lock();
        let cache = tempfile::tempdir().unwrap();
        set_override(Some(cache.path().to_path_buf()));

        let key = "b".repeat(64);
        let payload = tracer_verdict();
        let foreign = "f".repeat(64);
        let entry = Entry {
            schema_version: PREFLIGHT_CACHE_SCHEMA,
            key: key.clone(),
            binary: foreign.clone(),
            self_hash: compute_self_hash(PREFLIGHT_CACHE_SCHEMA, &key, &foreign, &payload),
            payload,
        };
        std::fs::write(
            cache.path().join(format!("{key}.json")),
            serde_json::to_string(&entry).unwrap(),
        )
        .unwrap();

        // Precondition, stated executably: an entry of this exact shape written
        // under THIS binary IS served, so the miss below is the binary check and
        // not some unrelated rejection.
        let mine = "c".repeat(64);
        store(&mine, &tracer_verdict());
        assert_eq!(
            lookup(&mine).map(|v| v.unknown),
            Some(TRACER),
            "precondition: a same-binary entry of this shape is served"
        );

        let got = lookup(&key);
        set_override(None);
        assert!(
            got.is_none(),
            "an entry from another engine binary must never be served"
        );
    }

    // ---------------------------------------------------------------------
    // 4. Stale content is never served
    // ---------------------------------------------------------------------

    #[test]
    fn editing_the_workspace_never_serves_the_stale_verdict() {
        let _g = lock();
        let cache = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        workspace(ws.path(), &[("a.al", CODEUNIT)]);
        set_override(Some(cache.path().to_path_buf()));

        store(&key_of(ws.path()), &tracer_verdict());
        assert_eq!(
            fresh_coverage(ws.path()).unwrap().unknown,
            TRACER,
            "precondition: the entry IS served before the edit"
        );

        std::fs::write(ws.path().join("a.al"), CODEUNIT_Q).unwrap();
        let after = fresh_coverage(ws.path()).unwrap();
        set_override(None);
        assert_ne!(
            after.unknown, TRACER,
            "an edited workspace must never be served the pre-edit verdict"
        );
    }

    // ---------------------------------------------------------------------
    // 5. `Err` is never cached
    // ---------------------------------------------------------------------

    #[test]
    fn could_not_verify_is_never_cached() {
        let _g = lock();
        let cache = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        // No `app.json` — the documented fail-closed layout, so the snapshot
        // build fails and `fresh_coverage` returns Err.
        std::fs::write(ws.path().join("a.al"), CODEUNIT).unwrap();
        set_override(Some(cache.path().to_path_buf()));

        let got = fresh_coverage(ws.path());
        let entries = std::fs::read_dir(cache.path()).unwrap().count();
        set_override(None);

        assert!(got.is_err(), "precondition: this workspace cannot verify");
        assert_eq!(
            entries, 0,
            "an Err captures transient environment and must never be persisted"
        );
    }

    // ---------------------------------------------------------------------
    // 6. A degraded (but Ok) verdict IS cached and round-trips exactly
    // ---------------------------------------------------------------------

    #[test]
    fn degraded_verdict_round_trips_field_for_field() {
        let _g = lock();
        let cache = tempfile::tempdir().unwrap();
        set_override(Some(cache.path().to_path_buf()));
        // Hand-stated: every field non-default, and `opaque_apps` deliberately
        // NOT in sorted order — pinning that load stores-as-is and never
        // re-sorts, because that order is formatter-visible output.
        let v = FreshCoverage {
            unknown: 7,
            coverage_holds: false,
            recovered_files: 3,
            opaque_apps: vec!["zeta".to_string(), "alpha".to_string()],
        };
        let key = "a".repeat(64);
        store(&key, &v);
        let got = lookup(&key);
        set_override(None);
        assert_eq!(
            got.as_ref(),
            Some(&v),
            "the verdict must round-trip exactly"
        );
    }
}
