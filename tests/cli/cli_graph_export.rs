//! `alsem graph` (#63): the call-graph export for diff-scoped test selection,
//! driven through the SHIPPED binary on `tests/fixtures/test-selection-graph/`
//! (spec `docs/superpowers/specs/2026-10-13-callgraph-export-design.md` §10.4).
//!
//! The assertions walk the output backwards the way the consumer
//! (bc-dev-mcp's `bcdev_test_select`) does: a node with `universal: true` is a
//! caller of every node, a path through an `Open` edge is `dynamic`, a path of
//! non-`Direct` edges with no `Open` edge is `event`. Expected source rows are
//! hand-stated from the fixture text, never computed by production code.
//!
//! The byte golden `tests/cli-query-goldens/graph.test-selection-graph.jsonl`
//! pins the format. Refresh: `REGEN_TEMP_GOLDENS=1 cargo test --test cli
//! cli_graph_export::` — then INSPECT the diff.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::regen;

const VERSION_OVERRIDE: &str = "cli-graph-v1";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture() -> PathBuf {
    repo_root()
        .join("tests")
        .join("fixtures")
        .join("test-selection-graph")
}

/// Run `alsem graph <ws>` and return stdout (asserting exit 0).
fn run_graph(ws: &Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("graph")
        .arg(ws)
        .env("ALCH_DRIVER_VERSION_OVERRIDE", VERSION_OVERRIDE)
        .output()
        .expect("spawn alsem");
    assert!(
        out.status.success(),
        "alsem graph failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf8")
}

/// The parsed export plus the consumer's reverse walk.
pub(crate) struct Graph {
    pub meta: Value,
    pub nodes: HashMap<String, Value>,
    pub edges: Vec<Value>,
    callers: HashMap<String, Vec<usize>>,
    universal: Vec<String>,
}

impl Graph {
    pub(crate) fn parse(text: &str) -> Graph {
        let mut lines = text.lines();
        let first: Value = serde_json::from_str(lines.next().expect("meta line")).expect("json");
        assert_eq!(first["t"], "meta", "meta comes first");
        let mut nodes = HashMap::new();
        let mut edges = Vec::new();
        for l in lines {
            let row: Value = serde_json::from_str(l).expect("json line");
            match row["t"].as_str() {
                Some("node") => {
                    let id = row["v"]["id"].as_str().expect("id").to_string();
                    assert!(nodes.insert(id, row["v"].clone()).is_none(), "unique ids");
                }
                Some("edge") => edges.push(row["v"].clone()),
                other => panic!("unexpected line type {other:?}"),
            }
        }
        let mut callers: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, e) in edges.iter().enumerate() {
            for end in ["caller", "callee"] {
                assert!(
                    nodes.contains_key(e[end].as_str().unwrap()),
                    "dangling {end}: {e}"
                );
            }
            callers
                .entry(e["callee"].as_str().unwrap().to_string())
                .or_default()
                .push(i);
        }
        let universal = nodes
            .iter()
            .filter(|(_, n)| n["universal"] == true)
            .map(|(id, _)| id.clone())
            .collect();
        Graph {
            meta: first["v"].clone(),
            nodes,
            edges,
            callers,
            universal,
        }
    }

    pub(crate) fn label(&self, id: &str) -> String {
        let n = &self.nodes[id];
        match n["kind"].as_str() {
            Some("Hub") => id.to_string(),
            Some("Object") => n["objectName"].as_str().unwrap().to_string(),
            _ => format!(
                "{}.{}",
                n["objectName"].as_str().unwrap(),
                n["memberName"].as_str().unwrap()
            ),
        }
    }

    /// Ids of the routine nodes labelled `Object.member`.
    pub(crate) fn ids(&self, label: &str) -> Vec<String> {
        let mut v: Vec<String> = self
            .nodes
            .keys()
            .filter(|id| self.nodes[*id]["kind"] != "Hub" && self.label(id) == label)
            .cloned()
            .collect();
        v.sort();
        v
    }

    pub(crate) fn id(&self, label: &str) -> String {
        let v = self.ids(label);
        assert_eq!(v.len(), 1, "exactly one node {label}: {v:?}");
        v[0].clone()
    }

    pub(crate) fn node(&self, label: &str) -> &Value {
        &self.nodes[&self.id(label)]
    }

    /// Edges from `from` to `to` (labels or hub ids).
    pub(crate) fn edges_between(&self, from: &str, to: &str) -> Vec<&Value> {
        let resolve = |l: &str| {
            if l.starts_with("h:") {
                l.to_string()
            } else {
                self.id(l)
            }
        };
        let (f, t) = (resolve(from), resolve(to));
        self.edges
            .iter()
            .filter(|e| e["caller"] == f.as_str() && e["callee"] == t.as_str())
            .collect()
    }

    /// The tests whose may-execute cone holds `label`, each with its best class
    /// (`direct` > `event` > `dynamic`), and the hidden-callers flags in the cone.
    pub(crate) fn tests_reaching(
        &self,
        label: &str,
    ) -> (BTreeMap<String, &'static str>, BTreeSet<(String, String)>) {
        let start = self.ids(label);
        assert!(!start.is_empty(), "no node {label}");
        let mut best: BTreeMap<String, &'static str> = BTreeMap::new();
        let mut hidden = BTreeSet::new();
        type Allowed = fn(&Value) -> bool;
        let classes: [(&'static str, Allowed); 3] = [
            ("direct", |e| {
                e["kind"] == "Direct" && e["confidence"] != "Open"
            }),
            ("event", |e| e["confidence"] != "Open"),
            ("dynamic", |_| true),
        ];
        for (class, allowed) in classes {
            let mut seen: BTreeSet<String> = start.iter().cloned().collect();
            let mut todo: Vec<String> = start.clone();
            while let Some(x) = todo.pop() {
                let mut ups: Vec<String> = self
                    .callers
                    .get(&x)
                    .into_iter()
                    .flatten()
                    .map(|&i| &self.edges[i])
                    .filter(|e| allowed(e))
                    .map(|e| e["caller"].as_str().unwrap().to_string())
                    .collect();
                if class == "dynamic" {
                    ups.extend(self.universal.iter().cloned());
                }
                for u in ups {
                    if seen.insert(u.clone()) {
                        todo.push(u);
                    }
                }
            }
            for id in &seen {
                let n = &self.nodes[id];
                for f in n["hiddenCallers"].as_array().into_iter().flatten() {
                    hidden.insert((self.label(id), f.as_str().unwrap().to_string()));
                }
                if n["test"] == true {
                    best.entry(self.label(id)).or_insert(class);
                }
            }
        }
        (best, hidden)
    }
}

fn map(pairs: &[(&str, &'static str)]) -> BTreeMap<String, &'static str> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

/// The #63 acceptance and the may-execute rules on the fixture.
#[test]
fn graph_export_selects_the_tests_that_reach_each_change() {
    let g = Graph::parse(&run_graph(&fixture()));
    assert_eq!(g.meta["schema"], "al-sem.callgraph");
    assert_eq!(g.meta["version"], 1);
    assert_eq!(g.meta["engine"], VERSION_OVERRIDE);
    assert_eq!(g.meta["coverageHolds"], true);

    // D4/D5: the `Codeunit.Run`-only procedure and the trigger reach their test
    // through a Run edge, statically, with no coverage map.
    for change in ["Acc Nightly Recalc.ApplyDecay", "Acc Nightly Recalc.OnRun"] {
        let (tests, hidden) = g.tests_reaching(change);
        assert_eq!(
            tests.get("Acc Loyalty Tests.NightlyRecalcDecays"),
            Some(&"event"),
            "{change}: {tests:?}"
        );
        assert!(hidden.is_empty(), "{change}: {hidden:?}");
    }
    let run = g.edges_between("Acc Loyalty Rules.RecalcNightly", "Acc Nightly Recalc");
    assert_eq!(run.len(), 1);
    assert_eq!(
        (&run[0]["kind"], &run[0]["confidence"]),
        (&Value::from("Run"), &Value::from("Resolved"))
    );

    // Interface dispatch: one `Interface`/`OverApprox` edge per implementation.
    for imp in ["Acc Tax DK", "Acc Tax DE", "Acc Tax Exempt"] {
        let e = g.edges_between("Acc Tax Core.TaxAmount", &format!("{imp}.RatePct"));
        assert_eq!(e.len(), 1, "{imp}");
        assert_eq!(
            (&e[0]["kind"], &e[0]["confidence"]),
            (&Value::from("Interface"), &Value::from("OverApprox"))
        );
    }
    // Events, labelled.
    for (publisher, subscriber) in [
        (
            "Acc Invoice Core.OnAfterInvoiceTotal",
            "Acc Cash Rounding Subscriber.CashRoundOnInvoiceTotal",
        ),
        (
            "Acc Loyalty Core.OnAfterCalcPoints",
            "Acc Loyalty Bonus Subscriber.AddBonusOnCalcPoints",
        ),
    ] {
        let e = g.edges_between(publisher, subscriber);
        assert_eq!(e.len(), 1, "{publisher}");
        assert_eq!(
            (&e[0]["kind"], &e[0]["via"]),
            (&Value::from("Event"), &Value::from("eventFlow"))
        );
    }
    assert_eq!(
        g.tests_reaching("Acc Cash Rounding Subscriber.RoundToNickel")
            .0,
        map(&[("Acc Invoice Tests.InvoiceTotalRounds", "event")])
    );

    // D6: nothing reaches `ExportLine`, and nothing hides a caller of it.
    let (tests, hidden) = g.tests_reaching("Acc Legacy Export.ExportLine");
    assert!(
        tests.is_empty() && hidden.is_empty(),
        "{tests:?} {hidden:?}"
    );

    // Page lifecycle: the TestPage test and the modal-handler test reach the
    // page's triggers (OnAfterGetRecord, a field OnValidate, an action) and the
    // page its action opens; the handler reaches its own code through the test.
    for hook in [
        "Sel Ledger Hooks.Describe",
        "Sel Ledger Hooks.CardNoValidated",
        "Sel Ledger Hooks.RecalcFromPage",
        "Sel Ledger Hooks.HistoryOpened",
    ] {
        assert_eq!(
            g.tests_reaching(hook).0,
            map(&[
                ("Sel Page Tests.CardShowsRecord", "event"),
                ("Sel Page Tests.OpenCardModally", "event"),
            ]),
            "{hook}"
        );
    }
    assert_eq!(
        g.tests_reaching("Sel Ledger Hooks.InModalHandler").0,
        map(&[("Sel Page Tests.OpenCardModally", "event")])
    );
    // The table trigger reaches the test that inserts the record.
    assert_eq!(
        g.tests_reaching("Sel Ledger Hooks.Stamp")
            .0
            .get("Sel Page Tests.InsertLedger"),
        Some(&"event")
    );
    // A subscriber to a platform table event: no record operation raises it yet.
    assert!(
        g.tests_reaching("Sel Ledger Hooks.AfterInsertHook")
            .1
            .contains(&(
                "Sel Ledger.OnAfterInsertEvent".to_string(),
                "unraisedPublisher".to_string()
            ))
    );
    assert_eq!(
        g.node("Sel Unbound Subscriber.OnNoSuchEvent")["hiddenCallers"],
        serde_json::json!(["unboundSubscriber"])
    );

    // Open sites reach their hubs, and the tests calling them reach the hub
    // targets as `dynamic`.
    for (site, hub) in [
        ("Sel Dynamic.RunById", "h:run:codeunit"),
        ("Sel Dynamic.CallVariant", "h:name:ping/0"),
        ("Sel Dynamic.InsertViaRef", "h:record:insert"),
        ("Sel Dynamic.SaveReport", "h:run:report"),
    ] {
        let e = g.edges_between(site, hub);
        assert_eq!(e.len(), 1, "{site} -> {hub}");
        assert_eq!(e[0]["confidence"], "Open");
    }
    for (change, test) in [
        (
            "Acc Nightly Recalc.ApplyDecay",
            "Sel Dynamic Tests.DynamicRun",
        ),
        ("Sel Pingable.Ping", "Sel Dynamic Tests.VariantPing"),
        (
            "Sel Ledger Hooks.Stamp",
            "Sel Dynamic Tests.RecordRefInsert",
        ),
        (
            "Sel Ledger Hooks.ReportHook",
            "Sel Dynamic Tests.ReportSaved",
        ),
    ] {
        assert_eq!(
            g.tests_reaching(change).0.get(test),
            Some(&"dynamic"),
            "{change} <- {test}"
        );
    }
}

/// Every workspace routine node carries its location, and a hand-stated sample
/// matches the fixture text: `line` is the keyword or access-modifier row
/// (attributes excluded), 1-based; same-named triggers are told apart by it.
#[test]
fn graph_export_nodes_carry_their_source_rows() {
    let g = Graph::parse(&run_graph(&fixture()));
    for n in g.nodes.values() {
        if matches!(n["kind"].as_str(), Some("Method" | "Trigger"))
            && n["dependency"] == false
            && n["synthetic"] != true
        {
            assert!(
                n["sourcePath"].is_string() && n["line"].is_u64() && n["endLine"].is_u64(),
                "{n}"
            );
        }
    }
    for (label, path, line, end, member) in [
        ("Acc Nightly Recalc.OnRun", "src/Loyalty.al", 42, 48, None),
        (
            "Acc Nightly Recalc.ApplyDecay",
            "src/Loyalty.al",
            50,
            53,
            None,
        ),
        ("Sel Ledger.OnValidate", "src/Ledger.al", 9, 14, Some("No.")),
        (
            "Sel Ledger Card.OnValidate",
            "src/Ledger.al",
            40,
            45,
            Some("No."),
        ),
        (
            "Sel Page Tests.OpenCardModally",
            "src/Tests.al",
            67,
            72,
            None,
        ),
    ] {
        let n = g.node(label);
        assert_eq!(n["sourcePath"], path, "{label}");
        assert_eq!(
            (n["line"].as_u64(), n["endLine"].as_u64()),
            (Some(line), Some(end)),
            "{label}"
        );
        assert_eq!(n["member"].as_str(), member, "{label}");
    }
    assert_eq!(g.node("Acc Nightly Recalc.OnRun")["kind"], "Trigger");
    assert_eq!(g.node("Sel Page Tests.OpenCardModally")["test"], true);
}

const WS_GUID: &str = "aaaa1111-0000-0000-0000-000000000063";
const DEP_GUID: &str = "bbbb2222-0000-0000-0000-000000000063";
const SYM_GUID: &str = "cccc3333-0000-0000-0000-000000000063";

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A workspace over a source-shipping dependency (`XDep`) and a symbol-only one
/// (`SymLib`), with a test per dependency path.
fn cross_app_workspace(dir: &Path) {
    write(
        &dir.join("app.json"),
        &format!(
            r#"{{"id":"{WS_GUID}","name":"GWs","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50200,"to":50299}}],"dependencies":[{{"id":"{DEP_GUID}","name":"XDep","publisher":"probe","version":"1.0.0.0"}},{{"id":"{SYM_GUID}","name":"SymLib","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    );
    write(
        &dir.join("src/Ws.al"),
        "codeunit 50280 \"Ws Hooks\"
{
    procedure Handled()
    begin
    end;

    procedure SymHandled()
    begin
    end;

    procedure AfterInsert()
    begin
    end;

    procedure ExtAfterInsert()
    begin
    end;
}

codeunit 50281 \"Ws Subscribers\"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::\"Dep Events\", 'OnWorked', '', false, false)]
    local procedure OnWorkedHandler()
    var
        Hooks: Codeunit \"Ws Hooks\";
    begin
        Hooks.Handled();
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::\"Sym Api\", 'OnComputed', '', false, false)]
    local procedure OnComputedHandler()
    var
        Hooks: Codeunit \"Ws Hooks\";
    begin
        Hooks.SymHandled();
    end;

    [EventSubscriber(ObjectType::Table, Database::\"Dep Item\", 'OnAfterInsertEvent', '', false, false)]
    local procedure OnAfterInsertDepItem(var Rec: Record \"Dep Item\"; RunTrigger: Boolean)
    var
        Hooks: Codeunit \"Ws Hooks\";
    begin
        Hooks.AfterInsert();
    end;
}

codeunit 50283 \"Ws Pages\"
{
    procedure Show()
    var
        W: Codeunit \"Dep Worker\";
    begin
        W.ShowPage();
    end;
}

tableextension 50282 \"Dep Item Ext\" extends \"Dep Item\"
{
    trigger OnAfterInsert()
    var
        Hooks: Codeunit \"Ws Hooks\";
    begin
        Hooks.ExtAfterInsert();
    end;
}

codeunit 50290 \"Ws Tests\"
{
    Subtype = Test;

    [Test]
    procedure WorkRaises()
    var
        W: Codeunit \"Dep Worker\";
    begin
        W.Work();
    end;

    [Test]
    procedure InsertItem()
    var
        Item: Record \"Dep Item\";
    begin
        Item.Insert(true);
    end;

    [Test]
    procedure ComputeSym()
    var
        S: Codeunit \"Sym Api\";
    begin
        S.Compute();
    end;
}
",
    );
    let dep_symbols = format!(
        r#"{{"RuntimeVersion":"13.0","Codeunits":[{{"Id":50100,"Name":"Dep Worker","Methods":[{{"Name":"Work","Parameters":[]}},{{"Name":"ShowPage","Parameters":[]}}]}},{{"Id":50101,"Name":"Dep Events","Methods":[]}}],"AppId":"{DEP_GUID}","Name":"XDep","Publisher":"probe","Version":"1.0.0.0"}}"#
    );
    crate::symbol_app::write_source_app(
        &dir.join(".alpackages/probe_XDep_1.0.0.0.app"),
        DEP_GUID,
        "XDep",
        "1.0.0.0",
        &dep_symbols,
        &[
            (
                "src/Worker.al",
                "codeunit 50100 \"Dep Worker\"
{
    procedure Work()
    var
        E: Codeunit \"Dep Events\";
    begin
        E.OnWorked();
    end;

    procedure ShowPage()
    begin
        Page.Run(Page::\"Dep Page\");
    end;
}
",
            ),
            (
                "src/Page.al",
                "page 50120 \"Dep Page\"
{
    SourceTable = \"Dep Item\";
}
",
            ),
            (
                "src/Events.al",
                "codeunit 50101 \"Dep Events\"
{
    [IntegrationEvent(false, false)]
    procedure OnWorked()
    begin
    end;
}
",
            ),
            (
                "src/Item.al",
                "table 50110 \"Dep Item\"
{
    fields
    {
        field(1; \"No.\"; Code[20])
        {
        }
    }

    trigger OnInsert()
    var
        E: Codeunit \"Dep Events\";
    begin
        E.OnWorked();
    end;
}
",
            ),
        ],
        "",
    );
    let sym_symbols = format!(
        r#"{{"RuntimeVersion":"13.0","Codeunits":[{{"Id":50300,"Name":"Sym Api","Methods":[{{"Name":"Compute","Parameters":[]}},{{"Name":"OnComputed","Parameters":[],"Attributes":[{{"Name":"IntegrationEvent","Arguments":[{{"Value":"false"}},{{"Value":"false"}}]}}]}}]}}],"AppId":"{SYM_GUID}","Name":"SymLib","Publisher":"probe","Version":"1.0.0.0"}}"#
    );
    crate::symbol_app::write_symbol_app(
        &dir.join(".alpackages/probe_SymLib_1.0.0.0.app"),
        SYM_GUID,
        "SymLib",
        "1.0.0.0",
        &sym_symbols,
    );
}

/// Dependencies (spec §4.6, §6, §10.5): a source-shipping dependency's routines
/// are full nodes with their own `sourcePath`, its body edges carry a workspace
/// test to the workspace subscriber of an event it raises, and its table's
/// synthetic `OnAfterInsertEvent` publisher survives pruning with its flag. A
/// symbol-only dependency's routines have no location, and a subscriber to its
/// event reaches the test calling it only through the callbacks hub (`dynamic`).
#[test]
fn graph_export_covers_dependencies() {
    let dir = tempfile::tempdir().expect("tempdir");
    cross_app_workspace(dir.path());
    let g = Graph::parse(&run_graph(dir.path()));

    let mut sources: Vec<(String, String)> = g.meta["apps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                a["name"].as_str().unwrap().to_string(),
                a["source"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    sources.sort();
    assert_eq!(
        sources,
        vec![
            ("SymLib".to_string(), "symbolOnly".to_string()),
            ("XDep".to_string(), "embedded".to_string())
        ]
    );

    let work = g.node("Dep Worker.Work");
    assert_eq!(work["dependency"], true);
    assert_eq!(work["symbolOnly"], false);
    assert_eq!(
        work["sourcePath"], "src/Worker.al",
        "relative to the dependency"
    );
    assert_eq!(
        (work["line"].as_u64(), work["endLine"].as_u64()),
        (Some(3), Some(8))
    );
    assert_eq!(
        g.tests_reaching("Ws Hooks.Handled").0,
        map(&[
            ("Ws Tests.InsertItem", "event"),
            ("Ws Tests.WorkRaises", "event")
        ]),
        "the dependency bodies (Work, the table's OnInsert) raise the event"
    );

    let compute = g.node("Sym Api.Compute");
    assert_eq!(
        (&compute["dependency"], &compute["symbolOnly"]),
        (&Value::from(true), &Value::from(true))
    );
    assert!(compute.get("sourcePath").is_none());
    assert_eq!(
        g.tests_reaching("Ws Hooks.SymHandled").0,
        map(&[("Ws Tests.ComputeSym", "dynamic")]),
        "only through the symbol-only app's callbacks hub"
    );
    let hub = format!("h:callbacks:{SYM_GUID}");
    assert_eq!(
        g.edges_between("Sym Api.Compute", &hub)[0]["via"],
        "opaqueBody"
    );

    // The dependency table's trigger fires on the workspace test's insert; the
    // workspace extension's OnAfterInsert does not yet (F5) and says so.
    let on_insert = g.node("Dep Item.OnInsert");
    assert_eq!(
        (
            on_insert["dependency"].clone(),
            on_insert["sourcePath"].clone()
        ),
        (Value::from(true), Value::from("src/Item.al"))
    );
    assert!(
        !g.edges_between("Ws Tests.InsertItem", "Dep Item.OnInsert")
            .is_empty()
    );
    assert_eq!(
        g.node("Dep Item Ext.OnAfterInsert")["hiddenCallers"],
        serde_json::json!(["unmodeledRecordTrigger"])
    );

    // A subscriber to a DEPENDENCY table's platform event: the synthetic
    // publisher lives on the dependency table, has no incoming edge, and must
    // survive pruning with its flag (spec §4.6).
    let synth = g.node("Dep Item.OnAfterInsertEvent");
    assert_eq!(synth["dependency"], true);
    assert_eq!(
        synth["hiddenCallers"],
        serde_json::json!(["unraisedPublisher"])
    );
    assert_eq!(
        g.edges_between(
            "Dep Item.OnAfterInsertEvent",
            "Ws Subscribers.OnAfterInsertDepItem"
        )[0]["kind"],
        "Event"
    );
    assert!(g.tests_reaching("Ws Hooks.AfterInsert").1.contains(&(
        "Dep Item.OnAfterInsertEvent".to_string(),
        "unraisedPublisher".to_string()
    )));

    // #160 §8.3: a DEPENDENCY body's run of a page that declares no entry
    // trigger. Its `run` edge comes only from the dependency's site facts
    // (`run_target`), so it pins that `ResolvedProgram::build` merges them.
    let runs: Vec<(&Value, &Value, &Value)> = g
        .edges_between("Dep Worker.ShowPage", "Dep Page")
        .iter()
        .map(|e| (&e["kind"], &e["confidence"], &e["via"]))
        .collect();
    assert_eq!(
        runs,
        vec![(
            &Value::from("Run"),
            &Value::from("Resolved"),
            &Value::from("run")
        )]
    );
}

/// #160 spec §4.3.2 / §8.3: `meta.histogram.dependencyBodies` counts the
/// dependency's call sites and nothing else. The dependency's source has
/// exactly two call sites (`A` calls its own `B` twice); the workspace's one
/// call to `A` is not a dependency call site. Hand-stated literal.
#[test]
fn dependency_bodies_histogram_is_the_dependency_call_sites() {
    const WS: &str = "aaaa1111-0000-0000-0000-000000000160";
    const DEP: &str = "bbbb2222-0000-0000-0000-000000000160";
    let dir = tempfile::tempdir().expect("tempdir");
    let ws = dir.path();
    write(
        &ws.join("app.json"),
        &format!(
            r#"{{"id":"{WS}","name":"HWs","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50200,"to":50299}}],"dependencies":[{{"id":"{DEP}","name":"HDep","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    );
    write(
        &ws.join("src/Ws.al"),
        "codeunit 50200 \"Ws Caller\"
{
    procedure Go()
    var
        L: Codeunit \"Dep Lib\";
    begin
        L.A();
    end;
}
",
    );
    let dep_symbols = format!(
        r#"{{"RuntimeVersion":"13.0","Codeunits":[{{"Id":50100,"Name":"Dep Lib","Methods":[{{"Name":"A","Parameters":[]}},{{"Name":"B","Parameters":[]}}]}}],"AppId":"{DEP}","Name":"HDep","Publisher":"probe","Version":"1.0.0.0"}}"#
    );
    crate::symbol_app::write_source_app(
        &ws.join(".alpackages/probe_HDep_1.0.0.0.app"),
        DEP,
        "HDep",
        "1.0.0.0",
        &dep_symbols,
        &[(
            "src/Lib.al",
            "codeunit 50100 \"Dep Lib\"
{
    procedure A()
    begin
        B();
        B();
    end;

    procedure B()
    begin
    end;
}
",
        )],
        "",
    );
    let g = Graph::parse(&run_graph(ws));
    assert_eq!(
        g.meta["histogram"]["dependencyBodies"],
        serde_json::json!({
            "total": 2,
            "resolvedSource": 2,
            "resolvedCatalog": 0,
            "resolvedAbiExternal": 0,
            "conditionalResolved": 0,
            "honestDynamic": 0,
            "honestEmpty": 0,
            "unknown": 0,
            "ambiguousResolved": 0,
        })
    );
}

/// Code review CR2: the export of a workspace-scoped build would lose every
/// dependency call-site edge without a word, so `export_from_program` refuses it.
/// The same workspace built cross-app exports (the dependency has source).
#[test]
fn export_refuses_a_workspace_scoped_program() {
    use al_sem::program::resolved::{ProgramScope, ResolvedProgram};
    let dir = tempfile::tempdir().expect("tempdir");
    cross_app_workspace(dir.path());
    let inputs = al_sem::program::inputs::WorkspaceInputs::read(dir.path(), &Default::default());
    let export = |scope| {
        let program = ResolvedProgram::build(&inputs, scope).expect("the fixture builds");
        al_sem::program::impact_graph::export_from_program(&program)
    };
    let err = export(ProgramScope::Workspace)
        .err()
        .expect("a workspace-scoped program is refused");
    assert!(err.contains("cross-app"), "{err}");
    assert!(export(ProgramScope::CrossApp).is_ok());
}

/// The byte golden: pins the wire format end to end.
#[test]
fn graph_export_golden() {
    let got = run_graph(&fixture());
    let path = repo_root()
        .join("tests")
        .join("cli-query-goldens")
        .join("graph.test-selection-graph.jsonl");
    if regen::regen_mode() {
        std::fs::write(&path, &got).expect("write golden");
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e} (REGEN_TEMP_GOLDENS=1 mints it)", path.display()))
        .replace('\r', "");
    assert_eq!(got, want, "golden drifted: {}", path.display());
}

/// A tempdir copy of `tests/r0-corpus/ws-d8-commit-in-tx` whose `app.json` is
/// `app_json` (`None`: no `app.json` at all). Keep the `TempDir` alive.
fn d8_copy(app_json: Option<&str>) -> tempfile::TempDir {
    let src = repo_root()
        .join("tests")
        .join("r0-corpus")
        .join("ws-d8-commit-in-tx");
    let dir = tempfile::tempdir().expect("tempdir");
    for f in ["posting.al", "subscriber.al"] {
        let text = std::fs::read_to_string(src.join("src").join(f)).expect("read fixture");
        write(&dir.path().join("src").join(f), &text);
    }
    if let Some(text) = app_json {
        write(&dir.path().join("app.json"), text);
    }
    dir
}

/// Run `alsem <args>` and return (exit code, stdout, stderr).
fn run_alsem(args: &[&std::ffi::OsStr]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .output()
        .expect("spawn alsem");
    (
        out.status.code().expect("exit code"),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn graph_of(ws: &Path) -> (i32, String, String) {
    run_alsem(&["graph".as_ref(), ws.as_os_str()])
}

fn digest_of(ws: &Path) -> (i32, String, String) {
    run_alsem(&[
        "digest".as_ref(),
        ws.as_os_str(),
        "--file".as_ref(),
        "src/posting.al".as_ref(),
    ])
}

const NO_ID: &str = "could not compute modelInstanceId";

/// #73: `graph` applies the gate's identity rule — an `app.json` whose `id` is
/// missing, empty or not a string is rejected, not exported with an empty id.
#[test]
fn graph_rejects_an_app_json_without_an_id() {
    for app_json in [
        r#"{"name":"x"}"#,
        r#"{"id":"","name":"x"}"#,
        r#"{"id":5,"name":"x"}"#,
    ] {
        let ws = d8_copy(Some(app_json));
        let (code, stdout, stderr) = graph_of(ws.path());
        assert_eq!(code, 1, "{app_json}: exit; stderr={stderr}");
        assert!(
            stdout.is_empty(),
            "{app_json}: stdout must be empty: {stdout}"
        );
        assert!(
            stderr.contains(&format!("al-sem graph: {NO_ID}")),
            "{app_json}: stderr={stderr:?}"
        );
    }
}

/// #73: `graph` and a gate leaf (`digest`) agree on which workspaces have an
/// identity. A consistency check; T1 carries the discrimination.
#[test]
fn graph_and_digest_agree_on_workspace_identity() {
    let valid = d8_copy(Some(
        &std::fs::read_to_string(
            repo_root()
                .join("tests")
                .join("r0-corpus")
                .join("ws-d8-commit-in-tx")
                .join("app.json"),
        )
        .expect("read app.json"),
    ));
    for (name, (code, _, stderr)) in [
        ("graph", graph_of(valid.path())),
        ("digest", digest_of(valid.path())),
    ] {
        assert_eq!(code, 0, "{name} on the valid fixture: stderr={stderr}");
    }
    let no_id = d8_copy(Some(r#"{"name":"x"}"#));
    for (name, (code, _, stderr)) in [
        ("graph", graph_of(no_id.path())),
        ("digest", digest_of(no_id.path())),
    ] {
        assert_eq!(code, 1, "{name} on an id-less app.json: stderr={stderr}");
        assert!(stderr.contains(NO_ID), "{name}: stderr={stderr:?}");
    }
}

/// #73: a missing or unparseable `app.json` fails in the build first and keeps
/// its existing error; the id check never sees it.
#[test]
fn graph_keeps_its_errors_for_a_missing_or_unparseable_app_json() {
    for (app_json, want) in [
        (None, "al-sem graph: snapshot build failed: read"),
        (
            Some("not json"),
            "al-sem graph: snapshot build failed: parse",
        ),
    ] {
        let ws = d8_copy(app_json);
        let (code, _, stderr) = graph_of(ws.path());
        assert_eq!(code, 1, "{app_json:?}: stderr={stderr}");
        assert!(stderr.contains(want), "{app_json:?}: stderr={stderr:?}");
        assert!(!stderr.contains(NO_ID), "{app_json:?}: stderr={stderr:?}");
    }
}
