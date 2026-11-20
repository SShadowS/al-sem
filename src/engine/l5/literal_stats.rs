//! `aldump --literal-pruning-stats <workspace>`: what the literal pruning ([`LiteralReach`])
//! knows and proves on a whole workspace, as deterministic JSON. ONE measure that every change
//! to the pruning reports (ENG-30, ENG-135, ENG-132..134), so each change's own move is visible
//! by running it on both sides.
//!
//! Workspace totals, counting each stable routine id once (all its `#if` arms together, as the
//! frames merge them):
//! - **guard frames** (Boolean parameters, `param_guard`): routines with a frame, usable
//!   parameters, and parameters UNUSABLE BY A WRITE: in every arm by-value, never assigned, in a
//!   routine with a control tree (every other test the frame makes), yet not usable, because
//!   some call may write them. These are the exclusions the frames as built apply: a version
//!   that misses a write counts that parameter usable, not unusable.
//! - **value frames** (r3 E1, Integer/Option/Enum/... parameters, `values`): the same three, the
//!   usable parameters split by domain.
//! - **flow** (r3 E2, `lflow`): routines with a flow program, the parameters calls bind through
//!   it, and the (call site, tracked variable) pairs a call may change (`Havoc`).
//! - **one-hop pruning**: for each call site (in any arm) that reaches a routine, the bindings
//!   production derives for the callee from the caller with an empty entry state (no caller
//!   context): its literal and label arguments, its own records' tables and what its flow knows,
//!   the callee instance's globals included,
//!   and the callee's sites (operations and call sites, every arm) those bindings prove dead and
//!   no binding at all does not (the pruning the call itself brings, from an empty caller
//!   state; a zero-argument call can bring some through the instance's globals), in total and
//!   by the frame that proves it (a site several frames prove dead counts in each).

use super::*;
use crate::program::model::workspace::ModelRoutine;
use std::collections::BTreeMap;

/// One frame family's parameter counts.
#[derive(Debug, Default, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FrameCounts {
    pub routines_with_frame: usize,
    pub params_usable: usize,
    pub params_unusable_by_write: usize,
}

/// The value frames' counts, with the usable parameters by domain.
#[derive(Debug, Default, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ValueCounts {
    #[serde(flatten)]
    pub frame: FrameCounts,
    pub usable_by_domain: BTreeMap<String, usize>,
}

/// The flow programs' counts.
#[derive(Debug, Default, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FlowCounts {
    pub routines_with_program: usize,
    pub params_bound: usize,
    pub havoc_writes: usize,
}

/// The one-hop pruning counts.
#[derive(Debug, Default, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OneHop {
    /// (call site, reached routine) pairs.
    pub calls: usize,
    /// Those whose arguments bind something in the callee.
    pub calls_binding: usize,
    /// (call, callee site) pairs proved dead, in total and by frame.
    pub dead_sites: usize,
    pub dead_by_guard: usize,
    pub dead_by_value: usize,
    pub dead_by_flow: usize,
}

#[derive(Debug, Default, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LiteralPruningStats {
    pub routines: usize,
    pub guard_frames: FrameCounts,
    pub value_frames: ValueCounts,
    pub flow: FlowCounts,
    pub one_hop: OneHop,
}

/// Whether a flow evaluation (program and result, when the routine has a program) proves
/// `site` dead.
fn flow_dead(f: &Option<(&lflow::FlowProgram, Arc<lflow::FlowEval>)>, site: &str) -> bool {
    f.as_ref().is_some_and(|(p, ev)| p.dead(ev, site))
}

/// The variant name of a domain (`Enum("x")` → `Enum`).
fn domain_name(d: &values::Domain) -> String {
    let s = format!("{d:?}");
    s.split(['(', ' ', '{'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Whether Boolean parameter `p` of `r` passes every guard-frame test but the write test.
fn guard_eligible(r: &ModelRoutine, p: &crate::program::model::workspace::ModelParameter) -> bool {
    if (r.body_available && r.statement_tree.is_none())
        || p.is_var
        || !p.type_text.trim().eq_fold_identifier("boolean")
    {
        return false;
    }
    let name = p.name.trim().trim_matches('"');
    !r.var_assignments
        .iter()
        .any(|a| a.lhs_name.trim().trim_matches('"').eq_fold_identifier(name))
}

/// Whether parameter `p` of `r` passes every value-frame test but the write test.
fn value_eligible(
    r: &ModelRoutine,
    p: &crate::program::model::workspace::ModelParameter,
    loops: &[String],
) -> bool {
    if r.statement_tree.is_none() || p.is_var || values::domain_of(&p.type_text).is_none() {
        return false;
    }
    let name = values::fold(&p.name);
    !(r.var_assignments
        .iter()
        .any(|a| values::fold(&a.lhs_name) == name)
        || loops.contains(&name))
}

impl<'a> LiteralReach<'a> {
    /// The workspace's literal pruning counts (module doc).
    pub(crate) fn pruning_stats(&self) -> LiteralPruningStats {
        let mut s = LiteralPruningStats::default();
        // One row per stable id, in a fixed order.
        let mut stables: Vec<(&str, &ModelRoutine)> =
            self.by_stable.iter().map(|(k, r)| (*k, *r)).collect();
        stables.sort_by(|a, b| a.0.cmp(b.0));
        s.routines = stables.len();
        // Every row of a stable id (its `#if` arms); the frames take what is usable in all.
        let rows_of = |sid: &str, first: &'a ModelRoutine| -> Vec<&'a ModelRoutine> {
            self.arm_rows
                .get(sid)
                .cloned()
                .unwrap_or_else(|| vec![first])
        };
        // Whether parameter `index` passes `test` in every row (sol: a parameter one arm
        // assigns is excluded by that, not by a write).
        let in_every_row = |rows: &[&'a ModelRoutine],
                            index: u32,
                            test: &dyn Fn(
            &ModelRoutine,
            &crate::program::model::workspace::ModelParameter,
        ) -> bool| {
            rows.iter().all(|row| {
                row.parameters
                    .iter()
                    .find(|q| q.index == index)
                    .is_some_and(|q| test(row, q))
            })
        };
        for (sid, r) in &stables {
            let rows = rows_of(sid, r);
            // Guard frames.
            let usable_g = self
                .frames
                .get(sid)
                .map(|f| f.usable.as_slice())
                .unwrap_or(&[]);
            if !usable_g.is_empty() {
                s.guard_frames.routines_with_frame += 1;
            }
            s.guard_frames.params_usable += usable_g.len();
            s.guard_frames.params_unusable_by_write += r
                .parameters
                .iter()
                .filter(|p| {
                    in_every_row(&rows, p.index, &guard_eligible) && !usable_g.contains(&p.index)
                })
                .count();
            // Value frames.
            let vf = self.value_frames.get(sid);
            let usable_v: Vec<u32> = vf
                .map(|v| v.usable.iter().map(|(i, _)| *i).collect())
                .unwrap_or_default();
            if let Some(v) = vf.filter(|v| !v.usable.is_empty()) {
                s.value_frames.frame.routines_with_frame += 1;
                for (_, d) in &v.usable {
                    *s.value_frames
                        .usable_by_domain
                        .entry(domain_name(d))
                        .or_default() += 1;
                }
            }
            s.value_frames.frame.params_usable += usable_v.len();
            let value_ok =
                |row: &ModelRoutine, p: &crate::program::model::workspace::ModelParameter| {
                    let mut loops = Vec::new();
                    if let Some(tree) = &row.statement_tree {
                        values::loop_vars(tree, &mut loops);
                    }
                    value_eligible(row, p, &loops)
                };
            s.value_frames.frame.params_unusable_by_write += r
                .parameters
                .iter()
                .filter(|p| in_every_row(&rows, p.index, &value_ok) && !usable_v.contains(&p.index))
                .count();
            // Flow.
            if let Some(p) = self.flow_progs.get(sid) {
                s.flow.routines_with_program += 1;
                s.flow.havoc_writes += p.havoc_writes();
            }
            s.flow.params_bound += self.flow_bind.get(sid).map_or(0, Vec::len);
        }
        // One-hop pruning: the search's own edges ([`Self::successors`]) from each routine (every
        // `#if` arm's), each (call site, callee) once; an edge binds only where the search binds
        // (`binds`: not an object run, an event edge or any edge without a call site; sol
        // round 6), so the counter credits no pruning the search does not make.
        let none = Bindings::default();
        for (sid, _) in &stables {
            let edges: std::collections::BTreeSet<(&str, &str, bool)> = self
                .out
                .get(sid)
                .into_iter()
                .flatten()
                .filter_map(|&(to, site, binds, kind)| {
                    let s = site.and_then(EdgeSite::guarded)?;
                    // The search's own edge filter (`successors`): an edge dead under the
                    // empty caller state is never entered, so it brings no pruning (sol round 7).
                    (!self.edge_dead(sid, s, to, kind, &none, ALL)).then_some((s, to, binds))
                })
                .collect();
            for (cs_id, callee, binds) in edges {
                s.one_hop.calls += 1;
                let b = if binds {
                    self.bind(sid, cs_id, callee, &none)
                } else {
                    Bindings::default()
                };
                // Any binding counts, a table or a global's included (sol round 3).
                if b.bools.is_empty()
                    && b.values.is_empty()
                    && b.flow.is_empty()
                    && b.tables.is_empty()
                    && b.globals.is_empty()
                {
                    continue;
                }
                s.one_hop.calls_binding += 1;
                let Some(c) = self.by_stable.get(callee) else {
                    continue;
                };
                let flow_b = self
                    .flow_progs
                    .get(callee)
                    .zip(self.flow_eval(callee, &b, ALL));
                // Every callee row's sites, once each.
                let callee_sites: std::collections::BTreeSet<&str> = rows_of(callee, c)
                    .iter()
                    .flat_map(|row| {
                        row.operation_sites
                            .iter()
                            .map(|o| o.id.as_str())
                            .chain(row.call_sites.iter().map(|x| x.id.as_str()))
                    })
                    .collect();
                // A site counts only when the call's ARGUMENTS make it dead: dead under
                // the bindings and live under none (sol: a branch the callee's own code
                // already kills is no pruning of this call's).
                for site in callee_sites {
                    // A site dead with no binding at all is no pruning of this call's, by
                    // any frame (sol round 2: else a frame's count could pass the total).
                    if self.dead_at(callee, site, &none) {
                        continue;
                    }
                    let by_guard = contradicts(self.reqs_at(callee, site), &b);
                    let by_value = values::contradicts(self.value_reqs_at(callee, site), &b);
                    let by_flow = flow_dead(&flow_b, site);
                    s.one_hop.dead_by_guard += usize::from(by_guard);
                    s.one_hop.dead_by_value += usize::from(by_value);
                    s.one_hop.dead_by_flow += usize::from(by_flow);
                    s.one_hop.dead_sites += usize::from(self.dead_at(callee, site, &b));
                }
            }
        }
        s
    }
}

/// `aldump --literal-pruning-stats`: the counts of workspace `ws` (its cross-app model, every
/// witness), as pretty JSON.
pub fn literal_pruning_stats_json(ws: &std::path::Path) -> anyhow::Result<String> {
    use crate::engine::l4::cone_derived::{ConeScope, WitnessScope};
    use crate::engine::l5::detector_context::{compose_capabilities, det_cone_inputs};
    use crate::engine::l5::snapshot::{TreeApps, TreeRaw, compose_tree, source_less_apps};
    let inputs = crate::program::inputs::WorkspaceInputs::read(ws, &Default::default());
    let base = crate::engine::l4::capability_cone::build_cross_app_base(&inputs)
        .map_err(|e| anyhow::anyhow!("cross-app base: {e:?}"))?;
    let model = &base.resolved;
    let rel = base.rel();
    let (mut caps, raw) = compose_capabilities(
        model,
        rel.cone(),
        det_cone_inputs(model, rel),
        ConeScope {
            derived: false,
            raw: WitnessScope::All,
        },
        1,
    );
    let pd = std::borrow::Cow::Owned(caps.pd.take().unwrap_or_default());
    let tree = compose_tree(
        model,
        rel.cone(),
        base.dependency_calls_or_empty(),
        &caps,
        TreeRaw::Drain(raw),
        pd,
        &TreeApps {
            names: HashMap::new(),
            source_less: source_less_apps(&base.dependency_apps),
        },
    );
    let reach = LiteralReach::new(&tree, model);
    Ok(serde_json::to_string_pretty(&reach.pruning_stats())? + "\n")
}
