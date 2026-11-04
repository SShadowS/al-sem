//! MCP output wave 2, W2-0a (design
//! `docs/superpowers/specs/2026-11-04-mcp-output-wave2-design.md` §3.3,
//! §3.4.1, §3.4.3, §11): the read-side indexes — crossing records, the
//! event id on the literal search's event edges, and the terminal-fact index
//! — on in-process adversarial workspaces. Each workspace is written to a temp
//! folder, so no golden family reads it and none moves.
//!
//! The operation index's tests sit beside the collided-row fixture they need
//! (`detector_context_11b_tests.rs`).

use super::*;
use crate::engine::l4::capability_cone::R3a5CrossAppBase;
use crate::engine::l4::cone_derived::{ConeScope, WitnessScope};
use crate::engine::l5::detector_context::{compose_capabilities, det_cone_inputs};
use crate::engine::l5::literal_reach::LiteralReach;
use crate::engine::l5::snapshot::{
    CapabilitySnapshot, SnapshotGraphEdge, TreeApps, TreeRaw, compose_tree, source_less_apps,
};
use crate::engine::l5::terminal_facts::{TerminalFacts, TerminalSite, TerminalTemp};
use std::io::Write as _;
use std::path::Path;

const WS_GUID: &str = "aaaa2000-0000-4000-8000-0000000000a2";
const DEP_GUID: &str = "bbbb2000-0000-4000-8000-0000000000b2";

/// The dependency (`XW2`, shipped with its sources).
const DEP_TABLES: &str = r#"table 18 Customer
{
    fields
    {
        field(1; "No."; Code[20])
        {
        }
    }
}

table 19 "Dep Ledger"
{
    fields
    {
        field(1; Id; Integer)
        {
        }
    }
}
"#;

const DEP_LIB: &str = r#"codeunit 12 "Dep Rec Lib"
{
    procedure PostWith(Flag: Boolean)
    var
        L: Record "Dep Ledger";
    begin
        if Flag then
            L.Insert();
    end;

    procedure Fill(var Rec: Record "Dep Ledger")
    begin
        Rec.Insert();
    end;

    procedure Log(N: Integer)
    var
        L: Record "Dep Ledger";
    begin
        L.Modify();
    end;

    procedure Log(T: Text)
    var
        C: Record Customer;
    begin
        C.Modify();
    end;
}

interface "Dep IWriter"
{
    procedure Write();
}

codeunit 13 "Dep Writer" implements "Dep IWriter"
{
    procedure Write()
    var
        C: Record Customer;
    begin
        C.Modify();
    end;
}
"#;

/// The workspace: one root per adversarial shape (design §3.4.3 item 5).
const WS_AL: &str = r#"codeunit 50190 "Ws W2"
{
    // Two crossings to one entry point with different literal arguments,
    // after a SHORTER workspace route to the same table.
    procedure TwoCalls()
    var
        Lib: Codeunit "Dep Rec Lib";
    begin
        WriteLedger();
        Lib.PostWith(true);
        Lib.PostWith(false);
    end;

    // The same two crossings with no workspace route.
    procedure TwoCallsOnly()
    var
        Lib: Codeunit "Dep Rec Lib";
    begin
        Lib.PostWith(true);
        Lib.PostWith(false);
    end;

    local procedure WriteLedger()
    var
        L: Record "Dep Ledger";
    begin
        L.Insert();
    end;

    // One interface call: a workspace and a dependency implementer share
    // its call site.
    procedure MixedDispatch(W: Interface "Dep IWriter")
    begin
        W.Write();
    end;

    // A temporary and a persistent insert on one table, the temporary one
    // on the shorter route (in the root itself); and a resource-less commit.
    procedure TempAndPersistent()
    var
        TempL: Record "Dep Ledger" temporary;
    begin
        TempL.Insert();
        WriteLedger();
        Commit();
    end;

    // One parameterized dependency writer called with a temporary and with
    // a persistent argument.
    procedure BothArgs()
    var
        Lib: Codeunit "Dep Rec Lib";
        TempL: Record "Dep Ledger" temporary;
        L: Record "Dep Ledger";
    begin
        Lib.Fill(TempL);
        Lib.Fill(L);
    end;

    // Two overloads with one display, one call each.
    procedure Overloads()
    var
        Lib: Codeunit "Dep Rec Lib";
    begin
        Lib.Log(1);
        Lib.Log('x');
    end;

    // The workspace twin of a parameterized writer.
    procedure WsFill(var R: Record "Dep Ledger")
    begin
        R.Insert();
    end;
}

codeunit 50191 "Ws Writer" implements "Dep IWriter"
{
    procedure Write()
    var
        C: Record Customer;
    begin
        C.Insert();
    end;
}
"#;

fn write_w2(ws: &Path) {
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        format!(
            r#"{{"id":"{WS_GUID}","name":"WsW2","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50100,"to":50199}}],"dependencies":[{{"id":"{DEP_GUID}","name":"XW2","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    )
    .unwrap();
    std::fs::write(ws.join("src/Ws.al"), WS_AL).unwrap();
    let manifest = format!(
        r#"<?xml version="1.0" encoding="utf-8"?><Package xmlns="http://schemas.microsoft.com/navx/2015/manifest"><App Id="{DEP_GUID}" Name="XW2" Publisher="probe" Version="1.0.0.0" Runtime="13.0" /></Package>"#
    );
    let ledger_rec = r#"{"Name":"Rec","IsVar":true,"TypeDefinition":{"Name":"Record","Subtype":{"Name":"Dep Ledger","Id":19}}}"#;
    let symbols = format!(
        r#"{{"RuntimeVersion":"13.0","Tables":[{{"Id":18,"Name":"Customer","Fields":[{{"Id":1,"Name":"No.","TypeDefinition":{{"Name":"Code"}}}}]}},{{"Id":19,"Name":"Dep Ledger","Fields":[{{"Id":1,"Name":"Id","TypeDefinition":{{"Name":"Integer"}}}}]}}],"Interfaces":[{{"Name":"Dep IWriter","Methods":[{{"Name":"Write","Parameters":[]}}]}}],"Codeunits":[{{"Id":12,"Name":"Dep Rec Lib","Methods":[{{"Name":"PostWith","Parameters":[{{"Name":"Flag","TypeDefinition":{{"Name":"Boolean"}}}}]}},{{"Name":"Fill","Parameters":[{ledger_rec}]}},{{"Name":"Log","Parameters":[{{"Name":"N","TypeDefinition":{{"Name":"Integer"}}}}]}},{{"Name":"Log","Parameters":[{{"Name":"T","TypeDefinition":{{"Name":"Text"}}}}]}}]}},{{"Id":13,"Name":"Dep Writer","ImplementedInterfaces":["Dep IWriter"],"Methods":[{{"Name":"Write","Parameters":[]}}]}}],"AppId":"{DEP_GUID}","Name":"XW2","Publisher":"probe","Version":"1.0.0.0"}}"#
    );
    let mut bytes = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut bytes);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, text) in [
            ("NavxManifest.xml", manifest.as_str()),
            ("SymbolReference.json", symbols.as_str()),
            ("src/Tables.al", DEP_TABLES),
            ("src/Lib.al", DEP_LIB),
        ] {
            zip.start_file(name, opts).unwrap();
            zip.write_all(text.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }
    let pkgs = ws.join(".alpackages");
    std::fs::create_dir_all(&pkgs).unwrap();
    let mut out = std::fs::File::create(pkgs.join("probe_XW2_1.0.0.0.app")).unwrap();
    out.write_all(&[0u8; 40]).unwrap();
    out.write_all(bytes.get_ref()).unwrap();
}

/// The cross-app base of the fixture and digest's tree over it.
fn fixture() -> (tempfile::TempDir, R3a5CrossAppBase, CapabilitySnapshot) {
    let tmp = tempfile::tempdir().unwrap();
    write_w2(tmp.path());
    let inputs = crate::program::inputs::WorkspaceInputs::read(tmp.path(), &Default::default());
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
            raw: WitnessScope::All,
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
    (tmp, base, tree)
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

fn effects_of(tree: &CapabilitySnapshot, model: &Model, root: &str) -> Vec<DigestEffectResult> {
    let mut entries = compute_digest_effects_cli(tree, model, Some(&[root.to_string()]), false);
    assert_eq!(entries.len(), 1, "one entry for {root}");
    entries.remove(0).effects
}

/// The call sites of `from`'s edges into `to` (direct, variable-typed or
/// interface calls), in edge order.
fn call_sites(tree: &CapabilitySnapshot, from: &str, to: &str) -> Vec<String> {
    tree.typed_edges
        .iter()
        .filter(|e| e.edge_from() == from && edge_to(e) == Some(to))
        .filter_map(|e| edge_callsite_id(e).map(str::to_string))
        .collect()
}

/// The two crossings of `root` into `PostWith`, by call site, from the
/// tree's crossing index, checked to be typed edge sets of one edge each.
fn post_with_crossings(
    tree: &CapabilitySnapshot,
    model: &Model,
    root: &str,
) -> (Vec<String>, Vec<Arc<Crossing>>) {
    let post_with = stable(model, "Dep Rec Lib", "PostWith");
    let sites = call_sites(tree, root, &post_with);
    assert_eq!(
        sites.len(),
        2,
        "precondition: two calls of PostWith: {sites:?}"
    );
    let idx = build_fingerprint_indexes(tree);
    let crossings: Vec<Arc<Crossing>> = sites
        .iter()
        .map(|s| {
            Arc::clone(
                idx.crossings
                    .get(&format!("cs:{s}"))
                    .expect("one crossing per call site"),
            )
        })
        .collect();
    for (cs, c) in sites.iter().zip(&crossings) {
        let site = CrossingSite::Callsite(cs.clone());
        assert_eq!(c.site, site);
        assert_eq!(c.targets, vec![post_with.clone()]);
        assert_eq!(c.edges.len(), 1, "one typed edge: {:?}", c.edges);
        assert_eq!(
            (
                c.edges[0].from.as_str(),
                c.edges[0].to.as_str(),
                &c.edges[0].site
            ),
            (root, post_with.as_str(), &site)
        );
        assert_eq!(c.edges[0].kind, "variable-typed-call");
        assert_eq!(c.sites.len(), 1, "a call crossing has one site");
        assert!(c.contains(root, &post_with, &site));
    }
    let (a, b) = (&crossings[0], &crossings[1]);
    assert!(
        !a.contains(root, &post_with, &b.site),
        "a crossing never holds another's edge"
    );
    assert_ne!(a.sites, b.sites, "two sites, two anchors");
    (sites, crossings)
}

/// §3.3 and §3.4.3 item 5: two calls of one entry point with different
/// literal arguments, after a SHORTER workspace route to the same table, are
/// two crossings, each its own typed edge set. Today's digest shows neither:
/// its one `Dep Ledger` row is witnessed through the workspace route — the
/// shape behind G1 (design §1 item 11), which W2-0b's crossing-constrained
/// search must not repeat. Pinned so that change shows as a moved assertion.
/// Break: grouping every boundary edge of one target into one crossing (the
/// per-site lookups fail).
#[test]
fn a_shorter_workspace_route_hides_both_crossings_today() {
    let (_tmp, base, tree) = fixture();
    let model = &base.resolved;
    let root = stable(model, "Ws W2", "TwoCalls");
    post_with_crossings(&tree, model, &root);
    let effects = effects_of(&tree, model, &root);
    assert!(
        effects
            .iter()
            .all(|e| !e.boundary_group && e.crossing.is_none()),
        "no group row today: {:?}",
        effects.iter().map(|e| &e.detail).collect::<Vec<_>>()
    );
}

/// §3.3: with no workspace route, the group row beyond `PostWith` carries
/// its own crossing's record, whose site is the row's identity. Only the
/// `PostWith(true)` crossing has a row today (the witness walk already
/// follows the literal argument at the crossing); the `false` crossing is in
/// the index with no row. Pinned so W2-0c's crossing-local status (§3.3.1)
/// is measured against it.
/// Breaks: dropping the group row's record (`crossing: None`); taking
/// another identity's record (the site assertion).
#[test]
fn a_crossings_group_rows_carry_its_record() {
    let (_tmp, base, tree) = fixture();
    let model = &base.resolved;
    let root = stable(model, "Ws W2", "TwoCallsOnly");
    let (sites, crossings) = post_with_crossings(&tree, model, &root);
    let true_site = sites
        .iter()
        .find(|s| {
            model.workspace.routines.iter().any(|r| {
                r.call_sites.iter().any(|cs| {
                    &cs.id == *s
                        && cs
                            .argument_texts
                            .first()
                            .is_some_and(|t| t.trim() == "true")
                })
            })
        })
        .expect("precondition: one call passes true");
    let effects = effects_of(&tree, model, &root);
    let row_crossings: BTreeSet<String> = effects
        .iter()
        .filter(|e| e.boundary_group)
        .map(|e| {
            let c = e
                .crossing
                .as_ref()
                .expect("every group row has its crossing");
            assert_eq!(
                e.evidence_callsite_id.as_deref(),
                c.site.guarded_id(),
                "its own identity's record"
            );
            assert!(
                crossings.iter().any(|k| **k == **c),
                "a record of the index"
            );
            c.id.clone()
        })
        .collect();
    assert_eq!(
        row_crossings,
        BTreeSet::from([format!("cs:{true_site}")]),
        "the true crossing's group only"
    );
}

/// §3.3, astra 2: one interface call whose implementers are a workspace
/// codeunit and a dependency codeunit has ONE call-site id and two edges;
/// only the dependency edge is the crossing's, so membership by the bare site
/// id would wrongly count the workspace edge as crossed.
/// Break: dropping `build_crossings`' "into a dependency routine" filter puts
/// the workspace edge in the set.
#[test]
fn a_workspace_edge_sharing_a_crossings_site_is_not_in_it() {
    let (_tmp, base, tree) = fixture();
    let model = &base.resolved;
    let root = stable(model, "Ws W2", "MixedDispatch");
    let ws_write = stable(model, "Ws Writer", "Write");
    let dep_write = stable(model, "Dep Writer", "Write");
    let ws_sites = call_sites(&tree, &root, &ws_write);
    let dep_sites = call_sites(&tree, &root, &dep_write);
    assert_eq!(
        ws_sites.len(),
        1,
        "precondition: one edge to the workspace implementer"
    );
    assert_eq!(
        ws_sites, dep_sites,
        "precondition: both edges share one call site"
    );
    let site = CrossingSite::Callsite(ws_sites[0].clone());

    let idx = build_fingerprint_indexes(&tree);
    let c = idx
        .crossings
        .get(&format!("cs:{}", ws_sites[0]))
        .expect("the call is a crossing");
    assert_eq!(
        c.targets,
        vec![dep_write.clone()],
        "only the dependency target"
    );
    assert_eq!(c.edges.len(), 1, "{:?}", c.edges);
    assert!(c.contains(&root, &dep_write, &site));
    assert!(
        !c.contains(&root, &ws_write, &site),
        "the workspace edge shares the site id but crosses nothing"
    );
    assert_eq!(c.edges[0].kind, "interface-dispatch");
    // The group row beyond the dependency implementer carries that record.
    let effects = effects_of(&tree, model, &root);
    let groups: Vec<&DigestEffectResult> = effects.iter().filter(|e| e.boundary_group).collect();
    assert!(
        !groups.is_empty(),
        "precondition: a group beyond Dep Writer"
    );
    assert!(groups.iter().all(|e| e.crossing.as_deref() == Some(&**c)));
}

/// §3.3 (Fable N3) and §3.4.3 item 1 (Fable N1): an event crossing is one per
/// EVENT, so two workspace publishers of one event with a dependency
/// subscriber are one crossing with two sites; a workspace subscriber of the
/// same event is no part of it; and the literal search's own adjacency names
/// the event on each event edge. The edges are stated by hand (a dependency
/// cannot subscribe to a workspace event in compiled AL), the
/// `f1_s2`-style precondition of `digest_rule_b_tests`.
/// Breaks: `edge_parts` giving an event edge no site (the adjacency
/// assertion fails); keeping only the first site anchor (`sites.len()`).
#[test]
fn an_event_crossing_is_one_per_event_with_every_publish_site() {
    let (_tmp, base, mut tree) = fixture();
    let model = &base.resolved;
    let (pub_a, pub_b) = (
        stable(model, "Ws W2", "TwoCalls"),
        stable(model, "Ws W2", "BothArgs"),
    );
    let dep_sub = stable(model, "Dep Rec Lib", "Fill");
    let ws_sub = stable(model, "Ws W2", "WsFill");
    let anchor_of = |r: &str| {
        tree.typed_edges
            .iter()
            .find_map(|e| match e {
                SnapshotGraphEdge::DirectCall {
                    from,
                    source_anchor,
                    ..
                }
                | SnapshotGraphEdge::VariableTypedCall {
                    from,
                    source_anchor,
                    ..
                } if from == r => Some(source_anchor.clone()),
                _ => None,
            })
            .expect("precondition: the publisher makes a call (an anchor to borrow)")
    };
    let (a_at, b_at) = (anchor_of(&pub_a), anchor_of(&pub_b));
    assert_ne!(a_at, b_at, "precondition: two publish anchors");
    let event = "test/Codeunit/50190/event/onw2".to_string();
    let edge =
        |from: &str, to: &str, at: &crate::engine::l5::snapshot::SnapshotSourceAnchor, n: u32| {
            SnapshotGraphEdge::EventDispatch {
                kind: "event-dispatch",
                from: from.to_string(),
                to: to.to_string(),
                event_id: event.clone(),
                publish_anchor: at.clone(),
                subscriber_anchor: at.clone(),
                edge_id: format!("w2-event-{n}"),
            }
        };
    let added = [
        edge(&pub_a, &dep_sub, &a_at, 1),
        edge(&pub_b, &dep_sub, &b_at, 2),
        edge(&pub_a, &ws_sub, &a_at, 3),
    ];
    std::sync::Arc::make_mut(&mut tree.typed_edges).extend(added);

    let idx = build_fingerprint_indexes(&tree);
    let c = idx
        .crossings
        .get(&format!("ev:{event}"))
        .expect("an event crossing");
    let site = CrossingSite::Event(event.clone());
    assert_eq!(c.site, site);
    assert_eq!(c.site.guarded_id(), None, "an event edge has no guard");
    assert_eq!(c.targets, vec![dep_sub.clone()]);
    assert_eq!(
        c.edges.len(),
        2,
        "both publishers' edges, not the workspace subscriber's"
    );
    assert!(c.contains(&pub_a, &dep_sub, &site) && c.contains(&pub_b, &dep_sub, &site));
    assert!(!c.contains(&pub_a, &ws_sub, &site));
    assert_eq!(
        c.sites.len(),
        2,
        "one site per publish anchor: {:?}",
        c.sites
    );

    let reach = LiteralReach::new(&tree.typed_edges, &tree.capability_facts, model);
    let mut from_a = reach.event_edges_from(&pub_a);
    from_a.sort_unstable();
    let mut want = vec![
        (dep_sub.as_str(), event.as_str()),
        (ws_sub.as_str(), event.as_str()),
    ];
    want.sort_unstable();
    assert_eq!(
        from_a, want,
        "every event edge of the search carries its event id"
    );
}

/// §3.4.1, astra 3: the terminal records keep what a class-correct witness
/// needs. On one table: the root's temporary `Insert` (the SHORTER route)
/// and the helper's persistent one are two records with two classes; a
/// parameterized writer (workspace and dependency) is a raw producer; the
/// resource-less `Commit` has its own record; every record's source fact is
/// the fact it was read from. And the digest's rows today choose by class:
/// the physical row's evidence is the persistent operation, never the
/// shorter temporary one, and the dependency writer called with both
/// arguments gives one temporary and one non-temporary group, each on its
/// own crossing — the behaviour W2-0b's witnesses must keep.
/// Breaks: dropping the temp state from a record (every class assertion
/// fails); reading the digest type instead of `opSubtype` (the op kind).
#[test]
fn terminal_records_keep_class_producer_and_original_kind() {
    let (_tmp, base, tree) = fixture();
    let model = &base.resolved;
    let (tp, write_ledger, fill, ws_fill, both) = (
        stable(model, "Ws W2", "TempAndPersistent"),
        stable(model, "Ws W2", "WriteLedger"),
        stable(model, "Dep Rec Lib", "Fill"),
        stable(model, "Ws W2", "WsFill"),
        stable(model, "Ws W2", "BothArgs"),
    );
    let ledger = format!("{DEP_GUID}/table/19");
    // The literal search's own index (the production holder), and a fresh
    // one over the same facts: equal record for record.
    let reach = LiteralReach::new(&tree.typed_edges, &tree.capability_facts, model);
    let terms = reach.terminals();
    assert_eq!(
        terms.records(),
        TerminalFacts::new(&tree.capability_facts).records(),
        "LiteralReach holds the tree's terminal index"
    );
    for r in terms.records() {
        let f = &tree.capability_facts[r.fact];
        assert_eq!(
            (f.subject(), f.provenance()),
            (r.owner, "direct"),
            "source fact identity"
        );
        assert_eq!(f.resource_id(), r.resource);
    }
    let inserts: Vec<(&str, TerminalTemp, Option<&str>)> = terms
        .of_kind(("DB_INSERT", Some(ledger.as_str())))
        .map(|r| (r.owner, r.temp, r.op_kind))
        .collect();
    let has = |owner: &str, temp: TerminalTemp| {
        inserts
            .iter()
            .any(|&(o, t, k)| o == owner && t == temp && k == Some("Insert"))
    };
    assert!(
        has(&tp, TerminalTemp::Known(true)),
        "the root's temporary insert: {inserts:?}"
    );
    assert!(
        has(&write_ledger, TerminalTemp::Known(false)),
        "the persistent insert: {inserts:?}"
    );
    assert!(
        has(&fill, TerminalTemp::ParameterDependent(0)),
        "the dependency producer: {inserts:?}"
    );
    assert!(
        has(&ws_fill, TerminalTemp::ParameterDependent(0)),
        "the workspace producer: {inserts:?}"
    );
    assert!(
        terms
            .of_kind(("DB_INSERT", Some(ledger.as_str())))
            .filter(|r| r.owner == fill)
            .all(|r| r.producer()),
        "a parameter-dependent record is a producer"
    );
    let commits: Vec<_> = terms
        .of_kind(("COMMIT", None))
        .filter(|r| r.owner == tp)
        .collect();
    assert_eq!(commits.len(), 1, "the resource-less commit has a record");
    let TerminalSite::Operation(commit_op) = commits[0].terminal else {
        panic!("a commit sits at an operation: {:?}", commits[0]);
    };
    assert_eq!(terms.at(commit_op).count(), 1, "found by its terminal id");

    // The root's own rows on the table pick their evidence by class.
    let op_of = |owner: &str, temp: TerminalTemp| {
        terms
            .of_kind(("DB_INSERT", Some(ledger.as_str())))
            .find(|r| r.owner == owner && r.temp == temp)
            .map(|r| r.terminal.id().to_string())
            .unwrap()
    };
    let (temp_op, persistent_op) = (
        op_of(&tp, TerminalTemp::Known(true)),
        op_of(&write_ledger, TerminalTemp::Known(false)),
    );
    let effects = effects_of(&tree, model, &tp);
    let rows: Vec<(bool, Option<&str>)> = effects
        .iter()
        .filter(|e| {
            e.effect_type == "DB_INSERT"
                && !e.boundary_group
                && e.detail
                    .iter()
                    .any(|(k, v)| k == "resourceId" && *v == ledger)
        })
        .map(|e| {
            (
                is_known_temp_state(e.temp_state.as_ref()),
                e.evidence_operation_id.as_deref(),
            )
        })
        .collect();
    assert!(
        rows.contains(&(true, Some(temp_op.as_str()))),
        "the temporary row: {rows:?}"
    );
    assert!(
        rows.contains(&(false, Some(persistent_op.as_str()))),
        "the physical row ends at the persistent insert, not the shorter temporary one: {rows:?}"
    );

    // One dependency writer, called with both arguments: one group per class,
    // each on its own call site's crossing.
    let effects = effects_of(&tree, model, &both);
    let groups: BTreeSet<(bool, String)> = effects
        .iter()
        .filter(|e| e.boundary_group && e.effect_type == "DB_INSERT")
        .map(|e| {
            (
                is_known_temp_state(e.temp_state.as_ref()),
                e.crossing.as_ref().unwrap().id.clone(),
            )
        })
        .collect();
    let sites = call_sites(&tree, &both, &fill);
    assert_eq!(sites.len(), 2, "precondition: two calls of Fill");
    let classes: BTreeSet<bool> = groups.iter().map(|(kt, _)| *kt).collect();
    assert_eq!(
        classes,
        BTreeSet::from([false, true]),
        "a temporary and a non-temporary group: {groups:?}"
    );
    let ids: BTreeSet<String> = groups.iter().map(|(_, id)| id.clone()).collect();
    assert_eq!(
        ids,
        sites.iter().map(|s| format!("cs:{s}")).collect(),
        "each on its own crossing"
    );
}

/// §3.3, T-W2-24's W2-0a half: two overloads share one display, so a crossing
/// names its targets by STABLE id; the two calls are two crossings whose
/// targets differ although a display-keyed join would merge them.
/// Break: building `targets` from `boundary_display` (the targets compare
/// equal).
#[test]
fn overloads_with_one_display_are_two_targets() {
    let (_tmp, base, tree) = fixture();
    let model = &base.resolved;
    let root = stable(model, "Ws W2", "Overloads");
    let logs: Vec<&str> = model
        .workspace
        .routines
        .iter()
        .filter(|r| r.name == "Log")
        .map(|r| r.stable_routine_id.as_str())
        .collect();
    assert_eq!(logs.len(), 2, "precondition: two Log overloads");
    assert_ne!(logs[0], logs[1], "precondition: two stable ids");
    let idx = build_fingerprint_indexes(&tree);
    assert_eq!(
        boundary_display(logs[0], &idx),
        boundary_display(logs[1], &idx),
        "precondition: one display"
    );
    let mut targets: Vec<Vec<String>> = logs
        .iter()
        .map(|l| {
            let sites = call_sites(&tree, &root, l);
            assert_eq!(sites.len(), 1, "precondition: one call of each overload");
            idx.crossings[&format!("cs:{}", sites[0])].targets.clone()
        })
        .collect();
    targets.sort();
    let mut want: Vec<Vec<String>> = logs.iter().map(|l| vec![(*l).to_string()]).collect();
    want.sort();
    assert_eq!(targets, want, "each crossing enters its own overload");
}
