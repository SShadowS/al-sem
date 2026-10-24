//! cli-b/b3 — FINGERPRINT QUERY + projection + human renderer.
//!
//! Ports:
//!   - `src/cli/fingerprint-query.ts`       → `fingerprint_query` + `FingerprintFilters`
//!   - `src/contracts/fingerprint-query.ts` → `project_fingerprint_query`
//!   - `src/cli/format-fingerprint.ts`      → `format_fingerprint_human`
//!
//! REUSES B1's `build_fingerprint_indexes` + `reconstruct_witness_paths` +
//! `project_path` machinery from `digest.rs` (shared, NOT duplicated).
//!
//! ## Format dispatch contract (fingerprint.ts:207-298)
//!
//! - `--format cbor` / `--format cbor.gz` / `--shard` / `--format json` (no query flag)
//!   → B0's snapshot serializers — **NOT this module**.
//! - `--format json` WITH query flag (`--witness`, `--roots`, `--routine`,
//!   `--include-inherited`) → `fingerprint_query` → `project_fingerprint_query`
//!   → JSON envelope (kind `fingerprint-query`, schemaVersion `1.3.0`).
//! - `--format human` → `fingerprint_query` → `format_fingerprint_human`.
//!
//! ## Witness modes
//!
//! - `false`   → no reconstruction; facts have **no** `witness` field in JSON.
//! - `0`       → witness objects present but zero paths.
//! - `N`       → cap at N paths (1–256).
//! - `"all"`   → uncapped (HARD_PATH_CAP = 256).

use std::collections::HashMap;

use crate::engine::ids::sha256_hex;
use crate::engine::l4::effect_query_cli::{OwnerIndex, owner_qualified};
use crate::engine::l5::digest::{
    CarrierCache, DagCache, FingerprintIndexesPub, HopCache, HumanHop, ProjectedPath,
    QueryWitnessHop, TerminalHopInfo, WitnessIndex, build_fingerprint_indexes_pub,
    reconstruct_witness_paths_pub, witness_index,
};
use crate::engine::l5::selector_index::{SelectorIndexes, narrow_to_owner, resolve_selector};
use crate::engine::l5::snapshot::{CapabilitySnapshot, SnapCapabilityExtra, SnapValueSource};

// Re-export for use by the CLI module.
pub use self::types::*;

mod types {
    use super::*;

    /// Root-kind filter set (al-sem `RootKind` values).
    pub type RootKindSet = std::collections::BTreeSet<String>;

    /// Witness limit — mirrors `FingerprintFilters.witnessLimit`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum WitnessLimit {
        /// No reconstruction.
        Disabled,
        /// Reconstruct; cap at `n` paths (0 = objects present but empty).
        Capped(usize),
        /// Reconstruct; uncapped (uses HARD_PATH_CAP).
        All,
    }

    impl WitnessLimit {
        /// Parse CLI string: `false` → Disabled, `all` → All, digits → Capped(n).
        pub fn parse(s: &str) -> Option<Self> {
            match s {
                "false" => Some(WitnessLimit::Disabled),
                "all" => Some(WitnessLimit::All),
                _ => s.parse::<usize>().ok().map(WitnessLimit::Capped),
            }
        }
    }

    /// Filters for a fingerprint query (mirrors `FingerprintFilters` in TS).
    pub struct FingerprintFilters {
        /// When Some, only root classifications whose `kinds` intersect this set are rendered.
        pub roots: Option<RootKindSet>,
        /// Routine selectors (display names or StableRoutineIds).
        pub routine_selectors: Vec<String>,
        /// When false, only direct facts are rendered.
        pub include_inherited: bool,
        /// Witness reconstruction limit.
        pub witness_limit: WitnessLimit,
    }

    /// A block witness (one fact + reconstructed paths).
    #[derive(Debug, Clone)]
    pub struct BlockWitness {
        /// Raw fact index in the block's rendered_facts list.
        pub fact_index: usize,
        /// Projected witness paths (empty when limit=0 or disabled).
        pub paths: Vec<ProjectedPath>,
        pub truncated: bool,
        pub incomplete: bool,
        pub diagnostics: Vec<WitnessDiagnostic>,
        /// The witness's terminal evidence (#160 step 9 S9-2, S9-16), computed
        /// while the snapshot is alive: the boundary of the FIRST path that has
        /// one, else the fact's operation evidence (also when `paths` is empty).
        pub evidence: Option<FactEvidence>,
    }

    /// A fact's terminal evidence in its witness object.
    #[derive(Debug, Clone)]
    pub enum FactEvidence {
        /// The fact's own operation (`witness_operation_id`) and its anchor.
        Operation {
            operation_id: String,
            file: String,
            line: u32,
            column: u32,
            excerpt: String,
        },
        /// Rule B: the boundary the first path was cut at.
        Boundary(crate::engine::l5::digest::PathBoundary),
    }

    #[derive(Debug, Clone)]
    pub struct WitnessDiagnostic {
        pub kind: String,
        pub detail: Option<String>,
    }

    /// A coverage summary for a block.
    #[derive(Debug, Clone)]
    pub struct BlockCoverage {
        pub status: String,
        pub direct_status: String,
        pub inherited_status: String,
        pub reasons: Vec<String>,
        pub unknown_targets: Vec<String>,
        pub unknown_target_displays: Vec<String>,
        pub inherited_excluded: bool,
    }

    #[derive(Debug, Clone)]
    pub struct BlockResource {
        pub id: Option<String>,
        pub display: String,
        pub source: Option<SnapValueSource>,
        pub ops: Vec<String>,
        /// Indices into the block's rendered_facts list.
        pub fact_indices: Vec<usize>,
    }

    #[derive(Debug, Clone)]
    pub struct BlockFamily {
        pub kind: String,
        pub cone_coverage: String,
        pub resources: Vec<BlockResource>,
    }

    #[derive(Debug, Clone)]
    pub struct BlockCommit {
        pub presence: &'static str, // "yes" | "no" | "unknown"
        /// Index into rendered_facts of the commit fact, if present.
        pub witness_fact_index: Option<usize>,
    }

    #[derive(Debug, Clone)]
    pub struct DispatchInstance {
        pub object_type: String,
        pub target_id: Option<String>,
        pub target_display: Option<String>,
        pub confidence: String,
        pub provenance: String,
        pub via: String,
        /// Witness callsite id — the stable sort key for unresolved dispatch
        /// instances (fingerprint-query.ts:468). NOT serialized in JSON.
        pub witness_callsite_id: Option<String>,
    }

    #[derive(Debug, Clone)]
    pub struct PermissionLine {
        pub target_kind: &'static str,
        pub target_id: String,
        pub target_display: String,
        pub rights: String,
        pub coverage: String,
    }

    /// A single root-classification block (one rendered entry).
    #[derive(Debug, Clone)]
    pub struct FingerprintBlock {
        pub routine_id: String,
        pub object_id: Option<String>,
        pub object_display: String,
        pub routine_display: String,
        pub kinds: Vec<String>,
        pub classification_source: String,
        pub config_entry_id: Option<String>,
        pub coverage: BlockCoverage,
        pub families: Vec<BlockFamily>,
        pub may_commit: BlockCommit,
        pub dispatch_resolved: Vec<DispatchInstance>,
        pub dispatch_unresolved: Vec<DispatchInstance>,
        pub required_permissions: Vec<PermissionLine>,
        pub witnesses: Vec<BlockWitness>,
        /// The rendered facts (parallel to witnesses).
        pub rendered_facts: Vec<crate::engine::l5::snapshot::SnapshotCapabilityFact>,
    }

    #[derive(Debug, Clone)]
    pub enum FingerprintQueryDiagnostic {
        SelectorUnresolved {
            selector: String,
        },
        SelectorAmbiguous {
            selector: String,
            matched_form: String,
            /// `(stableId, display)` pairs (≤ MAX_AMBIGUOUS_CANDIDATES). The human
            /// renderer prints `  - <display>  (<stableId>)` per candidate
            /// (fingerprint.ts:311-313).
            candidates: Vec<(String, String)>,
        },
    }

    pub struct FingerprintQueryResult {
        pub blocks: Vec<FingerprintBlock>,
        pub diagnostics: Vec<FingerprintQueryDiagnostic>,
        pub total_classifications: usize,
        pub rendered_blocks: usize,
        pub roots_config_ignored: bool,
    }
}

// ---------------------------------------------------------------------------
// Capability resource kind order (mirrors CAPABILITY_RESOURCE_KIND_ORDER in TS)
// ---------------------------------------------------------------------------

const CAPABILITY_RESOURCE_KIND_ORDER: &[&str] = &[
    "table",
    "event",
    "codeunit",
    "page",
    "report",
    "http",
    "telemetry",
    "isolated-storage",
    "file",
    "transaction",
    "ui",
    "background",
];

// ---------------------------------------------------------------------------
// Selector resolution: shared with digest_cli via `selector_index` (see that
// module's docs — this used to be a locally-duplicated copy that rebuilt its
// bucket index from a HashMap, making SelectorAmbiguous.candidates order
// process-random; both call sites now share one deterministic implementation).
// ---------------------------------------------------------------------------

const MAX_AMBIGUOUS_CANDIDATES: usize = 16;

// ---------------------------------------------------------------------------
// Build the per-block data
// ---------------------------------------------------------------------------

type SnapFact = crate::engine::l5::snapshot::SnapshotCapabilityFact;

fn parse_object_id_from_routine(rid: &str) -> Option<String> {
    let hash_at = rid.rfind('#')?;
    if hash_at == 0 {
        return None;
    }
    Some(rid[..hash_at].to_string())
}

fn render_value_source(vs: &SnapValueSource) -> String {
    match vs {
        SnapValueSource::Literal { value } => value.clone(),
        SnapValueSource::Enum { enum_name, member } => match member {
            Some(m) => format!("{enum_name}.{m}"),
            None => enum_name.clone(),
        },
        SnapValueSource::ConstantVar { var_name, .. } => var_name.clone(),
        SnapValueSource::Parameter { var_name, .. } => var_name.clone(),
        SnapValueSource::TableField {
            table_id,
            field_name,
        } => {
            format!("{table_id}.{field_name}")
        }
        SnapValueSource::Expression => "<expression>".to_string(),
        SnapValueSource::Unknown => "<unknown>".to_string(),
    }
}

fn resolve_resource_display(
    fact: &SnapFact,
    stable_id_to_display: &HashMap<String, String>,
) -> (Option<String>, String, Option<SnapValueSource>) {
    if let Some(rid) = &fact.resource_id {
        let display = stable_id_to_display
            .get(rid)
            .cloned()
            .unwrap_or_else(|| rid.clone());
        return (Some(rid.clone()), display, None);
    }
    if let Some(ras) = &fact.resource_arg_source {
        let display = render_value_source(ras);
        return (None, display, Some(ras.clone()));
    }
    (None, "<unknown>".to_string(), None)
}

fn table_op_to_right(op: &str) -> Option<&'static str> {
    match op {
        "read" => Some("R"),
        "insert" => Some("W"),
        "modify" => Some("M"),
        "delete" => Some("D"),
        _ => None,
    }
}

/// A witness's terminal evidence (#160 step 9 S9-2): the boundary of the first
/// of `paths` that has one, in their order; else the fact's own operation
/// evidence from the snapshot's operation index (`None` when the fact names no
/// operation the index holds).
fn fact_evidence(
    fact: &SnapFact,
    paths: &[ProjectedPath],
    idx: &FingerprintIndexesPub<'_>,
) -> Option<FactEvidence> {
    if let Some(b) = paths.iter().find_map(|p| p.boundary.as_ref()) {
        return Some(FactEvidence::Boundary(b.clone()));
    }
    let op_id = fact.witness_operation_id.as_ref()?;
    let ev = idx.operation_by_id.get(op_id.as_str())?;
    Some(FactEvidence::Operation {
        operation_id: op_id.clone(),
        file: normalize_anchor_path(&ev.source_file),
        line: ev.start_line,
        column: ev.start_column,
        excerpt: ev.display_text.clone(),
    })
}

/// The stable ids of the model's `Role::Primary` routines: the query's
/// `is_primary` (#160 step 9, Decision 28). A stable id any primary row
/// carries is primary.
pub(crate) fn primary_stable_ids(
    model: &crate::program::model::workspace::Model,
) -> std::collections::HashSet<&str> {
    model
        .workspace
        .routines
        .iter()
        .filter(|r| model.routine_role(r) == crate::program::model::role::Role::Primary)
        .map(|r| r.stable_routine_id.as_str())
        .collect()
}

fn build_block<'w>(
    rid: &str,
    root: &crate::engine::l5::snapshot::SnapshotRootClassificationSlot,
    idx: &FingerprintIndexesPub<'_>,
    widx: &'w WitnessIndex<'_>,
    carriers: &CarrierCache<'w>,
    filters: &FingerprintFilters,
    owners: &OwnerIndex<'_>,
) -> FingerprintBlock {
    let routine_display = idx
        .routine_display_by_id
        .get(rid)
        .cloned()
        .unwrap_or_else(|| rid.to_string());
    // The owner comes from the model index, never the identity table (#78).
    let object_display = owners.object_display(rid);
    let object_id = parse_object_id_from_routine(rid);

    // Coverage record.
    let cov_rec = idx.coverage_by_routine.get(rid);

    // Rendered facts: all or direct-only.
    let all_facts: &[SnapFact] = idx
        .facts_by_routine
        .get(rid)
        .map(|v| v.as_slice())
        .unwrap_or(&[]);
    let rendered_facts: Vec<SnapFact> = if filters.include_inherited {
        all_facts.to_vec()
    } else {
        all_facts
            .iter()
            .filter(|f| f.provenance == "direct")
            .cloned()
            .collect()
    };

    // Coverage.
    let (cov_status, direct_status, inherited_status, reasons, unknown_targets) = {
        match cov_rec {
            Some(c) => {
                let status = if filters.include_inherited {
                    c.inherited_status.clone()
                } else {
                    c.direct_status.clone()
                };
                (
                    status,
                    c.direct_status.clone(),
                    c.inherited_status.clone(),
                    c.reasons.clone(),
                    c.unknown_targets.clone(),
                )
            }
            None => (
                "unknown".to_string(),
                "unknown".to_string(),
                "unknown".to_string(),
                Vec::new(),
                Vec::new(),
            ),
        }
    };
    let unknown_target_displays: Vec<String> = unknown_targets
        .iter()
        .map(|t| {
            idx.stable_id_to_display
                .get(t)
                .cloned()
                .unwrap_or_else(|| t.clone())
        })
        .collect();
    let coverage = BlockCoverage {
        status: cov_status.clone(),
        direct_status,
        inherited_status,
        reasons: {
            let mut r = reasons;
            r.sort();
            r
        },
        unknown_targets,
        unknown_target_displays,
        inherited_excluded: !filters.include_inherited,
    };

    // Families (non-dispatch facts, grouped by resource kind).
    let mut by_kind: HashMap<&str, Vec<(usize, &SnapFact)>> = HashMap::new();
    for (fi, f) in rendered_facts.iter().enumerate() {
        // Dispatch facts go into dispatch summary, not a family.
        if f.op == "execute" && matches!(&f.extra, Some(SnapCapabilityExtra::Dispatch { .. })) {
            continue;
        }
        by_kind.entry(&f.resource_kind).or_default().push((fi, f));
    }

    let mut families: Vec<BlockFamily> = Vec::new();
    for &kind in CAPABILITY_RESOURCE_KIND_ORDER {
        let facts_for_kind = match by_kind.get(kind) {
            Some(v) if !v.is_empty() => v,
            _ => continue,
        };
        // Group by resource key (id or display).
        let mut res_by_key: Vec<(String, BlockResource)> = Vec::new();
        let mut key_pos: HashMap<String, usize> = HashMap::new();
        for (fi, f) in facts_for_kind {
            let (id, display, source) = resolve_resource_display(f, &idx.stable_id_to_display);
            let key = id.clone().unwrap_or_else(|| display.clone());
            if let Some(&pos) = key_pos.get(&key) {
                let r = &mut res_by_key[pos].1;
                if !r.ops.contains(&f.op) {
                    r.ops.push(f.op.clone());
                }
                r.fact_indices.push(*fi);
            } else {
                let pos = res_by_key.len();
                key_pos.insert(key.clone(), pos);
                res_by_key.push((
                    key,
                    BlockResource {
                        id: id.clone(),
                        display: display.clone(),
                        source,
                        ops: vec![f.op.clone()],
                        fact_indices: vec![*fi],
                    },
                ));
            }
        }
        // Sort resources by display, then id.
        res_by_key.sort_by(|(_, a), (_, b)| {
            let cmp = a.display.cmp(&b.display);
            if cmp != std::cmp::Ordering::Equal {
                return cmp;
            }
            match (&a.id, &b.id) {
                (Some(ai), Some(bi)) => ai.cmp(bi),
                _ => std::cmp::Ordering::Equal,
            }
        });
        let resources: Vec<BlockResource> = res_by_key.into_iter().map(|(_, r)| r).collect();
        families.push(BlockFamily {
            kind: kind.to_string(),
            cone_coverage: cov_status.clone(),
            resources,
        });
    }

    // mayCommit. `no` needs complete coverage over the facts the query renders
    // (#160 step 9 S9-3): the inherited coverage when inherited facts are shown.
    let commit_fi = rendered_facts.iter().position(|f| f.op == "commit");
    let commit_coverage = if filters.include_inherited {
        &coverage.inherited_status
    } else {
        &coverage.direct_status
    };
    let may_commit = BlockCommit {
        presence: if commit_fi.is_some() {
            "yes"
        } else if commit_coverage == "complete" {
            "no"
        } else {
            "unknown"
        },
        witness_fact_index: commit_fi,
    };

    // Dispatch.
    let mut dispatch_resolved: Vec<DispatchInstance> = Vec::new();
    let mut dispatch_unresolved: Vec<DispatchInstance> = Vec::new();
    for f in &rendered_facts {
        if f.op != "execute" {
            continue;
        }
        let Some(SnapCapabilityExtra::Dispatch { object_type, .. }) = &f.extra else {
            continue;
        };
        let target_id = f.resource_id.clone();
        let target_display = target_id
            .as_ref()
            .and_then(|id| idx.stable_id_to_display.get(id))
            .cloned();
        let inst = DispatchInstance {
            object_type: object_type.clone(),
            target_id: target_id.clone(),
            target_display,
            confidence: f.confidence.clone(),
            provenance: f.provenance.clone(),
            via: f.via.clone(),
            witness_callsite_id: f.witness_callsite_id.clone(),
        };
        if target_id.is_some() {
            dispatch_resolved.push(inst);
        } else {
            dispatch_unresolved.push(inst);
        }
    }
    dispatch_resolved.sort_by(|a, b| {
        a.target_id
            .as_deref()
            .unwrap_or("")
            .cmp(b.target_id.as_deref().unwrap_or(""))
    });
    // unresolved: stable-sort by witnessCallsiteId ("" when absent) — fingerprint-query.ts:468.
    dispatch_unresolved.sort_by(|a, b| {
        a.witness_callsite_id
            .as_deref()
            .unwrap_or("")
            .cmp(b.witness_callsite_id.as_deref().unwrap_or(""))
    });

    // Required permissions.
    let mut perm_map: Vec<(String, PermissionLine)> = Vec::new();
    let mut perm_pos: HashMap<String, usize> = HashMap::new();
    for f in &rendered_facts {
        if f.resource_kind != "table" {
            continue;
        }
        let Some(rid_res) = &f.resource_id else {
            continue;
        };
        let Some(right) = table_op_to_right(&f.op) else {
            continue;
        };
        // #20: a provably temporary record needs no TableData permission. Per
        // fact, before its right is merged into the table's line, so a physical
        // access to the same table still contributes its own right (#33 keys
        // temp and physical facts apart in the cone).
        if f.is_known_temp() {
            continue;
        }
        let target_display = idx
            .stable_id_to_display
            .get(rid_res)
            .cloned()
            .unwrap_or_else(|| rid_res.clone());
        let key = format!("table|{rid_res}");
        if let Some(&pos) = perm_pos.get(&key) {
            let p = &mut perm_map[pos].1;
            if !p.rights.contains(right) {
                p.rights.push_str(right);
                // Keep rights sorted.
                let mut chars: Vec<char> = p.rights.chars().collect();
                chars.sort();
                p.rights = chars.into_iter().collect();
            }
        } else {
            let pos = perm_map.len();
            perm_pos.insert(key.clone(), pos);
            perm_map.push((
                key,
                PermissionLine {
                    target_kind: "table",
                    target_id: rid_res.clone(),
                    target_display,
                    rights: right.to_string(),
                    coverage: cov_status.clone(),
                },
            ));
        }
    }
    perm_map.sort_by(|(_, a), (_, b)| a.target_display.cmp(&b.target_display));
    let required_permissions: Vec<PermissionLine> = perm_map.into_iter().map(|(_, p)| p).collect();

    // Witnesses. `all` is the hard cap, 256.
    let cap = match filters.witness_limit {
        WitnessLimit::Disabled => None,
        WitnessLimit::Capped(cap) => Some(cap),
        WitnessLimit::All => Some(256),
    };
    let witnesses: Vec<BlockWitness> = match cap {
        None => {
            // witness reconstruction disabled; no BlockWitness entries.
            Vec::new()
        }
        Some(0) => {
            // objects present but zero paths.
            rendered_facts
                .iter()
                .enumerate()
                .map(|(fi, f)| BlockWitness {
                    fact_index: fi,
                    paths: Vec::new(),
                    truncated: false,
                    incomplete: false,
                    diagnostics: Vec::new(),
                    evidence: fact_evidence(f, &[], idx),
                })
                .collect()
        }
        Some(cap) => {
            // One pair of caches per root, across its facts (spec §14.3.1 item 3).
            let (mut hops, mut dags) = (HopCache::default(), DagCache::default());
            rendered_facts
                .iter()
                .enumerate()
                .map(|(fi, f)| {
                    let outcome = reconstruct_witness_paths_pub(
                        rid, f, widx, cap, &mut hops, &mut dags, carriers,
                    );
                    BlockWitness {
                        fact_index: fi,
                        evidence: fact_evidence(f, &outcome.paths, idx),
                        paths: outcome.paths,
                        truncated: outcome.truncated,
                        incomplete: outcome.incomplete,
                        diagnostics: outcome
                            .diagnostics
                            .into_iter()
                            .map(|d| WitnessDiagnostic {
                                kind: d.kind,
                                detail: d.detail,
                            })
                            .collect(),
                    }
                })
                .collect()
        }
    };

    FingerprintBlock {
        routine_id: rid.to_string(),
        object_id,
        object_display,
        routine_display,
        kinds: root.kinds.clone(),
        classification_source: root.source.clone(),
        config_entry_id: root.config_entry_id.clone(),
        coverage,
        families,
        may_commit,
        dispatch_resolved,
        dispatch_unresolved,
        required_permissions,
        witnesses,
        rendered_facts,
    }
}

// ---------------------------------------------------------------------------
// fingerprintQuery (al-sem fingerprintQuery)
// ---------------------------------------------------------------------------

/// Run the fingerprint query over a fully-composed `CapabilitySnapshot`.
/// Mirrors `fingerprintQuery` in `src/cli/fingerprint-query.ts`.
///
/// `roots_config_ignored`: true when a `roots.config.json` existed but the
/// caller skipped loading it (`--no-roots-config`) — surfaced verbatim into
/// `FingerprintQueryResult.roots_config_ignored` (`summary.rootsConfigIgnored`
/// in the JSON envelope). The `CapabilitySnapshot` carries no `inputsMetadata`
/// of its own, so the caller (which resolved the workspace and knows both the
/// flag and whether the file existed) computes and passes this in.
///
/// `owners`: the model's [`OwnerIndex`] (#78). It names each block's owning
/// object, narrows an `<owner>::<routine>` selector and labels ambiguity
/// candidates.
///
/// Roles (#160 step 9, epic Decision 28): `total_classifications` counts the
/// classifications whose routine `is_primary` (by stable id, read off the
/// model's roles), before the selector and `--roots` filters. Single-app every
/// row is primary.
///
/// `pool`: the routines `--routine` names, resolved once by
/// [`resolve_routine_selectors`] (final panel C5); `None` when no `--routine`
/// was given. A failing selector never reaches the query: its result is
/// [`selector_failure_result`], with no tree composed.
pub fn fingerprint_query(
    snap: &CapabilitySnapshot,
    filters: &FingerprintFilters,
    pool: Option<&std::collections::BTreeSet<String>>,
    roots_config_ignored: bool,
    owners: &OwnerIndex<'_>,
    is_primary: &dyn Fn(&str) -> bool,
) -> FingerprintQueryResult {
    // Build shared indexes (reuse B1's public entry point).
    let idx = build_fingerprint_indexes_pub(snap);

    let total_classifications = snap
        .root_classifications
        .iter()
        .filter(|r| is_primary(&r.routine_id))
        .count();

    // Filter root pool.
    let root_pool: Vec<&crate::engine::l5::snapshot::SnapshotRootClassificationSlot> = snap
        .root_classifications
        .iter()
        .filter(|r| {
            if let Some(roots) = &filters.roots {
                let intersects = r.kinds.iter().any(|k| roots.contains(k.as_str()));
                if !intersects {
                    return false;
                }
            }
            pool.is_none_or(|p| p.contains(r.routine_id.as_str()))
        })
        .collect();

    // Build blocks.
    let widx = witness_index(&idx);
    // One carrier cache for the whole query (final panel F1-C8).
    let carriers = CarrierCache::default();
    let mut blocks: Vec<FingerprintBlock> = Vec::new();
    for root in root_pool {
        let block = build_block(
            &root.routine_id,
            root,
            &idx,
            &widx,
            &carriers,
            filters,
            owners,
        );
        blocks.push(block);
    }

    // Sort blocks by routine_id.
    blocks.sort_by(|a, b| a.routine_id.cmp(&b.routine_id));

    let rendered_blocks = blocks.len();
    FingerprintQueryResult {
        blocks,
        diagnostics: Vec::new(),
        total_classifications,
        rendered_blocks,
        roots_config_ignored,
    }
}

/// `--routine`'s selectors resolved once, against the model's own identity
/// table (the one every tree holds), before any tree is composed (final panel
/// C5): the routines they name, or one diagnostic per failing selector, in
/// selector order. `idx` is [`build_selector_indexes`] of that table with the
/// primary app's guid, so a selector never names a dependency routine.
pub(crate) fn resolve_routine_selectors(
    selectors: &[String],
    idx: &SelectorIndexes,
    owners: &OwnerIndex<'_>,
) -> Result<std::collections::BTreeSet<String>, Vec<FingerprintQueryDiagnostic>> {
    let mut pool = std::collections::BTreeSet::new();
    let mut diagnostics = Vec::new();
    for sel in selectors {
        let (matches, matched_form) = resolve_selector(sel, idx);
        let matches = narrow_to_owner(sel, matched_form, matches, |id| owners.object_display(id));
        if matches.is_empty() {
            diagnostics.push(FingerprintQueryDiagnostic::SelectorUnresolved {
                selector: sel.clone(),
            });
        } else if matches.len() >= 2 {
            // candidates: (stableId, `<owner>::<display>`) — display falls back
            // to "" (TS: `routineDisplayById.get(id) ?? ""`), NOT to the id.
            let candidates = matches
                .iter()
                .take(MAX_AMBIGUOUS_CANDIDATES)
                .map(|id| {
                    let display = idx.routine_display_by_id.get(id).map_or("", String::as_str);
                    (id.clone(), owners.qualified(id, display))
                })
                .collect();
            diagnostics.push(FingerprintQueryDiagnostic::SelectorAmbiguous {
                selector: sel.clone(),
                matched_form: matched_form.to_string(),
                candidates,
            });
        } else {
            pool.extend(matches);
        }
    }
    if diagnostics.is_empty() {
        Ok(pool)
    } else {
        Err(diagnostics)
    }
}

/// A query whose selectors failed: no block, the diagnostics, and the count
/// [`fingerprint_query`] reads off a tree, here off the model the tree is
/// derived from (`derive_root_classifications` keeps a model classification
/// whose routine has a stable id), so no tree is composed (final panel O4).
pub(crate) fn selector_failure_result(
    model: &crate::program::model::workspace::Model,
    diagnostics: Vec<FingerprintQueryDiagnostic>,
    roots_config_ignored: bool,
) -> FingerprintQueryResult {
    let primary = primary_stable_ids(model);
    let stable = crate::engine::l4::capability_cone::routine_to_stable_of(&model.workspace);
    let total_classifications = model
        .root_classifications
        .iter()
        .filter(|c| {
            stable
                .get(&c.routine_id)
                .is_some_and(|s| primary.contains(s.as_str()))
        })
        .count();
    FingerprintQueryResult {
        blocks: Vec::new(),
        diagnostics,
        total_classifications,
        rendered_blocks: 0,
        roots_config_ignored,
    }
}

// ---------------------------------------------------------------------------
// factIdOf — 16-hex sha256 over the semantic key (mirrors factIdOf in TS)
// ---------------------------------------------------------------------------

pub fn fact_id_of(f: &SnapFact) -> String {
    let key = [
        f.subject.as_str(),
        f.op.as_str(),
        f.resource_kind.as_str(),
        f.resource_id.as_deref().unwrap_or(""),
        f.confidence.as_str(),
        f.provenance.as_str(),
        f.via.as_str(),
    ]
    .join("|");
    sha256_hex(&key)[..16].to_string()
}

// ---------------------------------------------------------------------------
// project_fingerprint_query — JSON envelope (fingerprint-query, 1.3.0)
// Mirrors `projectFingerprintQuery` in `src/contracts/fingerprint-query.ts`.
// ---------------------------------------------------------------------------

/// 1.3.0 (#160 step 9, O9-25): additive, a path's `boundary` object and a
/// boundary form of a witness's `evidence` (rule B, cross-app only).
pub const FINGERPRINT_QUERY_CONTRACT_VERSION: &str = "1.3.0";

/// Build a `normalizeAnchorPath`-normalized file path: forward slashes,
/// keep the `ws:` prefix intact (it is NOT stripped — the golden keeps it).
///
/// al-sem `normalizeAnchorPath(file, workspaceRoot)` strips ONLY a leading
/// absolute `workspaceRoot` prefix. In the fingerprint snapshot every source
/// path is already in `ws:`-relative form (never the absolute root), so the
/// `workspaceRoot` argument never matches and the function reduces to the
/// slash-normalization below — hence no `workspace_root` parameter here.
fn normalize_anchor_path(source_file: &str) -> String {
    source_file.replace('\\', "/")
}

/// A rule-B boundary's JSON object (#160 step 9 S9-2): `anchor`, `id`, `kind`
/// and the kind's own id key (`callsiteId` | `operationId` | `eventId`).
fn boundary_to_json(b: &crate::engine::l5::digest::PathBoundary) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    if let Some(file) = &b.file {
        let mut anch = serde_json::Map::new();
        anch.insert("sourceKind".into(), "source".into());
        anch.insert("file".into(), normalize_anchor_path(file).into());
        if let Some(l) = b.line {
            anch.insert("line".into(), l.into());
        }
        if let Some(c) = b.column {
            anch.insert("column".into(), c.into());
        }
        m.insert("anchor".into(), serde_json::Value::Object(anch));
    }
    m.insert("id".into(), b.id.clone().into());
    m.insert("kind".into(), b.kind.as_str().into());
    m.insert(b.target_key().into(), b.target.clone().into());
    serde_json::Value::Object(m)
}

/// Project one block witness path set into the JSON contract form. The
/// witness carries its own evidence (#160 step 9 S9-16), so no snapshot is read.
fn project_witness_json(bw: &BlockWitness, workspace_root: &str) -> serde_json::Value {
    // Terminal evidence (anchor keys: column, excerpt, file, line, sourceKind — alphabetical).
    let evidence = bw.evidence.as_ref().map(|e| match e {
        FactEvidence::Operation {
            operation_id,
            file,
            line,
            column,
            excerpt,
        } => {
            // Sorted: column, excerpt, file, line, sourceKind
            let mut anchor = serde_json::Map::new();
            anchor.insert("column".into(), (*column).into());
            anchor.insert("excerpt".into(), excerpt.clone().into());
            anchor.insert("file".into(), file.clone().into());
            anchor.insert("line".into(), (*line).into());
            anchor.insert("sourceKind".into(), "source".into());
            // Sorted: anchor, operationId
            let mut ev_obj = serde_json::Map::new();
            ev_obj.insert("anchor".into(), serde_json::Value::Object(anchor));
            ev_obj.insert("operationId".into(), operation_id.clone().into());
            serde_json::Value::Object(ev_obj)
        }
        FactEvidence::Boundary(b) => boundary_to_json(b),
    });

    // Projected paths (already projected via reconstruct_witness_paths_pub);
    // a path cut at a rule-B boundary names it (absent on the cut).
    let projected_paths: Vec<serde_json::Value> = bw
        .paths
        .iter()
        .map(|path| {
            let hops: Vec<serde_json::Value> = path
                .query_hops
                .iter()
                .map(|h| query_hop_to_json(h, workspace_root))
                .collect();
            match &path.boundary {
                Some(b) => serde_json::json!({"boundary": boundary_to_json(b), "hops": hops}),
                None => serde_json::json!({"hops": hops}),
            }
        })
        .collect();

    // Diagnostics (keys sorted: detail before kind).
    let diags: Vec<serde_json::Value> = bw
        .diagnostics
        .iter()
        .map(|d| {
            let mut m = serde_json::Map::new();
            // Sorted: detail (if present) before kind
            if let Some(ref detail) = d.detail {
                m.insert("detail".into(), detail.clone().into());
            }
            m.insert("kind".into(), d.kind.clone().into());
            serde_json::Value::Object(m)
        })
        .collect();

    // Witness object keys sorted alphabetically:
    // diagnostics, evidence (if present), incomplete, paths, truncated
    let mut w = serde_json::Map::new();
    w.insert("diagnostics".into(), serde_json::Value::Array(diags));
    if let Some(ev) = evidence {
        w.insert("evidence".into(), ev);
    }
    w.insert("incomplete".into(), bw.incomplete.into());
    w.insert("paths".into(), serde_json::Value::Array(projected_paths));
    w.insert("truncated".into(), bw.truncated.into());
    serde_json::Value::Object(w)
}

fn query_hop_to_json(hop: &QueryWitnessHop, _workspace_root: &str) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    m.insert("kind".into(), hop.kind.into());
    m.insert("fromRoutineId".into(), hop.from_routine_id.clone().into());
    m.insert("fromDisplay".into(), hop.from_display.clone().into());
    if let Some(v) = &hop.to_routine_id {
        m.insert("toRoutineId".into(), v.clone().into());
    }
    if let Some(v) = &hop.to_display {
        m.insert("toDisplay".into(), v.clone().into());
    }
    if let Some(v) = &hop.callee_display {
        m.insert("calleeDisplay".into(), v.clone().into());
    }
    if let Some(v) = &hop.callsite_id {
        m.insert("callsiteId".into(), v.clone().into());
    }
    if let Some(v) = &hop.event_id {
        m.insert("eventId".into(), v.clone().into());
    }
    if let Some(v) = &hop.target_app_guid {
        m.insert("targetAppGuid".into(), v.clone().into());
    }
    if let Some(v) = &hop.edge_kind {
        m.insert("edgeKind".into(), v.clone().into());
    }
    if let Some(v) = &hop.receiver_type {
        m.insert("receiverType".into(), v.clone().into());
    }
    if let Some(v) = &hop.interface_name {
        m.insert("interfaceName".into(), v.clone().into());
    }
    if let Some(cc) = hop.candidate_count {
        m.insert("candidateCount".into(), (cc as u64).into());
    }
    if let Some(a) = &hop.anchor {
        let file = normalize_anchor_path(&a.file);
        let mut anch = serde_json::Map::new();
        anch.insert("sourceKind".into(), "source".into());
        anch.insert("file".into(), file.into());
        if let Some(l) = a.line {
            anch.insert("line".into(), l.into());
        }
        if let Some(c) = a.column {
            anch.insert("column".into(), c.into());
        }
        m.insert("anchor".into(), serde_json::Value::Object(anch));
    }
    serde_json::Value::Object(m)
}

/// One node of the query document, serialized exactly as the
/// `serde_json::Value` it stands for once sorted and null-dropped
/// (`serialize_document_value`), but with its big arrays built element by
/// element while serializing (#160 step 9, the B1q fix): no `Value` of the
/// whole document is ever alive. serde_json's pretty serializer makes the same
/// calls (`serialize_map` with sorted keys, `serialize_seq` with the length)
/// for a `Value` and for this, so the bytes are the same.
enum Doc<'a> {
    /// A small subtree, already sorted and null-dropped.
    Value(serde_json::Value),
    /// An object; its entries are sorted by key, and `null` values dropped,
    /// when serialized.
    Map(Vec<(&'static str, Doc<'a>)>),
    /// An array of `n` elements, the `i`-th built when it is serialized.
    Seq(usize, Box<dyn Fn(usize) -> Doc<'a> + 'a>),
}

impl Doc<'_> {
    /// A small subtree, normalized as `serialize_document_value` does.
    fn value(v: serde_json::Value) -> Self {
        Doc::Value(crate::engine::gate::format_json::sort_and_drop_nulls(v))
    }

    fn is_null(&self) -> bool {
        matches!(self, Doc::Value(serde_json::Value::Null))
    }
}

impl serde::Serialize for Doc<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::{SerializeMap, SerializeSeq};
        match self {
            Doc::Value(v) => v.serialize(s),
            Doc::Map(entries) => {
                let mut kept: Vec<&(&'static str, Doc<'_>)> =
                    entries.iter().filter(|(_, v)| !v.is_null()).collect();
                kept.sort_by(|a, b| a.0.cmp(b.0));
                let mut m = s.serialize_map(Some(kept.len()))?;
                for (k, v) in kept {
                    m.serialize_entry(k, v)?;
                }
                m.end()
            }
            Doc::Seq(n, item) => {
                let mut seq = s.serialize_seq(Some(*n))?;
                for i in 0..*n {
                    seq.serialize_element(&item(i))?;
                }
                seq.end()
            }
        }
    }
}

// Test hook (final panel S1): the bytes `serialize_doc` has written on this
// thread, and that count at each `fact_doc` build, so a test reads off the
// production projection whether a fact is built only after the previous one
// is written.
#[cfg(test)]
thread_local! {
    static DOC_WRITTEN: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static FACT_BUILT_AT: std::cell::RefCell<Vec<usize>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// The query document's output buffer (it feeds `DOC_WRITTEN` under test).
struct DocOut(Vec<u8>);

impl std::io::Write for DocOut {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.extend_from_slice(b);
        #[cfg(test)]
        DOC_WRITTEN.with(|c| c.set(self.0.len()));
        Ok(b.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// `format_json::serialize_document_value`'s bytes for a [`Doc`]: pretty,
/// trailing newline.
fn serialize_doc(doc: &Doc<'_>) -> String {
    let mut out = DocOut(Vec::new());
    let mut ser =
        serde_json::Serializer::with_formatter(&mut out, serde_json::ser::PrettyFormatter::new());
    serde::Serialize::serialize(doc, &mut ser).expect("query document serialization");
    let mut text = String::from_utf8(out.0).expect("serde_json writes UTF-8");
    text.push('\n');
    text
}

/// One fact of a block as JSON, with its witness (`build_block` makes the
/// witnesses positionally: the `fi`-th, when any, is fact `fi`'s).
fn fact_doc(block: &FingerprintBlock, fi: usize, workspace_root: &str) -> serde_json::Value {
    #[cfg(test)]
    FACT_BUILT_AT.with(|v| v.borrow_mut().push(DOC_WRITTEN.with(std::cell::Cell::get)));
    let fact = &block.rendered_facts[fi];
    let bw = block.witnesses.get(fi).filter(|w| w.fact_index == fi);
    let mut fj = serde_json::Map::new();
    fj.insert("factId".into(), fact_id_of(fact).into());
    fj.insert("op".into(), fact.op.clone().into());
    fj.insert("resourceKind".into(), fact.resource_kind.clone().into());
    if let Some(ref rid) = fact.resource_id {
        fj.insert("resourceId".into(), rid.clone().into());
    }
    fj.insert("provenance".into(), fact.provenance.clone().into());
    fj.insert("confidence".into(), fact.confidence.clone().into());
    fj.insert("via".into(), fact.via.clone().into());
    if let Some(bw) = bw {
        fj.insert("witness".into(), project_witness_json(bw, workspace_root));
    }
    serde_json::Value::Object(fj)
}

/// A block's families: every resource's facts built one by one.
fn families_doc<'a>(block: &'a FingerprintBlock, workspace_root: &'a str) -> Doc<'a> {
    Doc::Seq(
        block.families.len(),
        Box::new(move |f| {
            let family = &block.families[f];
            Doc::Map(vec![
                ("resourceKind", Doc::value(family.kind.clone().into())),
                (
                    "coneCoverage",
                    Doc::value(family.cone_coverage.clone().into()),
                ),
                (
                    "resources",
                    Doc::Seq(
                        family.resources.len(),
                        Box::new(move |r| {
                            let res = &family.resources[r];
                            Doc::Map(vec![
                                (
                                    "id",
                                    Doc::value(
                                        res.id.clone().map_or(serde_json::Value::Null, Into::into),
                                    ),
                                ),
                                ("display", Doc::value(res.display.clone().into())),
                                (
                                    "ops",
                                    Doc::value(serde_json::Value::Array(
                                        res.ops.iter().map(|o| o.clone().into()).collect(),
                                    )),
                                ),
                                (
                                    "facts",
                                    Doc::Seq(
                                        res.fact_indices.len(),
                                        Box::new(move |i| {
                                            Doc::value(fact_doc(
                                                block,
                                                res.fact_indices[i],
                                                workspace_root,
                                            ))
                                        }),
                                    ),
                                ),
                            ])
                        }),
                    ),
                ),
            ])
        }),
    )
}

/// Full `projectFingerprintQuery` with all parameters injected. The result is
/// self-contained (#160 step 9 S9-16): no snapshot is read here. The blocks
/// stream ([`Doc`], the B1q fix): each fact's JSON is built, written and
/// dropped in turn.
pub fn project_fingerprint_query_full(
    result: &FingerprintQueryResult,
    workspace_root: &str,
    filters: &FingerprintFilters,
    deterministic: bool,
    analyzer_diagnostics: &[(String, String, String)],
    driver_version: &str,
    workspace_fingerprint: &str,
) -> String {
    // Project blocks.
    let blocks = Doc::Seq(
        result.blocks.len(),
        Box::new(move |b| block_doc(&result.blocks[b], workspace_root)),
    );
    project_query_envelope(
        result,
        blocks,
        filters,
        deterministic,
        analyzer_diagnostics,
        driver_version,
        workspace_fingerprint,
    )
}

/// One block: its small parts as values, its families streamed.
fn block_doc<'a>(block: &'a FingerprintBlock, workspace_root: &'a str) -> Doc<'a> {
    {
        // mayCommit.
        let may_commit_fact_id = block
            .may_commit
            .witness_fact_index
            .map(|fi| fact_id_of(&block.rendered_facts[fi]));
        let mut may_commit_obj = serde_json::Map::new();
        may_commit_obj.insert("presence".into(), block.may_commit.presence.into());
        if let Some(fid) = may_commit_fact_id {
            may_commit_obj.insert("witnessFactId".into(), fid.into());
        }

        // Dispatch.
        let project_disp = |d: &DispatchInstance| -> serde_json::Value {
            let mut m = serde_json::Map::new();
            m.insert("objectType".into(), d.object_type.clone().into());
            if let Some(ref id) = d.target_id {
                m.insert("targetId".into(), id.clone().into());
            }
            if let Some(ref disp) = d.target_display {
                m.insert("targetDisplay".into(), disp.clone().into());
            }
            m.insert("confidence".into(), d.confidence.clone().into());
            m.insert("provenance".into(), d.provenance.clone().into());
            m.insert("via".into(), d.via.clone().into());
            serde_json::Value::Object(m)
        };
        let resolved_arr: Vec<serde_json::Value> =
            block.dispatch_resolved.iter().map(project_disp).collect();
        let unresolved_arr: Vec<serde_json::Value> =
            block.dispatch_unresolved.iter().map(project_disp).collect();

        // Required permissions.
        let perms_arr: Vec<serde_json::Value> = block
            .required_permissions
            .iter()
            .map(|p| {
                serde_json::json!({
                    "targetKind": p.target_kind,
                    "targetId": p.target_id,
                    "targetDisplay": p.target_display,
                    "rights": p.rights,
                    "coverage": p.coverage,
                })
            })
            .collect();

        // Coverage.
        let cov_obj = serde_json::json!({
            "status": block.coverage.status,
            "directStatus": block.coverage.direct_status,
            "inheritedStatus": block.coverage.inherited_status,
            "reasons": block.coverage.reasons,
            "unknownTargets": block.coverage.unknown_targets,
        });

        // Routine ref.
        let routine_ref = serde_json::json!({
            "stableId": block.routine_id,
            "display": block.routine_display,
            "objectDisplay": block.object_display,
        });

        Doc::Map(vec![
            ("routine", Doc::value(routine_ref)),
            (
                "rootKinds",
                Doc::value(serde_json::Value::Array(
                    block.kinds.iter().map(|k| k.clone().into()).collect(),
                )),
            ),
            (
                "classificationSource",
                Doc::value(block.classification_source.clone().into()),
            ),
            ("coverage", Doc::value(cov_obj)),
            ("families", families_doc(block, workspace_root)),
            (
                "mayCommit",
                Doc::value(serde_json::Value::Object(may_commit_obj)),
            ),
            (
                "dispatch",
                Doc::value(serde_json::json!({
                    "resolved": resolved_arr,
                    "unresolved": unresolved_arr,
                })),
            ),
            (
                "requiredPermissions",
                Doc::value(serde_json::Value::Array(perms_arr)),
            ),
        ])
    }
}

/// The query envelope around `blocks`: its small parts as values.
fn project_query_envelope(
    result: &FingerprintQueryResult,
    blocks: Doc<'_>,
    filters: &FingerprintFilters,
    deterministic: bool,
    analyzer_diagnostics: &[(String, String, String)],
    driver_version: &str,
    workspace_fingerprint: &str,
) -> String {
    // Payload diagnostics.
    let payload_diags: Vec<serde_json::Value> = result
        .diagnostics
        .iter()
        .map(|d| match d {
            FingerprintQueryDiagnostic::SelectorUnresolved { selector } => serde_json::json!({
                "kind": "selector-unresolved",
                "selector": selector,
                "detail": "triedForms=stable-routine-id,full-display,two-segment,one-segment,object-qualified",
            }),
            FingerprintQueryDiagnostic::SelectorAmbiguous { selector, matched_form, .. } => {
                serde_json::json!({
                    "kind": "selector-ambiguous",
                    "selector": selector,
                    "detail": format!("matchedForm={matched_form}"),
                })
            }
        })
        .collect();

    // Filters projection.
    let mut filters_map = serde_json::Map::new();
    if let Some(roots) = &filters.roots {
        let mut sorted_roots: Vec<&str> = roots.iter().map(|s| s.as_str()).collect();
        sorted_roots.sort();
        filters_map.insert(
            "roots".into(),
            serde_json::Value::Array(sorted_roots.iter().map(|r| (*r).into()).collect()),
        );
    }
    if !filters.routine_selectors.is_empty() {
        filters_map.insert(
            "routines".into(),
            serde_json::Value::Array(
                filters
                    .routine_selectors
                    .iter()
                    .map(|s| s.clone().into())
                    .collect(),
            ),
        );
    }
    filters_map.insert("includeInherited".into(), filters.include_inherited.into());
    match filters.witness_limit {
        WitnessLimit::Disabled => {
            filters_map.insert("witnessLimit".into(), false.into());
        }
        WitnessLimit::Capped(n) => {
            filters_map.insert("witnessLimit".into(), (n as u64).into());
        }
        WitnessLimit::All => {
            filters_map.insert("witnessLimit".into(), "all".into());
        }
    }

    // Payload.
    let payload = Doc::Map(vec![
        (
            "workspaceFingerprint",
            Doc::value(workspace_fingerprint.into()),
        ),
        (
            "filters",
            Doc::value(serde_json::Value::Object(filters_map)),
        ),
        ("blocks", blocks),
        (
            "diagnostics",
            Doc::value(serde_json::Value::Array(payload_diags)),
        ),
        (
            "summary",
            Doc::value(serde_json::json!({
                "totalClassifications": result.total_classifications,
                "renderedBlocks": result.rendered_blocks,
                "rootsConfigIgnored": result.roots_config_ignored,
            })),
        ),
    ]);

    // Build envelope.
    let generated_at = crate::engine::gate::format_json::pinned_or_now_iso8601(deterministic);

    let diags_arr: Vec<serde_json::Value> = analyzer_diagnostics
        .iter()
        .map(|(code, severity, message)| {
            serde_json::json!({
                "code": code,
                "severity": severity,
                "message": message,
            })
        })
        .collect();

    let envelope = Doc::Map(vec![
        ("kind", Doc::value("fingerprint-query".into())),
        (
            "schemaVersion",
            Doc::value(FINGERPRINT_QUERY_CONTRACT_VERSION.into()),
        ),
        ("alsemVersion", Doc::value(driver_version.into())),
        ("deterministic", Doc::value(deterministic.into())),
        ("generatedAt", Doc::value(generated_at.into())),
        (
            "diagnostics",
            Doc::value(serde_json::Value::Array(diags_arr)),
        ),
        ("payload", payload),
    ]);

    serialize_doc(&envelope)
}

// ---------------------------------------------------------------------------
// format_fingerprint_human — human-readable renderer
// Mirrors `formatFingerprint` in `src/cli/format-fingerprint.ts`.
// ---------------------------------------------------------------------------

const LABEL_WIDTH: usize = 12;

const EXTERNAL_FAMILIES: &[&str] = &[
    "http",
    "isolated-storage",
    "file",
    "background",
    "telemetry",
    "ui",
];

fn pad(label: &str) -> String {
    format!("{:<width$}", format!("{label}:"), width = LABEL_WIDTH)
}

fn q(s: &str) -> String {
    serde_json::json!(s).to_string()
}

fn kind_prefix(kind: &str) -> &'static str {
    match kind {
        "table" => "TableData",
        "event" => "Event",
        "codeunit" => "Codeunit",
        "page" => "Page",
        "report" => "Report",
        _ => "unknown",
    }
}

fn external_label(kind: &str) -> &'static str {
    match kind {
        "http" => "http",
        "isolated-storage" => "storage",
        "file" => "file",
        "background" => "background",
        "telemetry" => "telemetry",
        "ui" => "ui",
        _ => "unknown",
    }
}

fn render_family_line(
    block: &FingerprintBlock,
    label: &str,
    kind: &str,
    op: &str,
    lines: &mut Vec<String>,
) {
    let fam = block.families.iter().find(|f| f.kind == kind);
    let items: Vec<&BlockResource> = fam
        .map(|f| {
            f.resources
                .iter()
                .filter(|r| r.ops.contains(&op.to_string()))
                .collect()
        })
        .unwrap_or_default();
    if items.is_empty() && block.coverage.status == "complete" {
        return;
    }
    if items.is_empty() {
        lines.push(format!("  {}none known reachable", pad(label)));
        return;
    }
    let rendered: Vec<String> = items
        .iter()
        .map(|r| format!("{} {}", kind_prefix(kind), q(&r.display)))
        .collect();
    lines.push(format!("  {}{}", pad(label), rendered.join(", ")));
}

fn render_commit(block: &FingerprintBlock, lines: &mut Vec<String>) {
    if block.may_commit.presence == "no" {
        return; // omit "no" commits in compact
    }
    lines.push(format!("  {}{}", pad("commit"), block.may_commit.presence));
}

fn render_dispatch(block: &FingerprintBlock, lines: &mut Vec<String>) {
    let r = block.dispatch_resolved.len();
    let u = block.dispatch_unresolved.len();
    if r == 0 && u == 0 {
        return;
    }
    let mut parts: Vec<String> = Vec::new();
    if r > 0 {
        let list: Vec<String> = block
            .dispatch_resolved
            .iter()
            .take(4)
            .map(|d| {
                d.target_display
                    .clone()
                    .or_else(|| d.target_id.clone())
                    .unwrap_or_else(|| "?".to_string())
            })
            .collect();
        let more = if r > 4 {
            format!(", +{} more", r - 4)
        } else {
            String::new()
        };
        parts.push(format!("{r} static ({}{more})", list.join(", ")));
    }
    if u > 0 {
        parts.push(format!("{u} unresolved-dynamic"));
    }
    lines.push(format!("  {}{}", pad("dispatch"), parts.join("; ")));
}

fn render_external_families(block: &FingerprintBlock, verbosity: &str, lines: &mut Vec<String>) {
    let mut empty_external: Vec<&str> = Vec::new();
    for &kind in EXTERNAL_FAMILIES {
        let fam = block.families.iter().find(|f| f.kind == kind);
        let present = fam.map(|f| !f.resources.is_empty()).unwrap_or(false);
        if !present {
            empty_external.push(kind);
            if verbosity == "full" {
                let word = if block.coverage.status == "complete" {
                    "none"
                } else {
                    "none known reachable"
                };
                lines.push(format!("  {}{}", pad(external_label(kind)), word));
            }
            continue;
        }
        let resources = &fam.unwrap().resources;
        let rendered: Vec<String> = resources.iter().map(|r| q(&r.display)).collect();
        lines.push(format!(
            "  {}{}",
            pad(external_label(kind)),
            rendered.join(", ")
        ));
    }
    if verbosity == "compact" && !empty_external.is_empty() && block.coverage.status != "complete" {
        let list = empty_external
            .iter()
            .map(|k| external_label(k))
            .collect::<Vec<_>>()
            .join("/");
        lines.push(format!("  no known {list} capabilities — cone partial"));
    }
}

fn render_permissions(block: &FingerprintBlock, lines: &mut Vec<String>) {
    if block.required_permissions.is_empty() {
        return;
    }
    let all_complete = block
        .required_permissions
        .iter()
        .all(|p| p.coverage == "complete")
        && block.coverage.status == "complete";
    let heading = if all_complete {
        "permissions:"
    } else {
        "permissions (inferred, may be incomplete):"
    };
    lines.push(format!("  {heading}"));
    let mut sorted = block.required_permissions.clone();
    sorted.sort_by(|a, b| a.target_display.cmp(&b.target_display));
    for p in &sorted {
        let prefix = if p.target_kind == "table" {
            "TableData"
        } else {
            "Object"
        };
        lines.push(format!(
            "    {prefix} {} {}",
            q(&p.target_display),
            p.rights
        ));
    }
}

fn hop_arrow(depth: usize) -> &'static str {
    if depth == 0 { "" } else { "→ " }
}

/// `formatHop` (format-fingerprint.ts:270). Renders a raw-derived `HumanHop`.
/// The `routineDisplay` and the SHORT `eventDisplay` are read off the raw hop;
/// the anchor uses the raw `sourceFile` verbatim (with the `ws:` prefix).
pub fn format_hop(hop: &HumanHop) -> String {
    match hop {
        HumanHop::Call {
            routine_display,
            callee_display,
            source_file,
            line,
            column,
        } => format!(
            "{routine_display} (via {callee_display}{})",
            raw_anchor_suffix(source_file, *line, *column)
        ),
        HumanHop::ObjectRun {
            routine_display,
            target_display,
            source_file,
            line,
            column,
        } => format!(
            "{routine_display} (via Codeunit.Run {}{})",
            target_display.as_deref().unwrap_or("<unresolved>"),
            raw_anchor_suffix(source_file, *line, *column)
        ),
        HumanHop::EventDispatch { event_display } => format!("event {event_display}"),
        HumanHop::VariableTypedCall {
            routine_display,
            receiver_type,
            callee_display,
            source_file,
            line,
            column,
        } => format!(
            "{routine_display} (via {receiver_type}.{}{})",
            callee_display.as_deref().unwrap_or("?"),
            raw_anchor_suffix(source_file, *line, *column)
        ),
        HumanHop::InterfaceDispatch {
            routine_display,
            interface_name,
            candidate_count,
            source_file,
            line,
            column,
        } => format!(
            "{routine_display} (via interface {interface_name}, {candidate_count} candidate{}{})",
            if *candidate_count == 1 { "" } else { "s" },
            raw_anchor_suffix(source_file, *line, *column)
        ),
        HumanHop::ImplicitTrigger {
            routine_display,
            trigger_kind,
            source_file,
            line,
            column,
        } => format!(
            "{routine_display} (via trigger {trigger_kind}{})",
            raw_anchor_suffix(source_file, *line, *column)
        ),
    }
}

/// ` at <file>:<line>:<col>` suffix from a raw hop's source location.
/// Mirrors TS `${sourceFile ? ` at ${sourceFile}:${line ?? 0}:${column ?? 0}` : ""}`.
fn raw_anchor_suffix(
    source_file: &Option<String>,
    line: Option<u32>,
    column: Option<u32>,
) -> String {
    match source_file {
        Some(sf) => format!(" at {sf}:{}:{}", line.unwrap_or(0), column.unwrap_or(0)),
        None => String::new(),
    }
}

pub fn format_terminal_hop(t: &TerminalHopInfo) -> String {
    // Mirrors formatHop case "terminal" in format-fingerprint.ts.
    let loc = if let Some(ref sf) = t.source_file {
        let line = t.line.unwrap_or(0);
        let col = t.column.unwrap_or(0);
        format!(" at {sf}:{line}:{col}")
    } else {
        String::new()
    };
    match t.evidence_kind.as_str() {
        "operation" => format!("direct {}{}", t.display_text, loc),
        "callsite" => format!("call {}{}", t.display_text, loc),
        _ => t.display_text.clone(),
    }
}

fn fact_description(f: &SnapFact) -> String {
    if let Some(ref rid) = f.resource_id {
        format!("{} {} {}", f.op, f.resource_kind, q(rid))
    } else {
        format!("{} {}", f.op, f.resource_kind)
    }
}

fn render_witnesses(block: &FingerprintBlock, lines: &mut Vec<String>) {
    if block.witnesses.is_empty() {
        return;
    }
    let present: Vec<&BlockWitness> = block
        .witnesses
        .iter()
        .filter(|w| !w.paths.is_empty() || w.truncated)
        .collect();
    if present.is_empty() {
        return;
    }
    lines.push(String::new());
    lines.push("  witnesses:".to_string());
    for w in &present {
        let fact = &block.rendered_facts[w.fact_index];
        let desc = fact_description(fact);
        let total = w.paths.len();
        lines.push(format!(
            "    {desc} — {total} path{} shown",
            if total == 1 { "" } else { "s" }
        ));
        for (i, path) in w.paths.iter().enumerate() {
            lines.push(format!("      Path {}/{total} shown:", i + 1));
            // Render from the RAW-derived human hops (format-fingerprint.ts:247),
            // NOT the projected query_hops — those drop short eventDisplay etc.
            for (h, hop) in path.human_hops.iter().enumerate() {
                let indent = "        ".to_string() + &"  ".repeat(h);
                lines.push(format!("{indent}{}{}", hop_arrow(h), format_hop(hop)));
            }
            // Render the terminal hop (the evidence step) after all routing hops.
            if let Some(ref term) = path.terminal_hop {
                let depth = path.human_hops.len();
                let indent = "        ".to_string() + &"  ".repeat(depth);
                lines.push(format!(
                    "{indent}{}{}",
                    hop_arrow(depth),
                    format_terminal_hop(term)
                ));
            }
            lines.push(String::new());
        }
        if w.truncated {
            let cap = w
                .diagnostics
                .iter()
                .find(|d| d.kind == "path-limit-reached")
                .and_then(|d| d.detail.as_ref())
                .and_then(|det| det.strip_prefix("cap="))
                .and_then(|s| s.parse::<usize>().ok());
            let cap_str = cap
                .map(|c| c.to_string())
                .unwrap_or_else(|| "?".to_string());
            lines.push(format!(
                "      warning: witnesses truncated at {cap_str} for {desc}; narrow with --routine/--roots or raise --witness."
            ));
        }
    }
}

fn render_block(block: &FingerprintBlock, verbosity: &str, lines: &mut Vec<String>) {
    let marker = match block.classification_source.as_str() {
        "config" => " [config-root]",
        "ast+config" => " [config-asserted]",
        _ => "",
    };
    lines.push(format!(
        "{}  [{}]{marker}",
        owner_qualified(&block.object_display, &block.routine_display),
        block.kinds.join(", ")
    ));

    // Coverage. Reasons are sorted alphabetically (format-fingerprint.ts:88).
    let reasons = if block.coverage.reasons.is_empty() {
        String::new()
    } else {
        let mut sorted = block.coverage.reasons.clone();
        sorted.sort();
        format!(" — {}", sorted.join(", "))
    };
    lines.push(format!(
        "  {}{}{}",
        pad("coverage"),
        block.coverage.status,
        reasons
    ));
    if !block.coverage.unknown_targets.is_empty() {
        let shown: Vec<&String> = block
            .coverage
            .unknown_target_displays
            .iter()
            .take(5)
            .collect();
        let more = block
            .coverage
            .unknown_targets
            .len()
            .saturating_sub(shown.len());
        let more_suffix = if more > 0 {
            format!(" (+{more} more)")
        } else {
            String::new()
        };
        let count = block.coverage.unknown_targets.len();
        lines.push(format!(
            "  {}{count} opaque/unresolved targets: {}{}",
            " ".repeat(LABEL_WIDTH),
            shown
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            more_suffix
        ));
    }
    if block.coverage.inherited_excluded {
        lines.push(format!(
            "  {}(direct-only; coverage reflects direct cone)",
            " ".repeat(LABEL_WIDTH)
        ));
    }

    render_family_line(block, "writes", "table", "insert", lines);
    render_family_line(block, "reads", "table", "read", lines);
    render_commit(block, lines);
    render_family_line(block, "publish", "event", "publish", lines);
    render_dispatch(block, lines);
    render_external_families(block, verbosity, lines);
    render_permissions(block, lines);
    render_witnesses(block, lines);
}

/// Render the fingerprint query result as human-readable text.
/// Mirrors `formatFingerprint` in `src/cli/format-fingerprint.ts`. `verbosity`
/// is `"compact"` (default) or `"full"`.
pub fn format_fingerprint_human_verbosity(
    result: &FingerprintQueryResult,
    verbosity: &str,
) -> String {
    let mut lines: Vec<String> = Vec::new();

    if result.blocks.is_empty() {
        lines.push("No root classifications match the filters.".to_string());
        return format!("{}\n", lines.join("\n"));
    }

    let total_trunc: usize = result
        .blocks
        .iter()
        .flat_map(|b| &b.witnesses)
        .filter(|w| w.truncated)
        .count();
    let trunc_suffix = if total_trunc > 0 {
        format!(
            " {total_trunc} witness set{} truncated; details inline.",
            if total_trunc == 1 { "" } else { "s" }
        )
    } else {
        String::new()
    };

    let filter_clause = if result.rendered_blocks == result.total_classifications {
        format!(
            "Rendering {} root classification{}.",
            result.rendered_blocks,
            if result.rendered_blocks == 1 { "" } else { "s" }
        )
    } else {
        format!(
            "Rendering {} of {} root classifications.",
            result.rendered_blocks, result.total_classifications
        )
    };
    lines.push(format!("{filter_clause}{trunc_suffix}"));
    lines.push(String::new());

    for (i, block) in result.blocks.iter().enumerate() {
        if i > 0 {
            lines.push(String::new());
        }
        render_block(block, verbosity, &mut lines);
    }

    format!("{}\n", lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::l5::snapshot::SnapshotIdentityTable;

    /// #160 step 9 (the B1q fix): a [`Doc`] serializes to exactly the bytes
    /// `serialize_document_value` gives the `Value` it stands for: keys out of
    /// order, a `null` entry (dropped), a `null` inside a small value (dropped
    /// there too), nested maps, an empty and a non-empty streamed array.
    /// DISCRIMINATION: `Doc::Map` without its sort, or without its null drop,
    /// fails it.
    #[test]
    fn doc_serializes_as_the_normalized_value() {
        let doc = Doc::Map(vec![
            (
                "zeta",
                Doc::value(serde_json::json!({"b": 1, "a": null, "c": [1, 2]})),
            ),
            ("alpha", Doc::value(serde_json::Value::Null)),
            ("empty", Doc::Seq(0, Box::new(|_| unreachable!()))),
            (
                "items",
                Doc::Seq(
                    2,
                    Box::new(|i| {
                        Doc::Map(vec![
                            ("n", Doc::value((i as u64).into())),
                            ("m", Doc::value(serde_json::json!({"y": "x", "x": i}))),
                        ])
                    }),
                ),
            ),
            ("beta", Doc::Map(vec![("k", Doc::value(true.into()))])),
        ]);
        let want = crate::engine::gate::format_json::serialize_document_value(serde_json::json!({
            "zeta": {"b": 1, "a": null, "c": [1, 2]},
            "alpha": null,
            "empty": [],
            "items": [
                {"n": 0, "m": {"y": "x", "x": 0}},
                {"n": 1, "m": {"y": "x", "x": 1}},
            ],
            "beta": {"k": true},
        }));
        assert_eq!(serialize_doc(&doc), want);
    }

    /// #160 step 9 (the B1q fix): a streamed array builds each element while
    /// the serializer writes, so element `i` is built only after element
    /// `i - 1`'s bytes are out (no array is held whole). DISCRIMINATION:
    /// `Doc::Seq` collecting every element before writing fails it.
    #[test]
    fn doc_builds_seq_items_while_writing() {
        use std::cell::{Cell, RefCell};
        use std::rc::Rc;
        struct Counting(Rc<Cell<usize>>);
        impl std::io::Write for Counting {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.set(self.0.get() + b.len());
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let written = Rc::new(Cell::new(0usize));
        let seen: Rc<RefCell<Vec<usize>>> = Rc::new(RefCell::new(Vec::new()));
        let (w, s) = (Rc::clone(&written), Rc::clone(&seen));
        let doc = Doc::Seq(
            3,
            Box::new(move |i| {
                s.borrow_mut().push(w.get());
                Doc::value(serde_json::json!({"i": i, "pad": "0123456789"}))
            }),
        );
        let mut ser = serde_json::Serializer::with_formatter(
            Counting(Rc::clone(&written)),
            serde_json::ser::PrettyFormatter::new(),
        );
        serde::Serialize::serialize(&doc, &mut ser).unwrap();
        let at_build = seen.borrow().clone();
        assert_eq!(at_build.len(), 3);
        assert!(
            at_build.windows(2).all(|w| w[1] > w[0] + 10),
            "each element is built after the previous one is written: {at_build:?}"
        );
    }

    fn snap_with_identities(rows: &[(&str, &str)]) -> CapabilitySnapshot {
        let mut ids = SnapshotIdentityTable {
            stable_ids: Vec::new(),
            display_names: Vec::new(),
        };
        for (id, display) in rows {
            ids.stable_ids.push((*id).into());
            ids.display_names.push((*display).into());
        }
        CapabilitySnapshot {
            identities: ids,
            capability_facts: Vec::new(),
            typed_edges: Vec::new(),
            operation_index: Vec::new(),
            callsite_index: Vec::new(),
            callsite_resolutions: Vec::new(),
            analysis_gaps: Vec::new(),
            coverage: Vec::new(),
            event_declarations: Vec::new(),
            root_classifications: Vec::new(),
            routine_order_frames: None,
            pd_solution: Default::default(),
            boundary: None,
        }
    }

    /// Regression for the HashMap-order bug: `SelectorAmbiguous.candidates` used to
    /// be built by iterating a `HashMap`, so both the bucket order AND the
    /// per-bucket id order were process-random and the printed list reordered
    /// run-to-run. Assert against the ONE known-correct (identity-table insertion)
    /// order — a random order would fail this assertion on nearly every run.
    #[test]
    fn selector_ambiguous_candidates_are_deterministic() {
        let snap = snap_with_identities(&[
            ("app:Codeunit:1#aaa", "Codeunit \"A\"::Run"),
            ("app:Codeunit:2#bbb", "Codeunit \"B\"::Run"),
            ("app:Codeunit:3#ccc", "Codeunit \"C\"::Run"),
            ("app:Codeunit:4#ddd", "Codeunit \"D\"::Run"),
        ]);
        let idx = crate::engine::l5::selector_index::build_selector_indexes(&snap.identities, None);
        let diagnostics =
            resolve_routine_selectors(&["Run".to_string()], &idx, &OwnerIndex::default())
                .expect_err("Run names four routines");
        assert_eq!(diagnostics.len(), 1);
        match &diagnostics[0] {
            FingerprintQueryDiagnostic::SelectorAmbiguous {
                matched_form,
                candidates,
                ..
            } => {
                assert_eq!(matched_form, "one-segment");
                assert_eq!(
                    candidates,
                    &vec![
                        (
                            "app:Codeunit:1#aaa".to_string(),
                            "Codeunit \"A\"::Run".to_string()
                        ),
                        (
                            "app:Codeunit:2#bbb".to_string(),
                            "Codeunit \"B\"::Run".to_string()
                        ),
                        (
                            "app:Codeunit:3#ccc".to_string(),
                            "Codeunit \"C\"::Run".to_string()
                        ),
                        (
                            "app:Codeunit:4#ddd".to_string(),
                            "Codeunit \"D\"::Run".to_string()
                        ),
                    ],
                    "ambiguous candidates must be in deterministic identity order"
                );
            }
            other => panic!("expected SelectorAmbiguous, got {other:?}"),
        }
    }

    /// W8 (spec §14.3.1 item 3): the query holds one DAG cache per root, so
    /// two facts of one root that enter through one first hop build one DAG.
    #[test]
    fn w8_one_dag_per_root_across_its_facts() {
        let snap = root_with_two_inherited_facts();
        crate::engine::l5::digest::DAG_BUILDS.with(|c| c.set(0));
        let result = fingerprint_query(
            &snap,
            &witnessing_filters(),
            None,
            false,
            &OwnerIndex::default(),
            &|_| true,
        );
        let witnessed = result.blocks[0]
            .witnesses
            .iter()
            .filter(|w| w.paths.len() == 1)
            .count();
        assert_eq!(witnessed, 2, "precondition: both inherited facts witnessed");
        assert_eq!(crate::engine::l5::digest::DAG_BUILDS.with(|c| c.get()), 1);
    }

    /// #160 step 9 final panel S1: the production projection
    /// (`project_fingerprint_query_full`) streams a block's facts: each fact's
    /// JSON is built only after the previous fact's bytes are written, so no
    /// resource's facts are collected first. Read through the `FACT_BUILT_AT`
    /// hook. Precondition: one block whose one resource has two witnessed
    /// facts. DISCRIMINATION: `families_doc` collecting a resource's fact JSON
    /// before its `Doc::Seq` fails it.
    #[test]
    fn query_projection_streams_its_facts() {
        let snap = root_with_two_inherited_facts();
        let filters = witnessing_filters();
        let result = fingerprint_query(
            &snap,
            &filters,
            None,
            false,
            &OwnerIndex::default(),
            &|_| true,
        );
        let resources: Vec<usize> = result
            .blocks
            .iter()
            .flat_map(|b| &b.families)
            .flat_map(|f| &f.resources)
            .map(|r| r.fact_indices.len())
            .collect();
        assert_eq!(resources, [2], "precondition: one resource, two facts");
        DOC_WRITTEN.with(|c| c.set(0));
        FACT_BUILT_AT.with(|v| v.borrow_mut().clear());
        let text = project_fingerprint_query_full(&result, "ws", &filters, true, &[], "v", "fp");
        let at = FACT_BUILT_AT.with(|v| v.borrow().clone());
        assert_eq!(at.len(), 2, "precondition: each fact built once");
        // A fact's JSON holds at least its 16-hex `factId`.
        assert!(
            at[1] > at[0] + 16 && at[1] < text.len(),
            "fact 1 is built after fact 0 is written: {at:?}"
        );
    }

    fn witnessing_filters() -> FingerprintFilters {
        FingerprintFilters {
            roots: None,
            routine_selectors: Vec::new(),
            include_inherited: true,
            witness_limit: WitnessLimit::Capped(3),
        }
    }

    /// Root `R` calls `W` once (`cs1`); `W` inserts and modifies `T`, so `R`
    /// inherits both facts through one first hop.
    fn root_with_two_inherited_facts() -> CapabilitySnapshot {
        use crate::engine::l5::snapshot::{
            SnapshotGraphEdge, SnapshotRange, SnapshotRootClassificationSlot, SnapshotSourceAnchor,
        };
        let (r, w) = ("app:Codeunit:1#r", "app:Codeunit:1#w");
        let mut snap = snap_with_identities(&[(r, "R"), (w, "W")]);
        let fact = |subject: &str, op: &str, provenance: &str| SnapFact {
            subject: subject.to_string(),
            op: op.to_string(),
            resource_kind: "table".to_string(),
            resource_id: Some("T".to_string()),
            resource_arg_source: None,
            confidence: "static".to_string(),
            provenance: provenance.to_string(),
            via: "call".to_string(),
            witness_operation_id: (provenance == "direct").then(|| format!("op-{op}")),
            witness_callsite_id: (provenance != "direct").then(|| "cs1".to_string()),
            extra: None,
            witness_first_hop_operation_id: None,
            witness_first_hop_event_id: None,
        };
        snap.capability_facts = vec![
            fact(w, "insert", "direct"),
            fact(w, "modify", "direct"),
            fact(r, "insert", "inherited"),
            fact(r, "modify", "inherited"),
        ];
        snap.typed_edges = vec![SnapshotGraphEdge::DirectCall {
            kind: "direct-call",
            callsite_id: "cs1".to_string(),
            from: r.to_string(),
            to: w.to_string(),
            source_anchor: SnapshotSourceAnchor {
                source_unit_id: "su".to_string(),
                range: SnapshotRange {
                    start_line: 1,
                    start_column: 1,
                    end_line: 1,
                    end_column: 1,
                },
                enclosing_routine_id: r.to_string(),
                syntax_kind: "call".to_string(),
            },
            edge_id: "e-cs1".to_string(),
        }];
        snap.root_classifications = vec![SnapshotRootClassificationSlot {
            routine_id: r.to_string(),
            kinds: vec!["api".to_string()],
            externally_reachable: true,
            source: "test".to_string(),
            confidence: "static".to_string(),
            source_anchor: None,
            config_entry_id: None,
            resolution_status: None,
        }];
        snap
    }
}
