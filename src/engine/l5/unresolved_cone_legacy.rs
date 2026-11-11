//! TLS-17's oracle: `unresolved_cone` as it was before its snapshot indexes
//! moved into `ConeIndexes` (verbatim from commit 741f807a, `unresolved_cone.rs`
//! lines 98-290), with its two limits as parameters. Test-only: the
//! production search (`unresolved_cone_in`) is compared with it.

use super::*;

pub(crate) fn unresolved_cone_legacy(
    snap: &CapabilitySnapshot,
    root: &str,
    max_depth: u32,
    max_visited: usize,
) -> UnresolvedConeResult {
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
    for edge in snap.typed_edges.iter() {
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
    for row in snap.callsite_resolutions.iter() {
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
                if !is_listed(row) {
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
                    cs_key.clone(),
                    UnresolvedConeItem {
                        callsite_id: cs_key,
                        callsite_file,
                        callsite_line,
                        callsite_column,
                        callee_display: row.callee_display.clone(),
                        status: row.status.clone(),
                        candidates,
                        open_world,
                        closed_world_followable: row
                            .candidates
                            .as_ref()
                            .is_some_and(|c| !c.is_empty() && c.len() == row.resolved_edges.len())
                            && row.unresolved_candidates.as_ref().is_none_or(Vec::is_empty),
                        publisher_scoped: row.publisher_scoped,
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

    let mut all_items: Vec<UnresolvedConeItem> = items_by_csid.into_values().collect();
    all_items.sort_by(item_order);
    let dependency_routines = snap.boundary.as_ref().map_or(0, |b| {
        visited
            .iter()
            .filter(|v| v.as_str() != root && b.dependency_app.contains_key(v.as_str()))
            .count()
    });

    UnresolvedConeResult {
        items: all_items,
        traversal: UnresolvedTraversal {
            truncated,
            max_depth,
            visited_routines: visited.len(),
            dependency_routines,
        },
        visited_ids: visited,
    }
}
