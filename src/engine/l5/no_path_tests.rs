//! #369: own effect rows with no witness path (`evidence.sourceKind:
//! unavailable`, no `viaPaths`, no boundary group), and the `factId`s they
//! shared.

use super::*;
use crate::engine::l4::capability_cone::R3a5CrossAppBase;
use crate::engine::l4::cone_derived::{ConeScope, WitnessScope};
use crate::engine::l5::detector_context::{compose_capabilities, det_cone_inputs};
use crate::engine::l5::snapshot::{
    CapabilitySnapshot, TreeApps, TreeRaw, compose_tree, source_less_apps,
};
use std::path::Path;

/// The cross-app base of `ws` and digest's tree over it, its raw witness
/// facts kept for `scope`.
fn tree_over(ws: &Path, scope: WitnessScope) -> (R3a5CrossAppBase, CapabilitySnapshot) {
    let inputs = crate::program::inputs::WorkspaceInputs::read(ws, &Default::default());
    let base = crate::engine::l4::capability_cone::build_cross_app_base(&inputs)
        .unwrap_or_else(|_| panic!("cross-app base"));
    let model = &base.resolved;
    let rel = base.rel();
    let (mut caps, raw) = compose_capabilities(
        model,
        rel.cone(),
        det_cone_inputs(model, rel),
        ConeScope {
            derived: false,
            raw: scope,
        },
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
    (base, tree)
}

/// The stable id of the routine `object::name` (`object` matched by name).
fn stable(model: &Model, object: &str, name: &str) -> String {
    let obj: Vec<&str> = model
        .workspace
        .objects
        .iter()
        .filter(|o| o.name == object)
        .map(|o| o.id.as_str())
        .collect();
    let ids: BTreeSet<&str> = model
        .workspace
        .routines
        .iter()
        .filter(|r| r.name == name && obj.contains(&r.object_id.as_str()))
        .map(|r| r.stable_routine_id.as_str())
        .collect();
    assert_eq!(ids.len(), 1, "precondition: one {object}::{name}: {ids:?}");
    ids.into_iter().next().unwrap().to_string()
}

/// An own row with no witness: no path, no group, no anchor.
fn is_no_path(e: &DigestEffectResult) -> bool {
    !e.boundary_group && e.via_paths.is_empty() && e.evidence.source_kind == "unavailable"
}

/// The probe of the issue: for every no-path row of the root, each fact of
/// the root behind it and what the witness saw, and every `factId` shared by
/// two rows. Returns (rows, distinct `factId`s, no-path rows, the lines).
fn probe_root(
    tree: &CapabilitySnapshot,
    model: &Model,
    root: &str,
) -> (usize, usize, usize, Vec<String>) {
    let entries = compute_digest_effects_cli(tree, model, Some(&[root.to_string()]), false);
    let effects = &entries[0].effects;
    let idx = build_fingerprint_indexes(tree);
    let ops = crate::engine::l4::operation_index::OperationIndex::build(
        model,
        crate::engine::l4::operation_index::IndexScope::All,
    );
    let no_path: Vec<&DigestEffectResult> = effects.iter().filter(|e| is_no_path(e)).collect();
    let ids: BTreeSet<&str> = no_path.iter().map(|e| e.fact_id.as_str()).collect();
    let all_ids: BTreeSet<&str> = effects.iter().map(|e| e.fact_id.as_str()).collect();
    let mut out = vec![format!(
        "root {root}: {} effects ({} distinct factIds, {} groups), {} no-path rows, {} distinct factIds",
        effects.len(),
        all_ids.len(),
        effects.iter().filter(|e| e.boundary_group).count(),
        no_path.len(),
        ids.len()
    )];
    let mut by_id: BTreeMap<&str, Vec<&DigestEffectResult>> = BTreeMap::new();
    for e in effects {
        by_id.entry(e.fact_id.as_str()).or_default().push(e);
    }
    for (id, rows) in by_id.iter().filter(|(_, r)| r.len() > 1) {
        for e in rows {
            out.push(format!(
                "  COLLIDE {id} {} group={} key={} detail={:?} op={:?} cs={:?} origin={:?}",
                e.effect_type,
                e.boundary_group,
                e.canonical_key,
                e.detail,
                e.evidence_operation_id,
                e.evidence_callsite_id,
                e.origin_operation_ids
            ));
        }
    }
    let carrier_cache = CarrierCache::default();
    let mut hop_cache = HopCache::default();
    let mut dag_cache = DagCache::default();
    for e in &no_path {
        for op in &e.origin_operation_ids {
            let owner = match ops.lookup(op) {
                crate::engine::l4::operation_index::OwnerLookup::Found(c) => {
                    ops.routine(&c).stable_routine_id.clone()
                }
                other => format!("{other:?}"),
            };
            for f in idx.facts_by_routine[root]
                .iter()
                .filter(|f| f.witness_operation_id() == Some(op.as_str()))
                .filter(|f| effect_type_of(f) == Some(e.effect_type.as_str()))
            {
                let w = reconstruct_witness_paths(
                    root,
                    f,
                    &idx,
                    HARD_PATH_CAP,
                    &mut hop_cache,
                    &mut dag_cache,
                    &carrier_cache,
                );
                let carriers = carrier_cache.of(f, &idx);
                let owner_direct: Vec<String> = idx
                    .direct_facts_by_routine
                    .get(owner.as_str())
                    .into_iter()
                    .flatten()
                    .filter(|d| d.witness_operation_id() == Some(op.as_str()))
                    .map(|d| {
                        format!(
                            "pd={} kt={} carrier={} res={:?}",
                            is_pd_snap(d),
                            is_known_temp_snap(d),
                            is_bfs_carrier(d, f),
                            d.resource_id()
                        )
                    })
                    .collect();
                let first = fact_first_hop(f);
                let first_edges = idx
                    .outgoing_edges
                    .get(root)
                    .into_iter()
                    .flatten()
                    .filter(|x| is_first_hop(x, f))
                    .count();
                let dag = first.map(|h| {
                    let fe: Vec<&SnapshotGraphEdge> = idx
                        .outgoing_edges
                        .get(root)
                        .into_iter()
                        .flatten()
                        .filter(|x| is_first_hop(x, f))
                        .copied()
                        .collect();
                    dag_cache.get(root, h, &fe, &idx)
                });
                let owner_d = dag
                    .as_ref()
                    .and_then(|d| d.info.get(owner.as_str()).map(|x| x.0));
                let reached = dag.as_ref().map_or(0, |d| {
                    carriers.iter().filter(|c| d.info.contains_key(*c)).count()
                });
                out.push(format!(
                    "  {} {:?} op={op} owner={owner} ownerDep={} | prov={} kt={} pd={} first={first:?} firstEdges={first_edges} | diags={:?} paths={} | carriers={} reached={reached} ownerInCarriers={} ownerDist={owner_d:?} ownerDirect={owner_direct:?} pdStatesAtRoot={}",
                    e.effect_type,
                    f.resource_id(),
                    idx.is_dependency(&owner),
                    f.provenance(),
                    is_known_temp_snap(f),
                    is_pd_snap(f),
                    w.diagnostics,
                    w.paths.len(),
                    carriers.len(),
                    carriers.contains(owner.as_str()),
                    idx.pd.by_routine.get(root).map_or(0, Vec::len),
                ));
            }
        }
    }
    (effects.len(), all_ids.len(), no_path.len(), out)
}

/// #369's first cause, minimized: the workspace's `Run` passes a persistent
/// `Customer` to the dependency's `Entry`, which hands it down a lattice of
/// `layers` × 3 procedures, each calling all three of the next layer; the
/// last layer inserts through the `var` parameter. The insert is a producer
/// (its class is the caller's), so only the proof walk can witness it, and
/// its partial paths below the crossing number 3 + 9 + … + 3^(layers−1).
fn write_lattice(ws: &Path, layers: usize) {
    let rec = r#"[{"Name":"Rec","IsVar":true,"TypeDefinition":{"Name":"Record","Subtype":{"Name":"Customer","Id":18}}}]"#;
    let mut methods = vec![format!(r#"{{"Name":"Entry","Parameters":{rec}}}"#)];
    let mut lib = String::from(
        "codeunit 12 \"Dep Rec Lib\"\n{\n    procedure Entry(var Rec: Record Customer)\n    begin\n        L1x0(Rec);\n        L1x1(Rec);\n        L1x2(Rec);\n    end;\n",
    );
    for l in 1..=layers {
        for j in 0..3 {
            methods.push(format!(r#"{{"Name":"L{l}x{j}","Parameters":{rec}}}"#));
            let body = if l == layers {
                "        Rec.Insert();\n".to_string()
            } else {
                (0..3)
                    .map(|k| format!("        L{}x{k}(Rec);\n", l + 1))
                    .collect()
            };
            lib.push_str(&format!(
                "\n    procedure L{l}x{j}(var Rec: Record Customer)\n    begin\n{body}    end;\n"
            ));
        }
    }
    lib.push_str("}\n");
    let ws_al = r#"codeunit 50190 "Ws Lattice"
{
    procedure Run()
    var
        Lib: Codeunit "Dep Rec Lib";
        C: Record Customer;
    begin
        Lib.Entry(C);
    end;
}
"#;
    crate::engine::l5::test_support::write_ws_with_dep(
        ws,
        ws_al,
        &format!("[{}]", methods.join(",")),
        &lib,
    );
}

/// `Run`'s digest effects on the lattice of `layers`, as `type boundary
/// resources` (boundary `-` for an own row) plus whether a row has no path.
fn lattice_rows(layers: usize) -> Vec<String> {
    let tmp = tempfile::tempdir().unwrap();
    write_lattice(tmp.path(), layers);
    let (base, tree) = tree_over(tmp.path(), WitnessScope::All);
    let model = &base.resolved;
    let root = stable(model, "Ws Lattice", "Run");
    let entries = compute_digest_effects_cli(&tree, model, Some(&[root]), false);
    let detail = |e: &DigestEffectResult, k: &str| {
        e.detail
            .iter()
            .find(|(key, _)| key == k)
            .map_or("-".to_string(), |(_, v)| v.clone())
    };
    let mut rows: Vec<String> = entries[0]
        .effects
        .iter()
        .map(|e| {
            format!(
                "{} {} {}{}",
                e.effect_type,
                detail(e, "boundary"),
                if e.boundary_group {
                    detail(e, "resources")
                } else {
                    detail(e, "resourceDisplay")
                },
                if is_no_path(e) { " NO-PATH" } else { "" }
            )
        })
        .collect();
    rows.sort();
    rows
}

/// #369 cause 1: a write through a `var` record parameter deep in dependency
/// code is a member of the group of the call that crosses into it. Ten
/// layers make 29 523 partial paths before the first seed, past the old
/// walk's `MAX_STATES` (25 000): there it was an own row with no path.
#[test]
fn a_deep_producer_write_beyond_a_crossing_is_its_group_member() {
    let want = vec!["DB_INSERT Codeunit Dep Rec Lib::Entry Customer".to_string()];
    assert_eq!(lattice_rows(10), want);
    // Control: the shallow lattice, which the old walk also witnessed.
    assert_eq!(lattice_rows(2), want);
}

/// #369's second cause, minimized: the workspace's `Mid` inserts a `Ws Log`
/// row and then calls the dependency's `Unresolved`, which inserts through a
/// `RecordRef` whose table is not known. `Run` calls `Mid`.
fn write_unresolved_beside_typed(ws: &Path) {
    let methods = r#"[{"Name":"Unresolved","Parameters":[{"Name":"TableNo","TypeDefinition":{"Name":"Integer"}}]}]"#;
    let lib = r#"codeunit 12 "Dep Rec Lib"
{
    procedure Unresolved(TableNo: Integer)
    var
        RR: RecordRef;
    begin
        RR.Open(TableNo);
        RR.Insert();
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

codeunit 50190 "Ws Mixed"
{
    procedure Run()
    begin
        Mid();
    end;

    procedure Mid()
    var
        Lib: Codeunit "Dep Rec Lib";
        L: Record "Ws Log";
    begin
        L.Insert();
        Lib.Unresolved(50190);
    end;
}
"#;
    crate::engine::l5::test_support::write_ws_with_dep(ws, ws_al, methods, lib);
}

/// #369 cause 2: an insert whose table is not known is never witnessed by a
/// typed insert. `Run` inherits two inserts through `Mid`: `Ws Log` (Mid's
/// own) and the dependency's unresolved one. The witness's carrier test let
/// `Mid`'s typed insert carry the unresolved fact (`fact_equivalent` compares
/// tables only when both name one), so both rows ended at `Mid`'s operation
/// with one path, and shared a `factId`. The unresolved insert belongs to the
/// group of the call into `Unresolved`, which counts it as unresolved.
#[test]
fn an_unresolved_write_is_never_witnessed_by_a_typed_one() {
    let tmp = tempfile::tempdir().unwrap();
    write_unresolved_beside_typed(tmp.path());
    let (base, tree) = tree_over(tmp.path(), WitnessScope::All);
    let model = &base.resolved;
    let root = stable(model, "Ws Mixed", "Run");
    let entries = compute_digest_effects_cli(&tree, model, Some(&[root]), false);
    let effects = &entries[0].effects;
    let mut rows: Vec<String> = effects
        .iter()
        .map(|e| {
            let d: Vec<String> = e.detail.iter().map(|(k, v)| format!("{k}={v}")).collect();
            format!(
                "{} group={} {}",
                e.effect_type,
                e.boundary_group,
                d.join(";")
            )
        })
        .collect();
    rows.sort();
    assert_eq!(
        rows,
        vec![
            "DB_INSERT group=false resourceId=aaaa1604-0000-0000-0000-000000000160/table/50190;resourceDisplay=Ws Log".to_string(),
            "DB_INSERT group=true boundary=Codeunit Dep Rec Lib::Unresolved;resourceCount=0;resources=;unresolvedResources=1".to_string(),
        ],
        "{effects:?}"
    );
    let ids: BTreeSet<&str> = effects.iter().map(|e| e.fact_id.as_str()).collect();
    assert_eq!(ids.len(), effects.len(), "one factId per row");
}

/// The issue's probe as a CDO ratchet (`CDO_WS`): IssueReminder (IR) and
/// RunReportID (RR) keep no own row without a witness, and no two rows of an
/// entry share a `factId`. Measured on the baseline before #369: IR 275 rows,
/// 222 distinct ids, 57 no-path rows over 5 ids; RR 227, 210, 20 over 4. All
/// 78 facts behind those rows were producer writes (a `var` record parameter)
/// in dependency code whose proof walk hit `state-limit-exceeded`; the two
/// other collisions were unresolved dependency inserts witnessed at a typed
/// workspace insert. After: IR 220 rows, RR 206, every id distinct. On a
/// failure the probe's lines say, per fact, what the witness saw.
#[test]
fn cdo_i369_no_path_rows_and_fact_ids() {
    let Some(ws) = crate::cdo::cdo_ws_or_enforce() else {
        return;
    };
    let inputs = crate::program::inputs::WorkspaceInputs::read(&ws, &Default::default());
    let base = crate::engine::l4::capability_cone::build_cross_app_base(&inputs)
        .unwrap_or_else(|_| panic!("cross-app base"));
    let roots = [
        stable(&base.resolved, "CDO Issue Document", "IssueReminder"),
        stable(&base.resolved, "CDO E-Mail Handler", "RunReportID"),
    ];
    let subjects: BTreeSet<String> = base
        .resolved
        .workspace
        .routines
        .iter()
        .filter(|r| roots.contains(&r.stable_routine_id))
        .map(|r| r.id.clone())
        .collect();
    drop(base);
    let (base, tree) = tree_over(&ws, WitnessScope::Subjects(subjects));
    for root in &roots {
        let (rows, ids, no_path, lines) = probe_root(&tree, &base.resolved, root);
        println!("{}", lines[0]);
        let lines = lines.join("\n");
        assert_eq!(no_path, 0, "{lines}");
        assert_eq!(ids, rows, "{lines}");
    }
}
