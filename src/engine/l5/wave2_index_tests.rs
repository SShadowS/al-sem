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
    let (base, tree) = tree_over(tmp.path(), WitnessScope::All);
    (tmp, base, tree)
}

/// The cross-app base of `ws` and digest's tree over it, its raw witness
/// facts kept for `scope`.
pub(crate) fn tree_over(ws: &Path, scope: WitnessScope) -> (R3a5CrossAppBase, CapabilitySnapshot) {
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

pub(crate) fn effects_of(
    tree: &CapabilitySnapshot,
    model: &Model,
    root: &str,
) -> Vec<DigestEffectResult> {
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
    // The table rows: the statement `L.Insert()`'s implicit error (G20) is a
    // resource-less kind, which gets a group row per crossing whatever the
    // literal argument (`literal_reach` judges a group by its crossing).
    let effects: Vec<_> = effects_of(&tree, model, &root)
        .into_iter()
        .filter(|e| e.effect_type.starts_with("DB_"))
        .collect();
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
    // The table group rows (the statement insert's resource-less
    // `ERROR_IMPLICIT`, G20, has a group row at each crossing).
    let row_crossings: BTreeSet<String> = effects
        .iter()
        .filter(|e| e.boundary_group && e.effect_type.starts_with("DB_"))
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

    let reach = LiteralReach::new(&tree, model);
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
    let reach = LiteralReach::new(&tree, model);
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

/// The G1 shape (design §1 item 11, §3.4.3 item 5), stated on the fixture:
/// root `TwoCalls` writes `Dep Ledger` through the SHORTER workspace route
/// (`WriteLedger`, one hop) and also calls `PostWith(true)` and
/// `PostWith(false)`. A hand-stated re-entry edge (an implicit trigger that
/// `PostWith`'s own `if Flag then L.Insert()` fires, into `WriteLedger`)
/// makes `WriteLedger`'s insert reachable beyond the crossing too, but only
/// when `Flag` is true. The group row is the `TwoCallsOnly` group's, given
/// `TwoCalls`' crossing and that one operation (as CDO's RunReportID group
/// holds a workspace operation reached by re-entry).
struct G1 {
    _tmp: tempfile::TempDir,
    base: R3a5CrossAppBase,
    tree: CapabilitySnapshot,
    root: String,
    post_with: String,
    write_ledger: String,
    /// `WriteLedger`'s insert (the operation the group lists).
    ws_op: String,
    /// `PostWith`'s own insert (behind `if Flag`).
    dep_op: String,
    true_site: String,
    false_site: String,
}

fn g1() -> G1 {
    let (tmp, base, mut tree) = fixture();
    let model = &base.resolved;
    let root = stable(model, "Ws W2", "TwoCalls");
    let post_with = stable(model, "Dep Rec Lib", "PostWith");
    let write_ledger = stable(model, "Ws W2", "WriteLedger");
    let op_of = |stable: &str| {
        let ops: Vec<String> = model
            .workspace
            .routines
            .iter()
            .filter(|r| r.stable_routine_id == stable)
            .flat_map(|r| r.record_operations.iter().map(|o| o.id.clone()))
            .collect();
        assert_eq!(
            ops.len(),
            1,
            "precondition: one record operation in {stable}"
        );
        ops[0].clone()
    };
    let (ws_op, dep_op) = (op_of(&write_ledger), op_of(&post_with));
    let site_with = |arg: &str| {
        call_sites(&tree, &root, &post_with)
            .into_iter()
            .find(|s| {
                model.workspace.routines.iter().any(|r| {
                    r.call_sites.iter().any(|cs| {
                        &cs.id == s && cs.argument_texts.first().is_some_and(|t| t.trim() == arg)
                    })
                })
            })
            .unwrap_or_else(|| panic!("precondition: a PostWith({arg}) call"))
    };
    let (true_site, false_site) = (site_with("true"), site_with("false"));
    std::sync::Arc::make_mut(&mut tree.typed_edges).push(SnapshotGraphEdge::ImplicitTrigger {
        kind: "implicit-trigger",
        from: post_with.clone(),
        to: write_ledger.clone(),
        operation_id: dep_op.clone(),
        trigger_kind: "OnInsert".to_string(),
        fires: crate::engine::l4::combined_graph::TriggerFires::Always,
        anchor: None,
        edge_id: "w2-g1-reentry".to_string(),
    });
    G1 {
        _tmp: tmp,
        base,
        tree,
        root,
        post_with,
        write_ledger,
        ws_op,
        dep_op,
        true_site,
        false_site,
    }
}

impl G1 {
    /// A group row on `TwoCalls`' crossing at `site`, listing `Dep Ledger`
    /// with the operation `op`.
    fn group_row(&self, site: &str, op: &str) -> DigestEffectResult {
        let model = &self.base.resolved;
        let only = stable(model, "Ws W2", "TwoCallsOnly");
        let mut row = effects_of(&self.tree, model, &only)
            .into_iter()
            .find(|e| e.boundary_group)
            .expect("precondition: TwoCallsOnly has a group row");
        let idx = build_fingerprint_indexes(&self.tree);
        row.crossing = Some(Arc::clone(&idx.crossings[&format!("cs:{site}")]));
        row.group_resource_ops = vec![(format!("{DEP_GUID}/table/19"), Some(vec![op.to_string()]))];
        row
    }

    fn reach(&self) -> LiteralReach<'_> {
        LiteralReach::new(&self.tree, &self.base.resolved)
    }
}

/// W2-0b (G1): a group's witness traverses ITS crossing, with the bindings
/// the prefix established. Through the `true` crossing the path is
/// `TwoCalls → PostWith → WriteLedger`, never the shorter workspace route
/// `TwoCalls → WriteLedger` the old root-ordered search returned; through
/// the `false` crossing there is none (the re-entry edge needs `Flag`).
/// Breaks: dropping the crossing condition (first live state of the root
/// search) gives the workspace route; restarting the suffix with empty
/// bindings gives the `false` crossing a witness.
#[test]
fn g1_a_group_witness_traverses_its_crossing_with_its_bindings() {
    let g = g1();
    let model = &g.base.resolved;
    let reach = g.reach();
    let tree = reach.live_tree(&g.root).expect("the root's search");
    let paths = crate::engine::l5::group_path::GroupPaths::new(&g.tree.typed_edges, model);
    let mut cache = crate::engine::l5::literal_reach::SuffixCache::default();
    let run = |site: &str, cache: &mut _| {
        let row = g.group_row(site, &g.ws_op);
        let c = row.crossing.clone().unwrap();
        let entries = reach.crossing_entries(&tree, &c);
        assert_eq!(entries.len(), 1, "precondition: one entry state per call");
        paths.paths(
            &reach,
            &tree,
            &entries,
            &row,
            &row.group_resource_ops,
            cache,
            &|| false,
        )
    };
    let got = run(&g.true_site, &mut cache);
    assert_eq!(got.len(), 1, "a witness through the true crossing: {got:?}");
    let p = got[0].1.as_ref().expect("a path");
    assert!(p.through_entry_point && p.reason.is_none());
    assert_eq!(p.crossing, format!("cs:{}", g.true_site));
    let to: Vec<&str> = p.steps.iter().map(|s| s.to.as_str()).collect();
    assert_eq!(
        to,
        vec![g.post_with.as_str(), g.write_ledger.as_str()],
        "{p:?}"
    );
    assert_eq!(
        p.steps[0].edge_id.as_deref(),
        Some(g.true_site.as_str()),
        "the crossing's own edge"
    );
    assert_eq!(p.steps[1].edge, "implicit-trigger");
    assert_eq!(p.steps[1].edge_id.as_deref(), Some(g.dep_op.as_str()));
    assert!(
        p.steps[0].line.is_some() && p.steps[0].file.is_some(),
        "a call site's anchor"
    );
    assert_eq!(p.terminal.operation_id, g.ws_op);
    assert!(p.terminal.line.is_some());
    assert_eq!(
        p.hops,
        vec![
            "Ws W2.TwoCalls".to_string(),
            "variable-typed-call → Dep Rec Lib.PostWith".to_string(),
            "implicit-trigger → Ws W2.WriteLedger".to_string(),
        ],
        "the string hops keep their shape"
    );
    assert_eq!(p.steps[0].display, "Codeunit Dep Rec Lib::PostWith");
    // The false crossing: the re-entry needs Flag = true.
    let none = run(&g.false_site, &mut cache);
    assert_eq!(none.len(), 1);
    assert_eq!(
        none[0].1.as_ref().err().map(|n| n.reason),
        Some("no-witness"),
        "no witness through the false crossing: {none:?}"
    );
}

/// W2-0b (§3.4.3 item 3): when a suffix search stops early, the root tree's
/// own path is the fallback, CHECKED for crossing membership: the workspace
/// route is `Outside` (labelled, never "through"); a root path that does
/// traverse the crossing (to `PostWith`'s own insert) is an ordinary
/// witness. A cancelled search is the same outcome, its reason `cancelled`.
/// And an operation the root cannot reach at all has no path.
/// Breaks: `throughEntryPoint: true` for every fallback; skipping the
/// membership check.
#[test]
fn g1_a_capped_suffix_falls_back_to_a_checked_root_path() {
    let g = g1();
    let model = &g.base.resolved;
    let reach = g.reach();
    let tree = reach.live_tree(&g.root).expect("the root's search");
    let paths = crate::engine::l5::group_path::GroupPaths::new(&g.tree.typed_edges, model);
    let witness = |op: &str, per_tree: usize, cancel: &dyn Fn() -> bool| {
        let row = g.group_row(&g.true_site, op);
        let c = row.crossing.clone().unwrap();
        let entries = reach.crossing_entries(&tree, &c);
        let cache = crate::engine::l5::literal_reach::SuffixCache::with_limits(per_tree, per_tree);
        paths.paths(
            &reach,
            &tree,
            &entries,
            &row,
            &row.group_resource_ops,
            &cache,
            cancel,
        )
    };
    // Capped (a 0-state suffix budget): the workspace route, labelled.
    let one = |got: Vec<(String, crate::engine::l5::group_path::GroupPathOutcome)>| {
        assert_eq!(got.len(), 1, "{got:?}");
        got.into_iter().next().unwrap().1
    };
    let p = one(witness(&g.ws_op, 0, &|| false)).expect("the fallback path");
    assert!(!p.through_entry_point, "{p:?}");
    assert_eq!(p.reason, Some("search-cap"));
    let to: Vec<&str> = p.steps.iter().map(|s| s.to.as_str()).collect();
    assert_eq!(to, vec![g.write_ledger.as_str()], "the root's own route");
    // Capped, but the root path does traverse the crossing: a witness.
    let p = one(witness(&g.dep_op, 0, &|| false)).expect("a path");
    assert!(p.through_entry_point, "{p:?}");
    assert_eq!(p.steps[0].edge_id.as_deref(), Some(g.true_site.as_str()));
    // Cancelled: the same fallback, its own reason.
    let cap = crate::engine::l5::literal_reach::STATE_CAP;
    let p = one(witness(&g.ws_op, cap, &|| true)).expect("the fallback path");
    assert_eq!(p.reason, Some("cancelled"));
    // Uncapped, the true crossing's suffix completes: a real witness.
    let p = one(witness(&g.ws_op, cap, &|| false)).expect("a path");
    assert!(p.through_entry_point);
    // An operation nothing reaches: no path, with the stop that left it open
    // when a search was capped, and "no witness" when every search completed.
    let unreachable = model
        .workspace
        .routines
        .iter()
        .find(|r| r.name == "Write" && r.app_guid == DEP_GUID)
        .and_then(|r| r.record_operations.first())
        .map(|o| o.id.clone())
        .expect("precondition: Dep Writer's Modify");
    let reason = |per_tree| {
        one(witness(&unreachable, per_tree, &|| false))
            .err()
            .map(|n| n.reason)
    };
    assert_eq!(reason(0), Some("search-cap"));
    assert_eq!(reason(cap), Some("no-witness"));
}

/// W2-0b (§3.4.3 item 3): the suffix cache keeps an aggregate budget by
/// evicting the OLDEST tree first, rebuilds an evicted one with the same
/// result, and the witnesses do not depend on the order crossings are asked
/// in.
/// Break: no eviction (`held` exceeds the budget, nothing evicted).
#[test]
fn g1_the_suffix_cache_evicts_oldest_first_and_is_order_free() {
    let g = g1();
    let model = &g.base.resolved;
    let reach = g.reach();
    let tree = reach.live_tree(&g.root).expect("the root's search");
    let paths = crate::engine::l5::group_path::GroupPaths::new(&g.tree.typed_edges, model);
    let ask = |site: &str, cache: &mut _| {
        let row = g.group_row(site, &g.ws_op);
        let c = row.crossing.clone().unwrap();
        let entries = reach.crossing_entries(&tree, &c);
        paths.paths(
            &reach,
            &tree,
            &entries,
            &row,
            &row.group_resource_ops,
            cache,
            &|| false,
        )
    };
    // Each suffix tree's size, alone.
    let size = |site: &str| {
        let mut c = crate::engine::l5::literal_reach::SuffixCache::default();
        ask(site, &mut c);
        c.stats().held
    };
    let (nt, nf) = (size(&g.true_site), size(&g.false_site));
    assert!(
        nt > 0 && nf > 0,
        "precondition: two non-empty trees ({nt}, {nf})"
    );
    // Room for one tree: the second entry state evicts the first.
    let room = nt.max(nf);
    let mut small = crate::engine::l5::literal_reach::SuffixCache::with_limits(room, room);
    let a1 = ask(&g.true_site, &mut small);
    let b1 = ask(&g.false_site, &mut small);
    assert!(
        small.stats().held <= room,
        "the budget holds: {} > {room}",
        small.stats().held
    );
    assert_eq!(small.stats().evicted, 1, "the older tree went");
    assert_eq!(
        small.stats().held,
        nf,
        "the newer (false) tree is the one kept"
    );
    let a2 = ask(&g.true_site, &mut small);
    assert_eq!(small.stats().built, 3, "the evicted tree was rebuilt");
    assert_eq!(a1, a2, "a rebuilt tree answers the same");
    // Order-free.
    let mut fresh = crate::engine::l5::literal_reach::SuffixCache::default();
    let b2 = ask(&g.false_site, &mut fresh);
    let a3 = ask(&g.true_site, &mut fresh);
    assert_eq!((a1, b1), (a3, b2), "the asking order changes nothing");
}

/// TLS-22: the suffix cache shared by the literal pass's workers. On the G1
/// fixture the `true` and `false` crossings enter one dependency routine under
/// opposing bindings whose trees answer differently (asserted), so a cache
/// keyed without the bindings is caught. Two workers ask both crossings at
/// once, the first two misses held until both have missed (so both build the
/// first tree): each gets the serial answers, one copy of each tree is held,
/// and the second copy counts as a duplicate. A cancelled search is not kept.
///
/// DISCRIMINATION (2026-11-11): keying the cache by the target alone fails
/// the `false` answers; inserting the second copy too (adding to `held`
/// twice) fails `held`; restored, passes.
#[test]
fn g1_shared_suffix_cache_keeps_one_tree_per_entry_state() {
    use crate::engine::l5::literal_reach::SuffixCache;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let g = g1();
    let model = &g.base.resolved;
    let reach = g.reach();
    let tree = reach.live_tree(&g.root).expect("the root's search");
    let paths = crate::engine::l5::group_path::GroupPaths::new(&g.tree.typed_edges, model);
    let ask = |site: &str, cache: &_, cancel: &(dyn Fn() -> bool + Sync)| {
        let row = g.group_row(site, &g.ws_op);
        let c = row.crossing.clone().unwrap();
        let entries = reach.crossing_entries(&tree, &c);
        paths.paths(
            &reach,
            &tree,
            &entries,
            &row,
            &row.group_resource_ops,
            cache,
            cancel,
        )
    };
    let never = || false;
    // Serial, each alone in a fresh cache.
    let alone = |site: &str| {
        let c = SuffixCache::default();
        (ask(site, &c, &never), c.stats().held)
    };
    let ((want_t, nt), (want_f, nf)) = (alone(&g.true_site), alone(&g.false_site));
    assert_ne!(
        want_t, want_f,
        "precondition: the opposing bindings' trees answer differently"
    );

    let shared = SuffixCache::default();
    let misses = Arc::new(AtomicUsize::new(0));
    {
        let misses = Arc::clone(&misses);
        shared.set_miss_hook(Arc::new(move || {
            // The first two misses wait (at most 10 s) until both happened.
            if misses.fetch_add(1, Ordering::SeqCst) < 2 {
                let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
                while misses.load(Ordering::SeqCst) < 2 && std::time::Instant::now() < until {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
        }));
    }
    let worker = || {
        (
            ask(&g.true_site, &shared, &never),
            ask(&g.false_site, &shared, &never),
        )
    };
    let (a, b) = std::thread::scope(|s| {
        let a = s.spawn(worker);
        let b = s.spawn(worker);
        (a.join().unwrap(), b.join().unwrap())
    });
    for (who, (t, f)) in [("first", a), ("second", b)] {
        assert_eq!(
            t, want_t,
            "the {who} worker's `true` answer is the serial one"
        );
        assert_eq!(
            f, want_f,
            "the {who} worker's `false` answer is the serial one"
        );
    }
    let st = shared.stats();
    assert!(
        st.built >= 3,
        "precondition: both workers built the first tree ({st:?})"
    );
    assert_eq!(st.held, nt + nf, "one copy of each tree is held ({st:?})");
    assert_eq!(
        st.duplicate,
        st.built - 2,
        "every extra copy is a dropped duplicate ({st:?})"
    );

    // A cancelled search is not kept: the next ask builds and answers.
    let c = SuffixCache::default();
    let _ = ask(&g.true_site, &c, &|| true);
    assert_eq!(c.stats().held, 0, "a cancelled tree is not held");
    assert!(
        c.stats().built >= 1,
        "precondition: the search ran and was cancelled"
    );
    assert_eq!(ask(&g.true_site, &c, &never), want_t);
}

/// TLS-22: eviction under threads. A budget of one tree, the same per-tree
/// cap as the oracle, four workers asking both crossings twice: every answer
/// equals the serial run with the default budget, though trees were evicted
/// and rebuilt in an order only timing decides.
///
/// DISCRIMINATION (2026-11-11): a rebuilt tree searched under cap 0 (a
/// capped tree answers differently: asserted) fails; restored, passes.
#[test]
fn g1_eviction_under_threads_answers_as_serial() {
    use crate::engine::l5::literal_reach::SuffixCache;
    let g = g1();
    let model = &g.base.resolved;
    let reach = g.reach();
    let tree = reach.live_tree(&g.root).expect("the root's search");
    let paths = crate::engine::l5::group_path::GroupPaths::new(&g.tree.typed_edges, model);
    let ask = |site: &str, cache: &_| {
        let row = g.group_row(site, &g.ws_op);
        let c = row.crossing.clone().unwrap();
        let entries = reach.crossing_entries(&tree, &c);
        paths.paths(
            &reach,
            &tree,
            &entries,
            &row,
            &row.group_resource_ops,
            cache,
            &|| false,
        )
    };
    let size = |site: &str| {
        let c = SuffixCache::default();
        let _ = ask(site, &c);
        c.stats().held
    };
    let room = size(&g.true_site).max(size(&g.false_site));
    let serial = SuffixCache::default();
    let want = (ask(&g.true_site, &serial), ask(&g.false_site, &serial));
    // The break must be visible: a tree searched under another cap (0: G1's
    // trees are small, and a tree may hold `cap + 1` states) answers
    // differently.
    assert_ne!(
        ask(&g.true_site, &SuffixCache::with_limits(0, room)),
        want.0,
        "precondition: a capped tree answers differently"
    );

    let tight = SuffixCache::with_limits(room, room);
    let got: Vec<_> = std::thread::scope(|s| {
        let hs: Vec<_> = (0..4)
            .map(|_| {
                s.spawn(|| {
                    (0..2)
                        .map(|_| (ask(&g.true_site, &tight), ask(&g.false_site, &tight)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    for (t, f) in &got {
        assert_eq!((t, f), (&want.0, &want.1), "an answer under eviction");
    }
    let st = tight.stats();
    assert!(st.evicted > 0, "precondition: trees were evicted ({st:?})");
    assert!(st.built > 2, "precondition: a tree was rebuilt ({st:?})");
}

/// W2-0c (design §3.3.1): a resource's status beyond ONE crossing, from its
/// own entry states. On the G1 fixture, over every non-temporary `Dep Ledger`
/// insert (the terminal records): the `true` crossing reaches it
/// (`Reachable`); the `false` crossing reaches `PostWith`'s owner with guards
/// ignored but no state lets an insert run (`Disproved`). Nothing concluded
/// (`NotEstablished`) when the suffix search caps, when no candidate's owner
/// is reached even with guards ignored (Dep Writer's `Modify`: a graph the
/// search does not follow), or when the crossing has no entry.
/// Breaks: `Disproved` on a capped search; `Disproved` without the
/// guards-ignored reach (graph mismatch).
#[test]
fn w2_0c_a_resource_is_judged_from_its_own_crossing() {
    use crate::engine::l5::literal_reach::{ResourceStatus as S, SuffixCache, Undecided as U};
    let g = g1();
    let model = &g.base.resolved;
    let reach = g.reach();
    let tree = reach.live_tree(&g.root).expect("the root's search");
    let ledger = format!("{DEP_GUID}/table/19");
    let ops = reach
        .candidate_ops("DB_INSERT", &ledger, false, &[])
        .expect("every candidate names its operation");
    assert!(
        ops.contains(&g.ws_op) && ops.contains(&g.dep_op),
        "precondition: both persistent inserts are candidates: {ops:?}"
    );
    let status = |site: &str, ops: &[String], per_tree: usize| {
        let row = g.group_row(site, &g.ws_op);
        let entries = reach.crossing_entries(&tree, row.crossing.as_ref().unwrap());
        let cache = SuffixCache::with_limits(per_tree, per_tree);
        reach.resource_status(&entries, ops, &cache, &|| false)
    };
    let cap = crate::engine::l5::literal_reach::STATE_CAP;
    assert_eq!(status(&g.true_site, &ops, cap), S::Reachable);
    assert_eq!(status(&g.false_site, &ops, cap), S::Disproved);
    // A 0-state budget caps the `true` suffix (it has a second state); the
    // `false` suffix completes in its one state, so it stays `Disproved`.
    assert_eq!(
        status(&g.true_site, &ops, 0),
        S::NotEstablished(U::SearchStopped),
        "a capped search deletes nothing"
    );
    assert_eq!(
        status(&g.false_site, &ops, 0),
        S::Disproved,
        "a completed search may"
    );
    let dep_write = model
        .workspace
        .routines
        .iter()
        .find(|r| r.name == "Write" && r.app_guid == DEP_GUID)
        .and_then(|r| r.record_operations.first())
        .map(|o| o.id.clone())
        .unwrap();
    assert_eq!(
        status(&g.false_site, &[dep_write], cap),
        S::NotEstablished(U::OwnerNotReached),
        "an owner not reached even with guards ignored concludes nothing"
    );
    let none = crate::engine::l5::literal_reach::CrossingEntries::empty();
    let cache = SuffixCache::default();
    assert_eq!(
        reach.resource_status(&none, &ops, &cache, &|| false),
        S::NotEstablished(U::NoEntry)
    );
    // The temporary class: only the known-temporary insert and the producers.
    let kt = reach
        .candidate_ops("DB_INSERT", &ledger, true, &[])
        .unwrap();
    assert!(!kt.contains(&g.ws_op) && !kt.contains(&g.dep_op), "{kt:?}");
}

/// W2-0c through prove's pass: a group row on the `false` crossing that
/// lists `Dep Ledger` (hand-stated: the `TwoCallsOnly` group given its
/// `false` crossing, the shape root-level liveness cannot judge) is judged
/// `Disproved` by the pass, and the root-level check (G33) alone keeps it,
/// because the `true` crossing keeps the kind live. Step 2: the disproved
/// resource leaves the refined list (`dead_group_resources`) and stays in the
/// raw one (`group_resources`).
/// Break: skipping the merge into `dead_group_resources`.
#[test]
fn w2_0c_prove_pass_drops_a_disproved_resource_from_the_refined_list() {
    use crate::engine::l5::literal_reach::ResourceStatus as S;
    let (_tmp, base, tree) = fixture();
    let model = &base.resolved;
    let root = stable(model, "Ws W2", "TwoCallsOnly");
    let (sites, crossings) = post_with_crossings(&tree, model, &root);
    let false_ix = sites
        .iter()
        .position(|s| {
            model.workspace.routines.iter().any(|r| {
                r.call_sites.iter().any(|cs| {
                    &cs.id == s
                        && cs
                            .argument_texts
                            .first()
                            .is_some_and(|t| t.trim() == "false")
                })
            })
        })
        .unwrap();
    let mut effects = effects_of(&tree, model, &root);
    let row = effects
        .iter_mut()
        .find(|e| e.boundary_group && e.effect_type == "DB_INSERT")
        .expect("precondition: the true crossing's insert group");
    row.crossing = Some(Arc::clone(&crossings[false_ix]));
    let ledger = format!("{DEP_GUID}/table/19");
    assert!(
        row.group_resources.iter().any(|(r, _)| *r == ledger),
        "precondition"
    );
    // Root-level liveness alone keeps it.
    let reach = LiteralReach::new(&tree, model);
    let (_, g33) = reach.unreachable_effects(&root, &effects, &[]);
    assert!(
        g33.values().all(|d| !d.contains(&ledger)),
        "G33 alone keeps it: {g33:?}"
    );
    let mut entries = vec![DigestEntryResult {
        routine_id: root.clone(),
        effects,
        partial_site_types: Vec::new(),
        literal_dead_sites: Vec::new(),
        literal_narrowed_sites: Vec::new(),
        suppressed_commits: Vec::new(),
        try_root_refusal: false,
    }];
    crate::engine::l5::digest_cli::literal_reach_pass(
        &tree,
        model,
        &mut entries,
        crate::engine::l5::digest_cli::ReachWorkers::SERIAL,
    );
    let row = entries[0]
        .effects
        .iter()
        .find(|e| e.boundary_group && e.effect_type == "DB_INSERT")
        .unwrap();
    assert_eq!(
        row.group_resource_status
            .iter()
            .find(|(r, _)| *r == ledger)
            .map(|(_, s)| *s),
        Some(S::Disproved)
    );
    assert!(
        row.dead_group_resources.contains(&ledger),
        "dropped from the refined list"
    );
    assert!(
        row.group_resources.iter().any(|(r, _)| *r == ledger),
        "the raw list keeps it"
    );
}

/// W2-0b (§3.4.3 item 1, astra 2): the prefix phase enters a crossing only
/// by its typed edges. The interface call's workspace implementer shares the
/// call site, but its edge is no entry.
/// Break: matching an edge by its site id alone (from/to dropped) makes the
/// workspace edge a second entry.
#[test]
fn a_mixed_dispatch_is_entered_only_by_its_dependency_edge() {
    let (_tmp, base, tree) = fixture();
    let model = &base.resolved;
    let root = stable(model, "Ws W2", "MixedDispatch");
    let dep_write = stable(model, "Dep Writer", "Write");
    let cs = call_sites(&tree, &root, &dep_write);
    assert_eq!(cs.len(), 1, "precondition");
    let idx = build_fingerprint_indexes(&tree);
    let c = &idx.crossings[&format!("cs:{}", cs[0])];
    let reach = LiteralReach::new(&tree, model);
    let root_tree = reach.live_tree(&root).expect("the root's search");
    assert_eq!(
        reach.crossing_entries(&root_tree, c).len(),
        1,
        "one entry: the dependency edge"
    );
}

/// Final CDO output review, item 2 (design §3.4.2): an own write row whose
/// listed paths ALL cannot run under literal arguments, while the write
/// stays live, gets its witness from the root's guarded search, ending at
/// the row's own operation (CDO: IssueReminder's `CDO Queue Entry` Insert
/// printed its anchor with no `path:` and no reason). Precondition
/// hand-stated: `TwoCalls`' own `Dep Ledger` insert (in `WriteLedger`)
/// gets, in place of its one listed path, a path through `PostWith(false)`
/// to PostWith's `if Flag then L.Insert()`, which cannot run.
///
/// Discrimination (2026-11-05): dropping the `e.own_path = Some(...)`
/// assignment in `digest_cli::literal_reach_pass` fails at "a searched
/// witness" (`None`); restored, passes.
#[test]
fn an_own_row_whose_listed_paths_cannot_run_gets_a_searched_witness() {
    let (_tmp, base, tree) = fixture();
    let model = &base.resolved;
    let root = stable(model, "Ws W2", "TwoCalls");
    let post = stable(model, "Dep Rec Lib", "PostWith");
    let writer = stable(model, "Ws W2", "WriteLedger");
    let routine = |s: &str| {
        model
            .workspace
            .routines
            .iter()
            .find(|r| r.stable_routine_id == s)
            .unwrap()
    };
    let false_site = call_sites(&tree, &root, &post)
        .into_iter()
        .find(|s| {
            routine(&root).call_sites.iter().any(|cs| {
                &cs.id == s
                    && cs
                        .argument_texts
                        .first()
                        .is_some_and(|t| t.trim() == "false")
            })
        })
        .expect("precondition: the PostWith(false) call");
    let guarded_insert = routine(&post).record_operations[0].id.clone();
    let own_insert = routine(&writer).record_operations[0].id.clone();
    let hop = |from: &str, to: Option<&str>, cs: Option<&str>, op: Option<&str>| ProjectedHop {
        inner: QueryWitnessHop {
            kind: if to.is_some() { "call" } else { "terminal" },
            from_routine_id: from.to_string(),
            from_display: from.to_string(),
            to_routine_id: to.map(str::to_string),
            to_display: to.map(str::to_string),
            callee_display: None,
            callsite_id: cs.map(str::to_string),
            event_id: None,
            target_app_guid: None,
            edge_kind: to.map(|_| "variable-typed-call".to_string()),
            anchor: None,
            receiver_type: None,
            interface_name: None,
            candidate_count: None,
            operation_id: op.map(str::to_string),
        },
    };
    let mut effects = effects_of(&tree, model, &root);
    let row = effects
        .iter_mut()
        .find(|e| {
            !e.boundary_group
                && e.effect_type == "DB_INSERT"
                && e.origin_operation_ids.contains(&own_insert)
        })
        .expect("precondition: WriteLedger's insert is an own row of TwoCalls");
    row.via_paths = vec![vec![
        hop(&root, Some(&post), Some(&false_site), None),
        hop(&post, None, None, Some(&guarded_insert)),
    ]];
    let reach = LiteralReach::new(&tree, model);
    assert!(
        !reach.path_live(&row.via_paths[0]),
        "precondition: the listed path cannot run"
    );
    let mut entries = vec![DigestEntryResult {
        routine_id: root.clone(),
        effects,
        partial_site_types: Vec::new(),
        literal_dead_sites: Vec::new(),
        literal_narrowed_sites: Vec::new(),
        suppressed_commits: Vec::new(),
        try_root_refusal: false,
    }];
    crate::engine::l5::digest_cli::literal_reach_pass(
        &tree,
        model,
        &mut entries,
        crate::engine::l5::digest_cli::ReachWorkers::SERIAL,
    );
    let row = entries[0]
        .effects
        .iter()
        .find(|e| !e.boundary_group && e.origin_operation_ids.contains(&own_insert))
        .unwrap();
    assert!(!row.literal_unreachable, "the write stays live");
    assert!(row.via_paths.is_empty(), "the dead listed path is dropped");
    match &row.own_path {
        Some(Ok(w)) => {
            assert_eq!(
                w.steps.iter().map(|s| s.to.as_str()).collect::<Vec<_>>(),
                [writer.as_str()],
                "the live route: TwoCalls → WriteLedger"
            );
            assert_eq!(w.terminal.operation_id, own_insert, "the row's own insert");
        }
        other => panic!("a searched witness: {other:?}"),
    }
}

/// W2-0b's census (design §11.4): how many crossings of each site kind the
/// CDO baseline has, and how many group rows of IssueReminder (IR) and
/// RunReportID (RR) sit on an `ev:` crossing. Decides whether event crossings
/// are in W2-0b's scope. CDO-gated (`CDO_WS`); prints the counts and pins
/// them.
#[test]
fn cdo_crossing_census() {
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
    let model = &base.resolved;
    let idx = build_fingerprint_indexes(&tree);
    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    for c in idx.crossings.values() {
        let k = match c.site {
            CrossingSite::Callsite(_) => "callsite",
            CrossingSite::Operation(_) => "operation",
            CrossingSite::Event(_) => "event",
        };
        *by_kind.entry(k).or_default() += 1;
    }
    let mut rows: Vec<(usize, usize)> = Vec::new();
    for root in &roots {
        let effects = effects_of(&tree, model, root);
        let groups = effects.iter().filter(|e| e.boundary_group).count();
        let ev = effects
            .iter()
            .filter(|e| {
                e.crossing
                    .as_ref()
                    .is_some_and(|c| matches!(c.site, CrossingSite::Event(_)))
            })
            .count();
        rows.push((groups, ev));
    }
    // Measured 2026-11-04: 813 call-site and 15 operation crossings, no event
    // crossing; IR 108 and RR 91 group rows, none on an `ev:` crossing. So
    // W2-0b treats event crossings uniformly and fixtures them only by hand.
    assert_eq!(by_kind.get("event"), None, "no event crossing on CDO");
    assert!(rows.iter().all(|&(_, ev)| ev == 0), "{rows:?}");

    // W2-0b's suffix cache on every group resource of both roots: its
    // outcomes and cost.
    let reach = LiteralReach::new(&tree, model);
    let paths = crate::engine::l5::group_path::GroupPaths::new(&tree.typed_edges, model);
    let cache = crate::engine::l5::literal_reach::SuffixCache::default();
    let started = std::time::Instant::now();
    let (mut through, mut outside) = (0usize, 0usize);
    let mut none: BTreeMap<&str, usize> = BTreeMap::new();
    let mut resources = 0usize;
    for root in &roots {
        let effects = effects_of(&tree, model, root);
        let root_tree = reach.live_tree(root).expect("the root's search completes");
        for e in effects.iter().filter(|e| e.boundary_group) {
            let c = e.crossing.as_ref().unwrap();
            let entries = reach.crossing_entries(&root_tree, c);
            resources += e.group_resource_ops.len();
            let reps = &e.group_resource_ops;
            for (_, p) in paths.paths(&reach, &root_tree, &entries, e, reps, &cache, &|| false) {
                match p {
                    Ok(p) if p.through_entry_point => through += 1,
                    Ok(_) => outside += 1,
                    Err(n) => *none.entry(n.reason).or_default() += 1,
                }
            }
        }
    }
    eprintln!(
        "cdo_crossing_census: suffix cache over IR+RR: {resources} group resources: {through} through, {outside} outside, none {none:?}; {} trees built, {} evicted, peak {} states, {:?}",
        cache.stats().built,
        cache.stats().evicted,
        cache.stats().peak_held,
        started.elapsed()
    );
    eprintln!(
        "cdo_crossing_census: crossings by site kind {by_kind:?}; (group rows, ev rows) IR {:?} RR {:?}",
        rows[0], rows[1]
    );

    // W2-0c through prove's own pass, per root: resource statuses, the
    // groups they touch, and every DB table whose ONLY writes in the refined
    // rows were a disproved group resource (a writes-table answer that could
    // move).
    use crate::engine::l5::literal_reach::ResourceStatus as S;
    for (name, root) in ["IR", "RR"].iter().zip(&roots) {
        let mut entries = vec![DigestEntryResult {
            routine_id: root.clone(),
            effects: effects_of(&tree, model, root),
            partial_site_types: Vec::new(),
            literal_dead_sites: Vec::new(),
            literal_narrowed_sites: Vec::new(),
            suppressed_commits: Vec::new(),
            try_root_refusal: false,
        }];
        let started = std::time::Instant::now();
        crate::engine::l5::digest_cli::literal_reach_pass(
            &tree,
            model,
            &mut entries,
            crate::engine::l5::digest_cli::ReachWorkers::SERIAL,
        );
        let took = started.elapsed();
        let effects = &entries[0].effects;
        let mut by: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
        let mut groups_touched = 0usize;
        let mut disproved_db: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for e in effects.iter().filter(|e| e.boundary_group) {
            let mut touched = false;
            for (r, s) in &e.group_resource_status {
                let k = match s {
                    S::Reachable => "reachable".to_string(),
                    S::Disproved => "disproved".to_string(),
                    S::NotEstablished(u) => format!("not-established:{u:?}"),
                };
                *by.entry(e.effect_type.clone())
                    .or_default()
                    .entry(k)
                    .or_default() += 1;
                if *s == S::Disproved {
                    touched = true;
                    if e.effect_type.starts_with("DB_") {
                        let display = e
                            .group_resources
                            .iter()
                            .find(|(id, _)| id == r)
                            .map_or(r.as_str(), |(_, d)| d.as_str());
                        let kt = is_known_temp_state(e.temp_state.as_ref());
                        let owners: BTreeSet<String> = reach
                            .candidate_ops(&e.effect_type, r, kt, &[])
                            .unwrap_or_default()
                            .iter()
                            .filter_map(|o| reach.op_owner(o))
                            .map(|w| {
                                let r = reach.routine(w).unwrap();
                                let obj = model
                                    .workspace
                                    .objects
                                    .iter()
                                    .find(|o| o.id == r.object_id)
                                    .map_or("?", |o| o.name.as_str());
                                format!("{obj}::{}", r.name)
                            })
                            .collect();
                        let c = e.crossing.as_ref().unwrap();
                        let targets: Vec<String> = c
                            .targets
                            .iter()
                            .map(|t| boundary_display(t, &build_fingerprint_indexes(&tree)))
                            .collect();
                        disproved_db.entry(r.clone()).or_default().push(format!(
                            "{} {} beyond {targets:?} at {:?} (crossing {}); candidate owners {owners:?}",
                            e.effect_type, display, c.sites, c.id
                        ));
                    }
                }
            }
            groups_touched += usize::from(touched);
        }
        // Tables still written by a live row (own row naming it, or a group
        // whose refined list keeps it).
        let still: BTreeSet<String> = effects
            .iter()
            .filter(|e| e.effect_type.starts_with("DB_") && e.effect_type != "DB_READ")
            .filter(|e| !e.literal_unreachable)
            .flat_map(|e| {
                let own = e
                    .detail
                    .iter()
                    .filter(|(k, _)| k == "resourceId")
                    .map(|(_, v)| v.clone());
                let grp = e
                    .group_resources
                    .iter()
                    .filter(|(r, _)| !e.dead_group_resources.contains(r))
                    .map(|(r, _)| r.clone());
                own.chain(grp).collect::<Vec<_>>()
            })
            .collect();
        let only: Vec<&Vec<String>> = disproved_db
            .iter()
            .filter(|(r, _)| !still.contains(*r))
            .map(|(_, v)| v)
            .collect();
        eprintln!(
            "cdo_crossing_census: W2-0c {name}: {by:?}; {groups_touched} group rows lose a resource; {} DB tables disproved somewhere, {} of them written by no other live row: {only:?}; pass {took:?}",
            disproved_db.len(),
            only.len()
        );
    }
}
