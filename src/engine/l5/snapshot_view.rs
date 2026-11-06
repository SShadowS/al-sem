//! The view-independent half of a tree's per-view sections (plan
//! `2026-11-03-context-server-remaining.md` item L4a), derived once with the
//! [`TreeBase`](super::TreeBase) — per revision on the resident path — and the
//! per-view splice over it.
//!
//! Two views of one composition differ only by their subjects' raw inherited
//! facts (design §6.11; the plan's §1 Q3). What the whole per-view derivation
//! recomputed for every routine is computed here once:
//!
//! - **The evidence candidates.** `compute_routine_order` per row (the whole
//!   derivation ran it three times per view, and once more for the order
//!   frames), and the evidence rows of every operation and call site the base
//!   already references: every covered routine's direct witnesses and the
//!   typed edges' anchors. A view adds only the sites its subjects' inherited
//!   facts name and no base reference does; their rows are found through
//!   `op_rest` / `site_rest` (an id's hash → the rows holding it) and
//!   recomputed for those rows alone.
//! - **The direct-fact table.** Every covered routine's direct rows, interned
//!   once, each subject's run sorted. A view rebuilds only the runs of its
//!   subjects that carry raw facts (their direct and raw rows, re-sorted) and
//!   copies every other row's handle; positions after a rebuilt run shift by
//!   the raw rows it gained.
//! - **The pd arena's maps.** Each cone edge's snapshot edge and each
//!   producer's owner-direct row (a base position). A view walks only its
//!   subjects' state closure.
//!
//! A view's sections are byte-equal to the whole derivation's
//! (`derive_operation_evidence`, `derive_callsite_evidence`,
//! `derive_capability_facts`, `derive_pd_arena`), which the cut snapshot
//! (`compose_snapshot_from_base`) still runs.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use super::{
    CollidedRows, EdgeIx, FactRef, InheritedSlice, ModelRoutine, NO_HOP, OperationOrder, PdArena,
    PdEdgeKey, RoutineOrder, RoutineOrderFrames, ScopeFrame, SkippedEdgeKeys, SnapRep, SnapTables,
    SnapTablesBuilder, SnapshotCallsiteEvidence, SnapshotCapabilityFact, SnapshotGraphEdge,
    SnapshotOperationEvidence, StateNode, TreeInputs, capability_fact_sort_fields,
    compute_routine_order, render_callee, stable_routine_id,
};
use crate::engine::l4::pd_solve::ConeArena;

/// The view-independent half of a tree's per-view sections (module doc).
#[derive(Clone)]
pub(super) struct ViewBase {
    /// Internal routine id → stable routine id (the base's
    /// `routine_to_stable_of`), so a view need not rebuild it.
    pub(super) routine_to_stable: HashMap<String, String>,
    pub(super) facts: FactBase,
    pub(super) evidence: EvidenceBase,
    pub(super) pd: PdBase,
}

// ---------------------------------------------------------------------------
// Evidence
// ---------------------------------------------------------------------------

/// The operation and call-site evidence every view carries, and where the
/// rest of the candidates are.
#[derive(Clone)]
pub(super) struct EvidenceBase {
    /// The operation evidence of every id the base references, sorted by id
    /// (each id's LAST candidate, as the whole derivation's map keeps).
    ops: Vec<SnapshotOperationEvidence>,
    /// `(hash of the id, row)` of every operation candidate whose id the
    /// base does not reference, sorted.
    op_rest: Vec<(u64, u32)>,
    /// The call-site evidence of every id the base references, sorted by
    /// `(id, row)` and deduplicated.
    sites: Vec<SnapshotCallsiteEvidence>,
    /// As `op_rest`, for call sites.
    site_rest: Vec<(u64, u32)>,
}

fn id_hash(id: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    h.finish()
}

fn row_u32(ri: usize) -> u32 {
    u32::try_from(ri).expect("routine rows fit u32")
}

/// Row `ri`'s operation-evidence candidates in the whole derivation's order
/// (`derive_operation_evidence`): its operation sites, then its record
/// operations (each replaces an operation site of the same id, keeping that
/// site's control context and order). An id `keep` accepts goes to `out`;
/// any other id to `skip`.
#[allow(clippy::too_many_arguments)]
fn op_candidates(
    ri: usize,
    r: &ModelRoutine,
    order: &RoutineOrder,
    stable: &str,
    collided: &CollidedRows<'_>,
    keep: &dyn Fn(&str) -> bool,
    out: &mut Vec<(String, SnapshotOperationEvidence)>,
    skip: &mut dyn FnMut(&str),
) {
    let op_order_by_id: HashMap<&str, &OperationOrder> = order
        .operation_sites
        .iter()
        .filter_map(|op| op.order.as_ref().map(|o| (op.id.as_str(), o)))
        .collect();
    for op in &order.operation_sites {
        if !keep(&op.id) {
            skip(&op.id);
            continue;
        }
        let a = &op.source_anchor;
        let entry = SnapshotOperationEvidence {
            operation_id: op.id.clone(),
            routine: stable.to_string(),
            source_file: a.source_unit_id.clone(),
            start_line: a.start_line,
            start_column: a.start_column,
            end_line: a.end_line,
            end_column: a.end_column,
            display_text: op.kind.clone(),
            control_context: op.control_context.clone(),
            order: collided.order(ri, op.order),
            under_asserterror: if op.under_asserterror == Some(true) {
                Some(true)
            } else {
                None
            },
        };
        out.push((op.id.clone(), entry));
    }
    for ro in &r.record_operations {
        if !keep(&ro.id) {
            skip(&ro.id);
            continue;
        }
        let matching = r.operation_sites.iter().find(|op| op.id == ro.id);
        let a = &ro.source_anchor;
        let display = format!(
            "{}.{}",
            if ro.record_variable_name.is_empty() {
                "?".to_string()
            } else {
                ro.record_variable_name.clone()
            },
            ro.op
        );
        let entry = SnapshotOperationEvidence {
            operation_id: ro.id.clone(),
            routine: stable.to_string(),
            source_file: a.source_unit_id.clone(),
            start_line: a.start_line,
            start_column: a.start_column,
            end_line: a.end_line,
            end_column: a.end_column,
            display_text: display,
            control_context: matching.and_then(|op| op.control_context.clone()),
            order: collided.order(
                ri,
                matching.and_then(|_| op_order_by_id.get(ro.id.as_str()).map(|o| **o)),
            ),
            under_asserterror: None,
        };
        out.push((ro.id.clone(), entry));
    }
}

/// Row `ri`'s call-site evidence candidates (`derive_callsite_evidence`): an
/// id `keep` accepts goes to `out`, any other to `skip`.
fn site_candidates(
    ri: usize,
    order: &RoutineOrder,
    stable: &str,
    collided: &CollidedRows<'_>,
    keep: &dyn Fn(&str) -> bool,
    out: &mut Vec<SnapshotCallsiteEvidence>,
    skip: &mut dyn FnMut(&str),
) {
    for cs in &order.call_sites {
        if !keep(&cs.id) {
            skip(&cs.id);
            continue;
        }
        let a = &cs.source_anchor;
        out.push(SnapshotCallsiteEvidence {
            callsite_id: cs.id.clone(),
            routine: stable.to_string(),
            source_file: a.source_unit_id.clone(),
            start_line: a.start_line,
            start_column: a.start_column,
            end_line: a.end_line,
            end_column: a.end_column,
            callee_display: render_callee(&cs.callee),
            control_context: cs.control_context.clone(),
            order: collided.order(ri, cs.order),
            under_asserterror: if cs.under_asserterror == Some(true) {
                Some(true)
            } else {
                None
            },
        });
    }
}

/// The whole derivation's call-site order: by id, then by full value (only a
/// collided id's rows tie on the id); equal copies collapse.
fn sort_sites(sites: &mut Vec<SnapshotCallsiteEvidence>) {
    sites.sort_by(|a, b| a.callsite_id.cmp(&b.callsite_id).then_with(|| a.cmp(b)));
    sites.dedup();
}

/// The evidence base and the routine order frames (a whole section): one
/// `compute_routine_order` per row, rows in parallel on the big-stack pool
/// (the order walk recurses over the statement tree).
pub(super) fn derive_evidence_base(
    inp: &TreeInputs<'_>,
    collided: &CollidedRows<'_>,
) -> (EvidenceBase, Option<RoutineOrderFrames>) {
    use rayon::prelude::*;
    // The base half of the referenced sets: every covered routine's direct
    // witnesses and the typed edges' anchors. The whole derivation's sets add
    // the subjects' inherited witnesses (a view's own half, `view_evidence`).
    let mut op_ref: HashSet<&str> = HashSet::new();
    let mut site_ref: HashSet<&str> = HashSet::new();
    for r in inp.ws_routines {
        if !inp.coverage.contains_key(&r.id) {
            continue;
        }
        for f in inp.direct_full.get(&r.id).into_iter().flatten() {
            if let Some(w) = &f.witness_operation_id {
                op_ref.insert(w);
            }
            if let Some(w) = &f.witness_callsite_id {
                site_ref.insert(w);
            }
        }
    }
    // #96 D4: the firing op of every implicit-trigger edge; the typed edges'
    // call-site anchors.
    for e in inp.typed_edges {
        if let Some(op) = &e.operation_id {
            op_ref.insert(op);
        }
        if let Some(cid) = &e.callsite_id {
            site_ref.insert(cid);
        }
    }

    #[derive(Default)]
    struct Row {
        stable: String,
        ops: Vec<(String, SnapshotOperationEvidence)>,
        op_rest: Vec<u64>,
        sites: Vec<SnapshotCallsiteEvidence>,
        site_rest: Vec<u64>,
        frames: Vec<ScopeFrame>,
    }
    let rows: Vec<Row> = crate::big_stack::big_stack_pool().install(|| {
        inp.ws_routines
            .par_iter()
            .enumerate()
            .map(|(ri, r)| {
                let stable = stable_routine_id(&r.id, inp.routine_to_stable);
                let order = compute_routine_order(r);
                let (mut ops, mut op_rest) = (Vec::new(), Vec::new());
                op_candidates(
                    ri,
                    r,
                    &order,
                    &stable,
                    collided,
                    &|id| op_ref.contains(id),
                    &mut ops,
                    &mut |id| op_rest.push(id_hash(id)),
                );
                let (mut sites, mut site_rest) = (Vec::new(), Vec::new());
                site_candidates(
                    ri,
                    &order,
                    &stable,
                    collided,
                    &|id| site_ref.contains(id),
                    &mut sites,
                    &mut |id| site_rest.push(id_hash(id)),
                );
                Row {
                    stable,
                    ops,
                    op_rest,
                    sites,
                    site_rest,
                    frames: order.scope_frames,
                }
            })
            .collect()
    });

    // Rows in order: a later row's entry replaces an earlier one's of the
    // same id (operations) and of the same stable id (frames), as in the
    // whole derivation's maps.
    let mut by_id: BTreeMap<String, SnapshotOperationEvidence> = BTreeMap::new();
    let mut frames: BTreeMap<String, Vec<ScopeFrame>> = BTreeMap::new();
    let (mut sites, mut op_rest, mut site_rest) = (Vec::new(), Vec::new(), Vec::new());
    for (ri, (r, row)) in inp.ws_routines.iter().zip(rows).enumerate() {
        by_id.extend(row.ops);
        sites.extend(row.sites);
        let ri32 = row_u32(ri);
        op_rest.extend(row.op_rest.into_iter().map(|h| (h, ri32)));
        site_rest.extend(row.site_rest.into_iter().map(|h| (h, ri32)));
        // A collided id's one table holds every row's frames (`CollidedRows`).
        let f = match collided.frames.get(r.id.as_str()) {
            Some(table) => table.clone(),
            None => row.frames,
        };
        if !f.is_empty() {
            frames.insert(row.stable, f);
        }
    }
    sort_sites(&mut sites);
    op_rest.sort_unstable();
    op_rest.dedup();
    site_rest.sort_unstable();
    site_rest.dedup();
    let frames = (!frames.is_empty()).then(|| RoutineOrderFrames {
        entries: frames.into_iter().collect(),
    });
    (
        EvidenceBase {
            ops: by_id.into_values().collect(),
            op_rest,
            sites,
            site_rest,
        },
        frames,
    )
}

/// The rows `rest` names for the ids `ids` (by hash; a collision only adds a
/// row the candidates' own id test then skips).
fn rows_of(rest: &[(u64, u32)], ids: &BTreeSet<&str>, out: &mut BTreeSet<u32>) {
    for id in ids {
        let h = id_hash(id);
        let from = rest.partition_point(|&(x, _)| x < h);
        out.extend(
            rest[from..]
                .iter()
                .take_while(|&&(x, _)| x == h)
                .map(|&(_, ri)| ri),
        );
    }
}

/// A view's operation and call-site evidence: the base's, plus the sites its
/// subjects' inherited facts name that the base does not reference. `covered`
/// tells a covered routine row's id (the whole derivation reads only those
/// rows' facts).
pub(super) fn view_evidence<'r>(
    vb: &ViewBase,
    routines: &[ModelRoutine],
    subjects: &BTreeSet<String>,
    raw: &dyn Fn(&str) -> Option<InheritedSlice<'r>>,
) -> (
    Vec<SnapshotOperationEvidence>,
    Vec<SnapshotCallsiteEvidence>,
) {
    let eb = &vb.evidence;
    let mut op_extra: BTreeSet<&'r str> = BTreeSet::new();
    let mut site_extra: BTreeSet<&'r str> = BTreeSet::new();
    for id in subjects {
        if !vb.facts.group_of.contains_key(id.as_str()) {
            continue;
        }
        let Some(inherited) = raw(id) else { continue };
        for f in inherited.iter() {
            if let Some(w) = f.witness_operation_id()
                && eb
                    .ops
                    .binary_search_by(|e| e.operation_id.as_str().cmp(w))
                    .is_err()
            {
                op_extra.insert(w);
            }
            if let Some(w) = f.witness_callsite_id() {
                let k = eb.sites.partition_point(|e| e.callsite_id.as_str() < w);
                if eb.sites.get(k).is_none_or(|e| e.callsite_id != w) {
                    site_extra.insert(w);
                }
            }
        }
    }
    let mut ops = eb.ops.clone();
    let mut sites = eb.sites.clone();
    #[cfg(test)]
    tests::EXTRA_IDS.with(|n| n.set(n.get() + op_extra.len() + site_extra.len()));
    if op_extra.is_empty() && site_extra.is_empty() {
        return (ops, sites);
    }
    let mut rows = BTreeSet::new();
    rows_of(&eb.op_rest, &op_extra, &mut rows);
    rows_of(&eb.site_rest, &site_extra, &mut rows);
    let collided = CollidedRows::build(routines);
    let mut by_id: BTreeMap<String, SnapshotOperationEvidence> = BTreeMap::new();
    let mut more_sites = Vec::new();
    for ri in rows {
        let ri = ri as usize;
        let r = &routines[ri];
        let stable = stable_routine_id(&r.id, &vb.routine_to_stable);
        let order = compute_routine_order(r);
        let mut more_ops = Vec::new();
        op_candidates(
            ri,
            r,
            &order,
            &stable,
            &collided,
            &|id| op_extra.contains(id),
            &mut more_ops,
            &mut |_| {},
        );
        by_id.extend(more_ops);
        site_candidates(
            ri,
            &order,
            &stable,
            &collided,
            &|id| site_extra.contains(id),
            &mut more_sites,
            &mut |_| {},
        );
    }
    if !by_id.is_empty() {
        // Disjoint from the base's ids, so one sort by id interleaves them.
        ops.extend(by_id.into_values());
        ops.sort_by(|a, b| a.operation_id.cmp(&b.operation_id));
    }
    if !more_sites.is_empty() {
        sites.extend(more_sites);
        sort_sites(&mut sites);
    }
    (ops, sites)
}

// ---------------------------------------------------------------------------
// Capability facts
// ---------------------------------------------------------------------------

/// Every covered routine's direct rows, in the table's order.
#[derive(Clone)]
pub(super) struct FactBase {
    /// `derive_capability_facts`'s split mode: no emitted subject contains
    /// `|`, so the table is the subjects' runs in `subject + "|"` order, each
    /// run sorted alone. Otherwise one global sort interleaves subjects, and
    /// a view derives its facts whole (`split` false; only `group_of` is
    /// kept).
    pub(super) split: bool,
    /// The direct reps, one per direct fact, and every group's subject.
    tables: Arc<SnapTables>,
    /// The direct rows in output order, each run sorted.
    rows: Vec<SnapshotCapabilityFact>,
    /// One per stable subject, in output order.
    runs: Vec<RunInfo>,
    /// The emission groups in output order.
    order: Vec<u32>,
    /// Per emission group (a covered routine row's id, at its first row).
    groups: Vec<GroupInfo>,
    /// Covered routine id → its emission group.
    pub(super) group_of: HashMap<String, u32>,
}

#[derive(Clone)]
struct RunInfo {
    /// The run's direct rows in [`FactBase::rows`].
    rows: Range<usize>,
    /// The run's groups in [`FactBase::order`].
    groups: Range<usize>,
}

#[derive(Clone)]
struct GroupInfo {
    id: String,
    /// Into the base tables' subjects.
    subject: u32,
    /// The group's first direct rep; its reps are contiguous.
    direct_base: u32,
    direct_len: u32,
    run: u32,
}

impl FactBase {
    fn run_subject(&self, run: &RunInfo) -> &str {
        let g = &self.groups[self.order[run.groups.start] as usize];
        &self.tables.subjects[g.subject as usize]
    }

    /// The first direct row of `subject` witnessing `op`: `(run, base
    /// position)`, `derive_pd_arena`'s `direct_fact_ix` entry.
    fn direct_row(&self, subject: &str, op: &str) -> Option<(u32, usize)> {
        use crate::engine::joined_key::cmp_joined;
        let k = self.runs.partition_point(|r| {
            cmp_joined(&[self.run_subject(r), ""], &[subject, ""]) == std::cmp::Ordering::Less
        });
        let run = self.runs.get(k)?;
        if self.run_subject(run) != subject {
            return None;
        }
        run.rows
            .clone()
            .find(|&i| is_direct_witness(&self.rows[i], op))
            .map(|i| (u32::try_from(k).expect("runs fit u32"), i))
    }
}

fn is_direct_witness(f: &SnapshotCapabilityFact, op: &str) -> bool {
    f.provenance() == "direct" && f.witness_operation_id() == Some(op)
}

/// [`FactBase`] of `inp` (`derive_capability_facts` without raw facts: the
/// same groups, order, runs and per-run sort).
pub(super) fn derive_fact_base(inp: &TreeInputs<'_>) -> FactBase {
    use crate::engine::joined_key::cmp_joined;
    use rayon::prelude::*;
    let mut seen: HashSet<&str> = HashSet::new();
    let covered: Vec<&ModelRoutine> = inp
        .ws_routines
        .iter()
        .filter(|r| seen.insert(r.id.as_str()) && inp.coverage.contains_key(&r.id))
        .collect();
    let group_of: HashMap<String, u32> = covered
        .iter()
        .enumerate()
        .map(|(gi, r)| (r.id.clone(), row_u32(gi)))
        .collect();
    let subjects: Vec<String> = covered
        .iter()
        .map(|r| stable_routine_id(&r.id, inp.routine_to_stable))
        .collect();
    let split = !subjects.iter().any(|s| s.contains('|'));
    if !split {
        return FactBase {
            split,
            tables: Arc::default(),
            rows: Vec::new(),
            runs: Vec::new(),
            order: Vec::new(),
            groups: Vec::new(),
            group_of,
        };
    }
    let mut order: Vec<usize> = (0..covered.len()).collect();
    // Stable: equal subjects keep their emission order.
    order.sort_by(|&x, &y| cmp_joined(&[subjects[x].as_str(), ""], &[subjects[y].as_str(), ""]));
    let mut b = SnapTablesBuilder::default();
    let mut groups: Vec<Option<GroupInfo>> = vec![None; covered.len()];
    let mut runs: Vec<RunInfo> = Vec::new();
    let mut total = 0usize;
    for (k, &gi) in order.iter().enumerate() {
        let subject = b.subject(&subjects[gi]);
        let direct = inp
            .direct_full
            .get(&covered[gi].id)
            .map_or(&[][..], Vec::as_slice);
        let direct_base =
            crate::engine::l4::inherited_facts::table_ix(b.tables.reps.len(), "snapshot rep");
        for f in direct {
            b.push_rep(SnapRep::of(FactRef::Direct(f), subject));
        }
        if k == 0 || subjects[order[k - 1]] != subjects[gi] {
            runs.push(RunInfo {
                rows: total..total,
                groups: k..k,
            });
        }
        let run = runs.last_mut().expect("a run");
        run.rows.end += direct.len();
        run.groups.end = k + 1;
        total += direct.len();
        groups[gi] = Some(GroupInfo {
            id: covered[gi].id.clone(),
            subject,
            direct_base,
            direct_len: u32::try_from(direct.len()).expect("direct facts fit u32"),
            run: row_u32(runs.len() - 1),
        });
    }
    let groups: Vec<GroupInfo> = groups
        .into_iter()
        .map(|g| g.expect("every group placed"))
        .collect();
    let tables = Arc::new(b.tables);
    let mut rows: Vec<SnapshotCapabilityFact> = Vec::with_capacity(total);
    for &gi in &order {
        let g = &groups[gi];
        rows.extend((0..g.direct_len).map(|k| SnapshotCapabilityFact {
            tables: Arc::clone(&tables),
            rep: g.direct_base + k,
            hop: NO_HOP,
        }));
    }
    let mut slices: Vec<&mut [SnapshotCapabilityFact]> = Vec::with_capacity(runs.len());
    let mut rest: &mut [SnapshotCapabilityFact] = &mut rows;
    for run in &runs {
        let (head, tail) = std::mem::take(&mut rest).split_at_mut(run.rows.len());
        rest = tail;
        slices.push(head);
    }
    slices.into_par_iter().for_each(sort_run);
    FactBase {
        split,
        tables,
        rows,
        runs,
        order: order.into_iter().map(row_u32).collect(),
        groups,
        group_of,
    }
}

/// One run's sort: every row shares the subject, so the joined key reduces to
/// the seven remaining fields (stable).
fn sort_run(s: &mut [SnapshotCapabilityFact]) {
    use crate::engine::joined_key::cmp_joined;
    s.sort_by(|a, b| {
        cmp_joined(
            &capability_fact_sort_fields(a)[1..],
            &capability_fact_sort_fields(b)[1..],
        )
    });
}

/// A view's capability facts (split mode) and where its runs moved.
pub(super) struct ViewFacts {
    pub(super) rows: Vec<SnapshotCapabilityFact>,
    /// `(run, view start, view length)` of every rebuilt run, by run.
    rebuilt: Vec<(u32, usize, usize)>,
    /// The shift of the positions after each rebuilt run (cumulative).
    shift_after: Vec<usize>,
}

impl ViewFacts {
    /// The view position of the first direct row witnessing `op` in `run`,
    /// whose base position is `pos`.
    fn position(&self, run: u32, pos: usize, op: &str) -> usize {
        let k = self.rebuilt.partition_point(|r| r.0 < run);
        match self.rebuilt.get(k) {
            Some(&(r, start, len)) if r == run => (start..start + len)
                .find(|&i| is_direct_witness(&self.rows[i], op))
                .expect("a rebuilt run keeps its direct rows"),
            _ => pos + k.checked_sub(1).map_or(0, |j| self.shift_after[j]),
        }
    }
}

/// A view's capability facts over the base's direct table: the base's rows,
/// with every run of a subject that has raw facts rebuilt from its groups'
/// direct rows and raw rows (projected into the view's own tables), sorted
/// as `derive_capability_facts` sorts a run.
pub(super) fn view_facts<'r>(
    fb: &FactBase,
    subjects: &BTreeSet<String>,
    raw: &dyn Fn(&str) -> Option<InheritedSlice<'r>>,
    routine_to_stable: &HashMap<String, String>,
) -> ViewFacts {
    use rayon::prelude::*;
    debug_assert!(fb.split, "a view splices only a split table");
    let marked: BTreeSet<u32> = subjects
        .iter()
        .filter_map(|id| fb.group_of.get(id.as_str()).map(|&gi| (id, gi)))
        .filter(|(id, _)| raw(id).is_some())
        .map(|(_, gi)| fb.groups[gi as usize].run)
        .collect();
    // The raw facts of the marked runs' groups, groups in output order.
    let mut b = SnapTablesBuilder::default();
    // Per marked run: each group with its raw facts and their memo.
    type Placed<'r> = Vec<(u32, Option<(InheritedSlice<'r>, usize)>)>;
    let mut placed: Vec<(u32, Placed<'r>)> = Vec::new();
    for &run in &marked {
        let info = &fb.runs[run as usize];
        let groups = fb.order[info.groups.clone()]
            .iter()
            .map(|&gi| {
                let g = &fb.groups[gi as usize];
                let slice = raw(&g.id).map(|slice| {
                    let stable = &fb.tables.subjects[g.subject as usize];
                    let subject = b.subject(stable);
                    let mi = b.project_raw(gi, subject, stable, &slice, routine_to_stable);
                    (slice, mi)
                });
                (gi, slice)
            })
            .collect();
        placed.push((run, groups));
    }
    let SnapTablesBuilder {
        tables: own, memos, ..
    } = b;
    let own = Arc::new(own);
    let mut built: Vec<(u32, Vec<SnapshotCapabilityFact>)> = placed
        .into_iter()
        .map(|(run, groups)| {
            let mut v = Vec::new();
            for (gi, slice) in groups {
                let g = &fb.groups[gi as usize];
                v.extend((0..g.direct_len).map(|k| SnapshotCapabilityFact {
                    tables: Arc::clone(&fb.tables),
                    rep: g.direct_base + k,
                    hop: NO_HOP,
                }));
                if let Some((slice, mi)) = slice {
                    let m = &memos[mi];
                    v.extend(slice.refs().iter().map(|r| SnapshotCapabilityFact {
                        tables: Arc::clone(&own),
                        rep: m.reps[r.rep as usize],
                        hop: m.hops[r.hop as usize].0,
                    }));
                }
            }
            (run, v)
        })
        .collect();
    drop(memos);
    built.par_iter_mut().for_each(|(_, v)| sort_run(v));
    let extra: usize = built
        .iter()
        .map(|(run, v)| v.len() - fb.runs[*run as usize].rows.len())
        .sum();
    let mut rows = Vec::with_capacity(fb.rows.len() + extra);
    let (mut rebuilt, mut shift_after) = (Vec::new(), Vec::new());
    let (mut cursor, mut shift) = (0usize, 0usize);
    for (run, v) in built {
        let base = &fb.runs[run as usize].rows;
        rows.extend_from_slice(&fb.rows[cursor..base.start]);
        let start = rows.len();
        let len = v.len();
        shift += len - base.len();
        rows.extend(v);
        cursor = base.end;
        rebuilt.push((run, start, len));
        shift_after.push(shift);
    }
    rows.extend_from_slice(&fb.rows[cursor..]);
    ViewFacts {
        rows,
        rebuilt,
        shift_after,
    }
}

// ---------------------------------------------------------------------------
// The pd arena
// ---------------------------------------------------------------------------

/// No snapshot edge (skipped by `derive_typed_edges`, or crossed by no link).
const NO_EDGE: u32 = u32::MAX;

/// The view-independent maps of `derive_pd_arena`, over one cone arena.
#[derive(Clone)]
pub(super) struct PdBase {
    /// `(producers, states, links, edges)` of the arena the maps index.
    shape: [usize; 4],
    /// Cone edge → its index in the typed edges, or [`NO_EDGE`].
    edge_map: Vec<u32>,
    /// Producer → its owner's first direct row witnessing its operation:
    /// `(run, base position)`. Split mode only.
    producer_fact: Vec<Option<(u32, usize)>>,
}

fn shape(cone: &ConeArena) -> [usize; 4] {
    [
        cone.producers.len(),
        cone.states.len(),
        cone.links.len(),
        cone.edges.len(),
    ]
}

/// [`PdBase`] of `cone` over the base's typed edges and direct table.
pub(super) fn derive_pd_base(
    cone: &ConeArena,
    typed_edges: &[SnapshotGraphEdge],
    skipped: &SkippedEdgeKeys,
    fb: &FactBase,
    map: &HashMap<String, String>,
) -> PdBase {
    let shape = shape(cone);
    if cone.states.is_empty() {
        return PdBase {
            shape,
            edge_map: Vec::new(),
            producer_fact: Vec::new(),
        };
    }
    // A link's edge is an out-edge of its state's routine, so every link
    // crossing one cone edge comes from one routine (the solve dedups edges
    // by address in a routine's out-edge list): the edge's key is the
    // state-independent one `derive_pd_arena` builds per link.
    let mut edge_from: Vec<Option<&Arc<str>>> = vec![None; cone.edges.len()];
    for s in &cone.states {
        for l in &cone.links[s.links.clone()] {
            match edge_from[l.edge] {
                None => edge_from[l.edge] = Some(&s.routine),
                Some(f) => assert!(
                    Arc::ptr_eq(f, &s.routine) || **f == *s.routine,
                    "pd base: cone edge {} is crossed from {f} and from {}",
                    l.edge,
                    s.routine
                ),
            }
        }
    }
    // The key must be unique (`derive_pd_arena`).
    let mut edge_ix: HashMap<PdEdgeKey, EdgeIx> = HashMap::with_capacity(typed_edges.len());
    for (ix, e) in typed_edges.iter().enumerate() {
        let Some(to) = e.to_endpoint() else { continue };
        let key = (
            e.kind_str(),
            e.source_routine(),
            to,
            e.callsite_id().unwrap_or(""),
            e.edge_event_id().unwrap_or(""),
            e.edge_operation_id().unwrap_or(""),
        );
        let prev = edge_ix.insert(key, ix);
        debug_assert!(
            prev.is_none(),
            "pd arena: snapshot edges {prev:?} and {ix} share the key {key:?}"
        );
    }
    let edge_map = cone
        .edges
        .iter()
        .zip(&edge_from)
        .map(|(edge, from)| {
            let Some(from) = from else { return NO_EDGE };
            let routine = stable_routine_id(from, map);
            let to = stable_routine_id(&edge.to, map);
            let event = match edge.kind.as_str() {
                "event-dispatch" | "table-event" => edge.event_id.as_deref().unwrap_or(""),
                _ => "",
            };
            let key = (
                edge.kind.as_str(),
                routine.as_str(),
                to.as_str(),
                edge.callsite.as_deref().unwrap_or(""),
                event,
                edge.operation_id.as_deref().unwrap_or(""),
            );
            match edge_ix.get(&key) {
                Some(&e) => u32::try_from(e).expect("typed edges fit u32"),
                None => {
                    // R6-4: a link may miss only a row skipped for want of an
                    // anchor (or an unmodelled kind).
                    debug_assert!(
                        skipped.contains(&(
                            key.0.to_string(),
                            key.1.to_string(),
                            key.2.to_string(),
                            key.3.to_string(),
                            key.4.to_string(),
                            key.5.to_string(),
                        )),
                        "pd arena: cone edge {key:?} has no snapshot edge and was not skipped"
                    );
                    NO_EDGE
                }
            }
        })
        .collect();
    let producer_fact = if fb.split {
        cone.producers
            .iter()
            .map(|p| fb.direct_row(&stable_routine_id(&p.owner, map), &p.operation_id))
            .collect()
    } else {
        Vec::new()
    };
    PdBase {
        shape,
        edge_map,
        producer_fact,
    }
}

/// How a view finds a producer's owner-direct row.
pub(super) enum ProducerRows<'v> {
    /// Split mode: through the base position and the view's run shifts.
    Spliced(&'v PdBase, &'v ViewFacts),
    /// `(owner stable id, operation id)` → the first direct row, over facts
    /// derived whole.
    Whole(HashMap<(&'v str, &'v str), usize>),
}

impl<'v> ProducerRows<'v> {
    /// `derive_pd_arena`'s `direct_fact_ix` over facts derived whole.
    pub(super) fn whole(facts: &'v [SnapshotCapabilityFact]) -> Self {
        let mut ix: HashMap<(&str, &str), usize> = HashMap::new();
        for (i, f) in facts.iter().enumerate() {
            if let ("direct", Some(op)) = (f.provenance(), f.witness_operation_id()) {
                ix.entry((f.subject(), op)).or_insert(i);
            }
        }
        ProducerRows::Whole(ix)
    }

    fn row(&self, cone: &ConeArena, p: usize, map: &HashMap<String, String>) -> Option<usize> {
        let producer = &cone.producers[p];
        match self {
            ProducerRows::Spliced(pb, vf) => {
                pb.producer_fact[p].map(|(run, pos)| vf.position(run, pos, &producer.operation_id))
            }
            ProducerRows::Whole(ix) => {
                let owner = stable_routine_id(&producer.owner, map);
                ix.get(&(owner.as_str(), producer.operation_id.as_str()))
                    .copied()
            }
        }
    }
}

/// A view's pd arena (`derive_pd_arena` with subjects): the states reachable
/// from a subject's states through the links, each once, in state order.
pub(super) fn view_pd(
    cone: &ConeArena,
    subjects: &BTreeSet<String>,
    pb: &PdBase,
    producers: &ProducerRows<'_>,
    map: &HashMap<String, String>,
) -> PdArena {
    use crate::engine::l5::detector_context::cones_census;
    if cone.states.is_empty() {
        return PdArena::default();
    }
    assert_eq!(
        pb.shape,
        shape(cone),
        "a view's pd arena is the one its tree base mapped"
    );
    let mut seen = vec![0u64; cone.states.len().div_ceil(64)];
    let mut stack: Vec<usize> = subjects
        .iter()
        .filter_map(|s| cone.by_routine.get(s.as_str()))
        .flatten()
        .copied()
        .collect();
    while let Some(ix) = stack.pop() {
        let (w, bit) = (ix / 64, 1u64 << (ix % 64));
        if seen[w] & bit != 0 {
            continue;
        }
        seen[w] |= bit;
        stack.extend(
            cone.links[cone.states[ix].links.clone()]
                .iter()
                .map(|l| l.callee_state),
        );
    }
    let mut kept: Vec<usize> = Vec::new();
    for (w, &word) in seen.iter().enumerate() {
        let mut x = word;
        while x != 0 {
            kept.push(w * 64 + x.trailing_zeros() as usize);
            x &= x - 1;
        }
    }
    drop(seen);
    crate::engine::perf_trace::counter_delta("tree.pd_kept_states", kept.len() as u64);
    let new_ix = |old: usize| {
        kept.binary_search(&old)
            .expect("a kept state's callee state is kept")
    };
    let mut producer_fact: Vec<usize> = vec![usize::MAX; cone.producers.len()];
    let mut out = PdArena {
        states: Vec::with_capacity(kept.len()),
        ..PdArena::default()
    };
    let mut unmapped_edges: HashSet<usize> = HashSet::new();
    // Each cone routine's stable id and its states, by the routine's `Arc`
    // (the arena shares one per routine; an equal string under another
    // `Arc` only takes a second slot, merged below).
    let mut routine_slot: HashMap<*const u8, usize> = HashMap::new();
    let mut slots: Vec<(String, Vec<usize>)> = Vec::new();
    for (ix, &old) in kept.iter().enumerate() {
        let s = &cone.states[old];
        let slot = *routine_slot.entry(s.routine.as_ptr()).or_insert_with(|| {
            slots.push((stable_routine_id(&s.routine, map), Vec::new()));
            slots.len() - 1
        });
        slots[slot].1.push(ix);
        let routine = slots[slot].0.clone();
        if producer_fact[s.producer] == usize::MAX {
            producer_fact[s.producer] = producers
                .row(cone, s.producer, map)
                .expect("a producer is its owner's direct fact");
        }
        let start = out.links.len();
        for l in &cone.links[s.links.clone()] {
            match pb.edge_map[l.edge] {
                NO_EDGE => {
                    cones_census::add(&cones_census::PD_EDGE_UNMAPPED, 1);
                    unmapped_edges.insert(l.edge);
                }
                e => out.links.push((new_ix(l.callee_state), e as EdgeIx)),
            }
        }
        out.states.push(StateNode {
            producer_fact: producer_fact[s.producer],
            temp_state: s.t.to_kind(),
            routine,
            seed: s.seed,
            links: start..out.links.len(),
        });
    }
    // Slots in first-state order, each's states ascending: merging two slots
    // of one stable id keeps state order only by a sort.
    out.by_routine.reserve(slots.len());
    for (routine, states) in slots {
        match out.by_routine.entry(routine) {
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(states);
            }
            std::collections::hash_map::Entry::Occupied(mut o) => {
                o.get_mut().extend(states);
                o.get_mut().sort_unstable();
            }
        }
    }
    cones_census::add(
        &cones_census::PD_EDGE_UNMAPPED_KEYS,
        unmapped_edges.len() as u64,
    );
    if cones_census::enabled() {
        cones_census::add(
            &cones_census::PD_SNAPSHOT_BYTES,
            super::pd_arena_bytes(&out),
        );
    }
    out
}

#[cfg(test)]
pub(super) mod tests {
    use super::super::{
        CapabilitySnapshot, SubjectRaw, TreeRaw, compose_tree_over, derive_tree_base,
    };
    use crate::engine::gate::run::AnalysisTarget;
    use crate::engine::l4::cone_derived::ConeScope;
    use crate::engine::l4::exec_relation::ExecRelation;
    use crate::engine::session::{WitnessRequest, compose_cone, tree_apps};
    use crate::program::model::workspace::Model;
    use std::borrow::Cow;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;
    use std::sync::Arc;

    thread_local! {
        /// The ids a view added to its base's evidence (`view_evidence`'s
        /// extras), counted on the composing thread.
        pub(in crate::engine::l5::snapshot) static EXTRA_IDS: std::cell::Cell<usize> =
            const { std::cell::Cell::new(0) };
    }

    /// A tree, section by section: `Debug` of every field (the unserialized
    /// ones too), the facts also as their serialized JSON, the pd arena's
    /// `by_routine` and the boundary sorted.
    fn sections(t: &CapabilitySnapshot) -> Vec<(&'static str, String)> {
        let by_routine: BTreeMap<_, _> = t.pd_solution.by_routine.iter().collect();
        let boundary: Option<BTreeMap<_, _>> = t
            .boundary
            .as_ref()
            .map(|b| b.dependency_app.iter().collect());
        vec![
            ("identities", format!("{:?}", t.identities)),
            ("capability_facts", format!("{:?}", t.capability_facts)),
            (
                "capability_facts_json",
                serde_json::to_string(&t.capability_facts).expect("facts serialize"),
            ),
            ("typed_edges", format!("{:?}", t.typed_edges)),
            ("operation_index", format!("{:?}", t.operation_index)),
            ("callsite_index", format!("{:?}", t.callsite_index)),
            (
                "callsite_resolutions",
                format!("{:?}", t.callsite_resolutions),
            ),
            ("analysis_gaps", format!("{:?}", t.analysis_gaps)),
            ("coverage", format!("{:?}", t.coverage)),
            ("event_declarations", format!("{:?}", t.event_declarations)),
            (
                "root_classifications",
                format!("{:?}", t.root_classifications),
            ),
            (
                "routine_order_frames",
                format!("{:?}", t.routine_order_frames),
            ),
            ("pd_states", format!("{:?}", t.pd_solution.states)),
            ("pd_links", format!("{:?}", t.pd_solution.links)),
            ("pd_by_routine", format!("{by_routine:?}")),
            ("boundary", format!("{boundary:?}")),
        ]
    }

    fn diff(what: &str, new: &CapabilitySnapshot, old: &CapabilitySnapshot) -> Vec<String> {
        sections(new)
            .into_iter()
            .zip(sections(old))
            .filter(|((_, a), (_, b))| a != b)
            .map(|((name, _), _)| format!("{what}: {name} differs"))
            .collect()
    }

    fn inputs(ws: &Path) -> Arc<crate::program::inputs::WorkspaceInputs> {
        crate::program::inputs::WorkspaceInputs::read(ws, &Default::default())
    }

    /// Plan item L3: two views over one shared tree base hold its whole
    /// sections, not copies — every one of them is the base's `Arc`.
    #[test]
    fn views_over_one_base_share_its_whole_sections() {
        use std::sync::Arc;
        let dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus/ws-callsite-resolutions");
        let target = AnalysisTarget::build(&inputs(&dir), true, "digest").expect("a model");
        let model = target.resolved();
        let rel = ExecRelation::single_app(model);
        let apps = tree_apps(None);
        let primary = WitnessRequest::primary();
        let (caps, raw) = compose_cone(
            model,
            &rel,
            ConeScope {
                derived: true,
                raw: primary.scope(model),
            },
        );
        let no_arena = Default::default();
        let pd = caps.pd.as_ref().unwrap_or(&no_arena);
        let base = derive_tree_base(model, rel.cone(), &[], &caps, pd, &apps);
        let subjects: Vec<String> = primary.subjects(model).into_iter().take(2).collect();
        assert_eq!(subjects.len(), 2, "precondition: two primary routines");
        let views: Vec<CapabilitySnapshot> = subjects
            .iter()
            .map(|id| {
                let scope = BTreeSet::from([id.clone()]);
                compose_tree_over(
                    Cow::Borrowed(&base),
                    model,
                    rel.cone(),
                    &[],
                    &caps,
                    TreeRaw::Borrow(SubjectRaw::new(&raw, &scope)),
                    Cow::Borrowed(pd),
                    &apps,
                )
            })
            .collect();
        assert!(
            !base.typed_edges.is_empty() && !base.callsite_resolutions.is_empty(),
            "precondition: sections with rows"
        );
        let frames = |t: &CapabilitySnapshot| t.routine_order_frames.clone();
        for v in &views {
            assert!(
                Arc::ptr_eq(&v.typed_edges, &base.typed_edges),
                "typed edges"
            );
            assert!(
                Arc::ptr_eq(&v.callsite_resolutions, &base.callsite_resolutions),
                "call-site resolutions"
            );
            assert!(Arc::ptr_eq(&v.analysis_gaps, &base.analysis_gaps), "gaps");
            assert!(
                Arc::ptr_eq(&v.event_declarations, &base.event_declarations),
                "event declarations"
            );
            assert!(
                Arc::ptr_eq(&v.root_classifications, &base.root_classifications),
                "root classifications"
            );
            match (frames(v), &base.routine_order_frames) {
                (Some(a), Some(b)) => assert!(Arc::ptr_eq(&a, b), "order frames"),
                (None, None) => {}
                _ => panic!("order frames: one side absent"),
            }
            assert!(Arc::ptr_eq(&v.identities, &base.identities), "identities");
            assert!(Arc::ptr_eq(&v.coverage, &base.coverage), "coverage");
        }
    }

    /// The view's own half of the referenced sets (plan item L4a): a call
    /// site and an operation that only a subject's inherited facts name are
    /// in the view's evidence, found through the base's `*_rest` lists.
    ///
    /// No fixture produces one (the view-equality tests of commit `39dedcb4`
    /// counted 0 extra ids over 1,508 fixture views and six CDO views: an
    /// inherited row's call site is its first hop's, a typed-edge
    /// anchor, and its operation is a covered callee's direct witness), so the
    /// precondition is stated by hand: in a real base, the site and the
    /// operation a subject's inherited row names are moved, by assignment,
    /// from the base's rows into its `*_rest` lists (the rows holding them).
    /// The view over that base must equal the view over the untouched base,
    /// section by section, and give the same digest answer for the subject.
    #[test]
    fn a_site_only_a_subjects_inherited_fact_names_is_in_its_view() {
        use crate::engine::l5::digest_cli::digest_effects;
        let r0 = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus");
        let mut dirs: Vec<_> = walkdir::WalkDir::new(&r0)
            .into_iter()
            .map(|e| e.unwrap().into_path())
            .filter(|p| p.join("app.json").is_file())
            .collect();
        dirs.sort();
        for dir in &dirs {
            let Ok(target) = AnalysisTarget::build(&inputs(dir), true, "digest") else {
                continue;
            };
            let AnalysisTarget::SingleApp(model) = &target else {
                continue;
            };
            let model: &Model = model;
            let rel = ExecRelation::single_app(model);
            let apps = tree_apps(None);
            let primary = WitnessRequest::primary();
            let (caps, raw) = compose_cone(
                model,
                &rel,
                ConeScope {
                    derived: true,
                    raw: primary.scope(model),
                },
            );
            let no_arena = Default::default();
            let pd = caps.pd.as_ref().unwrap_or(&no_arena);
            let base = derive_tree_base(model, rel.cone(), &[], &caps, pd, &apps);
            // A subject whose inherited row names a site and an operation.
            let found = primary.subjects(model).into_iter().find_map(|id| {
                let slice = raw.get(&id)?;
                let f = slice.iter().find(|f| {
                    f.witness_callsite_id().is_some() && f.witness_operation_id().is_some()
                })?;
                Some((
                    id.clone(),
                    f.witness_callsite_id()?.to_string(),
                    f.witness_operation_id()?.to_string(),
                ))
            });
            let Some((subject, site, op)) = found else {
                continue;
            };
            let routines = &model.workspace.routines;
            let rows_with =
                |has: &dyn Fn(&crate::program::model::workspace::ModelRoutine) -> bool| {
                    routines
                        .iter()
                        .enumerate()
                        .filter(|(_, r)| has(r))
                        .map(|(ri, _)| super::row_u32(ri))
                        .collect::<Vec<_>>()
                };
            let site_rows = rows_with(&|r| r.call_sites.iter().any(|c| c.id == site));
            let op_rows = rows_with(&|r| {
                r.operation_sites.iter().any(|o| o.id == op)
                    || r.record_operations.iter().any(|o| o.id == op)
            });
            let mut moved = base.clone();
            let ev = &mut moved.view.evidence;
            let (n_sites, n_ops) = (ev.sites.len(), ev.ops.len());
            ev.sites.retain(|e| e.callsite_id != site);
            ev.ops.retain(|e| e.operation_id != op);
            assert!(
                ev.sites.len() < n_sites && ev.ops.len() < n_ops && !site_rows.is_empty(),
                "precondition: the base held {site:?} and {op:?}"
            );
            ev.site_rest
                .extend(site_rows.iter().map(|&ri| (super::id_hash(&site), ri)));
            ev.site_rest.sort_unstable();
            ev.op_rest
                .extend(op_rows.iter().map(|&ri| (super::id_hash(&op), ri)));
            ev.op_rest.sort_unstable();
            let scope = BTreeSet::from([subject.clone()]);
            let view = |b: &super::super::TreeBase| {
                compose_tree_over(
                    Cow::Borrowed(b),
                    model,
                    rel.cone(),
                    &[],
                    &caps,
                    TreeRaw::Borrow(SubjectRaw::new(&raw, &scope)),
                    Cow::Borrowed(pd),
                    &apps,
                )
            };
            let before = EXTRA_IDS.with(std::cell::Cell::get);
            let spliced = view(&moved);
            let added = EXTRA_IDS.with(std::cell::Cell::get) - before;
            let whole = view(&base);
            let what = dir.strip_prefix(&r0).unwrap_or(dir).display().to_string();
            let mut failures = diff(&format!("{what} {subject}"), &spliced, &whole);
            if added != 2 {
                failures.push(format!(
                    "{what} {subject}: the view added {added} ids, not 2"
                ));
            }
            let root = model
                .workspace
                .routines
                .iter()
                .find(|r| r.id == subject)
                .map(|r| r.stable_routine_id.clone())
                .expect("the subject's row");
            let answer = |t: &CapabilitySnapshot| {
                let d = digest_effects(t, model, std::slice::from_ref(&root), false, false, false);
                let cones: Vec<_> = d
                    .unresolved
                    .iter()
                    .map(|(k, c)| format!("{k} {:?} {:?}", c.items, c.traversal))
                    .collect();
                format!("{:?}\n{cones:?}", d.entries)
            };
            if answer(&spliced) != answer(&whole) {
                failures.push(format!("{what} {subject}: the digest answer differs"));
            }
            assert!(failures.is_empty(), "{}", failures.join("\n"));
            return;
        }
        panic!("precondition: some r0 subject's inherited row names a site and an operation");
    }
}
