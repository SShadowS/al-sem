//! R3a-4 NATIVE L4-DIRECT ORACLE — ground-truth-free invariants over the RUST
//! producer/consumer output (NOT vs the al-sem golden; these hold by construction of
//! a correct dep-hook pipeline). Complements the byte-differential: the differential
//! proves "same as al-sem", these prove "internally sound".
//!
//! Invariants:
//!   1. Every `intraAppCallEdge` is an OWN→OWN resolved edge (both ends are the dep's
//!      own routines; the producer's own→own filter).
//!   2. Every injected `typedEdge` ⟺ an `intraAppCallEdge` with BOTH ends in the
//!      merged model (the injection both-ends guard); injected edges are synthetic
//!      direct-call edges 1:1 with the admitted intra-app edges.
//!   3. The freshness stamp GATES stale artifacts: a stale stamp ⇒ the order index
//!      is treated as ABSENT (no order entries / return summaries collected).
//!   4. Cited evidence + order entries + return summaries are DEDUPED (unique keys)
//!      and SORTED (by operationId / routineId).

use al_sem::engine::deps::dep_artifact_l4::{
    ConsumerModel, DependencyArtifactL4, collect_cited_dep_evidence, collect_dep_order_index,
    inject_intra_app_call_edges, is_dep_order_index_stamp_fresh,
};
use std::collections::HashSet;
use std::path::PathBuf;

const MODEL_INSTANCE_ID: &str = "r0";

/// The chain dependency's artifact, built as the cross-app path builds it
/// (engine-switch S7.5): from the cross-app model of the workspace that depends on
/// it, `tests/r3a4-fixtures/ws`.
fn chain_dep_artifact() -> al_sem::engine::deps::dep_artifact_l4::DependencyArtifactL4 {
    let ws = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/r3a4-fixtures/ws");
    let x = al_sem::program::model::program_calls::assemble_and_resolve_cross_app_program(
        &ws,
        MODEL_INSTANCE_ID,
        false,
    )
    .expect("cross-app model");
    al_sem::engine::deps::dep_artifact_l4::dep_artifacts_from_model(&x)
        .into_iter()
        .find(|a| a.header.app_guid.starts_with("cccccccc-0001"))
        .expect("chain-dep artifact")
}

fn build_artifact() -> DependencyArtifactL4 {
    chain_dep_artifact()
}

/// Drive the consumer hooks over a merged model = the dep's own routines.
fn build_consumed() -> (DependencyArtifactL4, ConsumerModel) {
    let artifact = build_artifact();
    let mut model = ConsumerModel::with_routine_ids(artifact.abi.routines_ids.clone());
    inject_intra_app_call_edges(&mut model, std::slice::from_ref(&artifact));
    collect_cited_dep_evidence(&mut model, std::slice::from_ref(&artifact));
    collect_dep_order_index(&mut model, std::slice::from_ref(&artifact));
    (artifact, model)
}

#[test]
fn oracle_every_intra_app_edge_is_own_to_own() {
    let artifact = build_artifact();
    let own: HashSet<&String> = artifact.abi.routines_ids.iter().collect();
    assert!(
        !artifact.abi.intra_app_call_edges.is_empty(),
        "non-hollow fixture has ≥1 intraAppCallEdge"
    );
    for e in &artifact.abi.intra_app_call_edges {
        assert!(
            own.contains(&e.from),
            "edge `from` is the dep's own routine: {}",
            e.from
        );
        assert!(
            own.contains(&e.to),
            "edge `to` is the dep's own routine: {}",
            e.to
        );
        assert!(
            e.callsite_id.is_some(),
            "source-parsed edge carries a callsite"
        );
    }
}

#[test]
fn oracle_injected_edge_iff_intra_app_edge_both_ends_in_model() {
    let (artifact, model) = build_consumed();
    let member: HashSet<&String> = model.routine_ids.iter().collect();

    // Admitted intra-app edges (both ends in the merged model).
    let admitted: Vec<_> = artifact
        .abi
        .intra_app_call_edges
        .iter()
        .filter(|e| member.contains(&e.from) && member.contains(&e.to))
        .collect();
    assert_eq!(
        model.injected_typed_edges.len(),
        admitted.len(),
        "injected count == admitted intra-app-edge count"
    );

    // 1:1 correspondence: each injected edge mirrors an admitted intra-app edge,
    // synthetic direct-call.
    for inj in &model.injected_typed_edges {
        assert_eq!(inj.kind, "direct-call");
        assert_eq!(inj.syntax_kind, "synthetic");
        let matched = admitted
            .iter()
            .any(|e| e.from == inj.from && e.to == inj.to);
        assert!(
            matched,
            "injected edge {} -> {} corresponds to an admitted intra-app edge",
            inj.from, inj.to
        );
    }

    // Negative: a merged model with NO dep routine ids injects ZERO (the guard fires).
    let mut empty = ConsumerModel::with_routine_ids(vec!["unrelated/routine".into()]);
    inject_intra_app_call_edges(&mut empty, std::slice::from_ref(&artifact));
    assert!(
        empty.injected_typed_edges.is_empty(),
        "both-ends guard: absent ends inject nothing"
    );
}

#[test]
fn oracle_freshness_stamp_gates_stale_artifacts() {
    let artifact = build_artifact();
    let idx = artifact
        .abi
        .dep_order_index
        .as_ref()
        .expect("source-bearing dep has an order index");

    // Fresh against its own header (empty packageSemanticHash → conservatively fresh).
    assert!(
        is_dep_order_index_stamp_fresh(&idx.stamp, &artifact.header),
        "produced stamp is fresh against its own header"
    );

    // Make the artifact STALE (wrong appId on the stamp) → the consumer treats the
    // order index as ABSENT: zero order entries / return summaries collected.
    let mut stale = artifact.clone();
    if let Some(i) = stale.abi.dep_order_index.as_mut() {
        i.stamp.app_id = "deadbeef-0000-0000-0000-000000000000".into();
    }
    assert!(
        !is_dep_order_index_stamp_fresh(
            &stale.abi.dep_order_index.as_ref().unwrap().stamp,
            &stale.header
        ),
        "tampered stamp is stale"
    );
    let mut model = ConsumerModel::with_routine_ids(stale.abi.routines_ids.clone());
    collect_dep_order_index(&mut model, std::slice::from_ref(&stale));
    assert!(
        model.dep_routine_order_entries.is_empty(),
        "stale artifact contributes NO order entries (freshness barrier)"
    );
    assert!(
        model.dep_return_summaries.is_empty(),
        "stale artifact contributes NO return summaries (freshness barrier)"
    );
}

#[test]
fn oracle_evidence_and_order_deduped_and_sorted() {
    let (_artifact, model) = build_consumed();

    // cited evidence: unique operationIds, sorted ascending.
    let mut seen = HashSet::new();
    let mut prev: Option<&String> = None;
    for e in &model.cited_dep_operation_evidence {
        assert!(
            seen.insert(&e.operation_id),
            "cited evidence deduped by operationId: {}",
            e.operation_id
        );
        if let Some(p) = prev {
            assert!(p <= &e.operation_id, "cited evidence sorted by operationId");
        }
        prev = Some(&e.operation_id);
    }

    // order entries: keyed by routineId (BTreeMap → unique + sorted by construction);
    // assert ascending iteration.
    let order_ids: Vec<&String> = model.dep_routine_order_entries.keys().collect();
    let mut sorted = order_ids.clone();
    sorted.sort();
    assert_eq!(order_ids, sorted, "order entries sorted by routineId");
    assert_eq!(
        order_ids.len(),
        model.dep_routine_order_entries.len(),
        "order entries unique by routineId"
    );

    // return summaries: same — unique + sorted by routineId.
    let rs_ids: Vec<&String> = model.dep_return_summaries.keys().collect();
    let mut rs_sorted = rs_ids.clone();
    rs_sorted.sort();
    assert_eq!(rs_ids, rs_sorted, "return summaries sorted by routineId");
    assert_eq!(
        rs_ids.len(),
        model.dep_return_summaries.len(),
        "return summaries unique by routineId"
    );
}
