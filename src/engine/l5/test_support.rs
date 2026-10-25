//! Synthetic-input constructors shared by the L5 native-oracle tests. NOT a
//! golden fixture — these build minimal `ModelRoutine` / `PrimaryGraph` /
//! `CapabilityFact` / `FullRoutineSummary` values directly so each oracle
//! exercises the query functions on hand-built inputs (mirroring al-sem's
//! probe-style soundness oracles, not a byte-diff).
//!
//! `#[cfg(test)]`-only — never compiled into the shipping engine.

#![cfg(test)]

use std::collections::HashMap;

use crate::engine::l4::capability_cone::{CapabilityFact, CoverageRecord};
use crate::engine::l4::combined_graph::{CombinedEdge, PrimaryGraph};
use crate::engine::l4::cone_derived::{ConeDerivedBuilder, ConeDerivedStore};
use crate::engine::l5::detector_context::{DetectorArtifacts, DetectorContext};
use crate::engine::l5::full_summary::FullRoutineSummary;
use crate::program::body::features::{
    PAnchor, PCallArgumentBinding, PCallSite, PCallee, PLoop, POperationSite, PTempState,
};
use crate::program::model::workspace::{
    Model, ModelEntities, ModelObject, ModelRecordOperation, ModelRoutine, RoutineVariables,
};

/// ⟨C1⟩ The derived cone substrate for a set of hand-built summaries — each
/// routine's literal `reachable()` sequence folded into a row. Fixture summaries
/// carry their `capability_facts_inherited` as INPUT (there is no cone walk and
/// so no key-dedup), so folding the flat reachable list is exactly right here —
/// unlike the production path, which must fold key-deduped representatives.
/// Every hand-built `DetectorContext` uses this so its `cone_derived` field can
/// never silently disagree with its `summaries`.
pub fn cone_store_of(summaries: &HashMap<String, FullRoutineSummary>) -> ConeDerivedStore {
    let mut b = ConeDerivedBuilder::default();
    for (id, s) in summaries {
        // ⟨fix M3⟩ Fold keyed by `s.routine_id`, not the map key `id` — every
        // consumer (`ConeDerivedStore::row` and friends) looks the row up by
        // `summary.routine_id`, so a fixture whose map key diverges from its
        // own `routine_id` must still fold under the id callers will query.
        debug_assert_eq!(
            id, &s.routine_id,
            "cone_store_of: fixture map key {id:?} must equal summary.routine_id {:?} — \
             every derived-store consumer looks the row up by routine_id, not the map key",
            s.routine_id
        );
        // ⟨C1 Task 3⟩ `reachable_iter()` is gone (R6 — it would have silently
        // yielded a direct-only view once the analyze path stopped materializing
        // the inherited Vec). Fixture summaries always own their inherited facts,
        // so the chain is spelled out here; `inherited_raw()` panics loudly if a
        // fixture ever forgets to supply them.
        b.fold_routine(
            &s.routine_id,
            s.capability_facts_direct.iter().chain(s.inherited_raw()),
        );
    }
    b.finish()
}

/// A throwaway anchor (positions are irrelevant to the L5 query substrate).
pub fn dummy_anchor() -> PAnchor {
    PAnchor {
        source_unit_id: "ws:test".to_string(),
        start_line: 0,
        start_column: 0,
        end_line: 0,
        end_column: 0,
        syntax_kind: "test".to_string(),
    }
}

/// A resolved combined edge `from → to` with a callsite id.
pub fn edge(from: &str, to: &str, callsite_id: &str) -> CombinedEdge {
    CombinedEdge {
        from: from.to_string(),
        to: to.to_string(),
        kind: "direct".to_string(),
        callsite_id: Some(callsite_id.to_string()),
        operation_id: None,
        event_id: None,
        subscriber_app_id: None,
        dispatch: None,
        resolution: "resolved".to_string(),
    }
}

/// Build a `PrimaryGraph` from a node list + flat edge list. `nodes` is sorted;
/// `edges_by_from` is grouped (each per-from list in input order — the L5
/// reverse-graph builder is robust to per-from ordering). `edges_from_order`
/// records the first-appearance of each `from` key in the `edges` slice
/// (matching al-sem JS Map insertion order).
pub fn graph_from_edges(nodes: &[&str], edges: &[CombinedEdge]) -> PrimaryGraph {
    let mut node_vec: Vec<String> = nodes.iter().map(|n| n.to_string()).collect();
    node_vec.sort();
    let mut edges_by_from: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
    let mut edges_from_order: Vec<String> = Vec::new();
    for e in edges {
        if !edges_by_from.contains_key(&e.from) {
            edges_from_order.push(e.from.clone());
        }
        edges_by_from
            .entry(e.from.clone())
            .or_default()
            .push(e.clone());
    }
    PrimaryGraph {
        nodes: node_vec,
        edges_by_from,
        edges_from_order,
        uncertainty_edges: Vec::new(),
        collided_site_callee: HashMap::new(),
    }
}

/// A minimal `ModelRoutine` with the given internal id + kind. All other fields are
/// empty / defaulted — only the L5-substrate-relevant ones (id, kind,
/// operation_sites, call_sites) are exercised by the oracles.
pub fn routine(id: &str, kind: &str) -> ModelRoutine {
    ModelRoutine {
        id: id.to_string(),
        stable_routine_id: format!("stable::{id}"),
        object_id: "app/Codeunit/1".to_string(),
        object_type: "Codeunit".to_string(),
        name: id.to_string(),
        kind: kind.to_string(),
        attributes_parsed: Vec::new(),
        app_guid: "app".to_string(),
        object_number: 1,
        normalized_signature_hash: String::new(),
        body_available: true,
        parse_incomplete: false,
        record_variables: Vec::new(),
        record_operations: Vec::new(),
        field_accesses: Vec::new(),
        variables: RoutineVariables::default(),
        parameters: Vec::new(),
        access_modifier: None,
        return_type: None,
        call_sites: Vec::new(),
        operation_sites: Vec::new(),
        statement_tree: None,
        loops: Vec::new(),
        file: None,
        site_exprs: Default::default(),
        source_anchor: dummy_anchor(),
        declaration_start_line: 0,
        dead_marks_trusted: false,
        identifier_references: Vec::new(),
        unreachable_statements: Vec::new(),
        has_branching: false,
        var_assignments: Vec::new(),
        condition_references: Vec::new(),
        enclosing_member: None,
        originating_object: None,
        enclosing_member_range: None,
        entry_temp_guard_receiver: None,
    }
}

/// A routine with `kind` carrying `commit` operation sites for each given op id.
pub fn op_commit_routine(id: &str, kind: &str, commit_op_ids: &[&str]) -> ModelRoutine {
    let mut r = routine(id, kind);
    for op_id in commit_op_ids {
        r.operation_sites.push(POperationSite {
            id: op_id.to_string(),
            kind: "commit".to_string(),
            loop_stack: Vec::new(),
            source_anchor: dummy_anchor(),
            under_asserterror: None,
            control_context: None,
            order: None,
        });
    }
    r
}

/// An object-run call site (e.g. a `Codeunit.Run`) with the given object kind +
/// `objectRunReturnUsed` flag.
pub fn object_run_call_site(id: &str, object_kind: &str, return_used: Option<bool>) -> PCallSite {
    PCallSite {
        id: id.to_string(),
        operation_id: format!("{id}/op"),
        callee_text: format!("{object_kind}.Run"),
        callee: PCallee::ObjectRun {
            object_kind: object_kind.to_string(),
            target_type: object_kind.to_string(),
            target_ref: Some("50100".to_string()),
            target_is_name: false,
        },
        argument_texts: Vec::new(),
        argument_infos: Vec::new(),
        argument_bindings: Vec::new(),
        loop_stack: Vec::new(),
        source_anchor: dummy_anchor(),
        result_consumed: None,
        object_run_return_used: return_used,
        under_asserterror: None,
        control_context: None,
        order: None,
        in_statement_position: false,
        receiver_temp_state: None,
    }
}

/// A capability fact with the given op / resource kind / optional resource id.
/// Other fields are defaulted (they do not affect any L5 query helper).
pub fn fact(
    op: &'static str,
    resource_kind: &'static str,
    resource_id: Option<&str>,
) -> CapabilityFact {
    CapabilityFact {
        subject: "r".to_string(),
        op,
        resource_kind,
        resource_id: resource_id.map(|s| s.to_string()),
        resource_arg_source: None,
        confidence: "static",
        provenance: "direct",
        via: "self",
        witness_operation_id: None,
        witness_callsite_id: None,
        witness_first_hop_operation_id: None,
        witness_first_hop_event_id: None,
        extra: None,
    }
}

/// A coverage record whose `inherited_status` is the given value.
pub fn coverage(inherited_status: &str) -> CoverageRecord {
    CoverageRecord {
        subject: "r".to_string(),
        direct_status: inherited_status.to_string(),
        inherited_status: inherited_status.to_string(),
        reasons: Vec::new(),
        unknown_targets: Vec::new(),
    }
}

/// A `FullRoutineSummary` from direct + inherited facts + optional coverage.
/// ⟨C1 Task 3⟩ Fixture summaries are always MATERIALIZED (`Some(inherited)`) —
/// they carry their inherited facts as INPUT rather than as a cone output, so
/// `inherited_raw()` is always legal on them. A test that wants the production
/// (derived-only) shape builds it with `FullRoutineSummary::new` directly.
pub fn summary(
    routine_id: &str,
    direct: Vec<CapabilityFact>,
    inherited: Vec<CapabilityFact>,
    cov: Option<CoverageRecord>,
) -> FullRoutineSummary {
    FullRoutineSummary::with_inherited(routine_id.to_string(), direct, Some(inherited), cov)
}

// ---------------------------------------------------------------------------
// d1-reachability fixture constructors — shared by `d1_graph`'s and
// `d1_reach`'s test modules (hoisted here per Task 1's review: a second
// duplication of `d1_graph`'s local ctors moves to `test_support`).
// ---------------------------------------------------------------------------

/// A minimal `PLoop` with the given id (type `"for"`).
pub fn loop_def(id: &str) -> PLoop {
    PLoop {
        id: id.to_string(),
        loop_type: "for".to_string(),
        source_anchor: dummy_anchor(),
        exhausting_advance: None,
    }
}

/// An in-loop bare-call call site: `<callee_name>(...)` inside `loop_stack`.
pub fn call_site(id: &str, callee_name: &str, loop_stack: Vec<String>) -> PCallSite {
    PCallSite {
        id: id.to_string(),
        operation_id: format!("{id}/op"),
        callee_text: callee_name.to_string(),
        callee: PCallee::Bare {
            name: callee_name.to_string(),
        },
        argument_texts: Vec::new(),
        argument_infos: Vec::new(),
        argument_bindings: Vec::new(),
        loop_stack,
        source_anchor: dummy_anchor(),
        result_consumed: None,
        object_run_return_used: None,
        under_asserterror: None,
        control_context: None,
        order: None,
        in_statement_position: false,
        receiver_temp_state: None,
    }
}

/// A `known`/`parameter-dependent` `PTempState`.
pub fn ts_known(value: bool) -> PTempState {
    PTempState {
        kind: "known".to_string(),
        value: Some(value),
        parameter_index: None,
    }
}

pub fn ts_pd(idx: u32) -> PTempState {
    PTempState {
        kind: "parameter-dependent".to_string(),
        value: None,
        parameter_index: Some(idx),
    }
}

/// One argument binding of callee param `parameter_index` to `source_temp_state`.
pub fn arg_binding(
    parameter_index: u32,
    source_temp_state: Option<PTempState>,
) -> PCallArgumentBinding {
    PCallArgumentBinding {
        parameter_index,
        source_kind: "variable".to_string(),
        source_variable_name: Some("arg".to_string()),
        source_record_variable_id: None,
        source_parameter_index: None,
        caller_source_parameter_is_var: None,
        source_temp_state,
        argument_anchor: dummy_anchor(),
    }
}

/// A resolved combined edge `from -> to` with an explicit `kind` (unlike
/// [`edge`], which hardcodes `"direct"`).
pub fn edge_kind(from: &str, to: &str, callsite_id: &str, kind: &str) -> CombinedEdge {
    CombinedEdge {
        from: from.to_string(),
        to: to.to_string(),
        kind: kind.to_string(),
        callsite_id: Some(callsite_id.to_string()),
        operation_id: None,
        event_id: None,
        subscriber_app_id: None,
        dispatch: None,
        resolution: "resolved".to_string(),
    }
}

/// A minimal `ModelRecordOperation` (`temp_state` defaults to `None`; callers
/// mutate `.temp_state` for the PD/Known cases).
#[allow(clippy::too_many_arguments)]
pub fn record_op(
    id: &str,
    op: &str,
    record_variable_name: &str,
    table_id: Option<&str>,
    loop_stack: Vec<String>,
    in_until_condition: bool,
) -> ModelRecordOperation {
    ModelRecordOperation {
        id: id.to_string(),
        op: op.to_string(),
        record_variable_name: record_variable_name.to_string(),
        record_variable_id: None,
        table_id: table_id.map(|s| s.to_string()),
        temp_state: None,
        field_arguments: None,
        source_anchor: dummy_anchor(),
        loop_stack,
        field_argument_infos: None,
        in_until_condition,
        run_trigger: None,
    }
}

/// A model with nothing in it.
pub fn empty_resolved() -> Model {
    Model {
        workspace: ModelEntities {
            objects: vec![],
            tables: vec![],
            routines: vec![],
        },
        root_classifications: vec![],
        primary_app: None,
        infra_diagnostics: vec![],
        workspace_diagnostics: vec![],
        dependency_diagnostics: vec![],
        calls: Default::default(),
        events: Default::default(),
    }
}

/// The owner [`minimal_ctx`] returns: a routine slice and the artifacts a
/// [`DetectorContext`] borrows. Call sites do `let ctx = owned.ctx();`.
pub struct MinimalCtx<'a> {
    routines: &'a [ModelRoutine],
    objects: &'a [ModelObject],
    art: DetectorArtifacts,
}

impl<'a> MinimalCtx<'a> {
    pub fn ctx(&self) -> DetectorContext<'_> {
        DetectorContext::new(self.routines, self.objects, &[], &self.art, None)
    }

    /// The object rows the context's `declaring` index is built over (none by
    /// default).
    pub fn with_objects(mut self, objects: &'a [ModelObject]) -> Self {
        self.objects = objects;
        self
    }

    /// Mutate the artifacts BEFORE borrowing a context.
    pub fn art_mut(&mut self) -> &mut DetectorArtifacts {
        &mut self.art
    }
}

/// A minimal context owner sufficient for `build_d1_graph` / `search_loops`:
/// `routine_by_id` / `graph.edges_by_from` / `summaries` are populated from the
/// args, and `call_site_by_id` is built from every routine's own call sites (so
/// `D1Edge.loop_depth` derives non-zero when an edge's callsite carries a
/// `loop_stack`). Everything else is empty / default.
pub fn minimal_ctx<'a>(
    routines: &'a [ModelRoutine],
    graph_edges: HashMap<String, Vec<CombinedEdge>>,
    summaries: HashMap<String, FullRoutineSummary>,
) -> MinimalCtx<'a> {
    let graph = PrimaryGraph {
        nodes: vec![],
        edges_by_from: graph_edges,
        edges_from_order: vec![],
        uncertainty_edges: vec![],
        collided_site_callee: HashMap::new(),
    };
    MinimalCtx {
        routines,
        objects: &[],
        art: empty_artifacts(graph, summaries),
    }
}

/// Artifacts with `graph` and `summaries` (and the derived cone store folded
/// from them) and every other field empty / default.
pub fn empty_artifacts(
    graph: PrimaryGraph,
    summaries: HashMap<String, FullRoutineSummary>,
) -> DetectorArtifacts {
    DetectorArtifacts {
        demanded: 0,
        graph: std::sync::Arc::new(graph),
        cone: std::sync::Arc::new(Vec::new()),
        event_graph: crate::program::model::events::EventGraph {
            events: vec![],
            edges: vec![],
        },
        reverse_call_graph: std::collections::BTreeMap::new(),
        entry_points: std::collections::BTreeSet::new(),
        transaction_spans: vec![],
        resolved_call_edge_by_callsite: HashMap::new(),
        uncertainty_edges_by_from: HashMap::new(),
        uncertainties_by_node: HashMap::new(),
        uncertainties: Default::default(),
        // ⟨C1⟩ Fold BEFORE the move so the derived substrate always mirrors this
        // context's own summaries.
        cone_derived: cone_store_of(&summaries),
        summaries,
        event_flow_indexes: crate::engine::l5::event_flow::EventFlowIndexes::default(),
        parameter_roles_by_routine: HashMap::new(),
        upgraded_bindings_by_callsite: HashMap::new(),
        reachable_roots: std::collections::BTreeSet::new(),
        internal_reachable_externally: false,
        dep_routine_ids: std::collections::BTreeSet::new(),
        declared_dependencies: Vec::new(),
        app_versions: HashMap::new(),
        friends: HashMap::new(),
        root_classifications_by_routine: HashMap::new(),
        closed_world_temp_params: Default::default(),
        summarize_diagnostics: Vec::new(),
        reverse_effect_index: None,
        cross_extension_subscribers: std::collections::BTreeMap::new(),
    }
}

// ---------------------------------------------------------------------------
// #160 step 4: cross-app fixtures with a source-bearing dependency, shared by
// `capability_cone`'s step-4 pins and `exec_relation`'s parity oracle.
// ---------------------------------------------------------------------------

const STEP4_WS_GUID: &str = "aaaa1604-0000-0000-0000-000000000160";
const STEP4_DEP_GUID: &str = "bbbb1604-0000-0000-0000-000000000160";

/// A workspace (`src/Ws.al` = `ws_al`) depending on one embedded-source app
/// `XRec` (table 18 `Customer` plus `src/Lib.al` = `lib_al`), whose
/// `SymbolReference.json` lists codeunit 12 `Dep Rec Lib` with `methods_json`.
/// Written as `capability_cone`'s `write_fixture_107` does (40-byte header + zip).
/// The two are intentional siblings, not one writer: `write_fixture_107` has its
/// own guids, a second dependency codeunit and a symbol-only variant, which a
/// shared writer would have to grow parameters for.
fn write_ws_with_dep(ws: &std::path::Path, ws_al: &str, methods_json: &str, lib_al: &str) {
    use std::io::Write;
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        format!(
            r#"{{"id":"{STEP4_WS_GUID}","name":"Ws160","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50100,"to":50199}}],"dependencies":[{{"id":"{STEP4_DEP_GUID}","name":"XRec","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    )
    .unwrap();
    std::fs::write(ws.join("src/Ws.al"), ws_al).unwrap();
    let manifest = format!(
        r#"<?xml version="1.0" encoding="utf-8"?><Package xmlns="http://schemas.microsoft.com/navx/2015/manifest"><App Id="{STEP4_DEP_GUID}" Name="XRec" Publisher="probe" Version="1.0.0.0" Runtime="13.0" /></Package>"#
    );
    let symbols = format!(
        r#"{{"RuntimeVersion":"13.0","Tables":[{{"Id":18,"Name":"Customer","Fields":[{{"Id":1,"Name":"No.","TypeDefinition":{{"Name":"Code"}}}}]}}],"Codeunits":[{{"Id":12,"Name":"Dep Rec Lib","Methods":{methods_json}}}],"AppId":"{STEP4_DEP_GUID}","Name":"XRec","Publisher":"probe","Version":"1.0.0.0"}}"#
    );
    let customer = "table 18 Customer\n{\n    fields\n    {\n        field(1; \"No.\"; Code[20])\n        {\n        }\n    }\n}\n";
    let mut bytes = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut bytes);
        let opts = zip::write::SimpleFileOptions::default();
        // An empty `lib_al` writes a symbol-only app (no source).
        for (name, text) in [
            ("NavxManifest.xml", manifest.as_str()),
            ("SymbolReference.json", symbols.as_str()),
            ("src/Customer.al", customer),
            ("src/Lib.al", lib_al),
        ]
        .into_iter()
        .filter(|(name, _)| !(lib_al.is_empty() && name.starts_with("src/")))
        {
            zip.start_file(name, opts).unwrap();
            zip.write_all(text.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }
    let pkgs = ws.join(".alpackages");
    std::fs::create_dir_all(&pkgs).unwrap();
    let mut out = std::fs::File::create(pkgs.join("probe_XRec_1.0.0.0.app")).unwrap();
    out.write_all(&[0u8; 40]).unwrap();
    out.write_all(bytes.get_ref()).unwrap();
}

/// Spec §5.2: a dependency CALLER (`Post`) of a dependency publisher
/// (`OnDepPost`) with a workspace subscriber (`HandleWs`, `Modify` + `Commit`),
/// and a dependency `local` helper `Touch` whose only caller (`CallTouch`)
/// passes it a `temporary` record. `DoTouch` makes `CallTouch` demanded.
pub fn write_fixture_step4(ws: &std::path::Path) {
    let rec = r#"[{"Name":"Rec","IsVar":true,"TypeDefinition":{"Name":"Record","Subtype":{"Name":"Customer","Id":18}}}]"#;
    let ev = r#"[{"Name":"IntegrationEvent","Arguments":[{"Value":"False"},{"Value":"False"}]}]"#;
    let methods = format!(
        r#"[{{"Name":"Post","Parameters":{rec}}},{{"Name":"OnDepPost","Parameters":{rec},"Attributes":{ev}}},{{"Name":"CallTouch","Parameters":[]}}]"#
    );
    let lib = r#"codeunit 12 "Dep Rec Lib"
{
    procedure Post(var Rec: Record Customer)
    begin
        OnDepPost(Rec);
    end;

    [IntegrationEvent(false, false)]
    procedure OnDepPost(var Rec: Record Customer)
    begin
    end;

    procedure CallTouch()
    var
        T: Record Customer temporary;
    begin
        Touch(T);
    end;

    local procedure Touch(var R: Record Customer)
    begin
        R.Modify();
    end;
}
"#;
    let ws_al = r#"codeunit 50180 "Ws Fwd"
{
    procedure DoPost(var Rec: Record Customer)
    var
        Lib: Codeunit "Dep Rec Lib";
    begin
        Lib.Post(Rec);
    end;

    procedure DoTouch()
    var
        Lib: Codeunit "Dep Rec Lib";
    begin
        Lib.CallTouch();
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Dep Rec Lib", 'OnDepPost', '', false, false)]
    local procedure HandleWs(var Rec: Record Customer)
    begin
        Rec.Modify();
        Commit();
    end;
}
"#;
    write_ws_with_dep(ws, ws_al, &methods, lib);
}

/// Spec §5.4: a dependency routine `DepCaller` whose call names a method no
/// record has (a to-less call), called from the workspace so it is demanded.
pub fn write_fixture_dep_uncertainty(ws: &std::path::Path) {
    let lib = r#"codeunit 12 "Dep Rec Lib"
{
    procedure DepCaller()
    var
        R: Record Customer;
    begin
        R.NoSuchMethod();
    end;
}
"#;
    let ws_al = r#"codeunit 50181 "Ws Call"
{
    procedure Go()
    var
        Lib: Codeunit "Dep Rec Lib";
    begin
        Lib.DepCaller();
    end;
}
"#;
    write_ws_with_dep(ws, ws_al, r#"[{"Name":"DepCaller","Parameters":[]}]"#, lib);
}

/// #160 step 6 D6 (T7 (a)): the dependency's `DepCaller`, demanded by the
/// workspace's `Go`, makes a to-less call (`R.NoSuchMethod()`) and a resolved
/// object run whose result it reads (`Ok := Codeunit.Run(..)`).
pub fn write_fixture_dep_ledger(ws: &std::path::Path) {
    let lib = r#"codeunit 12 "Dep Rec Lib"
{
    trigger OnRun()
    begin
    end;

    procedure DepCaller()
    var
        R: Record Customer;
        Ok: Boolean;
    begin
        R.NoSuchMethod();
        Ok := Codeunit.Run(Codeunit::"Dep Rec Lib");
    end;
}
"#;
    let ws_al = r#"codeunit 50183 "Ws Ledger"
{
    procedure Go()
    var
        Lib: Codeunit "Dep Rec Lib";
    begin
        Lib.DepCaller();
    end;
}
"#;
    let methods = r#"[{"Name":"DepCaller","Parameters":[]}]"#;
    write_ws_with_dep(ws, ws_al, methods, lib);
}

/// #160 step 6 D6 (T7 (b)): the workspace's `Go` calls `Write` of a
/// SYMBOL-ONLY `XRec`: a resolved call into a body-less dependency row.
pub fn write_fixture_symbol_only_dep(ws: &std::path::Path) {
    let ws_al = r#"codeunit 50182 "Ws Sym"
{
    procedure Go()
    var
        Lib: Codeunit "Dep Rec Lib";
    begin
        Lib.Write();
    end;
}
"#;
    write_ws_with_dep(ws, ws_al, r#"[{"Name":"Write","Parameters":[]}]"#, "");
}

/// #160 step 6 rule B (spec §14.3.5): each workspace routine of `Ws RuleB`
/// reaches dependency code one way. `CallWrite` calls `Write` (a physical
/// `Modify`, B4); `FillAndPost` passes a `temporary` Customer to `Fill` (it
/// inserts into it, a producer proof) and calls `Post` (a physical insert,
/// B8); `CallRaise` calls `Raise`, which raises `OnRaised`, handled by the
/// workspace's `OnRaisedWs` with an insert into `Ws Log` (B6);
/// `WriteThenCommit` inserts into `Ws Log`, then calls `MaybeCommit`, which
/// commits inside an `if` (B7); `WriteCallCommit` does the same, then commits
/// itself, and `DepWriteThenCommit` calls `Write`, then commits (B7's MUST
/// commit and write sets).
pub fn write_fixture_rule_b(ws: &std::path::Path) {
    let rec = r#"[{"Name":"Rec","IsVar":true,"TypeDefinition":{"Name":"Record","Subtype":{"Name":"Customer","Id":18}}}]"#;
    let flag = r#"[{"Name":"Flag","TypeDefinition":{"Name":"Boolean"}}]"#;
    let ev = r#"[{"Name":"IntegrationEvent","Arguments":[{"Value":"False"},{"Value":"False"}]}]"#;
    let methods = format!(
        r#"[{{"Name":"Write","Parameters":[]}},{{"Name":"Fill","Parameters":{rec}}},{{"Name":"Post","Parameters":[]}},{{"Name":"Raise","Parameters":[]}},{{"Name":"OnRaised","Parameters":[],"Attributes":{ev}}},{{"Name":"MaybeCommit","Parameters":{flag}}}]"#
    );
    let lib = r#"codeunit 12 "Dep Rec Lib"
{
    procedure Write()
    var
        C: Record Customer;
    begin
        C.Modify();
    end;

    procedure Fill(var Rec: Record Customer)
    begin
        Rec.Insert();
    end;

    procedure Post()
    var
        C: Record Customer;
    begin
        C.Insert();
    end;

    procedure Raise()
    begin
        OnRaised();
    end;

    [IntegrationEvent(false, false)]
    procedure OnRaised()
    begin
    end;

    procedure MaybeCommit(Flag: Boolean)
    begin
        if Flag then
            Commit();
    end;
}
"#;
    let ws_al = r#"table 50190 "Ws Log"
{
    fields
    {
        field(1; Id; Integer)
        {
        }
    }
}

codeunit 50190 "Ws RuleB"
{
    procedure CallWrite()
    var
        Lib: Codeunit "Dep Rec Lib";
    begin
        Lib.Write();
    end;

    procedure FillAndPost()
    var
        Lib: Codeunit "Dep Rec Lib";
        T: Record Customer temporary;
    begin
        Lib.Fill(T);
        Lib.Post();
    end;

    procedure CallRaise()
    var
        Lib: Codeunit "Dep Rec Lib";
    begin
        Lib.Raise();
    end;

    procedure WriteThenCommit()
    var
        Lib: Codeunit "Dep Rec Lib";
        L: Record "Ws Log";
    begin
        L.Insert();
        Lib.MaybeCommit(true);
    end;

    procedure WriteCallCommit()
    var
        Lib: Codeunit "Dep Rec Lib";
        L: Record "Ws Log";
    begin
        L.Insert();
        Lib.MaybeCommit(true);
        Commit();
    end;

    procedure DepWriteThenCommit()
    var
        Lib: Codeunit "Dep Rec Lib";
    begin
        Lib.Write();
        Commit();
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Dep Rec Lib", 'OnRaised', '', false, false)]
    local procedure OnRaisedWs()
    var
        L: Record "Ws Log";
    begin
        L.Insert();
    end;
}
"#;
    write_ws_with_dep(ws, ws_al, &methods, lib);
}

/// #160 step 6 rule B, final panel F1-S2: the parts of an event dispatch from
/// primary code into a dependency subscriber (the `ev:` boundary). The
/// workspace's `WriteThenRaise` inserts into `Ws Log`, then raises its own
/// `OnWsEvent`; the dependency's `DoHttp` makes an HTTP call, and `CallHttp`
/// calls it (so the cone holds it). No compilable AL dispatches from the
/// workspace into a dependency subscriber (a dependency cannot see the
/// workspace), so the test adds that edge to the tree by hand.
pub fn write_fixture_rule_b_event(ws: &std::path::Path) {
    let lib = r#"codeunit 12 "Dep Rec Lib"
{
    procedure DoHttp()
    var
        Client: HttpClient;
        Resp: HttpResponseMessage;
    begin
        Client.Get('https://api.example.test/orders', Resp);
    end;
}
"#;
    let ws_al = r#"table 50190 "Ws Log"
{
    fields
    {
        field(1; Id; Integer)
        {
        }
    }
}

codeunit 50191 "Ws Pub"
{
    procedure CallHttp()
    var
        Lib: Codeunit "Dep Rec Lib";
    begin
        Lib.DoHttp();
    end;

    procedure WriteThenRaise()
    var
        L: Record "Ws Log";
    begin
        L.Insert();
        OnWsEvent();
    end;

    [IntegrationEvent(false, false)]
    procedure OnWsEvent()
    begin
    end;
}
"#;
    write_ws_with_dep(ws, ws_al, r#"[{"Name":"DoHttp","Parameters":[]}]"#, lib);
}

/// #160 step 5 (spec `2026-10-22-issue-160-step5-coverage-policy-design.md`
/// §5 T4, probe P6): a dependency routine `Inner` makes one ambiguous call and
/// one `Modify`; the dependency's `Outer` calls `Inner`; the workspace's
/// `Ws Call::Go` calls `Outer`. So the only call-site-local uncertainty is in
/// dependency code, and `Go` is `partial` only when the dependency half is
/// folded. Shared with step 6's acceptance.
///
/// NOT compilable AL: `O(L)` passes a Codeunit, which is neither a `Text` nor
/// an implementation of `Dep ILog`. It works because our overload resolver is
/// lenient (the `ws-overload-negatives` `O` shape). The tests that use it
/// assert the ambiguous edge exists, so a stricter resolver fails them loudly.
pub fn write_fixture_dep_ambiguous(ws: &std::path::Path) {
    write_fixture_dep_ambiguous_after(ws, "");
}

/// [`write_fixture_dep_ambiguous`] with `lead` written before `Inner`'s
/// ambiguous call (#288 P4: `"exit;"` makes it and the `Modify` dead).
pub fn write_fixture_dep_ambiguous_after(ws: &std::path::Path, lead: &str) {
    const CALL: &str = "        O(L);\n";
    let lib = r#"interface "Dep ILog"
{
    procedure Touch();
}

codeunit 12 "Dep Rec Lib"
{
    procedure Outer()
    begin
        Inner();
    end;

    procedure Inner()
    var
        L: Codeunit "Dep Rec Lib";
        R: Record Customer;
    begin
        O(L);
        R.Modify();
    end;

    procedure O(X: Interface "Dep ILog"): Integer
    begin
        exit(0);
    end;

    procedure O(X: Text): Integer
    begin
        exit(1);
    end;
}
"#;
    assert_eq!(lib.matches(CALL).count(), 1, "one ambiguous call to lead");
    let lib = if lead.is_empty() {
        lib.to_string()
    } else {
        lib.replace(CALL, &format!("        {lead}\n{CALL}"))
    };
    let ws_al = r#"codeunit 50181 "Ws Call"
{
    procedure Go()
    var
        Lib: Codeunit "Dep Rec Lib";
    begin
        Lib.Outer();
    end;
}
"#;
    write_ws_with_dep(
        ws,
        ws_al,
        r#"[{"Name":"Outer","Parameters":[]},{"Name":"Inner","Parameters":[]}]"#,
        &lib,
    );
}

/// The dependency half `ExecRelation::new` receives (spec §5.2, S2-3): the
/// cross-app model built a second time, independently, with the call
/// `build_cross_app_base` makes, and its UNCUT `resolved.calls.edges` kept
/// for callers whose `app_guid` differs from the primary app's (the base's own
/// rule). Emission order kept.
pub fn dependency_half(ws: &std::path::Path) -> Vec<crate::program::model::calls::CallEdge> {
    let inputs = crate::program::inputs::WorkspaceInputs::read(ws, &Default::default());
    let x = crate::program::model::program_calls::assemble_and_resolve_cross_app_program(
        &inputs, "r0", false,
    )
    .unwrap_or_else(|_| panic!("cross-app model for {}", ws.display()));
    assert!(x.resolved.primary_app.is_some(), "primary app");
    let dep: std::collections::BTreeSet<&str> = x
        .resolved
        .workspace
        .routines
        .iter()
        .filter(|r| x.resolved.routine_role(r) == crate::program::model::role::Role::Dependency)
        .map(|r| r.id.as_str())
        .collect();
    x.resolved
        .calls
        .edges
        .iter()
        .filter(|e| dep.contains(e.from.as_str()))
        .cloned()
        .collect()
}
