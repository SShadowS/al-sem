//! R3a-5 NATIVE L4-DIRECT ORACLES — properties asserted against the RUST output
//! itself (NOT the al-sem golden), so a co-evolving al-sem bug cannot mask a Rust
//! regression. These are the cross-app dep-fact-propagation invariants that make
//! R3a-5 the EXIT GATE for R3a:
//!
//!   O1. A PRIMARY routine calling a source-bearing dep routine with a capability
//!       fact HAS that fact in `capabilityFactsInherited` (provenance "inherited",
//!       NOT "direct") — the cross-app cone fired.
//!   O2. The inherited fact's WITNESS traces through the cross-app/injected dep
//!       edge: the `witnessCallsiteId` is a callsite on the PRIMARY routine, and
//!       the `witnessOperationId` is the DEP routine's own operation (the fact
//!       originated in the dep).
//!   O3. COVERAGE reflects the cross-app opaque surface: a primary calling a
//!       symbol-only (bodyless) dep routine carries an `opaque-dependency` reason
//!       + the dep routine in `unknownTargets`.
//!   O4. The dep routine's OWN `capabilityFactsDirect` (the propagated fact's
//!       origin) is UNCHANGED from its standalone (R3a-3-style) direct extraction —
//!       the merge/propagation does not perturb the source-of-truth fact.
//!   O5. A primary folds the dep-originated dbEffect as `via:"inherited"` (the
//!       cross-app dbEffect composition), while the dep routine keeps it `direct`.
//!   O6. (R3a-5 injection-coverage hardening, R3b carried follow-up) The INTRA-DEP
//!       injected call edge is LOAD-BEARING: the MIDDLE dep routine `DoIt` inherits
//!       the inner dep `DoWrite`'s Insert fact ONLY via the injected `DoIt→DoWrite`
//!       intra-dep edge (the workspace never calls `DoWrite` *through* `DoIt`'s body;
//!       the dep's own internals are reachable only through the injected edge). A
//!       future injection regression (the intra-dep edge dropped) → `DoIt` loses its
//!       inherited Insert fact → THIS oracle fails. The primary-side O1 alone would
//!       NOT catch it (the primary also calls `DoWrite` directly), so O6 is the
//!       gate on the injection path specifically. Asserted against the Rust output
//!       directly (no al-sem golden), so a co-evolving al-sem bug cannot mask it.

use std::path::PathBuf;

use al_sem::engine::l4::capability_cone::{
    PRoutineFullSummary, R3a5FullSummaryProjection, project_r3a5_cross_app,
};

const FIXTURE: &str = "cross-app-full-summary";
const MODEL_INSTANCE_ID: &str = "r0";

// Stable ids (from the committed fixture — appGuid:Type:Num#sigHash).
const PRIMARY_USECHAIN: &str = "33333333-0005-0000-0000-000000000003:Codeunit:71000#cd29682292055c2a9dfd1a910cc9281bbc9e7bb136e230bd4ee9577061070f74";
const PRIMARY_USESYMBOL: &str = "33333333-0005-0000-0000-000000000003:Codeunit:71001#0248ed06c571be25bd3963f79fc18689e74807aebe0c397abe96e8a45e9dd0bd";
const DEP_DOWRITE: &str = "cccccccc-0001-0000-0000-000000000001:Codeunit:50300#d1b6bb59d5282d960d0e08b598c8bffee337f14afa790440497610a6e76267c0";
const DEP_DOSOMETHING: &str = "55555555-0005-0000-0000-000000000001:Codeunit:55300#ddc256ff3ee6e5336bafb4fefca8c26bf703619ff54aac40bf4caeed3a6be15f";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn ws_fixture_dir() -> PathBuf {
    repo_root().join("tests").join("r3a5-fixtures").join("ws")
}

fn projection() -> R3a5FullSummaryProjection {
    project_r3a5_cross_app(&ws_fixture_dir(), MODEL_INSTANCE_ID, FIXTURE)
}

fn by_id<'a>(p: &'a R3a5FullSummaryProjection, id: &str) -> &'a PRoutineFullSummary {
    p.summaries
        .iter()
        .find(|s| s.routine_id == id)
        .unwrap_or_else(|| panic!("routine {id} not in R3a-5 projection"))
}

/// O1 — the cross-app cone fired: the primary UseChain INHERITS the dep DoWrite's
/// Insert capability fact, with provenance "inherited" (not "direct").
#[test]
fn o1_primary_inherits_dep_capability_fact() {
    let p = projection();
    let use_chain = by_id(&p, PRIMARY_USECHAIN);
    assert!(!use_chain.is_dep_routine, "UseChain is a PRIMARY routine");

    let insert: Vec<_> = use_chain
        .capability_facts_inherited
        .iter()
        .filter(|f| f.op == "insert" && f.resource_kind == "table")
        .collect();
    assert_eq!(
        insert.len(),
        1,
        "UseChain inherits exactly one Insert capability fact (the dep DoWrite's), got {:?}",
        use_chain.capability_facts_inherited
    );
    let fact = insert[0];
    assert_eq!(
        fact.provenance, "inherited",
        "the inherited dep fact has provenance=inherited, NOT direct"
    );
    assert_eq!(
        fact.via, "call",
        "the inherited fact propagated via a call edge"
    );
    // No primary inherited fact may have provenance=direct (a leaked dep direct).
    assert!(
        use_chain
            .capability_facts_inherited
            .iter()
            .all(|f| f.provenance == "inherited"),
        "every primary inherited fact has provenance=inherited"
    );
    // The primary has NO direct capability facts of its own (it only calls deps).
    assert!(
        use_chain.capability_facts_direct.is_empty(),
        "UseChain has no direct capability facts (it only calls the dep)"
    );
}

/// O2 — the inherited fact's witness traces through the cross-app dep edge: the
/// witnessCallsiteId is on the PRIMARY routine, the witnessOperationId is the DEP
/// routine's own op (the fact originated in the dep, reached via the call edge).
#[test]
fn o2_inherited_fact_witness_traces_through_injected_edge() {
    let p = projection();
    let use_chain = by_id(&p, PRIMARY_USECHAIN);
    let fact = use_chain
        .capability_facts_inherited
        .iter()
        .find(|f| f.op == "insert")
        .expect("UseChain inherits the Insert fact");

    let witness_cs = fact
        .witness_callsite_id
        .as_deref()
        .expect("inherited fact carries a witnessCallsiteId (the first-hop call edge)");
    assert!(
        witness_cs.starts_with(PRIMARY_USECHAIN),
        "the witness callsite is on the PRIMARY UseChain routine (the first hop), got {witness_cs}"
    );

    let witness_op = fact
        .witness_operation_id
        .as_deref()
        .expect("inherited fact carries a witnessOperationId (the dep's own op)");
    assert!(
        witness_op.starts_with(DEP_DOWRITE),
        "the witness operation is the DEP DoWrite's own Insert op (the fact origin), got {witness_op}"
    );
}

/// O3 — coverage reflects the cross-app opaque surface: UseSymbolOnly calls the
/// bodyless symbol-only dep routine → opaque-dependency reason + the dep routine
/// in unknownTargets; UseChain (all source-bearing deps) is complete.
#[test]
fn o3_coverage_reflects_cross_app_opaque_apps() {
    let p = projection();

    let use_symbol = by_id(&p, PRIMARY_USESYMBOL);
    assert_eq!(
        use_symbol.coverage.inherited_status, "partial",
        "UseSymbolOnly has partial inherited coverage (opaque dep)"
    );
    assert!(
        use_symbol
            .coverage
            .reasons
            .iter()
            .any(|r| r == "opaque-dependency"),
        "UseSymbolOnly coverage carries an opaque-dependency reason, got {:?}",
        use_symbol.coverage.reasons
    );
    assert!(
        use_symbol
            .coverage
            .unknown_targets
            .iter()
            .any(|t| t == DEP_DOSOMETHING),
        "the symbol-only dep routine is an unknownTarget, got {:?}",
        use_symbol.coverage.unknown_targets
    );

    let use_chain = by_id(&p, PRIMARY_USECHAIN);
    assert_eq!(
        use_chain.coverage.inherited_status, "complete",
        "UseChain (source-bearing dep) has complete coverage"
    );
    assert!(
        use_chain.coverage.reasons.is_empty(),
        "UseChain coverage carries no opaque reasons, got {:?}",
        use_chain.coverage.reasons
    );
}

/// O4 — the dep routine's OWN capabilityFactsDirect (the origin of the propagated
/// fact) is its intrinsic Insert fact: provenance=direct, via=self. The cross-app
/// merge does not perturb the source-of-truth fact.
#[test]
fn o4_dep_routine_direct_fact_unchanged() {
    let p = projection();
    let do_write = by_id(&p, DEP_DOWRITE);
    assert!(do_write.is_dep_routine, "DoWrite is a DEP routine");

    let direct: Vec<_> = do_write
        .capability_facts_direct
        .iter()
        .filter(|f| f.op == "insert" && f.resource_kind == "table")
        .collect();
    assert_eq!(
        direct.len(),
        1,
        "DoWrite has exactly one direct Insert fact (its intrinsic op), got {:?}",
        do_write.capability_facts_direct
    );
    assert_eq!(direct[0].provenance, "direct", "DoWrite's fact is direct");
    assert_eq!(direct[0].via, "self", "DoWrite's fact is via=self");
    // The dep routine does NOT inherit any fact of its own (it is a leaf origin).
    assert!(
        do_write.capability_facts_inherited.is_empty(),
        "DoWrite (the leaf origin) inherits nothing"
    );
}

/// O5 — the dep-originated dbEffect composes cross-app: the primary UseChain folds
/// it as via="inherited" while the dep DoWrite keeps it via="direct".
#[test]
fn o5_dbeffect_composes_cross_app() {
    let p = projection();

    let do_write = by_id(&p, DEP_DOWRITE);
    let dep_insert: Vec<_> = do_write
        .db_effects
        .iter()
        .filter(|e| e.op == "Insert")
        .collect();
    assert_eq!(dep_insert.len(), 1, "DoWrite has one Insert dbEffect");
    assert_eq!(
        dep_insert[0].via, "direct",
        "the dep's own Insert dbEffect is via=direct"
    );

    let use_chain = by_id(&p, PRIMARY_USECHAIN);
    let primary_insert: Vec<_> = use_chain
        .db_effects
        .iter()
        .filter(|e| e.op == "Insert")
        .collect();
    assert_eq!(
        primary_insert.len(),
        1,
        "UseChain folds exactly one Insert dbEffect from the dep"
    );
    assert_eq!(
        primary_insert[0].via, "inherited",
        "the primary's folded dbEffect is via=inherited (cross-app composition)"
    );
    // The folded effect's operationId is the DEP DoWrite's own op (the origin).
    assert!(
        primary_insert[0].operation_id.starts_with(DEP_DOWRITE),
        "the folded dbEffect's operationId is the dep DoWrite's own op, got {}",
        primary_insert[0].operation_id
    );
}

/// O6 — the INTRA-DEP injected edge is LOAD-BEARING (R3a-5 injection-coverage
/// hardening; the R3b carried follow-up). The MIDDLE dep routine `DoIt` (it calls
/// the inner dep `DoWrite` WITHIN the dep app) inherits `DoWrite`'s Insert fact
/// ONLY via the injected `DoIt→DoWrite` intra-dep edge — the workspace has no path
/// to `DoWrite` that goes THROUGH `DoIt`'s reachable body except that injected edge.
/// If a future regression drops the intra-dep injection, `DoIt`'s inherited Insert
/// fact disappears and THIS oracle fails — even though the primary-side O1 still
/// passes (the primary `UseChain` also calls `DoWrite` directly). So O6 gates the
/// injection path specifically.
#[test]
fn o6_intra_dep_injected_edge_is_load_bearing() {
    let p = projection();

    // `DoIt` (the middle dep) = the DEP routine on codeunit 50300 that (a) is a dep
    // routine, (b) is NOT the inner `DoWrite` leaf, and (c) inherits the Insert fact
    // whose witness operation is `DoWrite`'s own op. Find it structurally so the test
    // does not hard-code the (model-instance-independent but build-derived) sigHash.
    let do_it = p
        .summaries
        .iter()
        .find(|s| {
            s.is_dep_routine
                && s.routine_id != DEP_DOWRITE
                && s.routine_id.contains(":Codeunit:50300#")
                && s.capability_facts_inherited.iter().any(|f| {
                    f.op == "insert"
                        && f.witness_operation_id
                            .as_deref()
                            .map(|w| w.starts_with(DEP_DOWRITE))
                            .unwrap_or(false)
                })
        })
        .expect(
            "the middle dep `DoIt` must inherit `DoWrite`'s Insert fact via the injected \
             intra-dep edge — if absent, the intra-dep injection regressed",
        );

    // The inherited fact's witness CALLSITE is on `DoIt` itself (the injected edge's
    // first hop is the dep routine's OWN callsite into `DoWrite`), NOT on a primary.
    let fact = do_it
        .capability_facts_inherited
        .iter()
        .find(|f| f.op == "insert")
        .expect("DoIt inherits the Insert fact");
    assert_eq!(
        fact.provenance, "inherited",
        "DoIt's Insert fact is inherited (not its own direct), got {:?}",
        fact.provenance
    );
    let witness_cs = fact
        .witness_callsite_id
        .as_deref()
        .expect("the injected-edge inherited fact carries a witnessCallsiteId");
    assert!(
        witness_cs.starts_with(&do_it.routine_id),
        "the injected intra-dep edge's witness callsite is on the MIDDLE dep DoIt \
         itself (its own call into DoWrite), got {witness_cs}"
    );

    // `DoIt` has NO direct Insert capability fact of its own — the fact is PURELY
    // injection-derived (it would vanish entirely if the intra-dep edge were dropped).
    assert!(
        do_it
            .capability_facts_direct
            .iter()
            .all(|f| f.op != "insert"),
        "DoIt has no DIRECT Insert fact — its Insert is purely injection-inherited, \
         so the injected edge is load-bearing; got {:?}",
        do_it.capability_facts_direct
    );
}

/// O7 (#22) — a routine known only from symbols is distinguishable from one proven
/// not to write. Both have empty `dbEffects`; only the symbol-only one carries the
/// `opaque-body` self-uncertainty, and it reaches the primary that calls it. The
/// source-bearing dep beside it shows the real write and no such marker.
#[test]
fn o7_symbol_only_dep_routine_reads_as_effects_unknown() {
    let p = projection();
    let opaque_self = |s: &PRoutineFullSummary| {
        s.uncertainties
            .iter()
            .any(|u| u.kind == "opaque-body" && u.routine_id.as_deref() == Some(DEP_DOSOMETHING))
    };

    let symbol_only = by_id(&p, DEP_DOSOMETHING);
    assert!(
        symbol_only.db_effects.is_empty(),
        "no body, so no effects seen"
    );
    assert!(
        opaque_self(symbol_only),
        "the symbol-only routine must say its effects are unknown, got {:?}",
        symbol_only.uncertainties
    );

    let caller = by_id(&p, PRIMARY_USESYMBOL);
    assert!(
        opaque_self(caller),
        "the primary calling it inherits the marker, got {:?}",
        caller.uncertainties
    );

    let source_bearing = by_id(&p, DEP_DOWRITE);
    assert!(
        source_bearing.db_effects.iter().any(|e| e.op == "Insert"),
        "the source-bearing dep shows its real write, got {:?}",
        source_bearing.db_effects
    );
    assert!(
        source_bearing
            .uncertainties
            .iter()
            .all(|u| u.kind != "opaque-body"),
        "a source-bearing routine is not effects-unknown, got {:?}",
        source_bearing.uncertainties
    );
}
