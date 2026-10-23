//! `aldump --issue160-step6-probe` (#160 step 6 spec §8.4): MEASUREMENT ONLY,
//! deleted with the cut graph in step 6 Task 2. It runs the head pipeline up to
//! the effects in the head's exact configuration (one composition over
//! `rel.cone()`, the tree over the cone graph, the effects), or the parent's
//! shape (the cut base, its derive and the parent's effects), and prints
//! `probe6 key=value` lines: the stop rule's inputs.

use std::collections::{BTreeSet, HashMap};
use std::time::Instant;

use crate::engine::l4::capability_cone::{
    CapabilityFact, build_cross_app_base, build_r3a3_source_only_base,
};
use crate::engine::l4::cone_derived::{ConeScope, WitnessScope};
use crate::engine::l5::detector_context::{
    compose_capabilities, cones_census, det_cone_inputs, witness_non_primary_members,
};
use crate::engine::l5::snapshot::{
    CapabilitySnapshot, PdArena, compose_snapshot_from_base, compose_tree, derive_identity_table,
};
use crate::program::model::calls::CallEdge;
use crate::program::model::role::Role;
use crate::program::model::workspace::Model;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

/// The internal ids of the rows whose stable id is in `stable`.
fn internal_ids(model: &Model, stable: &[String]) -> BTreeSet<String> {
    let want: std::collections::HashSet<&str> = stable.iter().map(String::as_str).collect();
    model
        .workspace
        .routines
        .iter()
        .filter(|r| want.contains(r.stable_routine_id.as_str()))
        .map(|r| r.id.clone())
        .collect()
}

/// The digest roots of `file` (stable ids), resolved from the identity table
/// alone (P5: the file path reads the span index, the selector path the table).
fn file_roots(model: &Model, file: &str) -> Vec<String> {
    use crate::engine::l5::digest_cli::{ChangedInput, primary_guid_of, resolve_changed_roots};
    let table_only = CapabilitySnapshot {
        identities: derive_identity_table(model),
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
        pd_solution: PdArena::default(),
    };
    let input = ChangedInput {
        files: Some(vec![file.to_string()]),
        routines: None,
        diff_text: None,
    };
    // `--file` roots never read the owner index (it narrows `<owner>::` selectors only).
    let owners = crate::engine::l4::effect_query_cli::OwnerIndex::build(model);
    resolve_changed_roots(&table_only, model, &input, primary_guid_of(model), &owners).roots
}

/// Lower-bound bytes of the dependency calls: the edges at capacity plus the
/// owned strings (`dispatch_meta` and `external_type_ref` not walked).
fn call_bytes(calls: &[CallEdge]) -> usize {
    let opt = |s: &Option<String>| s.as_ref().map_or(0, String::capacity);
    std::mem::size_of_val(calls)
        + calls
            .iter()
            .map(|c| {
                c.from.capacity()
                    + opt(&c.to)
                    + c.callsite_id.capacity()
                    + c.operation_id.capacity()
                    + opt(&c.receiver_type)
                    + opt(&c.unknown_method_name)
                    + opt(&c.receiver_shape)
                    + c.candidates.as_ref().map_or(0, |v| {
                        v.capacity() * std::mem::size_of::<String>()
                            + v.iter().map(String::capacity).sum::<usize>()
                    })
            })
            .sum::<usize>()
}

/// The digest effects (E′ for `d`, the parent's half of S_P(d)): the S4
/// entries of `roots` and each root's unresolved cone.
fn digest_effects(snap: &CapabilitySnapshot, model: &Model, roots: &[String]) -> usize {
    let entries = crate::engine::l5::digest::compute_digest_effects_cli(snap, model, Some(roots));
    let cones: usize = roots
        .iter()
        .map(|r| {
            let c = crate::engine::l5::unresolved_cone::unresolved_cone(snap, r);
            std::hint::black_box(&c);
            1
        })
        .sum();
    std::hint::black_box(&entries);
    entries.len() + cones
}

/// The ordering cell (E′ for `o`) over `snap`, on the big-stack pool as
/// `fill_ordering` runs it. Returns (kept roots, roots with facts).
fn ordering_cell(snap: &CapabilitySnapshot, model: &Model) -> (usize, usize) {
    let kept = crate::engine::l5::digest::ordering_kept_roots(
        snap,
        crate::engine::l5::digest::reportable_roots(model),
    )
    .len();
    let facts = crate::big_stack::big_stack_pool()
        .install(|| crate::engine::l5::ordering_facts::compute_ordering_facts_over(snap, model));
    (kept, facts.len())
}

fn raw_stats(model: &Model, raw: &HashMap<String, Vec<CapabilityFact>>, out: &mut Vec<String>) {
    out.push(format!("raw_vecs={}", raw.len()));
    out.push(format!(
        "raw_facts={}",
        raw.values().map(Vec::len).sum::<usize>()
    ));
    out.push(format!(
        "non_primary_members={}",
        witness_non_primary_members(model, raw)
    ));
}

/// Run one configuration; `Err` on a usage or build error.
pub fn run(
    workspace: &std::path::Path,
    config: &str,
    file: Option<&str>,
) -> Result<String, String> {
    use std::sync::atomic::Ordering;
    let inputs = crate::program::inputs::InputSet::read(workspace, &Default::default());
    if config == "single-app" {
        let model = crate::program::model::program_calls::assemble_and_resolve_workspace_program(
            &inputs,
            crate::program::model::workspace::MODEL_INSTANCE_ID_DEFAULT,
            false,
        )
        .ok_or("no single-app model")?;
        let bodyless = model
            .workspace
            .routines
            .iter()
            .filter(|r| !r.body_available)
            .count();
        return Ok(format!("probe6 single_app_bodyless_rows={bodyless}\n"));
    }
    let base = build_cross_app_base(
        &inputs,
        crate::program::model::workspace::MODEL_INSTANCE_ID_DEFAULT,
    )
    .map_err(|_| "no cross-app base (no primary app or no program)".to_string())?;
    let model = &base.resolved;
    let rel = &base.rel;
    let mut out: Vec<String> = vec![format!("config={config}")];
    let unmapped0 = cones_census::PD_EDGE_UNMAPPED.load(Ordering::Relaxed);
    let keys0 = cones_census::PD_EDGE_UNMAPPED_KEYS.load(Ordering::Relaxed);
    out.push(format!("dependency_calls={}", rel.dependency_calls().len()));
    out.push(format!(
        "dependency_calls_bytes={}",
        call_bytes(rel.dependency_calls())
    ));

    let d_roots = || -> Result<Vec<String>, String> {
        let f = file.ok_or("config d/parent-d needs --file <F>")?;
        Ok(file_roots(model, f))
    };
    let tree_stats = |tree: &CapabilitySnapshot, out: &mut Vec<String>| {
        out.push(format!("pd_states={}", tree.pd_solution.states.len()));
        out.push(format!("pd_links={}", tree.pd_solution.links.len()));
    };

    match config {
        "d" | "o" | "policy" => {
            let (scope, roots) = match config {
                "d" => {
                    let roots = d_roots()?;
                    let scope = ConeScope {
                        derived: true,
                        raw: WitnessScope::Subjects(internal_ids(model, &roots)),
                    };
                    (scope, roots)
                }
                "o" => (
                    ConeScope {
                        derived: false,
                        raw: WitnessScope::Subjects(internal_ids(
                            model,
                            &crate::engine::l5::digest::reportable_roots(model),
                        )),
                    },
                    Vec::new(),
                ),
                _ => (
                    ConeScope {
                        derived: true,
                        raw: WitnessScope::Subjects(
                            model
                                .workspace
                                .routines
                                .iter()
                                .filter(|r| model.routine_role(r) == Role::Primary)
                                .map(|r| r.id.clone())
                                .collect(),
                        ),
                    },
                    Vec::new(),
                ),
            };
            if let WitnessScope::Subjects(s) = &scope.raw {
                out.push(format!("subjects={}", s.len()));
            }
            let t = Instant::now();
            let inputs = det_cone_inputs(model, rel);
            out.push(format!("k_inputs_ms={:.3}", ms(t)));
            let t = Instant::now();
            let (mut caps, raw) = compose_capabilities(model, rel.cone(), inputs, scope);
            out.push(format!("k_compose_ms={:.3}", ms(t)));
            raw_stats(model, &raw, &mut out);
            if config == "policy" {
                drop(raw);
            } else {
                let t = Instant::now();
                let tree = compose_tree(
                    model,
                    rel.cone(),
                    rel.dependency_calls(),
                    &mut caps,
                    raw,
                    None,
                );
                out.push(format!("t_ms={:.3}", ms(t)));
                tree_stats(&tree, &mut out);
                let t = Instant::now();
                if config == "d" {
                    out.push(format!("effects={}", digest_effects(&tree, model, &roots)));
                } else {
                    let (kept, with_facts) = ordering_cell(&tree, model);
                    out.push(format!("ordering_kept_roots={kept}"));
                    out.push(format!("ordering_roots_with_facts={with_facts}"));
                }
                out.push(format!("e_ms={:.3}", ms(t)));
            }
        }
        "parent-d" | "parent-o" => {
            let roots = if config == "parent-d" {
                d_roots()?
            } else {
                Vec::new()
            };
            let t = Instant::now();
            let cut = build_r3a3_source_only_base(model);
            let snap = compose_snapshot_from_base(model, &cut);
            drop(cut);
            if config == "parent-d" {
                out.push(format!("effects={}", digest_effects(&snap, model, &roots)));
            } else {
                let (kept, with_facts) = ordering_cell(&snap, model);
                out.push(format!("ordering_kept_roots={kept}"));
                out.push(format!("ordering_roots_with_facts={with_facts}"));
            }
            out.push(format!("s_p_ms={:.3}", ms(t)));
            tree_stats(&snap, &mut out);
            drop(snap);
            if config == "parent-d" {
                // D_P: the parent's det cone (derived only, with its inputs).
                let t = Instant::now();
                let inputs = det_cone_inputs(model, rel);
                let (caps, raw) = compose_capabilities(
                    model,
                    rel.cone(),
                    inputs,
                    ConeScope {
                        derived: true,
                        raw: WitnessScope::None,
                    },
                );
                out.push(format!("d_p_ms={:.3}", ms(t)));
                drop((caps, raw));
            }
        }
        _ => {
            return Err(format!(
                "unknown --config {config:?} (d, o, policy, parent-d, parent-o, single-app)"
            ));
        }
    }
    out.push(format!(
        "pd_edge_unmapped={}",
        cones_census::PD_EDGE_UNMAPPED.load(Ordering::Relaxed) - unmapped0
    ));
    out.push(format!(
        "anchorless_with_links={}",
        cones_census::PD_EDGE_UNMAPPED_KEYS.load(Ordering::Relaxed) - keys0
    ));
    Ok(out.iter().map(|l| format!("probe6 {l}\n")).collect())
}
