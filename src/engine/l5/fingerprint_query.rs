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
//!   → JSON envelope (kind `fingerprint-query`, schemaVersion `1.2.0`).
//! - `--format human` → `fingerprint_query` → `format_fingerprint_human`.
//!
//! ## Witness modes
//!
//! - `false`   → no reconstruction; facts have **no** `witness` field in JSON.
//! - `0`       → witness objects present but zero paths.
//! - `N`       → cap at N paths (1–256).
//! - `"all"`   → uncapped (HARD_PATH_CAP = 256).

use std::collections::HashMap;

use crate::engine::gate::format_json::serialize_document_value;
use crate::engine::ids::sha256_hex;
use crate::engine::l5::digest::{
    FingerprintIndexesPub, HumanHop, ProjectedPath, QueryWitnessHop, TerminalHopInfo,
    build_fingerprint_indexes_pub, reconstruct_witness_paths_pub,
};
use crate::engine::l5::selector_index::{build_selector_indexes, resolve_selector};
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

fn split_object_routine(display: &str) -> (&str, &str) {
    match display.rfind("::") {
        Some(idx) => (&display[..idx], &display[idx + 2..]),
        None => ("", display),
    }
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

fn build_block(
    rid: &str,
    root: &crate::engine::l5::snapshot::SnapshotRootClassificationSlot,
    idx: &FingerprintIndexesPub,
    filters: &FingerprintFilters,
) -> FingerprintBlock {
    let display_full = idx
        .routine_display_by_id
        .get(rid)
        .cloned()
        .unwrap_or_else(|| rid.to_string());
    let (object_display, routine_display) = split_object_routine(&display_full);
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

    // mayCommit.
    let commit_fi = rendered_facts.iter().position(|f| f.op == "commit");
    let may_commit = BlockCommit {
        presence: if commit_fi.is_some() {
            "yes"
        } else if coverage.direct_status == "complete" {
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

    // Witnesses.
    let witnesses: Vec<BlockWitness> = match filters.witness_limit {
        WitnessLimit::Disabled => {
            // witness reconstruction disabled; no BlockWitness entries.
            Vec::new()
        }
        WitnessLimit::Capped(0) => {
            // objects present but zero paths.
            rendered_facts
                .iter()
                .enumerate()
                .map(|(fi, _)| BlockWitness {
                    fact_index: fi,
                    paths: Vec::new(),
                    truncated: false,
                    incomplete: false,
                    diagnostics: Vec::new(),
                })
                .collect()
        }
        WitnessLimit::Capped(cap) => rendered_facts
            .iter()
            .enumerate()
            .map(|(fi, f)| {
                let outcome = reconstruct_witness_paths_pub(rid, f, idx, cap);
                BlockWitness {
                    fact_index: fi,
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
            .collect(),
        WitnessLimit::All => rendered_facts
            .iter()
            .enumerate()
            .map(|(fi, f)| {
                let outcome = reconstruct_witness_paths_pub(rid, f, idx, 256);
                BlockWitness {
                    fact_index: fi,
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
            .collect(),
    };

    FingerprintBlock {
        routine_id: rid.to_string(),
        object_id,
        object_display: object_display.to_string(),
        routine_display: routine_display.to_string(),
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
pub fn fingerprint_query(
    snap: &CapabilitySnapshot,
    filters: &FingerprintFilters,
    roots_config_ignored: bool,
) -> FingerprintQueryResult {
    // Build shared indexes (reuse B1's public entry point).
    let idx = build_fingerprint_indexes_pub(snap);

    // Selector index (5-form resolveSelector cascade) — built from the identity
    // table's source Vec, deterministic by construction. Shared with digest_cli.
    let selector_idx = build_selector_indexes(snap);

    let mut diagnostics: Vec<FingerprintQueryDiagnostic> = Vec::new();

    // Resolve routine selectors.
    let mut resolved_set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut any_selector_failed = false;
    for sel in &filters.routine_selectors {
        let (matches, matched_form) = resolve_selector(sel, &selector_idx);
        if matches.is_empty() {
            diagnostics.push(FingerprintQueryDiagnostic::SelectorUnresolved {
                selector: sel.clone(),
            });
            any_selector_failed = true;
        } else if matches.len() >= 2 {
            // candidates: (stableId, display) — display falls back to "" (TS:
            // `routineDisplayById.get(id) ?? ""`), NOT to the id.
            let candidates = matches
                .iter()
                .take(MAX_AMBIGUOUS_CANDIDATES)
                .map(|id| {
                    (
                        id.clone(),
                        idx.routine_display_by_id
                            .get(id)
                            .cloned()
                            .unwrap_or_default(),
                    )
                })
                .collect();
            diagnostics.push(FingerprintQueryDiagnostic::SelectorAmbiguous {
                selector: sel.clone(),
                matched_form: matched_form.to_string(),
                candidates,
            });
            any_selector_failed = true;
        } else {
            resolved_set.insert(matches.into_iter().next().unwrap());
        }
    }

    let total_classifications = snap.root_classifications.len();

    if any_selector_failed {
        return FingerprintQueryResult {
            blocks: Vec::new(),
            diagnostics,
            total_classifications,
            rendered_blocks: 0,
            roots_config_ignored,
        };
    }

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
            if !filters.routine_selectors.is_empty()
                && !resolved_set.contains(r.routine_id.as_str())
            {
                return false;
            }
            true
        })
        .collect();

    // Build blocks.
    let mut blocks: Vec<FingerprintBlock> = Vec::new();
    for root in root_pool {
        let block = build_block(&root.routine_id, root, &idx, filters);
        blocks.push(block);
    }

    // Sort blocks by routine_id.
    blocks.sort_by(|a, b| a.routine_id.cmp(&b.routine_id));

    let rendered_blocks = blocks.len();
    FingerprintQueryResult {
        blocks,
        diagnostics,
        total_classifications,
        rendered_blocks,
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
// project_fingerprint_query — JSON envelope (fingerprint-query, 1.2.0)
// Mirrors `projectFingerprintQuery` in `src/contracts/fingerprint-query.ts`.
// ---------------------------------------------------------------------------

pub const FINGERPRINT_QUERY_CONTRACT_VERSION: &str = "1.2.0";

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

/// Project one block witness path set into the JSON contract form.
fn project_witness_json(
    bw: &BlockWitness,
    fact: &SnapFact,
    workspace_root: &str,
    operation_index: &HashMap<String, &crate::engine::l5::snapshot::SnapshotOperationEvidence>,
) -> serde_json::Value {
    // Terminal evidence (anchor keys: column, excerpt, file, line, sourceKind — alphabetical).
    let evidence = if let Some(op_id) = &fact.witness_operation_id {
        if let Some(ev) = operation_index.get(op_id.as_str()) {
            let file = normalize_anchor_path(&ev.source_file);
            // Sorted: column, excerpt, file, line, sourceKind
            let mut anchor = serde_json::Map::new();
            anchor.insert("column".into(), ev.start_column.into());
            anchor.insert("excerpt".into(), ev.display_text.clone().into());
            anchor.insert("file".into(), file.into());
            anchor.insert("line".into(), ev.start_line.into());
            anchor.insert("sourceKind".into(), "source".into());
            // Sorted: anchor, operationId
            let mut ev_obj = serde_json::Map::new();
            ev_obj.insert("anchor".into(), serde_json::Value::Object(anchor));
            ev_obj.insert("operationId".into(), op_id.clone().into());
            Some(serde_json::Value::Object(ev_obj))
        } else {
            None
        }
    } else {
        None
    };

    // Projected paths (already projected via reconstruct_witness_paths_pub).
    let projected_paths: Vec<serde_json::Value> = bw
        .paths
        .iter()
        .map(|path| {
            let hops: Vec<serde_json::Value> = path
                .query_hops
                .iter()
                .map(|h| query_hop_to_json(h, workspace_root))
                .collect();
            serde_json::json!({"hops": hops})
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

/// Serialize the fingerprint-query envelope: sorted keys, null-drop,
/// 2-space indent, trailing newline. Mirrors `serializeDocument` (contracts/document.ts).
fn serialize_fingerprint_envelope(v: serde_json::Value) -> String {
    // Use the existing sorted-JSON serializer that the digest pipeline uses.
    // `serialize_document_value` always appends '\n'.
    serialize_document_value(v)
}

/// Full `projectFingerprintQuery` with all parameters injected.
#[allow(clippy::too_many_arguments)] // query-document fields; grouping would obscure
pub fn project_fingerprint_query_full(
    result: &FingerprintQueryResult,
    snap: &CapabilitySnapshot,
    workspace_root: &str,
    filters: &FingerprintFilters,
    deterministic: bool,
    analyzer_diagnostics: &[(String, String, String)],
    driver_version: &str,
    workspace_fingerprint: &str,
) -> String {
    // Operation index (borrowed from snap; lives as long as snap).
    let op_index: HashMap<String, &crate::engine::l5::snapshot::SnapshotOperationEvidence> = snap
        .operation_index
        .iter()
        .map(|e| (e.operation_id.clone(), e))
        .collect();

    // Project blocks.
    let mut blocks_arr: Vec<serde_json::Value> = Vec::new();
    for block in &result.blocks {
        let witness_by_fi: HashMap<usize, &BlockWitness> =
            block.witnesses.iter().map(|w| (w.fact_index, w)).collect();

        // Families.
        let mut families_arr: Vec<serde_json::Value> = Vec::new();
        for family in &block.families {
            let mut resources_arr: Vec<serde_json::Value> = Vec::new();
            for res in &family.resources {
                let mut facts_arr: Vec<serde_json::Value> = Vec::new();
                for &fi in &res.fact_indices {
                    let fact = &block.rendered_facts[fi];
                    let fact_id = fact_id_of(fact);
                    let bw = witness_by_fi.get(&fi).copied();

                    let witness_val =
                        bw.map(|bw| project_witness_json(bw, fact, workspace_root, &op_index));

                    let mut fj = serde_json::Map::new();
                    fj.insert("factId".into(), fact_id.into());
                    fj.insert("op".into(), fact.op.clone().into());
                    fj.insert("resourceKind".into(), fact.resource_kind.clone().into());
                    if let Some(ref rid) = fact.resource_id {
                        fj.insert("resourceId".into(), rid.clone().into());
                    }
                    fj.insert("provenance".into(), fact.provenance.clone().into());
                    fj.insert("confidence".into(), fact.confidence.clone().into());
                    fj.insert("via".into(), fact.via.clone().into());
                    if let Some(w) = witness_val {
                        fj.insert("witness".into(), w);
                    }
                    facts_arr.push(serde_json::Value::Object(fj));
                }
                let mut rj = serde_json::Map::new();
                if let Some(ref id) = res.id {
                    rj.insert("id".into(), id.clone().into());
                }
                rj.insert("display".into(), res.display.clone().into());
                rj.insert(
                    "ops".into(),
                    serde_json::Value::Array(res.ops.iter().map(|o| o.clone().into()).collect()),
                );
                rj.insert("facts".into(), serde_json::Value::Array(facts_arr));
                resources_arr.push(serde_json::Value::Object(rj));
            }
            let mut fam = serde_json::Map::new();
            fam.insert("resourceKind".into(), family.kind.clone().into());
            fam.insert("coneCoverage".into(), family.cone_coverage.clone().into());
            fam.insert("resources".into(), serde_json::Value::Array(resources_arr));
            families_arr.push(serde_json::Value::Object(fam));
        }

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

        let mut bj = serde_json::Map::new();
        bj.insert("routine".into(), routine_ref);
        bj.insert(
            "rootKinds".into(),
            serde_json::Value::Array(block.kinds.iter().map(|k| k.clone().into()).collect()),
        );
        bj.insert(
            "classificationSource".into(),
            block.classification_source.clone().into(),
        );
        bj.insert("coverage".into(), cov_obj);
        bj.insert("families".into(), serde_json::Value::Array(families_arr));
        bj.insert(
            "mayCommit".into(),
            serde_json::Value::Object(may_commit_obj),
        );
        bj.insert(
            "dispatch".into(),
            serde_json::json!({
                "resolved": resolved_arr,
                "unresolved": unresolved_arr,
            }),
        );
        bj.insert(
            "requiredPermissions".into(),
            serde_json::Value::Array(perms_arr),
        );

        blocks_arr.push(serde_json::Value::Object(bj));
    }

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
    let payload = serde_json::json!({
        "workspaceFingerprint": workspace_fingerprint,
        "filters": serde_json::Value::Object(filters_map),
        "blocks": blocks_arr,
        "diagnostics": payload_diags,
        "summary": {
            "totalClassifications": result.total_classifications,
            "renderedBlocks": result.rendered_blocks,
            "rootsConfigIgnored": result.roots_config_ignored,
        }
    });

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

    let envelope = serde_json::json!({
        "kind": "fingerprint-query",
        "schemaVersion": FINGERPRINT_QUERY_CONTRACT_VERSION,
        "alsemVersion": driver_version,
        "deterministic": deterministic,
        "generatedAt": generated_at,
        "diagnostics": diags_arr,
        "payload": payload,
    });

    serialize_fingerprint_envelope(envelope)
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
        "{}::{}  [{}]{marker}",
        block.object_display,
        block.routine_display,
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
        let filters = FingerprintFilters {
            roots: None,
            routine_selectors: vec!["Run".to_string()],
            include_inherited: true,
            witness_limit: WitnessLimit::Disabled,
        };
        let result = fingerprint_query(&snap, &filters, false);
        assert_eq!(result.diagnostics.len(), 1);
        match &result.diagnostics[0] {
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
}
