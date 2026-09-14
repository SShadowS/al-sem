//! 1B.3a Task 4 (L3-validated semantic edge golden + route-applicability
//! contract) + 1B.3b Task 1 (committed ANONYMIZED frozen goldens + the
//! load-frozen audits + the `ENFORCE_CDO_WS` guard).
//!
//! # Golden floor
//!
//! [`mint_l3_validated_golden`]/[`mint_l3_trigger_golden`] capture the
//! L3-oracle target set per call site into a [`SemanticGolden`] (a sorted
//! list keyed by column-ignoring [`GoldenSiteKey`]).
//! [`assert_against_semantic_golden`] compares a fresh canonical edge batch
//! against a (plaintext, in-repo-fixture-scale) golden and classifies every
//! site into: `match`, `fresh_wrong`, `fresh_missing`, `fresh_extra`,
//! `fresh_novel`, or `golden_missing`.
//!
//! # The critical invariant
//!
//! **`fresh_wrong.is_empty()`** — fresh must never confidently emit a target
//! that L3 says is wrong.  A per-site Histogram cannot catch this: it can
//! count "resolved" or "unknown" but cannot tell you WHICH target was chosen.
//! This golden does.
//!
//! # 1B.3b: committed, anonymized, frozen — no live L3 in the gate path
//!
//! The CDO-scale golden is too large and too proprietary to mint live on
//! every run. Instead: [`mint_l3_validated_golden`] / [`mint_l3_trigger_golden`]
//! / `crate::program::l3_mint::project_l3_event_rows` run ONCE, on a dev
//! machine with CDO access, via the dev-mint tool (`src/bin/mint-goldens.rs`,
//! OUTSIDE `src/program/resolve`). The tool ANONYMIZES every identifying
//! string (via [`anon::anon`] — see that module's docs for the full
//! domain-separation + HMAC-governance writeup) and writes the result to
//! three COMMITTED files under `tests/goldens/semantic-edges/`:
//! `cdo-anon.json` (Member/Interface), `cdo-trigger-anon.json`
//! (ImplicitTrigger), `cdo-event-anon.json` (EventFlow).
//! [`run_cdo_semantic_audit`]/[`run_cdo_trigger_audit`]/[`run_cdo_event_audit`]
//! LOAD these committed goldens and anonymize the FRESH side with the SAME
//! function at audit time.
//!
//! **1B.3b Task 3: the gate path is now fully L3-INDEPENDENT.** Neither this
//! module nor `differential.rs` imports `engine::l3`/`engine::l2` at all —
//! not even the three `run_cdo_*_audit` functions, nor [`route_applicability`].
//! The ONLY surviving L3-oracle access point in the library is
//! [`crate::program::l3_mint`] (OUTSIDE `src/program/resolve`), which
//! [`mint_l3_validated_golden`]/[`mint_l3_trigger_golden`] below delegate to —
//! and which only the dev-mint tool and the opt-in `REGEN_TEMP_GOLDENS`
//! fixture-regen test path ever call. The four live dual-run "fresh vs L3"
//! comparison gates that used to validate the fresh resolver on every
//! CDO-gated test run (`run_harness`/`run_site_harness`/
//! `run_resolution_harness`/`run_member_resolution_harness`/
//! `run_implicit_trigger_harness`/`run_event_flow_gate`, all in
//! `differential.rs`) were deleted in 1B.3b Task 3 — their coverage is now
//! the frozen goldens above plus [`route_applicability`]'s ported fan-out
//! applicability teeth.
//!
//! # Route-applicability contract
//!
//! [`route_applicability`] verifies the structural witness↔evidence contract
//! on every route and delegates the ABI ingestion check to
//! [`abi_ingestion_integrity`].
//!
//! # CDO audits
//!
//! [`run_cdo_semantic_audit`]/[`run_cdo_trigger_audit`]/[`run_cdo_event_audit`]
//! run the load-frozen comparison over a real workspace (env-gated; the
//! caller checks `CDO_WS` and applies the `ENFORCE_CDO_WS` hard-fail guard —
//! see `tests/program_resolve_harness.rs`).

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::program::graph::ProgramGraph;
use crate::program::l3_mint::{project_l3, project_l3_implicit_trigger_in_scope};
use al_syntax::IdentifierFoldExt;

use crate::program::node::{AppRef, ObjKey, ObjectKind, ObjectNodeId};
use crate::program::node_extract::{AbiParams, ObjectNode};
use crate::program::resolve::abi_check::{
    RawAbiIndex, abi_ingestion_integrity, build_raw_abi_index_from_snapshot,
};
use crate::program::resolve::anon::{self, AnonId};
use crate::program::resolve::applicability::{
    RecordOpCtx, RecordOpKind, RunTrigger, implicit_trigger_route_applicable,
    instance_builtin_route_applicable, interface_route_applicable,
};
use crate::program::resolve::differential::{
    CanonicalEdge, CanonicalEventRow, CanonicalKey, CanonicalTarget, project_fresh,
    verify_event_subscriber_route, witness_contract_holds,
};
use crate::program::resolve::edge::{
    DispatchShape, Edge, EdgeKind, RouteTarget, SiteId, callee_fp,
};
use crate::program::resolve::extract::{CalleeShape, extract_sites_for_routine};
use crate::program::resolve::index::ResolveIndex;
use crate::program::resolve::member_catalog::{MemberCatalogKind, member_builtin};
use crate::program::resolve::receiver::{FrameworkKind, ReceiverType, infer_receiver_type};
use crate::program::sig_fp::source_routine_node_id;
use crate::snapshot::ParsedUnit;

// ---------------------------------------------------------------------------
// Column-ignoring site key (serde-able)
// ---------------------------------------------------------------------------

/// Serde-able, column-ignoring key for one call site in the semantic golden.
///
/// Omits the column offset because L3 uses UTF-16 columns while the fresh
/// side uses byte columns — they agree on ASCII but may differ by a small
/// delta on non-ASCII identifiers.  The strong key `(unit, line, callee_fp)`
/// mirrors the invariant used by [`crate::program::resolve::differential::match_sites`].
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GoldenSiteKey {
    pub from_app_guid: String,
    pub from_object_kind: String,
    pub from_object_lc: String,
    pub from_routine_lc: String,
    /// `EdgeKind` discriminant: 0=Call, 1=Run, 2=ImplicitTrigger, 3=EventFlow.
    pub edge_kind: u8,
    pub unit: String,
    pub line: u32,
    pub callee_fp: u64,
}

/// Serde-able mirror of
/// [`CanonicalTarget`][crate::program::resolve::differential::CanonicalTarget].
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GoldenTarget {
    pub kind: u8,
    pub app: Option<String>,
    pub object_lc: String,
    pub routine_lc: Option<String>,
}

// ---------------------------------------------------------------------------
// SemanticGolden
// ---------------------------------------------------------------------------

/// One entry in the semantic golden: a call-site key paired with the set of
/// targets the L3 oracle resolved for that site.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GoldenEntry {
    pub site: GoldenSiteKey,
    /// Targets L3 resolved for this site.  Empty when L3 could not resolve.
    pub targets: BTreeSet<GoldenTarget>,
}

/// The L3-validated semantic golden: a sorted list of (site, targets) pairs.
///
/// Stored as a `Vec` so serde_json can serialize it (JSON maps require string
/// keys; `GoldenSiteKey` is a struct).  The list is always sorted by `site`
/// for determinism and binary-search lookups.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SemanticGolden {
    pub entries: Vec<GoldenEntry>,
}

impl SemanticGolden {
    /// Build from a `BTreeMap` (already sorted, so insertion order is preserved).
    fn from_map(map: std::collections::BTreeMap<GoldenSiteKey, BTreeSet<GoldenTarget>>) -> Self {
        SemanticGolden {
            entries: map
                .into_iter()
                .map(|(site, targets)| GoldenEntry { site, targets })
                .collect(),
        }
    }

    /// Lookup targets for `key` (binary search on sorted `entries`).
    fn get(&self, key: &GoldenSiteKey) -> Option<&BTreeSet<GoldenTarget>> {
        self.entries
            .binary_search_by(|e| e.site.cmp(key))
            .ok()
            .map(|i| &self.entries[i].targets)
    }
}

// ---------------------------------------------------------------------------
// 1B.3b Task 1: anonymized frozen-golden types (committed, no plaintext)
//
// See `anon.rs`'s module docs for the full governance writeup (HMAC vs salt,
// domain separation, the re-hash-don't-decrypt principle). In short:
// [`AnonSiteKey`]/[`AnonTarget`] are the SAME shape as [`GoldenSiteKey`]/
// [`GoldenTarget`] with every identifying string field replaced by an
// [`AnonId`]; non-sensitive labels (`from_object_kind`, `edge_kind`, `line`,
// `kind`) stay in CLEARTEXT so an anonymized diff still has semantic anchors.
// These are what gets WRITTEN to the committed `cdo-anon.json` /
// `cdo-trigger-anon.json` (same shape, different `site_domain` — see
// [`anon::SITE_DOMAIN_V1`] vs [`anon::TRIGGER_OP_DOMAIN_V1`]).
// ---------------------------------------------------------------------------

/// Schema version stamped into every anonymized committed golden
/// (`cdo-anon.json` / `cdo-trigger-anon.json` / `cdo-event-anon.json`). Bump
/// when the anonymization scheme or a golden's field shape changes; the
/// public-CI metadata-validation test asserts every committed golden carries
/// this value.
///
/// Bumped 1 -> 2 by the 1B.3b Task 1 fix: switched [`anon::anon`]'s key from a
/// lost session-local `CDO_ANON_KEY` to the fixed, committed `ANON_SALT` (see
/// `anon.rs`'s module docs), which changes every emitted [`AnonId`] for the
/// same plaintext, AND added the [`MintMetadata`] field to both
/// [`AnonSemanticGolden`] and [`AnonEventGolden`] (a golden's field shape
/// change).
pub const ANON_GOLDEN_SCHEMA_VERSION: u32 = 2;

/// Mint-time provenance metadata stamped into every committed golden (1B.3b
/// Task 1 fix, Fix 4): the CDO workspace's git HEAD SHA and dirty state at
/// mint time, captured by [`workspace_git_info`]. Audit time re-probes the
/// CURRENT workspace and warns on a mismatch -- or PANICS under
/// `ENFORCE_CDO_WS=1`; see the drift check, and
/// `run_cdo_semantic_audit`/`run_cdo_trigger_audit`/`run_cdo_event_audit`'s
/// drift-warning step. `#[serde(default)]` on both fields so a golden minted
/// before this field existed (or from a non-git workspace export) still
/// deserializes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintMetadata {
    /// `git -C <CDO_WS> rev-parse HEAD` at mint time. `None` when the
    /// workspace isn't inside a git repo, `git` isn't on `PATH`, or the
    /// command failed — this is best-effort provenance, never a hard
    /// requirement (mint/audit must still work against a non-git workspace
    /// export).
    #[serde(default)]
    pub workspace_git_sha: Option<String>,
    /// `true` when `git -C <CDO_WS> status --porcelain` produced non-empty
    /// output at mint time (uncommitted changes present). `None` when the
    /// git probe failed (same best-effort caveat as `workspace_git_sha`).
    #[serde(default)]
    pub workspace_dirty: Option<bool>,
    /// SHA-256 over the workspace `.alpackages` symbol closure at mint time
    /// -- see [`dependency_closure_digest`].
    ///
    /// **Why this exists.** `workspace_git_sha` + `workspace_dirty` describe only
    /// TRACKED files. `.alpackages` is gitignored, so a workspace can report
    /// `dirty: false` while the dependency symbols the resolver actually reads
    /// have been swapped wholesale. Cross-app resolution reads those bytes, so a
    /// golden pinned by git state alone is only HALF pinned -- and this repo has
    /// already lost a baseline that looked pinned and was not. `None` for a golden
    /// minted before this field existed, or a workspace with no `.alpackages`.
    #[serde(default)]
    pub dependency_closure_sha256: Option<String>,
}

/// SHA-256 over the workspace `.alpackages` symbol closure: every file NAME and
/// its BYTES, in sorted-name order.
///
/// `None` when the directory is absent or empty -- a workspace without dependency
/// symbols has no closure to pin, which is a legitimate state and must never be
/// confused with "the closure changed".
#[must_use]
pub fn dependency_closure_digest(workspace_root: &Path) -> Option<String> {
    let dir = workspace_root.join(".alpackages");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .collect();
    if files.is_empty() {
        return None;
    }
    files.sort();
    let mut hasher = Sha256::new();
    for f in &files {
        let name = f.file_name()?.to_string_lossy().to_string();
        hasher.update(name.as_bytes());
        hasher.update([0u8]);
        hasher.update(std::fs::read(f).ok()?);
        hasher.update([0u8]);
    }
    Some(format!("{:x}", hasher.finalize()))
}

/// Probe `workspace_root`'s git HEAD SHA + dirty state via the `git` CLI
/// (1B.3b Task 1 fix, Fix 4). Best-effort: returns `(None, None)` fields when
/// `workspace_root` isn't inside a git repo, `git` isn't on `PATH`, or either
/// command fails — this is provenance metadata, not a hard requirement. Used
/// by the dev-mint tool (to STAMP [`MintMetadata`] at mint time) and by the
/// `run_cdo_*_audit` functions (to compare the CURRENT workspace against the
/// loaded golden's stamp and warn on drift).
#[must_use]
pub fn workspace_git_info(workspace_root: &Path) -> (Option<String>, Option<bool>) {
    let sha = std::process::Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let dirty = std::process::Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .arg("status")
        .arg("--porcelain")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| !s.trim().is_empty());

    (sha, dirty)
}

/// Emit a `WARNING` on stderr when the CURRENT `workspace_root`'s git SHA/
/// dirty state differs from `stamped` (the golden's mint-time
/// **Ungated: warns. Under `ENFORCE_CDO_WS=1`: PANICS.** The old never-fails
/// behaviour is exactly how the previous baseline rotted -- every run printed a
/// drift warning for two months while the audit it guarded paired ZERO sites and
/// still reported a pass. A gated run is the one place an unreproducible baseline
/// must stop the build; ungated developer runs keep the warning, because drift
/// there is ordinary and asserts nothing.
/// last mint), not a resolver regression; see [`MintMetadata`]'s doc comment.
fn warn_on_workspace_drift(stamped: &MintMetadata, workspace_root: &Path) {
    let (current_sha, current_dirty) = workspace_git_info(workspace_root);
    let current_closure = dependency_closure_digest(workspace_root);
    // A stamp of `None` predates this field — do not report drift against a
    // golden that never recorded a closure, or every older golden becomes
    // permanently "drifted" and the signal is noise again.
    let closure_drifted = stamped
        .dependency_closure_sha256
        .as_ref()
        .is_some_and(|st| current_closure.as_ref() != Some(st));
    let git_drifted =
        current_sha != stamped.workspace_git_sha || current_dirty != stamped.workspace_dirty;
    if !git_drifted && !closure_drifted {
        return;
    }

    let msg = format!(
        "CDO workspace drifted from the golden mint stamp.\n  \
         git SHA: stamped {:?} (dirty={:?}), current {:?} (dirty={:?})\n  \
         .alpackages closure: stamped {:?}, current {:?}\n  \
         Audit diffs may reflect workspace drift rather than engine regressions \
         -- re-mint to advance the pin (see src/bin/mint-goldens.rs).",
        stamped.workspace_git_sha,
        stamped.workspace_dirty,
        current_sha,
        current_dirty,
        stamped.dependency_closure_sha256.as_deref(),
        current_closure.as_deref(),
    );

    // Under ENFORCE_CDO_WS=1 this is a HARD FAILURE, not a warning.
    //
    // Warning-only is exactly how the previous baseline rotted: the goldens were
    // minted from a tree whose SHA later existed in no checkout at all, every run
    // printed this message, and nobody acted on it for two months -- while the
    // audits it guards silently paired ZERO sites and still reported a pass. A
    // gated run (`scripts/cdo-gate`) is the one context where an unreproducible
    // baseline must stop the build rather than narrate at it. Ungated developer
    // runs keep the warning, because drift there is ordinary and claims nothing.
    assert!(
        std::env::var("ENFORCE_CDO_WS").as_deref() != Ok("1"),
        "{msg}"
    );
    eprintln!("WARNING: {msg}");
}

/// Anonymized, serde-able mirror of [`GoldenSiteKey`]. The four identifying
/// string fields (`from_app_guid`, `from_object_lc`, `from_routine_lc`,
/// `unit`) and the `callee_fp` fingerprint are EACH individually hashed via
/// [`anon::anon`] under `site_domain`. `from_object_kind` (an object-type
/// category, e.g. `"codeunit"`), `edge_kind` (the `EdgeKind` discriminant),
/// and `line` (a bare source line number, meaningless without the now-hashed
/// `unit`) are non-sensitive and stay CLEARTEXT.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AnonSiteKey {
    pub from_app_id: AnonId,
    pub from_object_kind: String,
    pub from_object_id: AnonId,
    pub from_routine_id: AnonId,
    pub edge_kind: u8,
    pub unit_id: AnonId,
    pub line: u32,
    pub callee_id: AnonId,
}

/// Anonymized, serde-able mirror of [`GoldenTarget`]. `kind` (the object-kind
/// tag, or the 254/255 AbiSymbol/Builtin sentinels) is a non-sensitive label
/// kept CLEARTEXT; `app`+`object_lc` are combined into one [`AnonId`]
/// (app-scoped object identity) and `routine_lc` is hashed separately — both
/// under [`anon::TARGET_DOMAIN_V1`], shared by every golden (a "target" means
/// the same thing regardless of which golden it appears in).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AnonTarget {
    pub kind: u8,
    pub object_id: AnonId,
    pub routine_id: Option<AnonId>,
}

/// One entry in an anonymized golden: an [`AnonSiteKey`] paired with its
/// anonymized target set.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnonGoldenEntry {
    pub site: AnonSiteKey,
    pub targets: BTreeSet<AnonTarget>,
}

/// The committed anonymized golden shape shared by `cdo-anon.json`
/// (`site_domain = `[`anon::SITE_DOMAIN_V1`]) and `cdo-trigger-anon.json`
/// (`site_domain = `[`anon::TRIGGER_OP_DOMAIN_V1`]). Always sorted by `site`
/// (determinism + binary-search lookups), same convention as
/// [`SemanticGolden`].
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AnonSemanticGolden {
    pub schema_version: u32,
    /// Mint-time CDO workspace provenance (1B.3b Task 1 fix). `#[serde(default)]`
    /// so a pre-fix golden without this field still deserializes (though it
    /// would fail the `schema_version` check first in practice).
    #[serde(default)]
    pub metadata: MintMetadata,
    pub entries: Vec<AnonGoldenEntry>,
}

impl AnonSemanticGolden {
    fn get(&self, key: &AnonSiteKey) -> Option<&BTreeSet<AnonTarget>> {
        self.entries
            .binary_search_by(|e| e.site.cmp(key))
            .ok()
            .map(|i| &self.entries[i].targets)
    }
}

/// Anonymized, serde-able EventFlow pair key — see
/// `differential.rs::CanonicalEventRow`'s docs for why this is keyed by
/// `CanonicalKey` rather than L3's proprietary `stable_routine_id` scheme.
/// Both `publisher_id` and `subscriber_id` hash the FULL `CanonicalKey`
/// (app_guid + object_kind + object_lc + routine_lc) under
/// [`anon::EVENT_PAIR_DOMAIN_V1`]; `event_name_id` hashes the bare event name
/// under the same domain.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AnonEventPairKey {
    pub publisher_id: AnonId,
    pub event_name_id: AnonId,
    pub subscriber_id: AnonId,
}

/// One entry in the committed `cdo-event-anon.json`: an anonymized pub→sub
/// pair plus the CLEARTEXT resolved publisher arity (a bare parameter count —
/// non-identifying).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnonEventEntry {
    pub pair: AnonEventPairKey,
    pub publisher_arity: Option<usize>,
}

/// The committed `cdo-event-anon.json` shape.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AnonEventGolden {
    pub schema_version: u32,
    /// Mint-time CDO workspace provenance (1B.3b Task 1 fix) — see
    /// [`AnonSemanticGolden::metadata`].
    #[serde(default)]
    pub metadata: MintMetadata,
    pub entries: Vec<AnonEventEntry>,
}

/// Hash `(app, object_lc)` into one [`AnonId`] under [`anon::TARGET_DOMAIN_V1`]
/// — app-scoped object identity (combined, not two separate ids, so a small
/// numeric `object_lc` from two different apps cannot collide).
fn anon_target_object_id(app: &Option<String>, object_lc: &str) -> AnonId {
    let canon = format!("{}\u{1}{object_lc}", app.as_deref().unwrap_or(""));
    anon::anon(anon::TARGET_DOMAIN_V1, &canon)
}

fn anon_target_routine_id(routine_lc: &str) -> AnonId {
    anon::anon(anon::TARGET_DOMAIN_V1, routine_lc)
}

/// Anonymize one [`GoldenTarget`] under [`anon::TARGET_DOMAIN_V1`].
#[must_use]
pub fn anonymize_target(t: &GoldenTarget) -> AnonTarget {
    AnonTarget {
        kind: t.kind,
        object_id: anon_target_object_id(&t.app, &t.object_lc),
        routine_id: t.routine_lc.as_deref().map(anon_target_routine_id),
    }
}

/// Anonymize one [`GoldenSiteKey`] under `site_domain` (either
/// [`anon::SITE_DOMAIN_V1`] or [`anon::TRIGGER_OP_DOMAIN_V1`]).
#[must_use]
pub fn anonymize_site_key(key: &GoldenSiteKey, site_domain: &str) -> AnonSiteKey {
    AnonSiteKey {
        from_app_id: anon::anon(site_domain, &key.from_app_guid),
        from_object_kind: key.from_object_kind.clone(),
        from_object_id: anon::anon(site_domain, &key.from_object_lc),
        from_routine_id: anon::anon(site_domain, &key.from_routine_lc),
        edge_kind: key.edge_kind,
        unit_id: anon::anon(site_domain, &key.unit),
        line: key.line,
        callee_id: anon::anon(site_domain, &format!("{}", key.callee_fp)),
    }
}

/// Record every hashed field of `(plain, anon_key)` into `deanon`
/// (`AnonId.0 -> human-readable plaintext`) — the local, GITIGNORED
/// `cdo-deanon-map.json` accumulator. See `anon.rs`'s module docs ("the
/// re-hash-don't-decrypt principle"): this is the ONLY place plaintext is
/// ever written back out, and only to a local, never-committed file.
fn record_site_deanon(
    plain: &GoldenSiteKey,
    anon_key: &AnonSiteKey,
    deanon: &mut BTreeMap<String, String>,
) {
    deanon
        .entry(anon_key.from_app_id.0.clone())
        .or_insert_with(|| format!("app_guid={}", plain.from_app_guid));
    deanon
        .entry(anon_key.from_object_id.0.clone())
        .or_insert_with(|| {
            format!(
                "object_lc={} (kind={})",
                plain.from_object_lc, plain.from_object_kind
            )
        });
    deanon
        .entry(anon_key.from_routine_id.0.clone())
        .or_insert_with(|| format!("routine_lc={}", plain.from_routine_lc));
    deanon
        .entry(anon_key.unit_id.0.clone())
        .or_insert_with(|| format!("unit={}", plain.unit));
    deanon
        .entry(anon_key.callee_id.0.clone())
        .or_insert_with(|| format!("callee_fp={}", plain.callee_fp));
}

fn record_target_deanon(
    plain: &GoldenTarget,
    anon_t: &AnonTarget,
    deanon: &mut BTreeMap<String, String>,
) {
    deanon.entry(anon_t.object_id.0.clone()).or_insert_with(|| {
        format!(
            "app={:?} object_lc={} (kind={})",
            plain.app, plain.object_lc, plain.kind
        )
    });
    if let (Some(rid), Some(rlc)) = (&anon_t.routine_id, &plain.routine_lc) {
        deanon
            .entry(rid.0.clone())
            .or_insert_with(|| format!("routine_lc={rlc}"));
    }
}

/// Anonymize `golden` under `site_domain`, ALSO recording every hashed
/// field's plaintext into `deanon`. The dev-mint tool's primary entry point —
/// mint + anonymize + populate the local de-anon map in one pass.
#[must_use]
pub fn anonymize_golden_with_deanon(
    golden: &SemanticGolden,
    site_domain: &str,
    deanon: &mut BTreeMap<String, String>,
) -> AnonSemanticGolden {
    let mut entries: Vec<AnonGoldenEntry> = golden
        .entries
        .iter()
        .map(|e| {
            let asite = anonymize_site_key(&e.site, site_domain);
            record_site_deanon(&e.site, &asite, deanon);
            let atargets: BTreeSet<AnonTarget> = e
                .targets
                .iter()
                .map(|t| {
                    let at = anonymize_target(t);
                    record_target_deanon(t, &at, deanon);
                    at
                })
                .collect();
            AnonGoldenEntry {
                site: asite,
                targets: atargets,
            }
        })
        .collect();
    entries.sort_by(|a, b| a.site.cmp(&b.site));
    AnonSemanticGolden {
        schema_version: ANON_GOLDEN_SCHEMA_VERSION,
        // Caller (the dev-mint tool) overwrites this with the real mint-time
        // stamp via `workspace_git_info`; callers that don't care (e.g. the
        // runtime audits anonymizing the fresh side for an in-memory
        // comparison, never serialized) leave the default.
        metadata: MintMetadata::default(),
        entries,
    }
}

/// Anonymize `golden` under `site_domain` without recording a de-anon map
/// (callers that don't have/want a local map — e.g. a one-off comparison).
#[must_use]
pub fn anonymize_golden(golden: &SemanticGolden, site_domain: &str) -> AnonSemanticGolden {
    let mut scratch = BTreeMap::new();
    anonymize_golden_with_deanon(golden, site_domain, &mut scratch)
}

/// Hash a [`CanonicalKey`] (all four fields, joined) into one [`AnonId`]
/// under [`anon::EVENT_PAIR_DOMAIN_V1`].
fn anon_canonical_key(k: &CanonicalKey, domain: &str) -> AnonId {
    let s = format!(
        "{}\u{1}{}\u{1}{}\u{1}{}",
        k.app_guid, k.object_kind, k.object_lc, k.routine_lc
    );
    anon::anon(domain, &s)
}

/// Anonymize a batch of [`CanonicalEventRow`]s into the committed
/// `cdo-event-anon.json` shape, recording plaintext into `deanon`.
#[must_use]
pub fn anonymize_event_rows_with_deanon(
    rows: &[CanonicalEventRow],
    deanon: &mut BTreeMap<String, String>,
) -> AnonEventGolden {
    let mut entries: Vec<AnonEventEntry> = rows
        .iter()
        .map(|r| {
            let pair = AnonEventPairKey {
                publisher_id: anon_canonical_key(&r.publisher, anon::EVENT_PAIR_DOMAIN_V1),
                event_name_id: anon::anon(anon::EVENT_PAIR_DOMAIN_V1, &r.event_name_lc),
                subscriber_id: anon_canonical_key(&r.subscriber, anon::EVENT_PAIR_DOMAIN_V1),
            };
            deanon
                .entry(pair.publisher_id.0.clone())
                .or_insert_with(|| {
                    format!(
                        "publisher={}:{}:{}",
                        r.publisher.object_kind, r.publisher.object_lc, r.publisher.routine_lc
                    )
                });
            deanon
                .entry(pair.event_name_id.0.clone())
                .or_insert_with(|| format!("event_name_lc={}", r.event_name_lc));
            deanon
                .entry(pair.subscriber_id.0.clone())
                .or_insert_with(|| {
                    format!(
                        "subscriber={}:{}:{}",
                        r.subscriber.object_kind, r.subscriber.object_lc, r.subscriber.routine_lc
                    )
                });
            AnonEventEntry {
                pair,
                publisher_arity: r.publisher_arity,
            }
        })
        .collect();
    entries.sort_by(|a, b| a.pair.cmp(&b.pair));
    AnonEventGolden {
        schema_version: ANON_GOLDEN_SCHEMA_VERSION,
        // See the analogous comment in `anonymize_golden_with_deanon` —
        // overwritten by the dev-mint tool's caller with the real stamp.
        metadata: MintMetadata::default(),
        entries,
    }
}

// ---------------------------------------------------------------------------
// Diff types
// ---------------------------------------------------------------------------

/// A site where the fresh resolver emitted confident (non-Unresolved) targets
/// that differ from the L3-oracle targets.
///
/// This is the **confidently-wrong** class — a Histogram cannot detect it.
#[derive(Clone, Debug)]
pub struct FreshWrong {
    pub site: GoldenSiteKey,
    pub fresh_targets: BTreeSet<GoldenTarget>,
    pub l3_targets: BTreeSet<GoldenTarget>,
}

/// A site formerly in `fresh_wrong` where fresh's targets REFINE L3's target —
/// fresh is MORE precise (Phase-4 Interface/Polymorphic fan-out or superset).
/// Not a bug; the graph's `implements` relationship confirms the refinement.
pub type FreshAheadDispatch = FreshWrong;

/// A site where L3 resolved to a concrete target but fresh emitted empty targets.
#[derive(Clone, Debug)]
pub struct FreshMissing {
    pub site: GoldenSiteKey,
    pub l3_targets: BTreeSet<GoldenTarget>,
}

/// A site where fresh resolved to targets but L3 had an empty target set.
/// Fresh was ahead of L3 — a verified improvement.
#[derive(Clone, Debug)]
pub struct FreshExtra {
    pub site: GoldenSiteKey,
    pub fresh_targets: BTreeSet<GoldenTarget>,
}

/// Full classification from comparing fresh edges against the semantic golden.
#[derive(Clone, Debug, Default)]
pub struct SemanticDiff {
    /// Total paired sites (present in both fresh and golden on the same key).
    pub total_paired: usize,
    /// Paired sites where fresh and L3 targets agree exactly.
    pub matches: usize,
    /// Paired sites where fresh confidently resolved to the WRONG target.
    pub fresh_wrong: Vec<FreshWrong>,
    /// Paired sites where L3 resolved but fresh emitted empty (a gap).
    pub fresh_missing: Vec<FreshMissing>,
    /// Paired sites where fresh resolved and L3 had empty (a win).
    pub fresh_extra: Vec<FreshExtra>,
    /// Fresh sites that have no golden entry (edges L3 never saw, e.g.
    /// `EventFlow`, `ImplicitTrigger`, dynamic ObjectRun sites).
    pub fresh_novel: usize,
    /// Golden sites with no fresh peer (fresh emitted no site for this key).
    pub golden_missing: usize,
}

// ---------------------------------------------------------------------------
// CDO audit report
// ---------------------------------------------------------------------------

/// Result of the CDO/L3 semantic audit over a real workspace.
#[derive(Clone, Debug, Default)]
pub struct CdoSemanticAuditReport {
    /// 1B.3b Task 1: `true` when the committed `cdo-anon.json` golden loaded
    /// and parsed successfully. The `ENFORCE_CDO_WS=1` guard hard-fails on
    /// `false` — see `tests/program_resolve_harness.rs`'s `cdo_ws_or_enforce`.
    pub golden_loaded: bool,
    pub l3_total: usize,
    pub fresh_total: usize,
    pub paired: usize,
    /// Total sites where fresh and L3 differ and both are non-empty.
    /// Equals `fresh_ahead_dispatch_count + genuine_wrong_count`.
    pub fresh_wrong_count: usize,
    /// Sites adjudicated as "fresh is more precise" (interface fan-out / superset).
    pub fresh_ahead_dispatch_count: usize,
    /// Sites adjudicated as genuinely wrong (disjoint target — a real bug).
    pub genuine_wrong_count: usize,
    /// Genuine_wrong site keys exposed for the HARD GATE set-membership check.
    /// The test asserts every site's `(unit, line, callee_fp)` is present in
    /// the committed manifest
    /// (`tests/goldens/semantic-edges/known-genuine-divergences.json`).
    pub genuine_wrong_sites: Vec<GoldenSiteKey>,
    pub fresh_missing_count: usize,
    pub fresh_extra_count: usize,
    pub fresh_novel: usize,
    pub golden_missing: usize,
    /// SHA-256 hex digest over the sorted site→(l3_targets, fresh_targets) pairs.
    /// Deterministic across runs; used as a pinnable CDO audit fingerprint.
    pub digest: String,
    /// What the adjudication overlay actually did on this run. Empty/default
    /// for a RAW audit ([`run_cdo_semantic_audit_on_raw`]), which applies none.
    pub overlay: OverlayOutcome,
}

/// Result of the L3/fresh ImplicitTrigger frozen-golden audit
/// (`cdo-trigger-anon.json`, `site_domain = `[`anon::TRIGGER_OP_DOMAIN_V1`]).
///
/// 1B.3b scope note: unlike [`CdoSemanticAuditReport`], this report does NOT
/// adjudicate `fresh_wrong` into fresh-ahead-dispatch vs genuine-wrong — that
/// classification (and the `known-genuine-divergences.json` manifest) is
/// scoped to the Member/Interface golden only. ImplicitTrigger correctness is
/// covered by this frozen audit plus the `tests/fixtures/implicit-trigger`
/// fixture (`implicit_trigger_fixture_resolves_exact_target_set`) and the
/// ported applicability teeth (`route_applicability`) — the old live,
/// CDO-gated `run_implicit_trigger_harness` dual-run gate was deleted in
/// 1B.3b Task 3. This audit exists to PROVE the frozen-load-and-anonymize
/// mechanism works for the ImplicitTrigger dispatch kind and to back the
/// `ENFORCE_CDO_WS` guard's `checked_sites>0` requirement.
#[derive(Clone, Debug, Default)]
pub struct AnonTriggerAuditReport {
    pub golden_loaded: bool,
    pub l3_total: usize,
    pub fresh_total: usize,
    pub total_paired: usize,
    pub matches: usize,
    pub fresh_wrong_count: usize,
    pub fresh_missing: usize,
    pub fresh_extra: usize,
    pub fresh_novel: usize,
    pub golden_missing: usize,
    pub digest: String,
}

/// Result of the L3/fresh EventFlow frozen-golden audit
/// (`cdo-event-anon.json`). Arity-agnostic pair-set comparison only (mirrors
/// the old `run_event_flow_gate`'s Stage-1 join, not its Stage-2 arity
/// adjudication) — see [`AnonTriggerAuditReport`]'s scope note; the same
/// reasoning applies here. EventFlow correctness is covered by this frozen
/// audit plus the `tests/fixtures/events` fixture
/// (`event_fixture_two_stage_join`) and the ported event-route teeth — the
/// old live, CDO-gated `run_event_flow_gate` dual-run gate was deleted in
/// 1B.3b Task 3.
#[derive(Clone, Debug, Default)]
pub struct AnonEventAuditReport {
    pub golden_loaded: bool,
    pub l3_total: usize,
    pub fresh_total: usize,
    pub matched_pairs: usize,
    pub pair_l3_only: usize,
    pub pair_fresh_only: usize,
    pub digest: String,
}

// ---------------------------------------------------------------------------
// 1B.3b Task 1: load-frozen audit infrastructure (anonymized diff)
// ---------------------------------------------------------------------------

/// An anonymized [`FreshWrong`] — same meaning, [`AnonTarget`] instead of
/// [`GoldenTarget`].
#[derive(Clone, Debug)]
pub struct AnonFreshWrong {
    pub site: AnonSiteKey,
    pub fresh_targets: BTreeSet<AnonTarget>,
    pub l3_targets: BTreeSet<AnonTarget>,
}

/// Anonymized counterpart of [`SemanticDiff`]. `fresh_missing`/`fresh_extra`
/// are plain counts (not `Vec`s) — the load-frozen audits don't need the
/// per-site detail beyond `fresh_wrong` (which DOES carry detail, because
/// that's the bucket the genuine-wrong adjudication and the deanon map need).
#[derive(Clone, Debug, Default)]
pub struct AnonSemanticDiff {
    pub total_paired: usize,
    pub matches: usize,
    pub fresh_wrong: Vec<AnonFreshWrong>,
    pub fresh_missing: usize,
    pub fresh_extra: usize,
    pub fresh_novel: usize,
    pub golden_missing: usize,
}

fn semantic_edges_golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/semantic-edges")
}

/// Path to the committed Member/Interface anonymized golden.
#[must_use]
pub fn cdo_anon_golden_path() -> PathBuf {
    semantic_edges_golden_dir().join("cdo-anon.json")
}

/// Path to the committed ImplicitTrigger anonymized golden.
#[must_use]
pub fn cdo_trigger_anon_golden_path() -> PathBuf {
    semantic_edges_golden_dir().join("cdo-trigger-anon.json")
}

/// Path to the committed EventFlow anonymized golden.
#[must_use]
pub fn cdo_event_anon_golden_path() -> PathBuf {
    semantic_edges_golden_dir().join("cdo-event-anon.json")
}

/// Path to the GITIGNORED local de-anonymization map
/// (`AnonId.0 -> human-readable plaintext`). NEVER committed — see `anon.rs`'s
/// module docs.
#[must_use]
pub fn cdo_deanon_map_path() -> PathBuf {
    semantic_edges_golden_dir().join("cdo-deanon-map.json")
}

/// Load + validate a committed [`AnonSemanticGolden`] (`cdo-anon.json` /
/// `cdo-trigger-anon.json`). Returns `None` when the file is missing,
/// unparseable, or carries a `schema_version` other than
/// [`ANON_GOLDEN_SCHEMA_VERSION`] — fail-closed, never panics; the
/// `ENFORCE_CDO_WS` guard is the caller's responsibility (see
/// `tests/program_resolve_harness.rs`).
#[must_use]
pub fn load_anon_golden(path: &Path) -> Option<AnonSemanticGolden> {
    let json = std::fs::read_to_string(path).ok()?;
    let golden: AnonSemanticGolden = serde_json::from_str(&json).ok()?;
    if golden.schema_version != ANON_GOLDEN_SCHEMA_VERSION {
        return None;
    }
    Some(golden)
}

/// Load + validate a committed [`AnonEventGolden`] (`cdo-event-anon.json`).
/// Same fail-closed contract as [`load_anon_golden`].
#[must_use]
pub fn load_anon_event_golden(path: &Path) -> Option<AnonEventGolden> {
    let json = std::fs::read_to_string(path).ok()?;
    let golden: AnonEventGolden = serde_json::from_str(&json).ok()?;
    if golden.schema_version != ANON_GOLDEN_SCHEMA_VERSION {
        return None;
    }
    Some(golden)
}

// ---------------------------------------------------------------------------
// beyond-1B.3b Task 3: source-adjudicated overlay for `genuine_wrong` sites
// ---------------------------------------------------------------------------

/// Verdict meaning L3 mis-resolved a genuine platform intrinsic to a
/// coincidentally-named source routine — fresh's `builtin` classification is
/// CORRECT. The only verdict [`apply_adjudicated_overrides`] acts on;
/// `fresh_false_builtin`/`needs_manual_review` entries (if any ever appear)
/// are recorded in the overlay but deliberately left OUT of the overlay
/// application — a site with either of those verdicts stays `genuine_wrong`
/// (visible to the hard gate) until the underlying fresh bug is fixed.
pub const VERDICT_L3_ERROR_INTRINSIC: &str = "l3_error_intrinsic";

/// Schema version for the committed `adjudicated-overrides.json`.
pub const ADJUDICATED_OVERRIDES_SCHEMA_VERSION: u32 = 1;

/// One committed, independently-source-adjudicated correction for a
/// `genuine_wrong` site enumerated in `known-genuine-divergences.json`.
///
/// # Independence (non-circularity invariant)
///
/// Every field is derived ONLY from: the AL SOURCE at `(unit, line)` (read
/// directly from a real CDO workspace — `callee_text`, `arity`,
/// `receiver_kind`), the source-symbol inventory (grepping the SAME unit for
/// a competing local `procedure <name>(` declaration — the lookup-precedence
/// shadow check Task 1 fixed), and the structural builtin catalog
/// (`builtins::is_global_builtin` / `member_catalog::member_builtin` —
/// `catalog_key`). `catalog_key` is a CANONICAL CATALOG STRING in the exact
/// `BuiltinId` format the catalog itself produces (bare lowercase name for a
/// global builtin, `Prefix::method` for a member builtin) — NEVER a
/// serialized fresh `Edge`/`CanonicalTarget`/graph-node id. None of these
/// fields are derived by calling [`crate::program::resolve::full::resolve_full_program`]
/// or reading a fresh-computed edge.
///
/// See `tests/goldens/semantic-edges/adjudicated-overrides.json` (the
/// committed dataset) and
/// `tests/program_resolve_harness.rs::cdo_genuine_wrong_is_precedence_adjudicated`
/// (the CDO-gated test that, at test time, INDEPENDENTLY of this struct's
/// own committed values: (a) re-hashes the unit and fails loudly on any
/// `source_sha256` mismatch — source drift, not a silent pass; (b) confirms
/// `callee_text` is still present on the claimed line; (c) cross-checks the
/// call SHAPE parsed from `callee_text` against `receiver_kind` and
/// `catalog_key`'s method component (`assert_shape_matches_receiver_kind`),
/// catching e.g. a Global-vs-member mislabel; (d) cross-checks `arity`
/// against an independently-counted argument list at the call site
/// (`count_call_arity_on_line`, best-effort — it conservatively skips call
/// forms it cannot soundly parse rather than guessing); and (e) re-derives
/// `verdict` from the structural catalog + a same-unit source-shadow scan
/// and asserts it matches. The site KEY fields
/// (`from_app_guid`/`from_object_kind`/`from_object_lc`/`from_routine_lc`/
/// `edge_kind`/`unit`/`line`/`callee_fp`) are identity fields used only to
/// locate the site, not independently re-derived facts — "every field" is
/// not a literal claim).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AdjudicatedOverride {
    pub from_app_guid: String,
    pub from_object_kind: String,
    pub from_object_lc: String,
    pub from_routine_lc: String,
    /// `EdgeKind` discriminant — see [`GoldenSiteKey::edge_kind`].
    pub edge_kind: u8,
    pub unit: String,
    pub line: u32,
    pub callee_fp: u64,
    pub callee_text: String,
    pub arity: usize,
    /// For the original 42 `builtin-catalog-fp-collision` entries: one of
    /// `Global`/`PageInstance`/`Record`/`RecordRef` (a structural-catalog
    /// member). beyond-1B.3b Task 5.5 adds a NEW shape, `CrossAppSourceProcedure`
    /// — L3 mis-resolves a call that fresh resolves, WITH SOURCE EVIDENCE, to a
    /// real cross-app procedure (e.g. Microsoft Base Application ships full
    /// ShowMyCode source; L3's frozen golden mis-assigned this site to an
    /// unrelated target — see `target_*` fields below). Entries of this NEW
    /// shape carry an empty `catalog_key` (not a builtin) and populate
    /// `target_kind`/`target_app_guid`/`target_object_lc`/`target_routine_lc`
    /// instead. pageext-merge-and-final-residual plan Task 2 adds a THIRD
    /// shape, `SameAppSourceProcedure` — the SAME target_* population as
    /// `CrossAppSourceProcedure`, but the target routine is declared in the
    /// CALLER'S OWN app (never a compiled `.alpackages` dependency), so
    /// verification reads `target_unit` directly from the live workspace
    /// source tree instead of extracting embedded source from a dependency
    /// `.app`. Used for the compiler-grounded bare-implicit-Rec suppression
    /// fix: a bare call L3 (minted before the grounding existed) paired with
    /// an unrelated/no target, that fresh now correctly resolves, WITH SOURCE
    /// EVIDENCE, to the SAME-APP SourceTable's own procedure.
    pub receiver_kind: String,
    /// Canonical catalog string (`builtin-catalog-fp-collision` shape only);
    /// empty for `CrossAppSourceProcedure`/`SameAppSourceProcedure` entries.
    #[serde(default)]
    pub catalog_key: String,
    /// `CrossAppSourceProcedure`/`SameAppSourceProcedure` shape ONLY
    /// (beyond-1B.3b Task 5.5; pageext-merge-and-final-residual plan Task 2):
    /// the REAL resolved target's `object_kind_tag` (`differential.rs`'s
    /// `object_kind_str_to_tag` encoding — codeunit=0, table=1, …), matching
    /// how `differential::project_target` encodes a `RouteTarget::Routine`.
    #[serde(default)]
    pub target_kind: Option<u8>,
    /// `CrossAppSourceProcedure`/`SameAppSourceProcedure` shape ONLY: the
    /// target routine's owning app's GUID (independently verified). For
    /// `CrossAppSourceProcedure` this is a genuine dependency app (re-derived
    /// from its own real `.app`); for `SameAppSourceProcedure` it is
    /// IDENTICAL to `from_app_guid` (the target lives in the caller's own
    /// app) — never read from any fresh-computed edge either way.
    #[serde(default)]
    pub target_app_guid: Option<String>,
    /// `CrossAppSourceProcedure`/`SameAppSourceProcedure` shape ONLY: the
    /// target object's declared numeric id (as a decimal string) or
    /// lowercased name.
    #[serde(default)]
    pub target_object_lc: Option<String>,
    /// `CrossAppSourceProcedure`/`SameAppSourceProcedure` shape ONLY: the
    /// target routine's lowercased name.
    #[serde(default)]
    pub target_routine_lc: Option<String>,
    /// `SameAppSourceProcedure` shape ONLY (pageext-merge-and-final-residual
    /// plan, Task 2): the target routine's OWN file, relative to the CDO
    /// workspace root — read DIRECTLY from the live source tree (never
    /// `.alpackages`, since the target is same-app, not a compiled
    /// dependency package).
    #[serde(default)]
    pub target_unit: Option<String>,
    pub verdict: String,
    pub source_sha256: String,
    pub note: String,
}

impl AdjudicatedOverride {
    /// Rebuild the plaintext [`GoldenSiteKey`] this override applies to (the
    /// SAME identity fields [`canonical_to_golden_key`] would have produced
    /// for the original fresh/L3 edge at this site).
    fn site_key(&self) -> GoldenSiteKey {
        GoldenSiteKey {
            from_app_guid: self.from_app_guid.clone(),
            from_object_kind: self.from_object_kind.clone(),
            from_object_lc: self.from_object_lc.clone(),
            from_routine_lc: self.from_routine_lc.clone(),
            edge_kind: self.edge_kind,
            unit: self.unit.clone(),
            line: self.line,
            callee_fp: self.callee_fp,
        }
    }
}

/// The committed adjudication overlay (`adjudicated-overrides.json`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AdjudicatedOverrides {
    pub description: String,
    pub schema_version: u32,
    pub entries: Vec<AdjudicatedOverride>,
}

/// Path to the committed adjudication overlay.
#[must_use]
pub fn adjudicated_overrides_path() -> PathBuf {
    semantic_edges_golden_dir().join("adjudicated-overrides.json")
}

/// Load + validate the committed adjudication overlay. Same fail-closed
/// contract as [`load_anon_golden`]: `None` on missing/unparseable/wrong-
/// schema-version, never panics.
#[must_use]
pub fn load_adjudicated_overrides(path: &Path) -> Option<AdjudicatedOverrides> {
    let json = std::fs::read_to_string(path).ok()?;
    let overrides: AdjudicatedOverrides = serde_json::from_str(&json).ok()?;
    if overrides.schema_version != ADJUDICATED_OVERRIDES_SCHEMA_VERSION {
        return None;
    }
    Some(overrides)
}

/// Overlay `overrides` onto `golden` IN-MEMORY: for every entry whose verdict
/// is [`VERDICT_L3_ERROR_INTRINSIC`], replace the anonymized L3 target set at
/// that site with the single adjudicated target, anonymized the SAME way
/// [`anonymize_target`] anonymizes a live route. Two target SHAPES:
///
/// - `catalog_key` populated (the original 42 `builtin-catalog-fp-collision`
///   entries): a `Builtin`-kind [`GoldenTarget`] — `kind: 255`, `object_lc:
///   catalog_key` — matching `differential.rs`'s own `RouteTarget::Builtin` →
///   `CanonicalTarget` encoding.
/// - `target_kind`/`target_app_guid`/`target_object_lc`/`target_routine_lc`
///   ALL populated (beyond-1B.3b Task 5.5's `CrossAppSourceProcedure` shape):
///   a `Routine`-kind [`GoldenTarget`], matching `differential.rs`'s
///   `RouteTarget::Routine` → `CanonicalTarget` encoding — used when fresh
///   resolves, WITH independently-verified source evidence, to a real
///   cross-app procedure that L3's frozen golden mis-assigned.
///
/// `cdo-anon.json` itself is NEVER touched by this function — it mutates only
/// the caller's IN-MEMORY `AnonSemanticGolden` (loaded fresh from disk each
/// call). Entries whose verdict is NOT [`VERDICT_L3_ERROR_INTRINSIC`] are
/// skipped (left as the original, still-wrong L3 target) — those sites stay
/// `genuine_wrong` until fixed at the source.
///
/// Returns the number of overrides actually applied (matched an existing
/// golden entry). A caller comparing this against the number of
/// `l3_error_intrinsic` entries in `overrides` can detect drift between the
/// committed golden and the committed overlay (e.g. a renamed/rebuilt
/// `cdo-anon.json` that no longer contains a site the overlay expects).
pub fn apply_adjudicated_overrides(
    golden: &mut AnonSemanticGolden,
    overrides: &AdjudicatedOverrides,
) -> usize {
    apply_adjudicated_overrides_detailed(golden, overrides).matched
}

/// What the overlay actually DID, as opposed to how many entries it contains.
///
/// [`apply_adjudicated_overrides`] returns only a matched count, which cannot
/// distinguish an override that CORRECTED the oracle from one that rewrote a
/// site with the value it already held. That distinction is load-bearing: a
/// no-op override is indistinguishable from a live one in every count, so a
/// stale overlay can look maintained forever. The committed overlay already
/// contained one such entry (a `Run()` site whose adjudicated target equalled
/// the raw golden's), which is what prompted this split.
#[derive(Clone, Debug, Default)]
pub struct OverlayOutcome {
    /// Entries carrying [`VERDICT_L3_ERROR_INTRINSIC`] — the only kind applied.
    pub intrinsic_total: usize,
    /// Entries that located an existing golden site.
    pub matched: usize,
    /// Entries that located a site AND changed its target set. A correction.
    pub changed: usize,
    /// Matched, but the adjudicated target already equalled the golden's — the
    /// override corrects nothing and is dead weight.
    pub no_op_sites: Vec<GoldenSiteKey>,
    /// Located no golden site at all — the overlay names a site the golden
    /// does not contain (a renamed unit, a re-mint, or a stale adjudication).
    pub unmatched_sites: Vec<GoldenSiteKey>,
}

/// [`apply_adjudicated_overrides`] with the full account of what it did.
pub fn apply_adjudicated_overrides_detailed(
    golden: &mut AnonSemanticGolden,
    overrides: &AdjudicatedOverrides,
) -> OverlayOutcome {
    let mut out = OverlayOutcome::default();
    for ov in &overrides.entries {
        if ov.verdict != VERDICT_L3_ERROR_INTRINSIC {
            continue;
        }
        out.intrinsic_total += 1;
        let anon_key = anonymize_site_key(&ov.site_key(), anon::SITE_DOMAIN_V1);
        let Ok(idx) = golden.entries.binary_search_by(|e| e.site.cmp(&anon_key)) else {
            out.unmatched_sites.push(ov.site_key());
            continue;
        };
        let target = match (
            ov.target_kind,
            &ov.target_app_guid,
            &ov.target_object_lc,
            &ov.target_routine_lc,
        ) {
            (Some(kind), Some(app_guid), Some(object_lc), Some(routine_lc)) => GoldenTarget {
                kind,
                app: Some(app_guid.clone()),
                object_lc: object_lc.clone(),
                routine_lc: Some(routine_lc.clone()),
            },
            _ => GoldenTarget {
                kind: 255,
                app: None,
                object_lc: ov.catalog_key.clone(),
                routine_lc: None,
            },
        };
        let replacement = BTreeSet::from([anonymize_target(&target)]);
        out.matched += 1;
        if golden.entries[idx].targets == replacement {
            out.no_op_sites.push(ov.site_key());
        } else {
            out.changed += 1;
        }
        golden.entries[idx].targets = replacement;
    }
    out
}

/// Merge `new_entries` into the GITIGNORED local de-anonymization map at
/// `path`, creating it if absent. Existing entries win on key collision
/// (first writer's plaintext is kept — there should never be a genuine
/// disagreement since the SAME plaintext always re-hashes to the SAME id).
/// Best-effort: I/O failures are swallowed — the map is a LOCAL debugging
/// aid, never required for correctness (see `anon.rs`'s module docs).
pub fn merge_deanon_map(path: &Path, new_entries: &BTreeMap<String, String>) {
    if new_entries.is_empty() {
        return;
    }
    let mut map: BTreeMap<String, String> = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    for (k, v) in new_entries {
        map.entry(k.clone()).or_insert_with(|| v.clone());
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(&map) {
        let _ = std::fs::write(path, json);
    }
}

/// Build the anonymized fresh-side site→targets map AND a reverse
/// `AnonSiteKey -> GoldenSiteKey` index. The reverse index is what lets
/// `run_cdo_semantic_audit` recover PLAINTEXT fresh identity for a failing
/// `fresh_wrong`/`genuine_wrong` site (for the deanon map and for
/// `CdoSemanticAuditReport::genuine_wrong_sites`, which stays plaintext
/// `GoldenSiteKey` because it only ever needs FRESH's own identity — see the
/// module-level "re-hash-don't-decrypt" principle in `anon.rs`).
fn anonymize_fresh_map(
    fresh_plain: &BTreeMap<GoldenSiteKey, BTreeSet<GoldenTarget>>,
    site_domain: &str,
) -> (
    BTreeMap<AnonSiteKey, BTreeSet<AnonTarget>>,
    HashMap<AnonSiteKey, GoldenSiteKey>,
) {
    let mut anon_map: BTreeMap<AnonSiteKey, BTreeSet<AnonTarget>> = BTreeMap::new();
    let mut reverse: HashMap<AnonSiteKey, GoldenSiteKey> = HashMap::new();
    for (site, targets) in fresh_plain {
        let asite = anonymize_site_key(site, site_domain);
        let atargets: BTreeSet<AnonTarget> = targets.iter().map(anonymize_target).collect();
        reverse.insert(asite.clone(), site.clone());
        anon_map.entry(asite).or_default().extend(atargets);
    }
    (anon_map, reverse)
}

/// Diff an anonymized fresh site→targets map against a loaded committed
/// golden. Same classification rule as [`assert_against_semantic_golden`],
/// operating on [`AnonSiteKey`]/[`AnonTarget`] instead of the plaintext types.
#[must_use]
fn diff_against_anon_golden(
    fresh_anon: &BTreeMap<AnonSiteKey, BTreeSet<AnonTarget>>,
    golden: &AnonSemanticGolden,
) -> AnonSemanticDiff {
    let mut diff = AnonSemanticDiff::default();
    for entry in &golden.entries {
        let l3_targets = &entry.targets;
        if let Some(fresh_targets) = fresh_anon.get(&entry.site) {
            diff.total_paired += 1;
            if fresh_targets == l3_targets {
                diff.matches += 1;
            } else if !l3_targets.is_empty() && !fresh_targets.is_empty() {
                diff.fresh_wrong.push(AnonFreshWrong {
                    site: entry.site.clone(),
                    fresh_targets: fresh_targets.clone(),
                    l3_targets: l3_targets.clone(),
                });
            } else if !l3_targets.is_empty() {
                diff.fresh_missing += 1;
            } else {
                diff.fresh_extra += 1;
            }
        } else {
            diff.golden_missing += 1;
        }
    }
    for key in fresh_anon.keys() {
        if golden.get(key).is_none() {
            diff.fresh_novel += 1;
        }
    }
    diff
}

// ---------------------------------------------------------------------------
// Route-applicability report
// ---------------------------------------------------------------------------

/// Result of the structural route-applicability contract check.
///
/// # Soundness vs. completeness (1B.3b Task 2)
///
/// [`witness_contract_violations`][Self::witness_contract_violations] and
/// [`abi_unmapped`][Self::abi_unmapped] are STRUCTURAL checks (every route's
/// evidence/witness pair is internally consistent; every `AbiSymbol` key maps
/// back to a real dep entry).
///
/// The four `*_violations` fields added by 1B.3b Task 2 are PER-ROUTE
/// SOUNDNESS checks: given that the resolver emitted a fan-out route, IS that
/// route well-formed/applicable for the call site that produced it (the
/// right method+arity on an object that genuinely implements the dispatched
/// interface; a trigger that genuinely fires for the record-op that produced
/// it; a catalog method that's genuinely in that object-kind's instance
/// catalog; a subscriber whose raw `[EventSubscriber]` attribute genuinely
/// names the publisher+event it claims to handle). This is explicitly NOT a
/// COMPLETENESS check — it does not ask "did the resolver emit every route it
/// should have" (that question is answered by the frozen, L3-validated
/// goldens from 1B.3a/1B.3b Task 1, which carry target-set
/// completeness/exactness for the dispatch kinds they cover). A clean
/// [`ApplicabilityReport`] only certifies that every route the resolver DID
/// emit is individually justified.
///
/// These four checks are the teeth that previously lived ONLY inside the
/// dual-run gates' FreshOnly branches (`differential::run_member_resolution_harness`
/// / `run_implicit_trigger_harness` / `run_event_flow_gate`) — ported here so
/// they survive the gate deletion in Task 3, now running over EVERY fan-out
/// route in [`crate::program::resolve::full::resolve_full_program`]'s full
/// edge set rather than only the FreshOnly-vs-L3 subset.
#[derive(Clone, Debug, Default)]
pub struct ApplicabilityReport {
    pub total_routes: usize,
    /// Routes where the `evidence`/`witness` pair is not valid.
    pub witness_contract_violations: usize,
    /// `AbiSymbol` routes whose key is absent from the raw-ABI index.
    pub abi_unmapped: usize,
    /// STRUCTURAL (Task 2 review fix, Finding 2): `RoutineNode`s in the graph
    /// where `abi_overload_collapsed` and `abi_params ==
    /// AbiParams::CollapsedUntrusted` are OUT OF LOCKSTEP.
    /// `build::dedup_routines_preserving_genuine_overloads` is supposed to
    /// set the two markers together for every `TrustTier::SymbolOnly`
    /// collapse survivor (see that function's doc); `arg_dispatch::
    /// candidate_param_infos_abi`'s whole fail-closed contract — a collapsed
    /// survivor's parameter list must be structurally unreadable, never
    /// merely conventionally untrusted — depends on this holding for EVERY
    /// routine in the graph, not just the hand-built fixtures `build.rs`'s
    /// own unit tests exercise in isolation. Non-zero here means a collapsed
    /// survivor's (possibly WRONG, arbitrary-JSON-order) parameter list could
    /// be read into an arg-type dispatch pick.
    pub abi_overload_collapsed_lockstep_violations: usize,
    /// SOUNDNESS: `DispatchShape::Polymorphic` (Interface fan-out) `Routine`
    /// routes that fail [`interface_route_applicable`] against the call
    /// site's dispatched `(iface, called_member, arity)` — or, when no
    /// call-site context could be recovered for the edge at all, ANY
    /// non-`Unresolved` route on it (fail-closed: an unverifiable route is
    /// not a proven-sound one).
    pub interface_applicability_violations: usize,
    /// SOUNDNESS: Catalog `Builtin` routes whose `BuiltinId` carries the
    /// `PageInstance::` / `ReportInstance::` / `Enum::` fan-out prefix and
    /// fail an independent re-check of the instance-builtin/enum-static
    /// catalog ([`instance_builtin_route_applicable`] for Page/Report; the
    /// `Enum` member-builtin catalog for `Enum::`).
    pub instance_builtin_violations: usize,
    /// SOUNDNESS: `DispatchShape::Multicast` (`EdgeKind::ImplicitTrigger`)
    /// `Routine` routes that fail [`implicit_trigger_route_applicable`]
    /// against the record-op call site's `RecordOpCtx` — `Validate` sites
    /// fall back to the coarser table/extension-identity check (the
    /// validated field name is not recoverable from `CalleeShape::RecordOp`,
    /// the same documented limitation as the live
    /// `differential::run_implicit_trigger_harness` FreshOnly gate) — or, when
    /// no call-site context was recovered, ANY non-`Unresolved` route
    /// (fail-closed, same rationale as the Interface case).
    pub implicit_trigger_violations: usize,
    /// SOUNDNESS: `EdgeKind::EventFlow` `Routine` (subscriber) routes whose
    /// raw `[EventSubscriber]` attribute (re-parsed at check time via
    /// [`verify_event_subscriber_route`], NOT from any cached index field)
    /// does not name the publisher object+event the edge claims, or whose
    /// arity exceeds the publisher's Sender-tolerant bound (Task 1) —
    /// `subscriber_arity_bound(publisher_params_count, publisher.include_sender)`:
    /// the publisher's own explicit arity, plus ONE more ONLY when the
    /// publisher's attribute declares `IncludeSender: true`. This is the SAME
    /// conditional bound `ResolveIndex::build`'s wiring uses to admit a
    /// candidate — see `event::subscriber_arity_bound`'s doc for why a
    /// blanket `+1` would be SYNCHRONIZED WRONGNESS.
    pub event_violations: usize,
    /// NON-VACUITY: number of routes [`interface_route_applicable`] was
    /// actually invoked on (i.e. `RouteTarget::Routine` routes on a Polymorphic
    /// edge WITH recovered call-site context — fail-closed routes don't call
    /// the predicate and are excluded). A collapse toward 0 with
    /// `interface_applicability_violations == 0` signals a vacuous pass (e.g.
    /// a [`build_fan_out_site_context`] regression silently dropping context),
    /// distinguishable from a genuine clean run.
    pub interface_routes_checked: usize,
    /// NON-VACUITY: number of `Builtin` routes whose `BuiltinId` matched a
    /// fan-out catalog prefix (`PageInstance::` / `ReportInstance::` /
    /// `Enum::`), i.e. routes the instance-builtin/enum-static re-check
    /// actually ran on. See `interface_routes_checked`'s doc comment for the
    /// non-vacuity rationale.
    pub instance_builtin_routes_checked: usize,
    /// NON-VACUITY: number of routes [`implicit_trigger_route_applicable`] (or
    /// its `Validate` table/extension-identity fallback,
    /// `target_is_on_table_or_extension`) was actually invoked on — `Routine`
    /// routes on a Multicast `ImplicitTrigger` edge WITH recovered call-site
    /// context. See `interface_routes_checked`'s doc comment for the
    /// non-vacuity rationale.
    pub implicit_trigger_routes_checked: usize,
    /// NON-VACUITY: number of `Routine` (subscriber) routes
    /// [`verify_event_subscriber_route`] was actually invoked on — excludes
    /// routes skipped because the publisher object could not be projected
    /// (1B.3b Task 2 fix; see `route_applicability`'s `EdgeKind::EventFlow`
    /// arm). See `interface_routes_checked`'s doc comment for the non-vacuity
    /// rationale.
    pub event_routes_checked: usize,
}

impl ApplicabilityReport {
    pub fn is_clean(&self) -> bool {
        self.witness_contract_violations == 0
            && self.abi_unmapped == 0
            && self.abi_overload_collapsed_lockstep_violations == 0
            && self.interface_applicability_violations == 0
            && self.instance_builtin_violations == 0
            && self.implicit_trigger_violations == 0
            && self.event_violations == 0
    }

    /// Sum of the four 1B.3b Task 2 fan-out SOUNDNESS violation counters
    /// (excludes the structural `witness_contract_violations`/`abi_unmapped`/
    /// `abi_overload_collapsed_lockstep_violations` checks).
    pub fn fan_out_violations(&self) -> usize {
        self.interface_applicability_violations
            + self.instance_builtin_violations
            + self.implicit_trigger_violations
            + self.event_violations
    }
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

fn canonical_to_golden_key(e: &CanonicalEdge) -> GoldenSiteKey {
    GoldenSiteKey {
        from_app_guid: e.from.app_guid.clone(),
        from_object_kind: e.from.object_kind.clone(),
        from_object_lc: e.from.object_lc.clone(),
        from_routine_lc: e.from.routine_lc.clone(),
        edge_kind: match e.kind {
            EdgeKind::Call => 0,
            EdgeKind::Run => 1,
            EdgeKind::ImplicitTrigger => 2,
            EdgeKind::EventFlow => 3,
        },
        unit: e.site.span.unit.clone(),
        line: e.site.span.start.line,
        callee_fp: e.site.callee_fp,
    }
}

fn canonical_targets_to_golden(targets: &BTreeSet<CanonicalTarget>) -> BTreeSet<GoldenTarget> {
    targets
        .iter()
        .map(|t| GoldenTarget {
            kind: t.kind,
            app: t.app.clone(),
            object_lc: t.object_lc.clone(),
            routine_lc: t.routine_lc.clone(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Adjudication helper
// ---------------------------------------------------------------------------

/// Adjudicate a `FreshWrong` site: are fresh's and L3's target sets in a
/// REFINEMENT relationship (allowed), or genuinely disjoint (a real divergence)?
///
/// Returns `true` (fresh_ahead_dispatch — allowed) when ANY of these holds:
/// 1. `l3 ⊆ fresh` — fresh is a superset that includes all of L3's answer
///    (Interface/Polymorphic fan-out: fresh is MORE precise).
/// 2. `fresh ⊆ l3` — fresh partially resolved a call site that L3 captured more
///    broadly (multiple physical calls can share one `(line, callee_fp)` bucket).
///    Every target fresh emitted IS in L3's set, so none are confidently wrong —
///    fresh is merely less complete, not wrong.
/// 3. Every L3 target is an interface (kind=11) AND every fresh target implements
///    it (verified via the graph's `ObjectNode.implements` field).
///
/// Returns `false` (genuine_wrong) only when the two non-empty target sets are
/// DISJOINT (or partially overlap with neither a subset nor interface-implements
/// relationship) — fresh and L3 confidently resolved the same site to unrelated
/// targets. NOTE: this is symmetric — it does NOT assert which side is correct;
/// adjudicating that is deferred to 1B.3b.
///
/// # Known partial-recall blind spot (named: 1B.3b-disambiguation)
///
/// Case 2 (`fresh ⊆ l3`) creates a partial-recall blind spot: when fresh finds
/// only a strict subset of the correct targets in a multi-target bucket (e.g.,
/// resolves 2 of 3 interface implementers), the site is classified
/// `fresh_ahead_dispatch` here — NOT as `fresh_missing` or `genuine_wrong`.
/// The dropped target is silently masked by this gate.
///
/// **Mitigation while L3 is the oracle**: the resolution/member harnesses assert
/// `regression_unexplained == 0` independently — any unexplained resolution
/// regression fires there and acts as defense-in-depth covering this blind spot.
///
/// Full per-target recall validation is a named 1B.3b-disambiguation follow-up.
///
/// # 1B.3b: ported to the anonymized identity space
///
/// `run_cdo_semantic_audit` no longer holds L3's plaintext target set (it
/// LOADS the committed anonymized golden) — only [`AnonTarget`]s. The THREE
/// CASES above are preserved EXACTLY; only the identity type changed, per
/// `anon.rs`'s "re-hash-don't-decrypt" principle: `obj_lookup_anon` is built
/// ONCE from the LIVE graph (real `ObjectNode`s, each keyed by its OWN
/// re-hashed identity), so both `l3` (anonymized, loaded from the frozen
/// golden) and `fresh` (anonymized, computed live) can look themselves up by
/// anonymized identity without ever inverting a committed id.
fn is_fresh_ahead_dispatch_anon(
    fresh: &BTreeSet<AnonTarget>,
    l3: &BTreeSet<AnonTarget>,
    obj_lookup_anon: &HashMap<AnonId, &ObjectNode>,
) -> bool {
    if fresh.is_empty() || l3.is_empty() {
        return false;
    }

    // Case 1: L3's targets ⊆ fresh's targets (fresh is a superset: includes all of L3's answer).
    if l3.is_subset(fresh) {
        return true;
    }

    // Case 3: fresh's targets ⊆ L3's targets — fresh partially resolved a compound call
    // that L3 captured more broadly (e.g. L3 follows both the primary dispatch and an
    // EventFlow edge on the same callee_fp).  Fresh is NOT wrong — every target it emitted
    // is in L3's set — it simply emitted fewer.  Classify as fresh_ahead_dispatch (really
    // "fresh_partial_correct") rather than genuine_wrong.
    if fresh.is_subset(l3) {
        return true;
    }

    // Case 2: All L3 targets are interfaces (kind=11) and all fresh targets implement them.
    if !l3.iter().all(|t| t.kind == 11) {
        return false;
    }

    for l3_target in l3 {
        let Some(l3_obj) = obj_lookup_anon.get(&l3_target.object_id) else {
            // Cannot find the interface object in the live graph → cannot verify → genuine_wrong.
            return false;
        };
        let iface_name_lc = l3_obj.name.fold_identifier();

        for fresh_target in fresh {
            // Routine names should agree for a valid interface dispatch.
            if fresh_target.routine_id != l3_target.routine_id {
                return false;
            }
            let Some(fresh_obj) = obj_lookup_anon.get(&fresh_target.object_id) else {
                return false;
            };
            // The concrete object must declare it implements the interface.
            if !fresh_obj
                .implements
                .iter()
                .any(|i| i.fold_identifier() == iface_name_lc)
            {
                return false;
            }
        }
    }
    true
}

/// Build `AnonId -> &ObjectNode` over every object in `graph`, keyed the SAME
/// way [`anon_target_object_id`] keys an [`AnonTarget`] — i.e. re-hashing
/// `(app_guid, object_lc)` for every LIVE object so an anonymized target's
/// `object_id` (whether from the loaded golden or freshly computed) can find
/// its `ObjectNode` without ever inverting a committed id.
fn build_obj_lookup_anon(
    graph: &crate::program::graph::ProgramGraph,
) -> HashMap<AnonId, &ObjectNode> {
    let mut lookup: HashMap<AnonId, &ObjectNode> = HashMap::new();
    for obj in &graph.objects {
        let guid = graph
            .apps
            .try_resolve(obj.id.app)
            .map(|a| a.guid.clone())
            .unwrap_or_default();
        let lc = match &obj.id.key {
            ObjKey::Id(n) => format!("{n}"),
            ObjKey::Name(s) => s.clone(),
        };
        lookup.insert(anon_target_object_id(&Some(guid), &lc), obj);
    }
    lookup
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Build a [`SemanticGolden`] from a batch of canonical edges. Oracle-
/// agnostic — the caller decides whether `edges` came from L3 or from the
/// fresh resolver. Shared by [`mint_l3_validated_golden`],
/// [`mint_l3_trigger_golden`], and [`mint_fresh_golden_for_kind`].
fn build_golden_from_canonical(edges: &[CanonicalEdge]) -> SemanticGolden {
    let mut map: BTreeMap<GoldenSiteKey, BTreeSet<GoldenTarget>> = BTreeMap::new();
    for edge in edges {
        let key = canonical_to_golden_key(edge);
        let targets = canonical_targets_to_golden(&edge.targets);
        map.entry(key).or_default().extend(targets);
    }
    SemanticGolden::from_map(map)
}

/// **SANCTIONED L3 ORACLE USE (1B.3b: the dev-mint tool is the only caller
/// post-freeze; the in-repo fixture's `REGEN_TEMP_GOLDENS` path also still
/// calls this directly — see `tests/program_resolve_harness.rs` Test 14)**:
/// mint the Member/Interface semantic golden from the L3 oracle.
///
/// Calls [`project_l3`] (`crate::program::l3_mint`, 1B.3b Task 3 — the sole
/// remaining L3-oracle access point in the library) over `workspace_root`,
/// collects per-site target sets into a [`SemanticGolden`] keyed by
/// column-ignoring [`GoldenSiteKey`].
///
/// Empty target sets (L3 Unknown/Unresolved) are retained — they record sites
/// that L3 extracted but could not resolve, so the golden covers them.
#[must_use]
pub fn mint_l3_validated_golden(workspace_root: &Path) -> SemanticGolden {
    build_golden_from_canonical(&project_l3(workspace_root))
}

/// **SANCTIONED L3 ORACLE USE (1B.3b dev-mint tool only)**: mint the
/// ImplicitTrigger semantic golden from L3's native `PRecordOperation`-keyed
/// edges ([`project_l3_implicit_trigger_in_scope`], `crate::program::l3_mint`).
/// Backs `cdo-trigger-anon.json`.
#[must_use]
pub fn mint_l3_trigger_golden(workspace_root: &Path) -> SemanticGolden {
    build_golden_from_canonical(&project_l3_implicit_trigger_in_scope(workspace_root))
}

/// L3-INDEPENDENT: mint a [`SemanticGolden`] from the FRESH resolver's OWN
/// output, filtered to one [`EdgeKind`]. Used to freeze fresh's own
/// resolution as a committed regression baseline for dispatch kinds a small
/// synthetic fixture exercises end-to-end without L3 at all (the
/// ImplicitTrigger target-set fixture — 1B.3b Task 1 Step 4). NOT used for
/// the CDO-derived goldens (those freeze the L3 VERDICT, not fresh's own
/// output — see [`mint_l3_validated_golden`]/[`mint_l3_trigger_golden`]).
#[must_use]
pub fn mint_fresh_golden_for_kind(workspace_root: &Path, kind: EdgeKind) -> SemanticGolden {
    use crate::program::resolve::full::{build_context, resolve_full_program_with};

    let Some(ctx) = build_context(workspace_root) else {
        return SemanticGolden::default();
    };
    let report = resolve_full_program_with(&ctx);
    mint_fresh_golden_for_kind_on(&ctx, &report, kind)
}

/// Substrate-taking core of [`mint_fresh_golden_for_kind`] — reads the
/// program graph and resolved report from `ctx`/`report` instead of
/// rebuilding the snapshot/graph/resolve pass internally.
#[must_use]
pub(crate) fn mint_fresh_golden_for_kind_on(
    ctx: &crate::program::resolve::full::ProgramContext,
    report: &crate::program::resolve::full::ProgramReport,
    kind: EdgeKind,
) -> SemanticGolden {
    let edges: Vec<Edge> = report
        .edges
        .iter()
        .map(|ce| ce.edge.clone())
        .filter(|e| e.kind == kind)
        .collect();
    let canonical = project_fresh(&edges, &ctx.graph.apps);
    build_golden_from_canonical(&canonical)
}

/// Compare a fresh canonical edge batch against the L3-minted golden.
///
/// Returns a [`SemanticDiff`] classifying every site.
///
/// **The critical invariant is `fresh_wrong.is_empty()`** — fresh must never
/// confidently emit a target that L3 says is wrong.  `fresh_missing` tracks
/// the Task-3 Unknown gap where L3 resolved but fresh did not (acceptable
/// progress gap — reduce it, never introduce new ones).
#[must_use]
pub fn assert_against_semantic_golden(
    fresh: &[CanonicalEdge],
    golden: &SemanticGolden,
) -> SemanticDiff {
    // Build fresh key → targets map (union duplicate keys).
    let mut fresh_map: BTreeMap<GoldenSiteKey, BTreeSet<GoldenTarget>> = BTreeMap::new();
    for edge in fresh {
        let key = canonical_to_golden_key(edge);
        let targets = canonical_targets_to_golden(&edge.targets);
        fresh_map.entry(key).or_default().extend(targets);
    }

    let mut diff = SemanticDiff::default();

    // Walk golden entries and classify.
    for entry in &golden.entries {
        let key = &entry.site;
        let l3_targets = &entry.targets;
        if let Some(fresh_targets) = fresh_map.get(key) {
            diff.total_paired += 1;
            if fresh_targets == l3_targets {
                diff.matches += 1;
            } else if !l3_targets.is_empty() && !fresh_targets.is_empty() {
                // Both sides resolved but to different targets — the confidently-wrong class.
                diff.fresh_wrong.push(FreshWrong {
                    site: key.clone(),
                    fresh_targets: fresh_targets.clone(),
                    l3_targets: l3_targets.clone(),
                });
            } else if !l3_targets.is_empty() {
                // L3 resolved; fresh did not — a gap.
                diff.fresh_missing.push(FreshMissing {
                    site: key.clone(),
                    l3_targets: l3_targets.clone(),
                });
            } else {
                // L3 empty; fresh resolved — fresh is ahead of L3 (a win).
                diff.fresh_extra.push(FreshExtra {
                    site: key.clone(),
                    fresh_targets: fresh_targets.clone(),
                });
            }
        } else {
            // Golden site has no fresh peer.
            diff.golden_missing += 1;
        }
    }

    // Count fresh sites not in the golden (EventFlow, ImplicitTrigger, etc.).
    for key in fresh_map.keys() {
        if golden.get(key).is_none() {
            diff.fresh_novel += 1;
        }
    }

    diff
}

// ---------------------------------------------------------------------------
// 1B.3b Task 2: fan-out call-site context (L3-INDEPENDENT)
// ---------------------------------------------------------------------------

/// Per-call-site context the fan-out applicability predicates need but
/// cannot recover from a [`Edge`]/[`Route`] alone:
///
/// - An `Interface` member call's `target.name_lc`/`target.params_count` are
///   tautologically equal to the call site's method/arity BY CONSTRUCTION
///   (`resolver::resolve_member`'s `Interface` arm only ever builds a
///   `Routine` route via `resolve_in_object(impl_id, …, method_lc, arity, …)`)
///   — but the DISPATCHED INTERFACE NAME is not recoverable from the route or
///   edge at all, since `Edge`/`Route` carry no receiver-type field. This
///   variant carries it.
/// - A `RecordOp` call site's record-operation kind + resolved table are
///   likewise absent from the `ImplicitTrigger` edge/route shape.
///
/// Built by [`build_fan_out_site_context`], which re-walks the SAME parsed
/// call sites `resolve_full_program` resolves (mirroring the (Task-3-deleted)
/// dual-run gates' FreshOnly receiver-type/`RecordOp` re-inference —
/// `differential::run_member_resolution_harness` /
/// `run_implicit_trigger_harness`), keyed by [`SiteId`] so it lines up 1:1
/// with `resolve_full_program`'s edges.
#[derive(Clone, Debug)]
pub enum FanOutSiteContext {
    /// `ReceiverType::Interface { name_lc }` — the call site dispatched via
    /// this interface, calling `called_member_lc` with `arity` arguments.
    Interface {
        iface_lc: String,
        called_member_lc: String,
        arity: usize,
    },
    /// A `RecordOp` call site's record-operation context.
    Trigger(RecordOpCtx),
}

/// Map a (lowercased) `CalleeShape::RecordOp` method name to the
/// [`RecordOpKind`] of the implicit trigger it fires — the inverse of
/// `resolver::resolve_implicit_trigger`'s `op → trigger_name` table.
/// `None` for any record-op method that is NOT an implicit-trigger-firing DML
/// operation (e.g. `SetRange`, `FindSet`, `CalcFields`, …) — the caller skips
/// those sites entirely (no [`FanOutSiteContext`] is recoverable, or needed,
/// for them).
///
/// 1B.3b Task 2 fix: previously had no `"rename"` arm, so a real
/// `Rec.Rename(...)` call site whose target table has an `OnRename` trigger
/// would silently drop context here, fall into `route_applicability`'s
/// fail-closed branch, and wrongly count the trigger's route a VIOLATION
/// (flagging a genuinely sound route as a false positive).
fn record_op_kind_for_method(op_lc: &str) -> Option<RecordOpKind> {
    match op_lc {
        "insert" => Some(RecordOpKind::Insert),
        "modify" => Some(RecordOpKind::Modify),
        "delete" => Some(RecordOpKind::Delete),
        "rename" => Some(RecordOpKind::Rename),
        "validate" => Some(RecordOpKind::Validate),
        _ => None,
    }
}

/// Re-walk every workspace `Member`/`RecordOp` call site to recover the
/// [`FanOutSiteContext`] the fan-out applicability predicates need.
///
/// Mirrors `full::resolve_full_program_from_parts`'s own Phase-1 walk
/// (same snapshot/parsed/`ws_file_set`/`primary_app_ref` scoping) — this is
/// INTENTIONALLY a second pass over the same call sites rather than a
/// plumbed-through field on [`Edge`]: `resolve_full_program`'s `Edge` shape
/// is shared by every consumer in the crate (CLI stats, snapshots,
/// fingerprints, …) and is deliberately receiver-type-free; adding a
/// receiver-type field there to serve only this soundness check would leak
/// Phase-3/4 resolver internals into the canonical edge shape. The re-walk
/// costs nothing the gates didn't already pay (they did the identical
/// re-walk) and keeps `Edge` clean.
///
/// L3-INDEPENDENT: only reads `graph`/`parsed`/`index` — never touches
/// `engine::l3`.
fn build_fan_out_site_context(
    graph: &ProgramGraph,
    index: &ResolveIndex,
    parsed: &[ParsedUnit],
    primary_app_ref: AppRef,
    ws_file_set: &HashSet<String>,
) -> HashMap<SiteId, FanOutSiteContext> {
    let obj_node_map: HashMap<ObjectNodeId, &ObjectNode> =
        graph.objects.iter().map(|o| (o.id.clone(), o)).collect();

    let mut ctx_map: HashMap<SiteId, FanOutSiteContext> = HashMap::new();

    for unit in parsed {
        let Some(app_ref) = graph.apps.find(&unit.app) else {
            continue;
        };
        if app_ref != primary_app_ref {
            continue;
        }

        for pf in &unit.files {
            if !ws_file_set.contains(&pf.virtual_path) {
                continue;
            }

            for (obj_idx, obj) in pf.file.objects.iter().enumerate() {
                let obj_key = match obj.id {
                    Some(n) => ObjKey::Id(n),
                    None => ObjKey::Name(obj.name.fold_identifier()),
                };
                let obj_node_id = ObjectNodeId {
                    app: primary_app_ref,
                    kind: obj.kind,
                    key: obj_key,
                };
                let Some(obj_node) = obj_node_map.get(&obj_node_id).copied() else {
                    continue;
                };

                let globals_rec: HashSet<String> = obj
                    .globals
                    .iter()
                    .filter(|v| {
                        v.ty.as_deref()
                            .map(|ty| ty.trim().to_ascii_lowercase().starts_with("record"))
                            .unwrap_or(false)
                    })
                    .map(|v| v.name.fold_identifier())
                    .collect();

                for (routine_idx, routine) in obj.routines.iter().enumerate() {
                    let caller = source_routine_node_id(obj_node_id.clone(), routine);

                    let sites = extract_sites_for_routine(
                        &pf.file,
                        &pf.text,
                        &pf.virtual_path,
                        &globals_rec,
                        obj_idx,
                        routine_idx,
                    );

                    for site in &sites {
                        let fp = callee_fp(&site.callee_text);
                        let site_id = SiteId {
                            caller: caller.clone(),
                            span: site.span.clone(),
                            callee_fingerprint: fp,
                        };

                        match &site.shape {
                            CalleeShape::Member {
                                receiver_text,
                                method,
                                ..
                            } => {
                                let receiver_lc = receiver_text.fold_identifier();
                                let recv = infer_receiver_type(
                                    &receiver_lc,
                                    routine,
                                    &obj.globals,
                                    obj_node,
                                    graph,
                                    index,
                                    None,
                                    None,
                                );
                                if let ReceiverType::Interface { name_lc } = recv {
                                    ctx_map.insert(
                                        site_id,
                                        FanOutSiteContext::Interface {
                                            iface_lc: name_lc,
                                            called_member_lc: method.fold_identifier(),
                                            arity: site.arity,
                                        },
                                    );
                                }
                            }
                            CalleeShape::RecordOp { receiver_text, op } => {
                                let receiver_lc = receiver_text.fold_identifier();
                                let op_lc = op.fold_identifier();
                                let Some(op_kind) = record_op_kind_for_method(&op_lc) else {
                                    continue;
                                };
                                let recv = infer_receiver_type(
                                    &receiver_lc,
                                    routine,
                                    &obj.globals,
                                    obj_node,
                                    graph,
                                    index,
                                    None,
                                    None,
                                );
                                if let ReceiverType::Record {
                                    table: Some(table_id),
                                } = recv
                                {
                                    ctx_map.insert(
                                        site_id,
                                        FanOutSiteContext::Trigger(RecordOpCtx {
                                            kind: op_kind,
                                            table: table_id,
                                            // The validated field name is not
                                            // recoverable from `CalleeShape::RecordOp`
                                            // (it carries no argument text) — same
                                            // documented limitation as the live
                                            // `run_implicit_trigger_harness` FreshOnly
                                            // gate; `route_applicability` falls back to
                                            // the coarser table/extension-identity
                                            // check for `Validate` sites (see its
                                            // doc comment).
                                            field: None,
                                            // The run-trigger boolean argument is not
                                            // statically recoverable at this layer
                                            // either (same conservative default the live
                                            // gate uses) — `Guarded` never short-circuits
                                            // the predicate to `false`, so it never masks
                                            // a real violation; it only means we cannot
                                            // independently confirm an `Insert(false)`
                                            // site suppressed its trigger edges (a gap
                                            // pre-existing in the ported logic, not
                                            // introduced here).
                                            run_trigger: RunTrigger::Guarded,
                                        }),
                                    );
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    ctx_map
}

/// `target_object == table_id` OR a `TableExtension` of it.
///
/// Used for `Validate` `ImplicitTrigger` routes, where
/// [`FanOutSiteContext::Trigger`]'s `field` is always `None` (see
/// [`build_fan_out_site_context`]'s doc comment) — with `field: None`, the
/// full [`implicit_trigger_route_applicable`] ALWAYS returns `false` for a
/// `Validate` target (it requires `(Some(ctx_field), Some(target_field))` to
/// match), so this coarser table-identity check is the fallback, mirroring
/// `differential::run_implicit_trigger_harness`'s identical, documented
/// `target_is_on_table_or_extension` helper (duplicated here rather than
/// imported to keep this module's `differential.rs`/`applicability.rs`
/// footprint at zero non-`pub` touches).
fn target_is_on_table_or_extension(
    target_object: &ObjectNodeId,
    table_id: &ObjectNodeId,
    graph: &ProgramGraph,
    index: &ResolveIndex,
) -> bool {
    if target_object == table_id {
        return true;
    }
    let table_name_lc: String = match &table_id.key {
        ObjKey::Name(s) => s.clone(),
        ObjKey::Id(_) => graph
            .objects
            .iter()
            .find(|o| &o.id == table_id)
            .map(|n| n.name.fold_identifier())
            .unwrap_or_default(),
    };
    if table_name_lc.is_empty() {
        return false;
    }
    index
        .table_extensions_of(&table_name_lc)
        .contains(target_object)
}

/// Route-applicability contract: structural (witness↔evidence, ABI ingestion)
/// AND, since 1B.3b Task 2, per-route fan-out SOUNDNESS (Interface,
/// instance-builtin/enum-static, ImplicitTrigger, EventFlow) — see
/// [`ApplicabilityReport`]'s doc comment for the soundness-vs-completeness
/// framing. All six violation counters must be zero for
/// [`ApplicabilityReport::is_clean`] to return `true`; the four
/// `*_routes_checked` counters are a NON-VACUITY audit (see their doc
/// comments) and are not part of `is_clean`.
///
/// `fan_out_ctx` (built by [`build_fan_out_site_context`]) supplies the
/// Interface/`RecordOp` call-site context `edges` alone cannot carry;
/// `graph`/`index` back the predicates' object/routine lookups; `parsed`
/// backs [`verify_event_subscriber_route`]'s independent raw-IR re-read.
#[must_use]
pub fn route_applicability(
    edges: &[Edge],
    raw_abi: &RawAbiIndex,
    graph: &ProgramGraph,
    index: &ResolveIndex,
    fan_out_ctx: &HashMap<SiteId, FanOutSiteContext>,
    parsed: &[ParsedUnit],
) -> ApplicabilityReport {
    let mut total_routes = 0usize;
    let mut witness_contract_violations = 0usize;
    let mut interface_applicability_violations = 0usize;
    let mut instance_builtin_violations = 0usize;
    let mut implicit_trigger_violations = 0usize;
    let mut event_violations = 0usize;
    let mut interface_routes_checked = 0usize;
    let mut instance_builtin_routes_checked = 0usize;
    let mut implicit_trigger_routes_checked = 0usize;
    let mut event_routes_checked = 0usize;

    for edge in edges {
        for route in edge.all_routes() {
            total_routes += 1;
            if !witness_contract_holds(route) {
                witness_contract_violations += 1;
            }
        }

        match edge.kind {
            // ── Interface (Polymorphic) fan-out ─────────────────────────────
            EdgeKind::Call if edge.shape == DispatchShape::Polymorphic => {
                match fan_out_ctx.get(&edge.site) {
                    Some(FanOutSiteContext::Interface {
                        iface_lc,
                        called_member_lc,
                        arity,
                    }) => {
                        for route in edge.all_routes() {
                            let ok = match &route.target {
                                RouteTarget::Routine(rid) => {
                                    interface_routes_checked += 1;
                                    interface_route_applicable(
                                        iface_lc,
                                        called_member_lc,
                                        *arity,
                                        rid,
                                        graph,
                                        index,
                                    )
                                }
                                // A SymbolOnly (cross-app dep) implementer: object-level
                                // applicability holds by construction (a known interface
                                // implementer read from SymbolReference); the member is
                                // opaque (no source) — same PASS rule as the live FreshOnly
                                // gate (differential.rs).
                                RouteTarget::AbiSymbol { .. } => true,
                                // Unresolved (Rule-1/2 failure) claims nothing → vacuously sound.
                                RouteTarget::Unresolved => true,
                                // A Builtin target on an interface fan-out site is anomalous.
                                RouteTarget::Builtin(_) => false,
                            };
                            if !ok {
                                interface_applicability_violations += 1;
                            }
                        }
                    }
                    // No recovered call-site context for this Polymorphic edge (or a
                    // context of the WRONG kind — shouldn't happen, but treated the
                    // same way) — cannot independently verify any concrete route it
                    // claims. Fail-closed: an unverifiable route is not a proven-sound
                    // one.
                    None | Some(FanOutSiteContext::Trigger(_)) => {
                        for route in edge.all_routes() {
                            if route.target != RouteTarget::Unresolved {
                                interface_applicability_violations += 1;
                            }
                        }
                    }
                }
            }

            // ── ImplicitTrigger (Multicast) fan-out ─────────────────────────
            EdgeKind::ImplicitTrigger if edge.shape == DispatchShape::Multicast => {
                match fan_out_ctx.get(&edge.site) {
                    Some(FanOutSiteContext::Trigger(ctx)) => {
                        for route in edge.all_routes() {
                            let ok = match &route.target {
                                RouteTarget::Routine(rid) => {
                                    implicit_trigger_routes_checked += 1;
                                    if matches!(ctx.kind, RecordOpKind::Validate) {
                                        target_is_on_table_or_extension(
                                            &rid.object,
                                            &ctx.table,
                                            graph,
                                            index,
                                        )
                                    } else {
                                        implicit_trigger_route_applicable(ctx, rid, graph, index)
                                    }
                                }
                                RouteTarget::Unresolved => true,
                                _ => false,
                            };
                            if !ok {
                                implicit_trigger_violations += 1;
                            }
                        }
                    }
                    // Same fail-closed rationale as the Interface case above.
                    None | Some(FanOutSiteContext::Interface { .. }) => {
                        for route in edge.all_routes() {
                            if route.target != RouteTarget::Unresolved {
                                implicit_trigger_violations += 1;
                            }
                        }
                    }
                }
            }

            // ── EventFlow ────────────────────────────────────────────────────
            EdgeKind::EventFlow => {
                // If the publisher object doesn't resolve in the graph, SKIP —
                // matching the old `differential::run_event_flow_gate`'s
                // `continue` + `fresh_unprojectable` counting (1B.3b Task 2
                // fix). Running the teeth against a `pub_name_lc` that silently
                // fell back to "" via `unwrap_or_default()` would be a
                // meaningless check, not a sound one.
                if let Some(pub_obj) = graph.objects.iter().find(|o| o.id == edge.from.object) {
                    let pub_name_lc = pub_obj.name.fold_identifier();
                    let pub_type_lc = format!("{:?}", edge.from.object.kind).to_ascii_lowercase();
                    // Look up the publisher ROUTINE's `include_sender` (Task 1) — the
                    // conditional Sender-tolerant bound needs it, and it is NOT
                    // recoverable from `edge.from` alone (a `RoutineNodeId`).
                    // `graph.routines` is sorted by id at construction, mirroring the
                    // `binary_search_by` lookup idiom used throughout `resolve/` (e.g.
                    // `index.rs`/`resolver.rs`).
                    let pub_include_sender = graph
                        .routines
                        .binary_search_by(|probe| probe.id.cmp(&edge.from))
                        .ok()
                        .and_then(|i| graph.routines[i].include_sender);
                    for route in edge.all_routes() {
                        if let RouteTarget::Routine(sub_rid) = &route.target {
                            event_routes_checked += 1;
                            let ok = verify_event_subscriber_route(
                                sub_rid,
                                &pub_type_lc,
                                &pub_name_lc,
                                &edge.from.name_lc,
                                edge.from.params_count,
                                pub_include_sender,
                                parsed,
                                &graph.apps,
                            );
                            if !ok {
                                event_violations += 1;
                            }
                        }
                    }
                }
            }

            _ => {}
        }

        // ── Instance-builtin / enum-static catalog fan-out ────────────────────
        // Route-level, independent of edge kind/shape: these are
        // `DispatchShape::Exact` `Evidence::Catalog` `Builtin` routes (the
        // `member_catalog_route` path in `resolver::resolve_member`),
        // identified by the `BuiltinId` prefix, not by shape.
        for route in edge.all_routes() {
            if let RouteTarget::Builtin(bid) = &route.target {
                // Not a fan-out catalog route (RecordRef::/Text::/JsonObject::/…) →
                // `None` — direct single-dispatch catalog routes need no applicability
                // check (the witness IS the proof; see the route-level loop's doc
                // comment above).
                let ok =
                    match bid.0.split_once("::") {
                        Some(("PageInstance", method_lc)) => Some(
                            instance_builtin_route_applicable(ObjectKind::Page, method_lc),
                        ),
                        Some(("ReportInstance", method_lc)) => Some(
                            instance_builtin_route_applicable(ObjectKind::Report, method_lc),
                        ),
                        Some(("Enum", method_lc)) => Some(member_builtin(
                            MemberCatalogKind::Framework(&FrameworkKind::Enum),
                            method_lc,
                        )),
                        _ => None,
                    };
                if ok.is_some() {
                    instance_builtin_routes_checked += 1;
                }
                if ok == Some(false) {
                    instance_builtin_violations += 1;
                }
            }
        }
    }

    let abi_report = abi_ingestion_integrity(edges, raw_abi);
    // STRUCTURAL (Task 2 review fix, Finding 2) — whole-graph, not just the
    // routes THIS edge set happened to touch: every `RoutineNode` in the
    // graph must keep `abi_overload_collapsed`/`abi_params` in lockstep (see
    // `ApplicabilityReport::abi_overload_collapsed_lockstep_violations`'s doc).
    let abi_overload_collapsed_lockstep_violations = graph
        .routines
        .iter()
        .filter(|r| {
            r.abi_overload_collapsed != matches!(r.abi_params, AbiParams::CollapsedUntrusted)
        })
        .count();
    ApplicabilityReport {
        total_routes,
        witness_contract_violations,
        abi_unmapped: abi_report.abi_unmapped,
        abi_overload_collapsed_lockstep_violations,
        interface_applicability_violations,
        instance_builtin_violations,
        implicit_trigger_violations,
        event_violations,
        interface_routes_checked,
        instance_builtin_routes_checked,
        implicit_trigger_routes_checked,
        event_routes_checked,
    }
}

/// Compare the fresh resolver's output for `workspace_root` against `golden`.
///
/// Internally builds the snapshot + graph (for `AppRegistry`) and calls
/// `resolve_full_program`.  Filters fresh edges to the workspace app before
/// projecting.  Used by the in-repo fixture assertion.
#[must_use]
pub fn run_semantic_diff(workspace_root: &Path, golden: &SemanticGolden) -> SemanticDiff {
    use crate::program::abi_ingest::AbiCache;
    use crate::program::build::build_program_graph;
    use crate::program::resolve::full::resolve_full_program;
    use crate::snapshot::SnapshotBuilder;

    let snap = match (SnapshotBuilder {
        workspace_root: workspace_root.to_path_buf(),
        local_providers: vec![],
    })
    .build()
    {
        Ok(s) => s,
        Err(_) => return SemanticDiff::default(),
    };
    let graph = build_program_graph(&snap, &AbiCache::new());
    let Some(ws_ref) = graph.apps.find(&snap.workspace_app) else {
        return SemanticDiff::default();
    };
    let Some(report) = resolve_full_program(workspace_root) else {
        return SemanticDiff::default();
    };
    // Filter to workspace app (matches L3's workspace-only scope).
    let ws_edges: Vec<Edge> = report
        .edges
        .into_iter()
        .filter(|ce| ce.edge.from.object.app == ws_ref)
        .map(|ce| ce.edge)
        .collect();
    let fresh_canonical = project_fresh(&ws_edges, &graph.apps);
    assert_against_semantic_golden(&fresh_canonical, golden)
}

/// Run the route-applicability check over `workspace_root`.
///
/// Builds the snapshot, raw-ABI index, [`ResolveIndex`], parsed units, and
/// (1B.3b Task 2) the [`FanOutSiteContext`] map internally, then delegates to
/// [`route_applicability`].
#[must_use]
pub fn run_route_applicability(workspace_root: &Path) -> ApplicabilityReport {
    use crate::program::resolve::full::{build_context, resolve_full_program_with};

    let Some(ctx) = build_context(workspace_root) else {
        return ApplicabilityReport::default();
    };
    let report = resolve_full_program_with(&ctx);
    run_route_applicability_on(&ctx, &report)
}

/// Substrate-taking core of [`run_route_applicability`] — builds the raw-ABI
/// index, [`ResolveIndex`], and [`FanOutSiteContext`] map from `ctx` instead
/// of rebuilding the snapshot/graph/parse internally, and reads the resolved
/// edges from `report` instead of calling [`crate::program::resolve::full::
/// resolve_full_program`] a second time.
#[must_use]
pub fn run_route_applicability_on(
    ctx: &crate::program::resolve::full::ProgramContext,
    report: &crate::program::resolve::full::ProgramReport,
) -> ApplicabilityReport {
    let raw_abi = build_raw_abi_index_from_snapshot(&ctx.snap, &ctx.graph.apps);
    let index = ResolveIndex::build(&ctx.graph);
    let fan_out_ctx = build_fan_out_site_context(
        &ctx.graph,
        &index,
        &ctx.parsed,
        ctx.primary_app_ref,
        &ctx.ws_file_set,
    );
    let all_edges: Vec<Edge> = report.edges.iter().map(|ce| ce.edge.clone()).collect();
    route_applicability(
        &all_edges,
        &raw_abi,
        &ctx.graph,
        &index,
        &fan_out_ctx,
        &ctx.parsed,
    )
}

/// Run the [`crate::program::resolve::index::count_unknown_include_sender_
/// plus1_subscribers`] preflight diagnostic over `workspace_root` (Task 1
/// round-2 addendum, folded in by Task 2) — builds the snapshot + graph
/// internally, mirroring [`run_route_applicability`]. Returns `None` when
/// the snapshot fails to build (fail-closed; callers should treat that as
/// "cannot measure", never as "0").
#[must_use]
pub fn run_unknown_include_sender_plus1_subscribers_preflight(
    workspace_root: &Path,
) -> Option<usize> {
    use crate::program::resolve::full::build_context;

    let ctx = build_context(workspace_root)?;
    Some(run_unknown_include_sender_plus1_subscribers_preflight_on(
        &ctx,
    ))
}

/// Substrate-taking core of [`run_unknown_include_sender_plus1_subscribers_preflight`]
/// — reads the program graph from `ctx` instead of rebuilding the
/// snapshot/graph internally.
#[must_use]
pub fn run_unknown_include_sender_plus1_subscribers_preflight_on(
    ctx: &crate::program::resolve::full::ProgramContext,
) -> usize {
    use crate::program::resolve::index::count_unknown_include_sender_plus1_subscribers;

    count_unknown_include_sender_plus1_subscribers(&ctx.graph)
}

/// CDO semantic audit: compare the fresh resolver against the COMMITTED,
/// ANONYMIZED, FROZEN L3 verdict (`cdo-anon.json`) over a real workspace.
///
/// 1B.3b Task 1: this NO LONGER calls [`project_l3`] (or builds an L3
/// workspace at all) — it LOADS the committed golden and anonymizes the
/// fresh side with the SAME [`anon::anon`] so the two align. 1B.3b Task 3
/// went further: `src/program/resolve` (this module + `differential.rs`) now
/// has ZERO `engine::l3`/`engine::l2` imports — [`project_l3`] itself moved
/// to [`crate::program::l3_mint`] (OUTSIDE `src/program/resolve`), called
/// only by [`mint_l3_validated_golden`]/[`mint_l3_trigger_golden`] (the
/// dev-mint tool's sanctioned callers; also Test 14's `REGEN_TEMP_GOLDENS`
/// path) — `run_cdo_semantic_audit` itself touches neither.
///
/// Callers should gate this on `CDO_WS` env var before calling — this
/// function still does a real fresh-resolution build, which is expensive on
/// CDO-scale workspaces.
///
/// Returns a [`CdoSemanticAuditReport`]. `golden_loaded == false` means
/// `cdo-anon.json` is missing/invalid (the `ENFORCE_CDO_WS` guard in
/// `tests/program_resolve_harness.rs` hard-fails on this).
#[must_use]
pub fn run_cdo_semantic_audit(workspace_root: &Path) -> CdoSemanticAuditReport {
    use crate::program::resolve::full::{build_context, resolve_full_program_with};

    let Some(ctx) = build_context(workspace_root) else {
        // Mirror the old snap-build/ws_ref-lookup failure arms: the golden
        // is still loaded (and its counts reported) even though there is no
        // fresh side to compare against.
        let golden = load_anon_golden(&cdo_anon_golden_path());
        let golden_loaded = golden.is_some();
        let l3_total = golden.map(|g| g.entries.len()).unwrap_or_default();
        return CdoSemanticAuditReport {
            golden_loaded,
            l3_total,
            ..Default::default()
        };
    };
    let report = resolve_full_program_with(&ctx);
    run_cdo_semantic_audit_on(&ctx, &report, workspace_root)
}

/// Substrate-taking core of [`run_cdo_semantic_audit`] — reads the program
/// graph from `ctx` and the resolved edges from `report` instead of
/// rebuilding the snapshot/graph/resolve pass internally. `workspace_root`
/// is still needed for the golden-drift warning stamp.
#[must_use]
pub fn run_cdo_semantic_audit_on(
    ctx: &crate::program::resolve::full::ProgramContext,
    report: &crate::program::resolve::full::ProgramReport,
    workspace_root: &Path,
) -> CdoSemanticAuditReport {
    run_cdo_semantic_audit_on_with(ctx, report, workspace_root, true)
}

/// The audit WITHOUT the adjudication overlay — fresh against the raw,
/// unmodified L3 golden.
///
/// This exists so a caller can ask the question the effective audit cannot:
/// *which sites does the overlay actually need to correct?* The overlay is
/// applied before the only diff, so an effective `genuine_wrong == 0` is
/// equally consistent with "nothing is wrong" and with "the overlay is
/// silencing something". Comparing the RAW genuine-wrong set against the
/// overlay's own entries is what makes an EMPTY overlay a checked claim rather
/// than an accepted one — delete a needed override and the raw site remains
/// while the overlay does not.
#[must_use]
pub fn run_cdo_semantic_audit_on_raw(
    ctx: &crate::program::resolve::full::ProgramContext,
    report: &crate::program::resolve::full::ProgramReport,
    workspace_root: &Path,
) -> CdoSemanticAuditReport {
    run_cdo_semantic_audit_on_with(ctx, report, workspace_root, false)
}

fn run_cdo_semantic_audit_on_with(
    ctx: &crate::program::resolve::full::ProgramContext,
    report: &crate::program::resolve::full::ProgramReport,
    workspace_root: &Path,
    apply_overlay: bool,
) -> CdoSemanticAuditReport {
    // ── Load the committed, anonymized golden (NO project_l3 call here) ──────
    let golden = load_anon_golden(&cdo_anon_golden_path());
    let golden_loaded = golden.is_some();
    let mut golden = golden.unwrap_or_default();
    let l3_total = golden.entries.len();
    // 1B.3b Task 1 fix (Fix 4): warn (never fail) when CDO_WS has drifted
    // from the golden's mint-time stamp.
    if golden_loaded {
        warn_on_workspace_drift(&golden.metadata, workspace_root);
    }

    // ── beyond-1B.3b Task 3: overlay the source-adjudicated corrections ──────
    // IN-MEMORY only — `cdo-anon.json` itself is never mutated. Sites whose
    // adjudicated verdict is `l3_error_intrinsic` get their L3 target
    // replaced with the independently-confirmed catalog target BEFORE the
    // diff below, so fresh is compared against the ADJUDICATED oracle, not
    // the raw (known-wrong) L3 one. See `apply_adjudicated_overrides`.
    let mut overlay = OverlayOutcome::default();
    if apply_overlay
        && let Some(overrides) = load_adjudicated_overrides(&adjudicated_overrides_path())
    {
        overlay = apply_adjudicated_overrides_detailed(&mut golden, &overrides);
        eprintln!(
            "Adjudication overlay: {}/{} l3_error_intrinsic override(s) applied \
             ({} changed a target, {} no-op, {} unmatched)",
            overlay.matched,
            overlay.intrinsic_total,
            overlay.changed,
            overlay.no_op_sites.len(),
            overlay.unmatched_sites.len(),
        );
    }

    // ── Graph for AppRegistry (needed for project_fresh) ──────────────────────
    let graph = &ctx.graph;
    let Some(ws_ref) = graph.apps.find(&ctx.snap.workspace_app) else {
        return CdoSemanticAuditReport {
            golden_loaded,
            l3_total,
            ..Default::default()
        };
    };

    // ── Fresh resolver ────────────────────────────────────────────────────────
    // Filter to workspace app (L3 is workspace-scoped).
    let ws_edges: Vec<Edge> = report
        .edges
        .iter()
        .filter(|ce| ce.edge.from.object.app == ws_ref)
        .map(|ce| ce.edge.clone())
        .collect();
    let fresh_total = ws_edges.len();

    // ── Project fresh → canonical → plaintext map → anonymized map ───────────
    let fresh_canonical = project_fresh(&ws_edges, &graph.apps);
    let mut fresh_plain: BTreeMap<GoldenSiteKey, BTreeSet<GoldenTarget>> = BTreeMap::new();
    for e in &fresh_canonical {
        let key = canonical_to_golden_key(e);
        let targets = canonical_targets_to_golden(&e.targets);
        fresh_plain.entry(key).or_default().extend(targets);
    }
    let (fresh_anon, reverse_site) = anonymize_fresh_map(&fresh_plain, anon::SITE_DOMAIN_V1);

    // ── Diff (anonymized) ─────────────────────────────────────────────────────
    let diff = diff_against_anon_golden(&fresh_anon, &golden);

    // ── Adjudicate fresh_wrong into fresh_ahead_dispatch vs genuine_wrong ────
    let obj_lookup_anon = build_obj_lookup_anon(graph);

    let mut fresh_ahead_dispatch_count = 0usize;
    let mut genuine_wrong_sites: Vec<GoldenSiteKey> = Vec::new();
    let mut deanon: BTreeMap<String, String> = BTreeMap::new();
    // Record plaintext for every fresh site/target this run touched, not just
    // the failures — cheap (already in memory) and keeps the local deanon map
    // maximally useful for root-causing ANY future failure, not just today's.
    for (site, targets) in &fresh_plain {
        let asite = anonymize_site_key(site, anon::SITE_DOMAIN_V1);
        record_site_deanon(site, &asite, &mut deanon);
        for t in targets {
            let at = anonymize_target(t);
            record_target_deanon(t, &at, &mut deanon);
        }
    }

    for fw in &diff.fresh_wrong {
        if is_fresh_ahead_dispatch_anon(&fw.fresh_targets, &fw.l3_targets, &obj_lookup_anon) {
            fresh_ahead_dispatch_count += 1;
        } else if let Some(plain_site) = reverse_site.get(&fw.site) {
            genuine_wrong_sites.push(plain_site.clone());
            if std::env::var("DEBUG_GENUINE_WRONG_TARGETS").is_ok() {
                eprintln!("  DEBUG_GENUINE_WRONG_TARGETS site={plain_site:?}");
                for t in &fw.fresh_targets {
                    eprintln!(
                        "    fresh_target: obj={:?} routine={:?}",
                        deanon.get(&t.object_id.0),
                        t.routine_id.as_ref().and_then(|r| deanon.get(&r.0)),
                    );
                }
                for t in &fw.l3_targets {
                    eprintln!(
                        "    l3_target:    obj={:?} routine={:?}",
                        deanon.get(&t.object_id.0),
                        t.routine_id.as_ref().and_then(|r| deanon.get(&r.0)),
                    );
                }
            }
        }
        // A fw.site with no `reverse_site` entry cannot happen: `fw` is built
        // from `fresh_anon`'s keys, and `reverse_site` is populated 1:1 with
        // `fresh_anon` by `anonymize_fresh_map`.
    }
    merge_deanon_map(&cdo_deanon_map_path(), &deanon);

    eprintln!(
        "\nAdjudication: fresh_wrong={} → fresh_ahead_dispatch={} genuine_wrong={}",
        diff.fresh_wrong.len(),
        fresh_ahead_dispatch_count,
        genuine_wrong_sites.len(),
    );
    for site in &genuine_wrong_sites {
        eprintln!("  GENUINE_WRONG site={site:?}");
    }

    // ── Deterministic digest (over the ANONYMIZED comparison) ────────────────
    let mut hasher = Sha256::new();
    for entry in &golden.entries {
        let fresh_targets = fresh_anon.get(&entry.site).cloned().unwrap_or_default();
        let k_json = serde_json::to_string(&entry.site).unwrap_or_default();
        let l_json = serde_json::to_string(&entry.targets).unwrap_or_default();
        let f_json = serde_json::to_string(&fresh_targets).unwrap_or_default();
        hasher.update(format!("{k_json}|{l_json}|{f_json}\n").as_bytes());
    }
    let digest_bytes = hasher.finalize();
    let digest: String = digest_bytes.iter().map(|b| format!("{b:02x}")).collect();

    CdoSemanticAuditReport {
        golden_loaded,
        l3_total,
        fresh_total,
        paired: diff.total_paired,
        fresh_wrong_count: diff.fresh_wrong.len(),
        fresh_ahead_dispatch_count,
        genuine_wrong_count: genuine_wrong_sites.len(),
        genuine_wrong_sites,
        fresh_missing_count: diff.fresh_missing,
        fresh_extra_count: diff.fresh_extra,
        fresh_novel: diff.fresh_novel,
        golden_missing: diff.golden_missing,
        digest,
        overlay,
    }
}

/// CDO ImplicitTrigger frozen-golden audit: compare the fresh resolver's
/// `ImplicitTrigger` edges against the committed, anonymized L3 verdict
/// (`cdo-trigger-anon.json`). See [`AnonTriggerAuditReport`]'s doc comment for
/// this audit's scope (mechanism proof + `ENFORCE_CDO_WS` backing — NOT a
/// genuine-wrong adjudication gate; that's covered by the frozen trigger
/// baseline + fixture + the ported applicability teeth, see
/// [`AnonTriggerAuditReport`]).
#[must_use]
pub fn run_cdo_trigger_audit(workspace_root: &Path) -> AnonTriggerAuditReport {
    use crate::program::resolve::full::{build_context, resolve_full_program_with};

    let fresh_golden = match build_context(workspace_root) {
        Some(ctx) => {
            let report = resolve_full_program_with(&ctx);
            mint_fresh_golden_for_kind_on(&ctx, &report, EdgeKind::ImplicitTrigger)
        }
        // Build failure: proceed with an EMPTY fresh side, exactly as the
        // pre-split body did (its `mint_fresh_golden_for_kind` returned
        // `SemanticGolden::default()` on snapshot/resolve failure and the
        // audit still ran — non-empty digest over the golden entries, drift
        // warning, deanon merge).
        None => SemanticGolden::default(),
    };
    trigger_audit_from_fresh(&fresh_golden, workspace_root)
}

/// Substrate-taking core of [`run_cdo_trigger_audit`] — mints the fresh
/// `ImplicitTrigger` golden from `ctx`/`report` instead of re-resolving the
/// whole program a second time.
#[must_use]
pub fn run_cdo_trigger_audit_on(
    ctx: &crate::program::resolve::full::ProgramContext,
    report: &crate::program::resolve::full::ProgramReport,
    workspace_root: &Path,
) -> AnonTriggerAuditReport {
    let fresh_golden = mint_fresh_golden_for_kind_on(ctx, report, EdgeKind::ImplicitTrigger);
    trigger_audit_from_fresh(&fresh_golden, workspace_root)
}

/// Shared audit body of [`run_cdo_trigger_audit`]/[`run_cdo_trigger_audit_on`]:
/// everything downstream of minting the fresh golden (golden load, drift warn,
/// anonymize, diff, deanon merge, digest). Taking the fresh side as a
/// parameter keeps the path wrapper's build-failure arm behavior-identical to
/// the pre-split body (empty fresh side, audit still runs).
fn trigger_audit_from_fresh(
    fresh_golden: &SemanticGolden,
    workspace_root: &Path,
) -> AnonTriggerAuditReport {
    let golden = load_anon_golden(&cdo_trigger_anon_golden_path());
    let golden_loaded = golden.is_some();
    let golden = golden.unwrap_or_default();
    let l3_total = golden.entries.len();
    if golden_loaded {
        warn_on_workspace_drift(&golden.metadata, workspace_root);
    }

    let fresh_total = fresh_golden.entries.len();

    let mut fresh_plain: BTreeMap<GoldenSiteKey, BTreeSet<GoldenTarget>> = BTreeMap::new();
    for e in &fresh_golden.entries {
        fresh_plain.insert(e.site.clone(), e.targets.clone());
    }
    let (fresh_anon, _reverse_site) = anonymize_fresh_map(&fresh_plain, anon::TRIGGER_OP_DOMAIN_V1);

    let diff = diff_against_anon_golden(&fresh_anon, &golden);

    let mut deanon: BTreeMap<String, String> = BTreeMap::new();
    for (site, targets) in &fresh_plain {
        let asite = anonymize_site_key(site, anon::TRIGGER_OP_DOMAIN_V1);
        record_site_deanon(site, &asite, &mut deanon);
        for t in targets {
            let at = anonymize_target(t);
            record_target_deanon(t, &at, &mut deanon);
        }
    }
    merge_deanon_map(&cdo_deanon_map_path(), &deanon);

    let mut hasher = Sha256::new();
    for entry in &golden.entries {
        let fresh_targets = fresh_anon.get(&entry.site).cloned().unwrap_or_default();
        let k_json = serde_json::to_string(&entry.site).unwrap_or_default();
        let l_json = serde_json::to_string(&entry.targets).unwrap_or_default();
        let f_json = serde_json::to_string(&fresh_targets).unwrap_or_default();
        hasher.update(format!("{k_json}|{l_json}|{f_json}\n").as_bytes());
    }
    let digest: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    AnonTriggerAuditReport {
        golden_loaded,
        l3_total,
        fresh_total,
        total_paired: diff.total_paired,
        matches: diff.matches,
        fresh_wrong_count: diff.fresh_wrong.len(),
        fresh_missing: diff.fresh_missing,
        fresh_extra: diff.fresh_extra,
        fresh_novel: diff.fresh_novel,
        golden_missing: diff.golden_missing,
        digest,
    }
}

/// CDO EventFlow frozen-golden audit: compare the fresh resolver's resolved
/// EventFlow publisher→subscriber pairs against the committed, anonymized L3
/// verdict (`cdo-event-anon.json`). Arity-agnostic pair-set comparison only —
/// see [`AnonEventAuditReport`]'s doc comment for scope.
#[must_use]
pub fn run_cdo_event_audit(workspace_root: &Path) -> AnonEventAuditReport {
    use crate::program::resolve::full::build_context;

    let fresh_rows = match build_context(workspace_root) {
        Some(ctx) => crate::program::resolve::differential::project_fresh_event_rows_on(&ctx),
        // Build failure: proceed with an EMPTY fresh side, exactly as the
        // pre-split body did (its `project_fresh_event_rows` returned an
        // empty vec on snapshot failure and the audit still ran — non-empty
        // digest over the golden pairs, drift warning, deanon merge).
        None => Vec::new(),
    };
    event_audit_from_fresh(&fresh_rows, workspace_root)
}

/// Substrate-taking core of [`run_cdo_event_audit`] — projects the fresh
/// EventFlow rows from `ctx` (via [`crate::program::resolve::differential::
/// project_fresh_event_rows_on`]) instead of rebuilding the snapshot/graph
/// internally. The resolve report is not needed here — EventFlow rows come
/// from [`crate::program::resolve::resolver::emit_event_flow_edges`] directly.
#[must_use]
pub fn run_cdo_event_audit_on(
    ctx: &crate::program::resolve::full::ProgramContext,
    workspace_root: &Path,
) -> AnonEventAuditReport {
    let fresh_rows = crate::program::resolve::differential::project_fresh_event_rows_on(ctx);
    event_audit_from_fresh(&fresh_rows, workspace_root)
}

/// Shared audit body of [`run_cdo_event_audit`]/[`run_cdo_event_audit_on`]:
/// everything downstream of projecting the fresh EventFlow rows (golden load,
/// drift warn, anonymize, deanon merge, pair-set diff, digest). Taking the
/// fresh side as a parameter keeps the path wrapper's build-failure arm
/// behavior-identical to the pre-split body (empty fresh side, audit still
/// runs).
fn event_audit_from_fresh(
    fresh_rows: &[crate::program::resolve::differential::CanonicalEventRow],
    workspace_root: &Path,
) -> AnonEventAuditReport {
    let golden = load_anon_event_golden(&cdo_event_anon_golden_path());
    let golden_loaded = golden.is_some();
    let golden = golden.unwrap_or_default();
    let l3_total = golden.entries.len();
    if golden_loaded {
        warn_on_workspace_drift(&golden.metadata, workspace_root);
    }

    let fresh_total = fresh_rows.len();

    let mut deanon: BTreeMap<String, String> = BTreeMap::new();
    let fresh_golden = anonymize_event_rows_with_deanon(fresh_rows, &mut deanon);
    merge_deanon_map(&cdo_deanon_map_path(), &deanon);

    let l3_pairs: BTreeSet<AnonEventPairKey> =
        golden.entries.iter().map(|e| e.pair.clone()).collect();
    let fresh_pairs: BTreeSet<AnonEventPairKey> = fresh_golden
        .entries
        .iter()
        .map(|e| e.pair.clone())
        .collect();

    let matched_pairs = l3_pairs.intersection(&fresh_pairs).count();
    let pair_l3_only = l3_pairs.difference(&fresh_pairs).count();
    let pair_fresh_only = fresh_pairs.difference(&l3_pairs).count();

    let mut hasher = Sha256::new();
    for pair in l3_pairs.union(&fresh_pairs) {
        let in_l3 = l3_pairs.contains(pair);
        let in_fresh = fresh_pairs.contains(pair);
        let p_json = serde_json::to_string(pair).unwrap_or_default();
        hasher.update(format!("{p_json}|{in_l3}|{in_fresh}\n").as_bytes());
    }
    let digest: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    AnonEventAuditReport {
        golden_loaded,
        l3_total,
        fresh_total,
        matched_pairs,
        pair_l3_only,
        pair_fresh_only,
        digest,
    }
}

// ---------------------------------------------------------------------------
// Tests (1B.3b Task 2): the ported fan-out applicability teeth actually bite
// ---------------------------------------------------------------------------
//
// These exercise `route_applicability`'s four SOUNDNESS checks directly,
// against hand-built `Edge`/`Route`/`FanOutSiteContext` fixtures — mirroring
// `applicability.rs`'s own predicate-level test style (`make_app`/`make_obj`/
// `build_graph`, duplicated rather than shared cross-module — the
// established convention in this crate's resolve test suites). Each kind
// gets a POSITIVE case (an applicable route → the matching violation counter
// stays 0) and a FABRICATED NEGATIVE case (a deliberately non-applicable
// route → the matching counter increments), proving the ported teeth are
// live, not vacuous. The end-to-end on-disk fixture + CDO_WS run lives in
// `tests/program_resolve_harness.rs` (Test 20).
#[cfg(test)]
mod tests {
    use super::*;

    use crate::engine::deps::symbol_reference::SymbolReferenceAbi;
    use crate::program::graph::ObjectIndex;
    use crate::program::node::{AppRegistry, RoutineNodeId};
    use crate::program::node_extract::{AbiParams, Access, RoutineNode, extract_nodes};
    use crate::program::resolve::edge::{
        BuiltinId, CanonicalSpan, Evidence, OpenWorldReason, Route, SetCompleteness, SourcePos,
        Witness,
    };
    use crate::program::topology::DependencyGraph;
    use crate::snapshot::{AppId, ParsedFile, Provenance, TrustTier};

    // -----------------------------------------------------------------------
    // Shared fixture helpers
    // -----------------------------------------------------------------------

    fn empty_raw_abi() -> RawAbiIndex {
        let empty: Vec<(AppRef, &SymbolReferenceAbi)> = Vec::new();
        RawAbiIndex::build(empty)
    }

    fn make_app() -> (AppRegistry, AppRef) {
        let mut apps = AppRegistry::default();
        let r = apps.intern(&AppId {
            guid: String::new(),
            name: "TestApp".into(),
            publisher: "T".into(),
            version: "1.0.0.0".into(),
        });
        (apps, r)
    }

    fn make_obj(app: AppRef, kind: ObjectKind, name: &str, implements: Vec<&str>) -> ObjectNode {
        ObjectNode {
            id: ObjectNodeId {
                app,
                kind,
                key: ObjKey::Name(name.to_ascii_lowercase()),
            },
            name: name.to_string(),
            declared_id: None,
            extends_target: None,
            implements: implements.into_iter().map(str::to_string).collect(),
            tier: TrustTier::Workspace,
            source_table: None,
            table_no: None,
            source_table_temporary: false,
            page_controls: vec![],
            fields: vec![],
            dataitems: vec![],
            parse_incomplete: false,
        }
    }

    fn make_routine_node(obj_id: &ObjectNodeId, name: &str, params: usize) -> RoutineNode {
        RoutineNode {
            id: RoutineNodeId {
                object: obj_id.clone(),
                name_lc: name.to_ascii_lowercase(),
                enclosing_member_lc: None,
                params_count: params,
                sig_fp: 0,
            },
            name: name.to_string(),
            is_trigger: matches!(
                name.to_ascii_lowercase().as_str(),
                "oninsert" | "onmodify" | "ondelete" | "onrename" | "onvalidate"
            ),
            access: Access::Public,
            tier: TrustTier::Workspace,
            event_subscribers: vec![],
            subscriber_instance_manual: false,
            publisher_kind: None,
            include_sender: None,
            abi_routine_kind: None,
            abi_event_kind: None,
            param_sig_key: String::new(),
            return_type: None,
            return_type_id: None,
            abi_overload_collapsed: false,
            source_overload_aliased: false,
            abi_params: AbiParams::Missing,
        }
    }

    fn build_synth_graph(
        apps: AppRegistry,
        objects: Vec<ObjectNode>,
        routines: Vec<RoutineNode>,
    ) -> (ProgramGraph, ResolveIndex) {
        let mut sorted_objects = objects;
        sorted_objects.sort_by(|a, b| a.id.cmp(&b.id));
        let obj_index = ObjectIndex::build(&sorted_objects);
        let graph = ProgramGraph {
            apps,
            topology: DependencyGraph::default(),
            objects: sorted_objects,
            routines,
            obj_index,
            ..Default::default()
        };
        let index = ResolveIndex::build(&graph);
        (graph, index)
    }

    fn test_span(line: u32) -> CanonicalSpan {
        CanonicalSpan {
            unit: "u.al".into(),
            start: SourcePos { line, col: 1 },
            end: SourcePos { line, col: 5 },
        }
    }

    fn source_route(target: RoutineNodeId) -> Route {
        Route {
            target: RouteTarget::Routine(target),
            evidence: Evidence::Source,
            conditions: vec![],
            witness: Witness::SourceSpan {
                file: "f.al".into(),
                span: (0, 1),
            },
            receiver_tier: None,
        }
    }

    // -----------------------------------------------------------------------
    // Interface (Polymorphic) fan-out
    // -----------------------------------------------------------------------

    #[test]
    fn interface_route_applicable_when_object_implements_iface() {
        let (apps, app) = make_app();
        let impl_obj = make_obj(app, ObjectKind::Codeunit, "FooImpl", vec!["ifoo"]);
        let impl_id = impl_obj.id.clone();
        let bar = make_routine_node(&impl_id, "bar", 0);
        let caller_obj = make_obj(app, ObjectKind::Codeunit, "Caller", vec![]);
        let caller_id = caller_obj.id.clone();
        let (graph, index) = build_synth_graph(apps, vec![impl_obj, caller_obj], vec![bar]);

        let caller_rid = RoutineNodeId {
            object: caller_id,
            name_lc: "go".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        let target_rid = RoutineNodeId {
            object: impl_id,
            name_lc: "bar".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        let site = SiteId {
            caller: caller_rid.clone(),
            span: test_span(1),
            callee_fingerprint: 1,
        };
        let edge = Edge {
            from: caller_rid,
            site: site.clone(),
            kind: EdgeKind::Call,
            shape: DispatchShape::Polymorphic,
            completeness: SetCompleteness::Partial {
                reason: OpenWorldReason::ReverseDependentImplementers,
            },
            routes: vec![source_route(target_rid)],
        };
        let mut ctx: HashMap<SiteId, FanOutSiteContext> = HashMap::new();
        ctx.insert(
            site,
            FanOutSiteContext::Interface {
                iface_lc: "ifoo".into(),
                called_member_lc: "bar".into(),
                arity: 0,
            },
        );

        let raw_abi = empty_raw_abi();
        let report = route_applicability(&[edge], &raw_abi, &graph, &index, &ctx, &[]);
        assert_eq!(
            report.interface_applicability_violations, 0,
            "FooImpl implements ifoo with a unique Bar() → applicable"
        );
    }

    #[test]
    fn interface_route_violation_when_object_does_not_implement_iface() {
        let (apps, app) = make_app();
        // NotImpl does NOT implement "ifoo" — a fabricated non-applicable route.
        let not_impl = make_obj(app, ObjectKind::Codeunit, "NotImpl", vec![]);
        let not_impl_id = not_impl.id.clone();
        let bar = make_routine_node(&not_impl_id, "bar", 0);
        let caller_obj = make_obj(app, ObjectKind::Codeunit, "Caller", vec![]);
        let caller_id = caller_obj.id.clone();
        let (graph, index) = build_synth_graph(apps, vec![not_impl, caller_obj], vec![bar]);

        let caller_rid = RoutineNodeId {
            object: caller_id,
            name_lc: "go".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        let target_rid = RoutineNodeId {
            object: not_impl_id,
            name_lc: "bar".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        let site = SiteId {
            caller: caller_rid.clone(),
            span: test_span(1),
            callee_fingerprint: 1,
        };
        let edge = Edge {
            from: caller_rid,
            site: site.clone(),
            kind: EdgeKind::Call,
            shape: DispatchShape::Polymorphic,
            completeness: SetCompleteness::Partial {
                reason: OpenWorldReason::ReverseDependentImplementers,
            },
            routes: vec![source_route(target_rid)],
        };
        let mut ctx: HashMap<SiteId, FanOutSiteContext> = HashMap::new();
        ctx.insert(
            site,
            FanOutSiteContext::Interface {
                iface_lc: "ifoo".into(),
                called_member_lc: "bar".into(),
                arity: 0,
            },
        );

        let raw_abi = empty_raw_abi();
        let report = route_applicability(&[edge], &raw_abi, &graph, &index, &ctx, &[]);
        assert_eq!(
            report.interface_applicability_violations, 1,
            "NotImpl does not implement ifoo → the ported teeth must catch it"
        );
    }

    #[test]
    fn interface_polymorphic_edge_with_no_recovered_context_fails_closed() {
        let (apps, app) = make_app();
        let impl_obj = make_obj(app, ObjectKind::Codeunit, "FooImpl", vec!["ifoo"]);
        let impl_id = impl_obj.id.clone();
        let bar = make_routine_node(&impl_id, "bar", 0);
        let (graph, index) = build_synth_graph(apps, vec![impl_obj], vec![bar]);

        let caller_rid = RoutineNodeId {
            object: impl_id.clone(),
            name_lc: "go".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        let target_rid = RoutineNodeId {
            object: impl_id,
            name_lc: "bar".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        let edge = Edge {
            from: caller_rid.clone(),
            site: SiteId {
                caller: caller_rid,
                span: test_span(1),
                callee_fingerprint: 1,
            },
            kind: EdgeKind::Call,
            shape: DispatchShape::Polymorphic,
            completeness: SetCompleteness::Partial {
                reason: OpenWorldReason::ReverseDependentImplementers,
            },
            routes: vec![source_route(target_rid)],
        };

        // No fan_out_ctx entry for this edge's SiteId — fail-closed.
        let raw_abi = empty_raw_abi();
        let report = route_applicability(&[edge], &raw_abi, &graph, &index, &HashMap::new(), &[]);
        assert_eq!(
            report.interface_applicability_violations, 1,
            "a Polymorphic edge with no recovered call-site context must fail closed, \
             not silently pass"
        );
    }

    // -----------------------------------------------------------------------
    // Instance-builtin / enum-static catalog fan-out
    // -----------------------------------------------------------------------

    fn builtin_route(id: &str) -> Route {
        Route {
            target: RouteTarget::Builtin(BuiltinId(id.into())),
            evidence: Evidence::Catalog,
            conditions: vec![],
            witness: Witness::CatalogEntry {
                id: BuiltinId(id.into()),
                catalog_version: "test".into(),
            },
            receiver_tier: None,
        }
    }

    fn builtin_edge(app: AppRef, route: Route) -> Edge {
        let rid = RoutineNodeId {
            object: ObjectNodeId {
                app,
                kind: ObjectKind::Codeunit,
                key: ObjKey::Name("caller".into()),
            },
            name_lc: "go".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        Edge {
            from: rid.clone(),
            site: SiteId {
                caller: rid,
                span: test_span(1),
                callee_fingerprint: 1,
            },
            kind: EdgeKind::Call,
            shape: DispatchShape::Exact,
            completeness: SetCompleteness::Complete,
            routes: vec![route],
        }
    }

    #[test]
    fn instance_builtin_route_passes_for_known_method() {
        let (apps, app) = make_app();
        let (graph, index) = build_synth_graph(apps, vec![], vec![]);
        let edge = builtin_edge(app, builtin_route("PageInstance::runmodal"));
        let raw_abi = empty_raw_abi();
        let report = route_applicability(&[edge], &raw_abi, &graph, &index, &HashMap::new(), &[]);
        assert_eq!(report.instance_builtin_violations, 0);
    }

    #[test]
    fn instance_builtin_route_violation_for_unknown_method() {
        let (apps, app) = make_app();
        let (graph, index) = build_synth_graph(apps, vec![], vec![]);
        // Fabricated: "notamethod" is not in the PAGE_INSTANCE catalog.
        let edge = builtin_edge(app, builtin_route("PageInstance::notamethod"));
        let raw_abi = empty_raw_abi();
        let report = route_applicability(&[edge], &raw_abi, &graph, &index, &HashMap::new(), &[]);
        assert_eq!(
            report.instance_builtin_violations, 1,
            "PageInstance::notamethod must be caught by the independent catalog re-check"
        );
    }

    // -----------------------------------------------------------------------
    // ImplicitTrigger (Multicast) fan-out
    // -----------------------------------------------------------------------

    #[test]
    fn implicit_trigger_route_passes_for_correct_table() {
        let (apps, app) = make_app();
        let table = make_obj(app, ObjectKind::Table, "Customer", vec![]);
        let table_id = table.id.clone();
        let oninsert = make_routine_node(&table_id, "oninsert", 0);
        let (graph, index) = build_synth_graph(apps, vec![table], vec![oninsert]);

        let caller_rid = RoutineNodeId {
            object: table_id.clone(),
            name_lc: "go".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        let target_rid = RoutineNodeId {
            object: table_id.clone(),
            name_lc: "oninsert".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        let site = SiteId {
            caller: caller_rid.clone(),
            span: test_span(1),
            callee_fingerprint: 1,
        };
        let edge = Edge {
            from: caller_rid,
            site: site.clone(),
            kind: EdgeKind::ImplicitTrigger,
            shape: DispatchShape::Multicast,
            completeness: SetCompleteness::Partial {
                reason: OpenWorldReason::ReverseDependentExtensions,
            },
            routes: vec![source_route(target_rid)],
        };
        let mut ctx: HashMap<SiteId, FanOutSiteContext> = HashMap::new();
        ctx.insert(
            site,
            FanOutSiteContext::Trigger(RecordOpCtx {
                kind: RecordOpKind::Insert,
                table: table_id,
                field: None,
                run_trigger: RunTrigger::Guarded,
            }),
        );

        let raw_abi = empty_raw_abi();
        let report = route_applicability(&[edge], &raw_abi, &graph, &index, &ctx, &[]);
        assert_eq!(report.implicit_trigger_violations, 0);
    }

    #[test]
    fn implicit_trigger_route_violation_for_unrelated_table() {
        let (apps, app) = make_app();
        let customer = make_obj(app, ObjectKind::Table, "Customer", vec![]);
        let customer_id = customer.id.clone();
        // Fabricated: Vendor's OnInsert is unrelated to Customer.
        let vendor = make_obj(app, ObjectKind::Table, "Vendor", vec![]);
        let vendor_id = vendor.id.clone();
        let vendor_oninsert = make_routine_node(&vendor_id, "oninsert", 0);
        let (graph, index) = build_synth_graph(apps, vec![customer, vendor], vec![vendor_oninsert]);

        let caller_rid = RoutineNodeId {
            object: customer_id.clone(),
            name_lc: "go".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        let target_rid = RoutineNodeId {
            object: vendor_id,
            name_lc: "oninsert".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        let site = SiteId {
            caller: caller_rid.clone(),
            span: test_span(1),
            callee_fingerprint: 1,
        };
        let edge = Edge {
            from: caller_rid,
            site: site.clone(),
            kind: EdgeKind::ImplicitTrigger,
            shape: DispatchShape::Multicast,
            completeness: SetCompleteness::Partial {
                reason: OpenWorldReason::ReverseDependentExtensions,
            },
            routes: vec![source_route(target_rid)],
        };
        let mut ctx: HashMap<SiteId, FanOutSiteContext> = HashMap::new();
        ctx.insert(
            site,
            FanOutSiteContext::Trigger(RecordOpCtx {
                kind: RecordOpKind::Insert,
                table: customer_id,
                field: None,
                run_trigger: RunTrigger::Guarded,
            }),
        );

        let raw_abi = empty_raw_abi();
        let report = route_applicability(&[edge], &raw_abi, &graph, &index, &ctx, &[]);
        assert_eq!(
            report.implicit_trigger_violations, 1,
            "Vendor's OnInsert must not fire for a Customer Insert — the ported teeth \
             must catch it"
        );
    }

    #[test]
    fn implicit_trigger_edge_with_no_recovered_context_fails_closed() {
        // Mirrors `interface_polymorphic_edge_with_no_recovered_context_fails_closed`
        // for the ImplicitTrigger/Multicast side (1B.3b Task 2 fix): a Multicast
        // ImplicitTrigger edge whose site has no recoverable RecordOpCtx must fail
        // closed, not silently pass.
        let (apps, app) = make_app();
        let table = make_obj(app, ObjectKind::Table, "Customer", vec![]);
        let table_id = table.id.clone();
        let oninsert = make_routine_node(&table_id, "oninsert", 0);
        let (graph, index) = build_synth_graph(apps, vec![table], vec![oninsert]);

        let caller_rid = RoutineNodeId {
            object: table_id.clone(),
            name_lc: "go".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        let target_rid = RoutineNodeId {
            object: table_id,
            name_lc: "oninsert".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        let edge = Edge {
            from: caller_rid.clone(),
            site: SiteId {
                caller: caller_rid,
                span: test_span(1),
                callee_fingerprint: 1,
            },
            kind: EdgeKind::ImplicitTrigger,
            shape: DispatchShape::Multicast,
            completeness: SetCompleteness::Partial {
                reason: OpenWorldReason::ReverseDependentExtensions,
            },
            routes: vec![source_route(target_rid)],
        };

        // No fan_out_ctx entry for this edge's SiteId — fail-closed.
        let raw_abi = empty_raw_abi();
        let report = route_applicability(&[edge], &raw_abi, &graph, &index, &HashMap::new(), &[]);
        assert_eq!(
            report.implicit_trigger_violations, 1,
            "a Multicast ImplicitTrigger edge with no recovered call-site context must \
             fail closed, not silently pass"
        );
    }

    #[test]
    fn record_op_kind_for_method_recognizes_all_five_dml_ops() {
        // Proves the `build_fan_out_site_context` op-kind match arm directly
        // (1B.3b Task 2 fix) — including the previously-missing `"rename"` arm.
        assert_eq!(
            record_op_kind_for_method("insert"),
            Some(RecordOpKind::Insert)
        );
        assert_eq!(
            record_op_kind_for_method("modify"),
            Some(RecordOpKind::Modify)
        );
        assert_eq!(
            record_op_kind_for_method("delete"),
            Some(RecordOpKind::Delete)
        );
        assert_eq!(
            record_op_kind_for_method("rename"),
            Some(RecordOpKind::Rename),
            "the \"rename\" => Some(RecordOpKind::Rename) arm must fire — its absence \
             previously caused every Rec.Rename() site's context to be silently \
             dropped, false-positive-flagging genuinely sound OnRename trigger routes"
        );
        assert_eq!(
            record_op_kind_for_method("validate"),
            Some(RecordOpKind::Validate)
        );
        // Non-DML record ops are NOT implicit-trigger-firing — must stay `None`.
        assert_eq!(record_op_kind_for_method("setrange"), None);
        assert_eq!(record_op_kind_for_method("findset"), None);
    }

    #[test]
    fn implicit_trigger_route_passes_for_rename_on_correct_table() {
        // End-to-end proof (1B.3b Task 2 fix) that a recovered
        // `RecordOpKind::Rename` context, paired with a route to the table's
        // `OnRename` trigger, is judged APPLICABLE by `route_applicability` — the
        // arm doesn't just populate a struct, it feeds a real, sound route.
        let (apps, app) = make_app();
        let table = make_obj(app, ObjectKind::Table, "Customer", vec![]);
        let table_id = table.id.clone();
        let onrename = make_routine_node(&table_id, "onrename", 0);
        let (graph, index) = build_synth_graph(apps, vec![table], vec![onrename]);

        let caller_rid = RoutineNodeId {
            object: table_id.clone(),
            name_lc: "go".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        let target_rid = RoutineNodeId {
            object: table_id.clone(),
            name_lc: "onrename".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        let site = SiteId {
            caller: caller_rid.clone(),
            span: test_span(1),
            callee_fingerprint: 1,
        };
        let edge = Edge {
            from: caller_rid,
            site: site.clone(),
            kind: EdgeKind::ImplicitTrigger,
            shape: DispatchShape::Multicast,
            completeness: SetCompleteness::Partial {
                reason: OpenWorldReason::ReverseDependentExtensions,
            },
            routes: vec![source_route(target_rid)],
        };
        let mut ctx: HashMap<SiteId, FanOutSiteContext> = HashMap::new();
        ctx.insert(
            site,
            FanOutSiteContext::Trigger(RecordOpCtx {
                kind: RecordOpKind::Rename,
                table: table_id,
                field: None,
                run_trigger: RunTrigger::Guarded,
            }),
        );

        let raw_abi = empty_raw_abi();
        let report = route_applicability(&[edge], &raw_abi, &graph, &index, &ctx, &[]);
        assert_eq!(
            report.implicit_trigger_violations, 0,
            "a Rec.Rename() site with an OnRename trigger on the table must be \
             APPLICABLE (no violation)"
        );
    }

    // -----------------------------------------------------------------------
    // EventFlow
    // -----------------------------------------------------------------------

    fn make_event_unit(src: &'static str) -> (AppId, ParsedUnit) {
        let app_id = AppId {
            guid: String::new(),
            name: "EvApp".into(),
            publisher: "T".into(),
            version: "1.0.0.0".into(),
        };
        let provenance = Provenance {
            app: app_id.clone(),
            tier: TrustTier::Workspace,
            content_hash: String::new(),
        };
        let unit = ParsedUnit {
            app: app_id.clone(),
            files: vec![ParsedFile {
                virtual_path: "Ev.al".into(),
                file: std::sync::Arc::new(al_syntax::parse(src)),
                provenance,
                text: src.into(),
            }],
        };
        (app_id, unit)
    }

    fn build_event_graph(app_id: &AppId, unit: &ParsedUnit) -> (ProgramGraph, ResolveIndex) {
        let mut apps = AppRegistry::default();
        let app_ref = apps.intern(app_id);
        let mut objects: Vec<ObjectNode> = Vec::new();
        let mut routines: Vec<RoutineNode> = Vec::new();
        for pf in &unit.files {
            extract_nodes(
                app_ref,
                &pf.file,
                pf.provenance.tier,
                &mut objects,
                &mut routines,
            );
        }
        objects.sort_by(|a, b| a.id.cmp(&b.id));
        routines.sort_by(|a, b| a.id.cmp(&b.id));
        let obj_index = ObjectIndex::build(&objects);
        let graph = ProgramGraph {
            apps,
            topology: DependencyGraph::default(),
            objects,
            routines,
            obj_index,
            ..Default::default()
        };
        let index = ResolveIndex::build(&graph);
        (graph, index)
    }

    fn find_obj_id(graph: &ProgramGraph, name_lc: &str) -> ObjectNodeId {
        graph
            .objects
            .iter()
            .find(|o| o.name.eq_ignore_ascii_case(name_lc))
            .unwrap_or_else(|| panic!("object {name_lc} not found"))
            .id
            .clone()
    }

    #[test]
    fn event_route_passes_when_subscriber_attr_names_publisher() {
        let src: &'static str = r#"
codeunit 50800 "EvPub"
{
    [IntegrationEvent(false, false)]
    procedure OnFoo()
    begin
    end;
}

codeunit 50801 "EvSub"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"EvPub", 'OnFoo', '', false, false)]
    local procedure Handle()
    begin
    end;
}
"#;
        let (app_id, unit) = make_event_unit(src);
        let (graph, index) = build_event_graph(&app_id, &unit);
        let pub_id = find_obj_id(&graph, "EvPub");
        let sub_id = find_obj_id(&graph, "EvSub");

        let pub_rid = RoutineNodeId {
            object: pub_id,
            name_lc: "onfoo".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        let sub_rid = RoutineNodeId {
            object: sub_id,
            name_lc: "handle".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        let edge = Edge {
            from: pub_rid.clone(),
            site: SiteId {
                caller: pub_rid,
                span: test_span(1),
                callee_fingerprint: 1,
            },
            kind: EdgeKind::EventFlow,
            shape: DispatchShape::Multicast,
            completeness: SetCompleteness::Partial {
                reason: OpenWorldReason::ReverseDependentSubscribers,
            },
            routes: vec![source_route(sub_rid)],
        };

        let raw_abi = empty_raw_abi();
        let units = [unit];
        let report =
            route_applicability(&[edge], &raw_abi, &graph, &index, &HashMap::new(), &units);
        assert_eq!(report.event_violations, 0);
    }

    #[test]
    fn event_route_violation_when_subscriber_attr_names_a_different_publisher() {
        // Fabricated: the Routine route claims EvSub2 subscribes to EvPub2.OnBar, but
        // EvSub2's raw [EventSubscriber] attribute actually names a different
        // publisher+event entirely.
        let src: &'static str = r#"
codeunit 50802 "EvPub2"
{
    [IntegrationEvent(false, false)]
    procedure OnBar()
    begin
    end;
}

codeunit 50803 "OtherPub2"
{
    [IntegrationEvent(false, false)]
    procedure OnOther()
    begin
    end;
}

codeunit 50804 "EvSub2"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"OtherPub2", 'OnOther', '', false, false)]
    local procedure Handle()
    begin
    end;
}
"#;
        let (app_id, unit) = make_event_unit(src);
        let (graph, index) = build_event_graph(&app_id, &unit);
        let pub_id = find_obj_id(&graph, "EvPub2");
        let sub_id = find_obj_id(&graph, "EvSub2");

        let pub_rid = RoutineNodeId {
            object: pub_id,
            name_lc: "onbar".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        let sub_rid = RoutineNodeId {
            object: sub_id,
            name_lc: "handle".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        let edge = Edge {
            from: pub_rid.clone(),
            site: SiteId {
                caller: pub_rid,
                span: test_span(1),
                callee_fingerprint: 1,
            },
            kind: EdgeKind::EventFlow,
            shape: DispatchShape::Multicast,
            completeness: SetCompleteness::Partial {
                reason: OpenWorldReason::ReverseDependentSubscribers,
            },
            routes: vec![source_route(sub_rid)],
        };

        let raw_abi = empty_raw_abi();
        let units = [unit];
        let report =
            route_applicability(&[edge], &raw_abi, &graph, &index, &HashMap::new(), &units);
        assert_eq!(
            report.event_violations, 1,
            "EvSub2's raw [EventSubscriber] attr names OtherPub2.OnOther, not \
             EvPub2.OnBar — the ported teeth must catch the mismatch"
        );
    }

    // ── Task 1: conditional IncludeSender +1 arity tolerance ────────────────

    /// (a) POSITIVE: `IncludeSender=true`, 0-arity publisher + 1-arity
    /// Sender-capturing subscriber → the route is applicable, `event_violations
    /// == 0`. Pre-fix (strict `sub_rid.params_count <= publisher_params_count`
    /// bound), this route was flagged a violation even though it is exactly the
    /// wiring's own Sender-tolerant `+1` population (`ae35e90`).
    #[test]
    fn event_route_passes_with_include_sender_true_and_arity_one_subscriber() {
        let src: &'static str = r#"
codeunit 50805 "EvPub3"
{
    [IntegrationEvent(true, false)]
    procedure OnBaz()
    begin
    end;
}

codeunit 50806 "EvSub3"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"EvPub3", 'OnBaz', '', false, false)]
    local procedure Handle(Sender: Codeunit "EvPub3")
    begin
    end;
}
"#;
        let (app_id, unit) = make_event_unit(src);
        let (graph, index) = build_event_graph(&app_id, &unit);
        let pub_id = find_obj_id(&graph, "EvPub3");
        let sub_id = find_obj_id(&graph, "EvSub3");

        let pub_rid = RoutineNodeId {
            object: pub_id,
            name_lc: "onbaz".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        let sub_rid = RoutineNodeId {
            object: sub_id,
            name_lc: "handle".into(),
            enclosing_member_lc: None,
            params_count: 1,
            sig_fp: 0,
        };
        let edge = Edge {
            from: pub_rid.clone(),
            site: SiteId {
                caller: pub_rid,
                span: test_span(1),
                callee_fingerprint: 1,
            },
            kind: EdgeKind::EventFlow,
            shape: DispatchShape::Multicast,
            completeness: SetCompleteness::Partial {
                reason: OpenWorldReason::ReverseDependentSubscribers,
            },
            routes: vec![source_route(sub_rid)],
        };

        let raw_abi = empty_raw_abi();
        let units = [unit];
        let report =
            route_applicability(&[edge], &raw_abi, &graph, &index, &HashMap::new(), &units);
        assert_eq!(
            report.event_violations, 0,
            "EvPub3 declares IncludeSender=true, so EvSub3's arity-1 Handle \
             (capturing the implicit Sender) must be applicable"
        );
    }

    /// (b) NEGATIVE: `IncludeSender=false`, 0-arity publisher + 1-arity
    /// subscriber whose raw attribute genuinely names the right publisher+event
    /// → the conditional bound must REJECT it (`event_violations == 1`) — proves
    /// the fix is CONDITIONAL, not a blanket re-admission of any `+1` arity.
    #[test]
    fn event_route_violation_when_include_sender_false_and_arity_one_subscriber() {
        let src: &'static str = r#"
codeunit 50807 "EvPub4"
{
    [IntegrationEvent(false, false)]
    procedure OnQux()
    begin
    end;
}

codeunit 50808 "EvSub4"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"EvPub4", 'OnQux', '', false, false)]
    local procedure Handle(NotASender: Codeunit "EvPub4")
    begin
    end;
}
"#;
        let (app_id, unit) = make_event_unit(src);
        let (graph, index) = build_event_graph(&app_id, &unit);
        let pub_id = find_obj_id(&graph, "EvPub4");
        let sub_id = find_obj_id(&graph, "EvSub4");

        let pub_rid = RoutineNodeId {
            object: pub_id,
            name_lc: "onqux".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        let sub_rid = RoutineNodeId {
            object: sub_id,
            name_lc: "handle".into(),
            enclosing_member_lc: None,
            params_count: 1,
            sig_fp: 0,
        };
        let edge = Edge {
            from: pub_rid.clone(),
            site: SiteId {
                caller: pub_rid,
                span: test_span(1),
                callee_fingerprint: 1,
            },
            kind: EdgeKind::EventFlow,
            shape: DispatchShape::Multicast,
            completeness: SetCompleteness::Partial {
                reason: OpenWorldReason::ReverseDependentSubscribers,
            },
            routes: vec![source_route(sub_rid)],
        };

        let raw_abi = empty_raw_abi();
        let units = [unit];
        let report =
            route_applicability(&[edge], &raw_abi, &graph, &index, &HashMap::new(), &units);
        assert_eq!(
            report.event_violations, 1,
            "EvPub4 declares IncludeSender=false — an arity-1 subscriber must NOT \
             be tolerated; the +1 Sender bound is conditional, never blanket"
        );
    }

    // -----------------------------------------------------------------------
    // Task 3 (sigfp-and-ambiguous-reclassification plan): `route_applicability`
    // must fall through to `_ => {}` for `DispatchShape::AmbiguousOverload` —
    // verified green here, not assumed (the plan's explicit instruction).
    // -----------------------------------------------------------------------

    fn ambiguous_dispatch_route(target: RoutineNodeId) -> Route {
        Route {
            target: RouteTarget::Routine(target),
            evidence: Evidence::Source,
            conditions: vec![crate::program::resolve::edge::Condition::AmbiguousDispatch],
            witness: Witness::SourceSpan {
                file: "f.al".into(),
                span: (0, 1),
            },
            receiver_tier: None,
        }
    }

    #[test]
    fn ambiguous_overload_edge_falls_through_route_applicability_cleanly() {
        let (apps, app) = make_app();
        let caller_obj = make_obj(app, ObjectKind::Codeunit, "Caller", vec![]);
        let caller_id = caller_obj.id.clone();
        let callee_obj = make_obj(app, ObjectKind::Codeunit, "Callee", vec![]);
        let callee_id = callee_obj.id.clone();
        let overload_a = make_routine_node(&callee_id, "bar", 0);
        let overload_a_id = overload_a.id.clone();
        let mut overload_b = make_routine_node(&callee_id, "bar", 0);
        // Distinguish the second candidate (real overloads differ by sig_fp; the
        // exact discriminator is irrelevant to this applicability-fallthrough test).
        overload_b.id.params_count = 1;
        let overload_b_id = overload_b.id.clone();

        let (graph, index) = build_synth_graph(
            apps,
            vec![caller_obj, callee_obj],
            vec![overload_a, overload_b],
        );

        let caller_rid = RoutineNodeId {
            object: caller_id,
            name_lc: "go".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        let edge = Edge {
            from: caller_rid.clone(),
            site: SiteId {
                caller: caller_rid,
                span: test_span(1),
                callee_fingerprint: 1,
            },
            kind: EdgeKind::Call,
            shape: DispatchShape::AmbiguousOverload,
            completeness: SetCompleteness::Complete,
            routes: vec![
                ambiguous_dispatch_route(overload_a_id),
                ambiguous_dispatch_route(overload_b_id),
            ],
        };

        let raw_abi = empty_raw_abi();
        let report = route_applicability(&[edge], &raw_abi, &graph, &index, &HashMap::new(), &[]);

        // Route-level: both Source/SourceSpan routes pass the witness contract
        // regardless of the AmbiguousDispatch condition (witness_contract_holds
        // is per-route, unaffected by cardinality/shape).
        assert_eq!(report.total_routes, 2);
        assert_eq!(report.witness_contract_violations, 0);

        // None of the shape-specific SOUNDNESS checks must fire — a `Call` edge
        // with `AmbiguousOverload` shape matches none of `EdgeKind::Call if
        // shape==Polymorphic` / `EdgeKind::ImplicitTrigger if shape==Multicast` /
        // `EdgeKind::EventFlow`, so it structurally falls through to `_ => {}`.
        assert_eq!(report.interface_applicability_violations, 0);
        assert_eq!(report.implicit_trigger_violations, 0);
        assert_eq!(report.instance_builtin_violations, 0);
        assert_eq!(report.event_violations, 0);

        // NON-VACUITY: none of the shape-specific checks RAN either (proves the
        // zero-violations above is a genuine fallthrough, not a vacuous pass).
        assert_eq!(report.interface_routes_checked, 0);
        assert_eq!(report.implicit_trigger_routes_checked, 0);
        assert_eq!(report.instance_builtin_routes_checked, 0);
        assert_eq!(report.event_routes_checked, 0);

        assert!(report.is_clean());
    }
}
