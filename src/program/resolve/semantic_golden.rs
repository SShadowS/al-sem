//! Semantic edge goldens, the route-applicability contract, and the shared
//! mint and drift plumbing.
//!
//! The CDO semantic-edges golden is now minted from the AL compiler's call
//! graph (`compiler_golden.rs`, engine-switch S9.0d). The L3-minted
//! anonymized goldens (`cdo-anon.json`, `cdo-trigger-anon.json`,
//! `cdo-event-anon.json`) and their adjudication overlay were removed.
//!
//! # Fresh-minted golden floor
//!
//! [`mint_fresh_golden_for_kind`] freezes the fresh resolver's own output for
//! one [`EdgeKind`] into a [`SemanticGolden`]: a sorted list keyed by the
//! column-ignoring [`GoldenSiteKey`]. The in-repo implicit-trigger fixture
//! golden is minted this way. [`build_golden_from_canonical`] builds one from
//! any batch of canonical edges.
//!
//! [`assert_against_semantic_golden`] compares a fresh canonical edge batch
//! against a golden and sorts every site into: `match`, `fresh_wrong`,
//! `fresh_missing`, `fresh_extra`, `fresh_novel`, or `golden_missing`. The
//! key invariant is **`fresh_wrong.is_empty()`**: fresh must never
//! confidently pick a target the golden says is wrong. A per-site histogram
//! cannot catch this, because it counts outcomes but cannot say WHICH target
//! was chosen.
//!
//! # Route-applicability contract
//!
//! [`route_applicability`] checks the witness/evidence contract on every
//! route, re-checks each fan-out route (interface, implicit trigger, instance
//! builtin, event subscriber) against its call site, and delegates the ABI
//! ingestion check to [`abi_ingestion_integrity`]. It certifies that every
//! route the resolver emitted is justified. It does not check that the
//! resolver emitted every route it should.
//!
//! # Shared mint and drift plumbing
//!
//! `compiler_golden.rs` and the dev-mint tool (`src/bin/mint-goldens.rs`)
//! reuse these: [`MintMetadata`] (the mint-time stamp), [`workspace_git_info`]
//! and [`dependency_closure_digest`] (what the stamp records),
//! [`workspace_drift`] and [`DriftHandler`] (report a moved workspace to the
//! caller, who decides what it means), and [`cdo_deanon_map_path`] /
//! [`merge_deanon_map`] (the gitignored local de-anonymization map).

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::program::graph::ProgramGraph;
use al_syntax::IdentifierFoldExt;

use crate::program::node::{AppRef, ObjKey, ObjectKind, ObjectNodeId};
use crate::program::node_extract::{AbiParams, ObjectNode};
use crate::program::resolve::abi_check::{
    RawAbiIndex, abi_ingestion_integrity, build_raw_abi_index_from_snapshot,
};
use crate::program::resolve::applicability::{
    RecordOpCtx, RecordOpKind, RunTrigger, implicit_trigger_route_applicable,
    instance_builtin_route_applicable, interface_route_applicable,
};
use crate::program::resolve::differential::{
    CanonicalEdge, CanonicalTarget, project_fresh, verify_event_subscriber_route,
    witness_contract_holds,
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
// Mint provenance and drift
// ---------------------------------------------------------------------------

/// Mint-time provenance metadata stamped into every committed golden (1B.3b
/// Task 1 fix, Fix 4): the CDO workspace's git HEAD SHA and dirty state at
/// mint time, captured by [`workspace_git_info`]. Audit time re-probes the
/// CURRENT workspace and, on a mismatch, hands the drift message to the
/// [`DriftHandler`] its caller supplied -- it does not decide what a mismatch
/// means. The gated-test handler (`drift_handler` in
/// `tests/program_resolve_harness.rs`) warns ungated and fails under
/// `ENFORCE_CDO_WS=1`. `#[serde(default)]` on both fields so a golden minted
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
    /// already lost a baseline that looked pinned and was not. Covers every
    /// `.alpackages` the resolver reads, ancestors included, and is tagged with
    /// its scheme (`closure-v2:<hex>`, or `closure-v2:empty` for no packages).
    /// `None` only in a golden minted before this field existed, which the drift
    /// check reports (#29).
    #[serde(default)]
    pub dependency_closure_sha256: Option<String>,
}

/// The tag of the current closure-digest scheme. A stamp from another scheme
/// can never equal a current digest, so a scheme change reads as drift until
/// the goldens are re-stamped (`mint-goldens --restamp`).
pub const CLOSURE_SCHEME: &str = "closure-v2";

/// SHA-256 over the dependency symbol closure the resolver ACTUALLY loads (#29):
/// every `.app` file [`crate::dependencies::discover_app_files`] finds, in every
/// scanned `.alpackages` (the workspace's own AND each ancestor's, up to the git
/// boundary), keyed by its path relative to `workspace_root` with `/`
/// separators (`.alpackages/x.app`, `../.alpackages/y.app`), sorted by that key.
/// Each key and each file's bytes are length-prefixed (u64 LE), so no two
/// closures frame identically.
///
/// `Ok("closure-v2:empty")` when no `.app` is found -- an explicitly tagged empty
/// closure, never `None`. `Err` when a cache folder cannot be listed or a file
/// cannot be read: a probe failure is not an empty closure.
pub fn dependency_closure_digest(workspace_root: &Path) -> Result<String, String> {
    let (files, unreadable) = crate::dependencies::discover_app_files(workspace_root);
    if let Some((folder, e)) = unreadable.first() {
        return Err(format!("cannot list {}: {e}", folder.display()));
    }
    let mut keyed: Vec<(String, PathBuf)> = Vec::with_capacity(files.len());
    for f in files {
        // How many levels above `workspace_root` the cache sits (0 = its own).
        let owner = f.folder.parent().unwrap_or(&f.folder);
        let ups = workspace_root
            .ancestors()
            .position(|a| a == owner)
            .ok_or_else(|| format!("{} is not an ancestor cache", f.folder.display()))?;
        let name = f
            .path
            .file_name()
            .ok_or_else(|| format!("no file name: {}", f.path.display()))?
            .to_string_lossy();
        keyed.push((format!("{}.alpackages/{name}", "../".repeat(ups)), f.path));
    }
    if keyed.is_empty() {
        return Ok(format!("{CLOSURE_SCHEME}:empty"));
    }
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = Sha256::new();
    for (key, path) in &keyed {
        let bytes =
            std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        hasher.update((key.len() as u64).to_le_bytes());
        hasher.update(key.as_bytes());
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    Ok(format!("{CLOSURE_SCHEME}:{:x}", hasher.finalize()))
}

/// Probe `workspace_root`'s git HEAD SHA + dirty state via the `git` CLI
/// (1B.3b Task 1 fix, Fix 4). Best-effort: returns `(None, None)` fields when
/// `workspace_root` isn't inside a git repo, `git` isn't on `PATH`, or either
/// command fails — this is provenance metadata, not a hard requirement. Used
/// by the dev-mint tool (to STAMP [`MintMetadata`] at mint time) and by
/// [`workspace_drift`] (to compare the CURRENT workspace against a loaded
/// golden's stamp).
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

/// What to do when the current workspace has drifted from a golden's mint
/// stamp. The library calls this and nothing else on drift: it neither reads
/// process state to choose a handler nor expresses failure itself, so a
/// handler that panics does so as the CALLER's choice, in the caller's code.
///
/// The gated-test handler is `drift_handler` in
/// `tests/program_resolve_harness.rs`, beside that file's own `ENFORCE_CDO_WS`
/// read (`enforce_audit_ran`) and the audit call sites it serves. It is not in
/// `tests/common/cdo.rs` next to `cdo_ws_or_enforce`, where it would sit more
/// naturally, because that file is `#[path]`-included verbatim by three test
/// binaries and only one of them runs these audits.
pub type DriftHandler = fn(&str);

/// Returns the drift message when the CURRENT `workspace_root`'s git SHA,
/// dirty state or `.alpackages` closure differs from `stamped` (the golden's
/// mint-time stamp), and `None` when they match.
///
/// Decides NOTHING about what to do with it -- no `std::env`, no `assert!`,
/// no stderr. (It still shells out to git and reads `.alpackages`, so it is
/// policy-free rather than pure.) Drift means an audit diff may reflect a moved
/// workspace rather than a resolver regression; see [`MintMetadata`]'s doc.
pub(crate) fn workspace_drift(stamped: &MintMetadata, workspace_root: &Path) -> Option<String> {
    let (current_sha, current_dirty) = workspace_git_info(workspace_root);
    let current_closure = dependency_closure_digest(workspace_root);
    // #29: every committed golden carries a stamp now, so a MISSING stamp is
    // itself drift (the caller's handler decides; under ENFORCE_CDO_WS it
    // fails), and so is a probe failure. A stamp from an older digest scheme
    // never equals a current digest and reads as drift until re-stamped.
    let closure_drifted = match (&stamped.dependency_closure_sha256, &current_closure) {
        (Some(st), Ok(cur)) => st != cur,
        _ => true,
    };
    let git_drifted =
        current_sha != stamped.workspace_git_sha || current_dirty != stamped.workspace_dirty;
    if !git_drifted && !closure_drifted {
        return None;
    }

    Some(format!(
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
        current_closure,
    ))
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
// Local de-anonymization map
// ---------------------------------------------------------------------------

/// Path to the GITIGNORED local de-anonymization map
/// (`AnonId.0 -> human-readable plaintext`). NEVER committed — see `anon.rs`'s
/// module docs.
#[must_use]
pub fn cdo_deanon_map_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/goldens/semantic-edges/cdo-deanon-map.json")
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
/// should have" (that question is answered by the committed semantic-edge
/// goldens, including the compiler-minted CDO golden in `compiler_golden.rs`,
/// which carry target-set completeness for what they cover). A clean
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
// Public API
// ---------------------------------------------------------------------------

/// Build a [`SemanticGolden`] from a batch of canonical edges. Oracle-
/// agnostic — the caller decides where `edges` came from. Used by
/// [`mint_fresh_golden_for_kind`].
pub fn build_golden_from_canonical(edges: &[CanonicalEdge]) -> SemanticGolden {
    let mut map: BTreeMap<GoldenSiteKey, BTreeSet<GoldenTarget>> = BTreeMap::new();
    for edge in edges {
        let key = canonical_to_golden_key(edge);
        let targets = canonical_targets_to_golden(&edge.targets);
        map.entry(key).or_default().extend(targets);
    }
    SemanticGolden::from_map(map)
}

/// L3-INDEPENDENT: mint a [`SemanticGolden`] from the FRESH resolver's OWN
/// output, filtered to one [`EdgeKind`]. Used to freeze fresh's own
/// resolution as a committed regression baseline for dispatch kinds a small
/// synthetic fixture exercises end-to-end without L3 at all (the
/// ImplicitTrigger target-set fixture — 1B.3b Task 1 Step 4). NOT used for
/// the CDO golden, which comes from the AL compiler's call graph (see
/// `compiler_golden.rs`).
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

/// Compare a fresh canonical edge batch against a [`SemanticGolden`].
///
/// Returns a [`SemanticDiff`] classifying every site. (The diff's `l3_*`
/// field names date from when goldens were L3-minted; they mean "the
/// golden's side".)
///
/// **The critical invariant is `fresh_wrong.is_empty()`** — fresh must never
/// confidently emit a target the golden says is wrong. `fresh_missing` tracks
/// sites the golden resolved but fresh did not (a progress gap — reduce it,
/// never introduce new ones).
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
/// (flagging a genuinely sound route as a false positive). S9.0c: the same
/// for `ModifyAll`/`DeleteAll`, which fire `OnModify`/`OnDelete` per row.
fn record_op_kind_for_method(op_lc: &str) -> Option<RecordOpKind> {
    match op_lc {
        "insert" => Some(RecordOpKind::Insert),
        "modify" | "modifyall" => Some(RecordOpKind::Modify),
        "delete" | "deleteall" => Some(RecordOpKind::Delete),
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
/// (workspace AND dependency units — a subscriber may live in either)
/// backs [`verify_event_subscriber_route`]'s independent raw-IR re-read.
#[must_use]
pub fn route_applicability(
    edges: &[Edge],
    raw_abi: &RawAbiIndex,
    graph: &ProgramGraph,
    index: &ResolveIndex,
    fan_out_ctx: &HashMap<SiteId, FanOutSiteContext>,
    parsed: &[&ParsedUnit],
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
                                pub_obj.declared_id,
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
        &ctx.all_units(),
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
            query_columns: Vec::new(),
            protected_vars: Vec::new(),
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
            preproc_context: Box::default(),
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
            objects: sorted_objects.into(),
            routines: routines.into(),
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
            objects: objects.into(),
            routines: routines.into(),
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
        let units = [&unit];
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
        let units = [&unit];
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
        let units = [&unit];
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
        let units = [&unit];
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

    // ── issue #30: the drift-enforcement boundary ───────────────────────────
    //
    // The defect: `warn_on_workspace_drift` read `ENFORCE_CDO_WS` from inside
    // the library and expressed failure with `assert!`.
    //
    // These tests need NO environment variable, and deliberately do not touch
    // one: `std::env::set_var` is `unsafe` because it races every other thread
    // in the process.

    /// #29: the closure digest pins what the resolver LOADS. An `.app` dropped
    /// into an ANCESTOR cache (inside the git boundary) changes it -- the swap
    /// the old workspace-only digest missed -- while a non-`.app` file and a
    /// cache above the boundary do not.
    #[test]
    fn issue29_closure_digest_covers_ancestor_caches_and_only_apps() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        let ws = repo.join("ws");
        std::fs::create_dir_all(ws.join(".alpackages")).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap(); // the boundary
        std::fs::write(ws.join(".alpackages").join("Dep_1.0.0.0.app"), b"v1").unwrap();
        let base = dependency_closure_digest(&ws).expect("digest");
        assert!(base.starts_with(&format!("{CLOSURE_SCHEME}:")), "{base}");

        // Noise the loader ignores must not move the digest.
        std::fs::write(ws.join(".alpackages").join("notes.txt"), b"x").unwrap();
        std::fs::create_dir_all(tmp.path().join(".alpackages")).unwrap(); // above .git
        std::fs::write(tmp.path().join(".alpackages").join("Out_9.app"), b"x").unwrap();
        assert_eq!(dependency_closure_digest(&ws).as_deref(), Ok(base.as_str()));

        // The ancestor swap: a same-name higher-version package one level up.
        std::fs::create_dir_all(repo.join(".alpackages")).unwrap();
        std::fs::write(repo.join(".alpackages").join("Dep_2.0.0.0.app"), b"v2").unwrap();
        let swapped = dependency_closure_digest(&ws).expect("digest");
        assert_ne!(swapped, base, "an ancestor-cache .app must move the digest");

        // A stamp minted before the swap is now drift.
        let stamped = MintMetadata {
            dependency_closure_sha256: Some(base),
            ..MintMetadata::default()
        };
        assert!(workspace_drift(&stamped, &ws).is_some());
    }

    /// A1 + A6: the helper REPORTS drift and decides nothing -- a value either
    /// way, never a panic and never a print.
    #[test]
    fn issue30_workspace_drift_reports_without_deciding() {
        let tmp = tempfile::tempdir().expect("tempdir");

        // Hand-stated precondition: a bare temp directory is not a git checkout
        // and has no `.alpackages`, so its CURRENT probe is (None, None) and an
        // explicitly tagged EMPTY closure (#29: never `None`).
        assert_eq!(
            workspace_git_info(tmp.path()),
            (None, None),
            "precondition: the temp dir must not be a git checkout"
        );
        let empty = format!("{CLOSURE_SCHEME}:empty");
        assert_eq!(
            dependency_closure_digest(tmp.path()).as_deref(),
            Ok(empty.as_str())
        );

        // A6: a stamp EXACTLY equal to that probe is not drift.
        let matching = MintMetadata {
            dependency_closure_sha256: Some(empty.clone()),
            ..MintMetadata::default()
        };
        assert_eq!(
            workspace_drift(&matching, tmp.path()),
            None,
            "a matching stamp is not drift"
        );
        // #29: a golden with NO closure stamp is drift, not a silent pass.
        assert!(
            workspace_drift(&MintMetadata::default(), tmp.path()).is_some(),
            "a missing closure stamp must be reported"
        );

        // A1: a stamp naming a SHA differs, so this is drift -- and the call
        // returns it rather than acting on it.
        let stamped = MintMetadata {
            workspace_git_sha: Some("bc3ccb18".to_string()),
            workspace_dirty: Some(false),
            dependency_closure_sha256: Some(empty),
        };
        let msg = workspace_drift(&stamped, tmp.path()).expect("a stamped SHA is drift here");
        assert!(
            msg.contains("CDO workspace drifted from the golden mint stamp."),
            "drift message text changed: {msg}"
        );
        assert!(
            msg.contains("bc3ccb18"),
            "the message must carry the stamped state: {msg}"
        );
    }
}
