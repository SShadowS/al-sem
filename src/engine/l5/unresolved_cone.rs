//! Port of al-sem `src/digest/unresolved-cone.ts`.
//!
//! BFS over the snapshot's typed-edge graph from a root routine, collecting
//! every callsite-resolution ledger row whose status is NOT "resolved" or
//! "builtin" (open-world honesty: "polymorphic" is always included).
//!
//! The BFS is DETERMINISTIC: the expansion queue is sorted by target routineId
//! at each step (matching the TS sort). Items are deduped by callsiteId string
//! and sorted by (owningRoutine, anchorFile, anchorLine, calleeDisplay).
//!
//! Default caps (must match witness BFS):
//!   maxDepth   = 64
//!   maxVisited = 25_000

use std::collections::{HashMap, HashSet, VecDeque};

use crate::engine::l5::snapshot::{CapabilitySnapshot, SnapshotGraphEdge};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct UnresolvedConeItem {
    /// Workspace-relative forward-slash anchor (may be absent).
    pub callsite_file: Option<String>,
    pub callsite_line: Option<u32>,
    pub callsite_column: Option<u32>,
    pub callee_display: String,
    pub status: String,
    pub candidates: Option<Vec<String>>,
    pub open_world: Option<bool>,
    pub owning_routine: String,
    pub owning_routine_display: String,
}

#[derive(Debug, Clone)]
pub struct UnresolvedTraversal {
    pub truncated: bool,
    pub max_depth: u32,
    pub visited_routines: usize,
}

pub struct UnresolvedConeResult {
    pub items: Vec<UnresolvedConeItem>,
    pub traversal: UnresolvedTraversal,
    /// All visited routine stable IDs (for gapsInCone computation).
    pub visited_ids: HashSet<String>,
}

pub(crate) const DEFAULT_MAX_DEPTH: u32 = 64;
const DEFAULT_MAX_VISITED: usize = 25_000;

// ---------------------------------------------------------------------------
// unresolvedCone
// ---------------------------------------------------------------------------

/// Normalize a source file path for the callsite anchor.
/// Mirrors TS `normalizeAnchorPath(ce.sourceFile, workspaceRoot)` where
/// workspaceRoot is the empty string (no filesystem prefix to strip) — so
/// this is just backslash normalization. The `ws:` / `app:` scheme prefixes
/// are PRESERVED (the caller passed no workspace root to strip against).
fn normalize_anchor_path(p: &str) -> String {
    p.replace('\\', "/")
}

pub fn unresolved_cone(snap: &CapabilitySnapshot, root: &str) -> UnresolvedConeResult {
    let max_depth = DEFAULT_MAX_DEPTH;
    let max_visited = DEFAULT_MAX_VISITED;

    // --- displayById (identities table) ---
    let mut display_by_id: HashMap<&str, &str> = HashMap::new();
    for i in 0..snap.identities.stable_ids.len() {
        let id = snap
            .identities
            .stable_ids
            .get(i)
            .map(|s| s.as_str())
            .unwrap_or("");
        let name = snap
            .identities
            .display_names
            .get(i)
            .map(|s| s.as_str())
            .unwrap_or("");
        if !id.is_empty() {
            display_by_id.insert(id, name);
        }
    }

    // --- callsiteById (callsiteIndex) ---
    let mut callsite_by_id: HashMap<&str, &crate::engine::l5::snapshot::SnapshotCallsiteEvidence> =
        HashMap::new();
    for cs in &snap.callsite_index {
        callsite_by_id.insert(cs.callsite_id.as_str(), cs);
    }

    // --- outgoing edges (typed-edge → to) for BFS expansion ---
    // Only edges that have a `to` (i.e. all except object-run-unresolved).
    let mut outgoing: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in &snap.typed_edges {
        let from = edge_from(edge);
        let to = edge_to(edge);
        if let Some(to) = to {
            outgoing.entry(from).or_default().push(to);
        }
    }
    // Sort each bucket for determinism.
    for bucket in outgoing.values_mut() {
        bucket.sort_unstable();
    }

    // --- ledger by routine ---
    let mut ledger_by_routine: HashMap<
        &str,
        Vec<&crate::engine::l5::snapshot::SnapshotCallsiteResolution>,
    > = HashMap::new();
    for row in &snap.callsite_resolutions {
        ledger_by_routine
            .entry(row.from.as_str())
            .or_default()
            .push(row);
    }

    // --- BFS ---
    let mut collected: HashSet<String> = HashSet::new(); // routine ids whose ledger was harvested
    let mut visited: HashSet<String> = HashSet::new();
    let mut truncated = false;

    // items deduped by callsiteId
    let mut items_by_csid: HashMap<String, UnresolvedConeItem> = HashMap::new();

    // Queue: (routineId, depth)
    let mut queue: VecDeque<(String, u32)> = VecDeque::new();
    visited.insert(root.to_string());
    queue.push_back((root.to_string(), 0));

    while let Some((routine, depth)) = queue.pop_front() {
        // Harvest ledger rows
        if !collected.contains(&routine) {
            collected.insert(routine.clone());
            let rows = ledger_by_routine
                .get(routine.as_str())
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            for row in rows {
                // Skip "resolved" and "builtin" (open-world rule)
                if row.status == "resolved" || row.status == "builtin" {
                    continue;
                }
                let cs_key = row.callsite_id.clone();
                if items_by_csid.contains_key(&cs_key) {
                    continue; // dedupe
                }

                // Join callsiteIndex for anchor
                let cs_ev = callsite_by_id.get(row.callsite_id.as_str()).copied();
                let (callsite_file, callsite_line, callsite_column) = match cs_ev {
                    Some(ev) => (
                        Some(normalize_anchor_path(&ev.source_file)),
                        Some(ev.start_line),
                        Some(ev.start_column),
                    ),
                    None => (None, None, None),
                };

                let candidates = if row
                    .candidates
                    .as_ref()
                    .map(|v| !v.is_empty())
                    .unwrap_or(false)
                {
                    row.candidates.clone()
                } else {
                    None
                };

                let open_world = if row.open_world == Some(true) {
                    Some(true)
                } else {
                    None
                };

                items_by_csid.insert(
                    cs_key,
                    UnresolvedConeItem {
                        callsite_file,
                        callsite_line,
                        callsite_column,
                        callee_display: row.callee_display.clone(),
                        status: row.status.clone(),
                        candidates,
                        open_world,
                        owning_routine: routine.clone(),
                        owning_routine_display: display_by_id
                            .get(routine.as_str())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| routine.clone()),
                    },
                );
            }
        }

        // Don't expand if at maxDepth
        if depth >= max_depth {
            let out = outgoing
                .get(routine.as_str())
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            if out.iter().any(|to| !visited.contains(*to)) {
                truncated = true;
            }
            continue;
        }

        // Expand: enqueue unvisited neighbors (sorted for determinism)
        let out = outgoing
            .get(routine.as_str())
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        for &to in out {
            if visited.contains(to) {
                continue;
            }
            if visited.len() >= max_visited {
                truncated = true;
                break;
            }
            visited.insert(to.to_string());
            queue.push_back((to.to_string(), depth + 1));
        }
    }

    // Sort items: (owningRoutine, anchorFile, anchorLine, calleeDisplay)
    let mut all_items: Vec<UnresolvedConeItem> = items_by_csid.into_values().collect();
    all_items.sort_by(|a, b| {
        let r = a.owning_routine.cmp(&b.owning_routine);
        if r != std::cmp::Ordering::Equal {
            return r;
        }
        let af = a.callsite_file.as_deref().unwrap_or("");
        let bf = b.callsite_file.as_deref().unwrap_or("");
        let r = af.cmp(bf);
        if r != std::cmp::Ordering::Equal {
            return r;
        }
        let al = a.callsite_line.unwrap_or(0);
        let bl = b.callsite_line.unwrap_or(0);
        if al != bl {
            return al.cmp(&bl);
        }
        a.callee_display.cmp(&b.callee_display)
    });

    UnresolvedConeResult {
        items: all_items,
        traversal: UnresolvedTraversal {
            truncated,
            max_depth,
            visited_routines: visited.len(),
        },
        visited_ids: visited,
    }
}

// ---------------------------------------------------------------------------
// Edge accessors (replicate from digest.rs — no pub access there)
// ---------------------------------------------------------------------------

fn edge_from(e: &SnapshotGraphEdge) -> &str {
    match e {
        SnapshotGraphEdge::DirectCall { from, .. }
        | SnapshotGraphEdge::VariableTypedCall { from, .. }
        | SnapshotGraphEdge::InterfaceDispatch { from, .. }
        | SnapshotGraphEdge::ObjectRunResolved { from, .. }
        | SnapshotGraphEdge::ObjectRunUnresolved { from, .. }
        | SnapshotGraphEdge::EventDispatch { from, .. } => from,
    }
}

fn edge_to(e: &SnapshotGraphEdge) -> Option<&str> {
    match e {
        SnapshotGraphEdge::DirectCall { to, .. }
        | SnapshotGraphEdge::VariableTypedCall { to, .. }
        | SnapshotGraphEdge::InterfaceDispatch { to, .. }
        | SnapshotGraphEdge::ObjectRunResolved { to, .. }
        | SnapshotGraphEdge::EventDispatch { to, .. } => Some(to),
        SnapshotGraphEdge::ObjectRunUnresolved { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::l5::snapshot::{
        CapabilitySnapshot, SnapshotCallsiteEvidence, SnapshotCallsiteResolution,
        SnapshotIdentityTable, SnapshotRange, SnapshotSourceAnchor,
    };

    fn empty_snap() -> CapabilitySnapshot {
        CapabilitySnapshot {
            identities: SnapshotIdentityTable {
                stable_ids: Vec::new(),
                display_names: Vec::new(),
            },
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
        }
    }

    fn cs_ev(id: &str, routine: &str, file: &str, line: u32) -> SnapshotCallsiteEvidence {
        SnapshotCallsiteEvidence {
            callsite_id: id.into(),
            routine: routine.into(),
            source_file: file.into(),
            start_line: line,
            start_column: 4,
            end_line: line,
            end_column: 20,
            callee_display: "Callee".into(),
            control_context: None,
            order: None,
            under_asserterror: None,
        }
    }

    fn resolution(
        cs_id: &str,
        from: &str,
        callee: &str,
        status: &str,
        candidates: Option<Vec<String>>,
        open_world: Option<bool>,
    ) -> SnapshotCallsiteResolution {
        SnapshotCallsiteResolution {
            callsite_id: cs_id.into(),
            from: from.into(),
            callee_display: callee.into(),
            dispatch_kind: "interface".into(),
            status: status.into(),
            resolved_edges: Vec::new(),
            candidates,
            open_world,
            unresolved_candidates: None,
            result_consumed: None,
            under_asserterror: None,
        }
    }

    #[test]
    fn polymorphic_row_with_candidates_and_open_world_included() {
        // ORACLE: a polymorphic (interface-dispatch) ledger row is INCLUDED in the
        // cone even though "polymorphic" is a distinct (non-resolved) status, and it
        // carries candidates[] + openWorld:true. 0 corpus coverage.
        let mut snap = empty_snap();
        snap.identities
            .stable_ids
            .push("app:Codeunit:1#root".into());
        snap.identities
            .display_names
            .push("Codeunit X::Root".into());
        snap.callsite_index
            .push(cs_ev("cs1", "app:Codeunit:1#root", "ws:src/X.al", 12));
        snap.callsite_resolutions.push(resolution(
            "cs1",
            "app:Codeunit:1#root",
            "IFoo.Bar",
            "polymorphic",
            Some(vec![
                "app:Codeunit:2#impl".into(),
                "app:Codeunit:3#impl".into(),
            ]),
            Some(true),
        ));

        let r = unresolved_cone(&snap, "app:Codeunit:1#root");
        assert_eq!(r.items.len(), 1, "polymorphic row must be included");
        let it = &r.items[0];
        assert_eq!(it.status, "polymorphic");
        assert_eq!(it.open_world, Some(true));
        assert_eq!(
            it.candidates.as_ref().map(|v| v.len()),
            Some(2),
            "candidates carried"
        );
        assert_eq!(it.callsite_file.as_deref(), Some("ws:src/X.al"));
        assert_eq!(it.callsite_line, Some(12));
        assert_eq!(it.owning_routine, "app:Codeunit:1#root");
        assert_eq!(it.owning_routine_display, "Codeunit X::Root");
    }

    #[test]
    fn resolved_and_builtin_rows_excluded() {
        let mut snap = empty_snap();
        snap.identities.stable_ids.push("r#1".into());
        snap.identities.display_names.push("R::One".into());
        snap.callsite_resolutions
            .push(resolution("csA", "r#1", "A", "resolved", None, None));
        snap.callsite_resolutions
            .push(resolution("csB", "r#1", "B", "builtin", None, None));
        let r = unresolved_cone(&snap, "r#1");
        assert!(r.items.is_empty(), "resolved + builtin are excluded");
    }

    #[test]
    fn depth_cap_truncation_only_on_unvisited_edge() {
        // ORACLE (#13): with maxDepth forced low via a deep chain, truncated=true only
        // when an outgoing edge points to an UNVISITED node. We build a chain longer
        // than DEFAULT_MAX_DEPTH so the cap trips on an unvisited target.
        let mut snap = empty_snap();
        // Build a linear chain r0 -> r1 -> ... -> r(N) with N > maxDepth.
        let n = (DEFAULT_MAX_DEPTH as usize) + 5;
        for i in 0..=n {
            let id = format!("r#{i}");
            snap.identities.stable_ids.push(id.clone());
            snap.identities.display_names.push(format!("R::{i}"));
        }
        for i in 0..n {
            let cs = format!("cs{i}");
            snap.typed_edges.push(SnapshotGraphEdge::DirectCall {
                kind: "direct-call",
                callsite_id: cs.clone(),
                from: format!("r#{i}"),
                to: format!("r#{}", i + 1),
                source_anchor: SnapshotSourceAnchor {
                    source_unit_id: "ws:src/Chain.al".into(),
                    range: SnapshotRange {
                        start_line: i as u32,
                        start_column: 0,
                        end_line: i as u32,
                        end_column: 1,
                    },
                    enclosing_routine_id: format!("r#{i}"),
                    syntax_kind: "call_expression".into(),
                },
                edge_id: format!("e{i}"),
            });
        }
        let r = unresolved_cone(&snap, "r#0");
        assert!(
            r.traversal.truncated,
            "a chain longer than maxDepth must truncate on an unvisited edge"
        );
        assert_eq!(r.traversal.max_depth, DEFAULT_MAX_DEPTH);
    }

    #[test]
    fn no_truncation_when_all_targets_visited() {
        // A short chain entirely within maxDepth → not truncated.
        let mut snap = empty_snap();
        for i in 0..=2 {
            snap.identities.stable_ids.push(format!("r#{i}"));
            snap.identities.display_names.push(format!("R::{i}"));
        }
        for i in 0..2 {
            snap.typed_edges.push(SnapshotGraphEdge::DirectCall {
                kind: "direct-call",
                callsite_id: format!("cs{i}"),
                from: format!("r#{i}"),
                to: format!("r#{}", i + 1),
                source_anchor: SnapshotSourceAnchor {
                    source_unit_id: "ws:src/Chain.al".into(),
                    range: SnapshotRange {
                        start_line: 0,
                        start_column: 0,
                        end_line: 0,
                        end_column: 1,
                    },
                    enclosing_routine_id: format!("r#{i}"),
                    syntax_kind: "call_expression".into(),
                },
                edge_id: format!("e{i}"),
            });
        }
        let r = unresolved_cone(&snap, "r#0");
        assert!(!r.traversal.truncated);
    }
}
