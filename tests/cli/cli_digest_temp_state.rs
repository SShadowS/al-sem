//! #65 / #69 — `alsem digest`'s temp-state coherence.
//!
//! #65: a DB_* effect carries `detail.tempState`, read from the SAME L4
//! db-effect store `alsem query effects` reads.
//!
//! #69 (design spec
//! `docs/superpowers/specs/2026-10-14-issue-69-digest-temp-state-coherence-design.md`,
//! "Acceptance matrix"): every statement a digest makes about a write's temp
//! state agrees — the ordering engine's physical-write filter, the witness
//! (evidence, conditionality, viaPaths), the effect identity, and
//! `detail.tempState` — and `Validate`/`Copy`/`TransferFields` are not DB writes.
//!
//! Every test drives the production entry point `run_digest_pipeline` on the
//! shared probe workspace (`tests/common/temp_state_probe.rs`), in BOTH modes
//! (single-app and the cross-app default). Each routine there states its own
//! precondition. Assertion messages carry the matrix's named failure text, so a
//! discrimination proof can quote it. Each `#[test]` collects every failing
//! claim before it panics, so one red run shows all of them.
//!
//! Stated limit (#66): the probe has no dependency, so the two modes' stores are
//! equal here; nothing in this file tells the cross-app store (with dependency
//! leaves) from the single-app one.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use al_sem::engine::gate::model_instance_id::compute_gate_model_instance_id;
use al_sem::engine::l4::effect_query_cli::{QuerySubstrate, render_query_effects};
use al_sem::engine::l5::digest::{build_fingerprint_indexes_pub, reconstruct_witness_paths_pub};
use al_sem::engine::l5::digest_cli::run_digest_pipeline;
use al_sem::engine::l5::snapshot::{
    CapabilitySnapshot, SnapCapabilityExtra, SnapTempState, compose_snapshot,
};
use al_sem::program::model::program_calls::assemble_and_resolve_workspace_program;
use serde_json::Value;

use crate::temp_state_probe::{probe_al, write_workspace};

// ---------------------------------------------------------------------------
// Running the pipeline (once per mode, shared by every test).
// ---------------------------------------------------------------------------

fn run_mode(single_app: bool) -> Value {
    let tmp = write_workspace();
    let result = run_digest_pipeline(
        tmp.path(),
        Some(vec!["src/Probe.al".to_string()]),
        None,
        None,
        "cli-b-v1",
        true,
        None,
        single_app,
    )
    .expect("digest runs");
    serde_json::from_str(&result.json_text).expect("digest json")
}

/// `[(mode label, digest document)]` for single-app and cross-app.
fn docs() -> &'static [(&'static str, Value)] {
    static DOCS: OnceLock<Vec<(&'static str, Value)>> = OnceLock::new();
    DOCS.get_or_init(|| {
        vec![
            ("single-app", run_mode(true)),
            ("cross-app", run_mode(false)),
        ]
    })
}

/// Run `check` on both modes; panic with every collected failure.
fn each_mode(check: impl Fn(&Value, &mut Vec<String>)) {
    let mut failures = Vec::new();
    for (mode, doc) in docs() {
        let mut f = Vec::new();
        check(doc, &mut f);
        failures.extend(f.into_iter().map(|m| format!("[{mode}] {m}")));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// ---------------------------------------------------------------------------
// Reading the digest document.
// ---------------------------------------------------------------------------

fn entry<'a>(doc: &'a Value, name: &str) -> &'a Value {
    doc["payload"]["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .find(|e| e["routine"]["display"] == name)
        .unwrap_or_else(|| panic!("{name}: no entry"))
}

fn effects(e: &Value) -> &[Value] {
    e["effects"].as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn db_effects(e: &Value) -> Vec<&Value> {
    effects(e)
        .iter()
        .filter(|x| x["type"].as_str().unwrap_or("").starts_with("DB_"))
        .collect()
}

fn of_type<'a>(e: &'a Value, ty: &str) -> Vec<&'a Value> {
    effects(e).iter().filter(|x| x["type"] == ty).collect()
}

fn temp_state(eff: &Value) -> Option<&str> {
    eff["detail"]["tempState"].as_str()
}

/// The entry's UI_MESSAGE effects carry `WRITE_PENDING_AT_UI`.
fn has_label(e: &Value) -> bool {
    of_type(e, "UI_MESSAGE").iter().any(|m| {
        m["guarantees"]
            .as_array()
            .is_some_and(|g| g.iter().any(|l| l == "WRITE_PENDING_AT_UI"))
    })
}

/// The 0-based line of the unique `// @marker` comment — the convention of a
/// digest viaPath hop's `anchor.line`.
fn line_of(marker: &str) -> u64 {
    let tag = format!("// @{marker}");
    let src = probe_al();
    let hits: Vec<usize> = src
        .lines()
        .enumerate()
        .filter(|(_, l)| l.trim_end().ends_with(&tag))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(hits.len(), 1, "marker {marker} must be unique: {hits:?}");
    hits[0] as u64
}

fn is_kt(eff: &Value) -> bool {
    temp_state(eff) == Some("known(true)")
}

fn is_nt(eff: &Value) -> bool {
    temp_state(eff).is_some_and(|s| s.split(',').any(|p| p != "known(true)"))
}

fn has_guarantee(e: &Value, label: &str) -> bool {
    effects(e).iter().any(|m| {
        m["guarantees"]
            .as_array()
            .is_some_and(|g| g.iter().any(|l| l == label))
    })
}

fn via_paths(eff: &Value) -> Vec<&Vec<Value>> {
    eff["viaPaths"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_array).collect())
        .unwrap_or_default()
}

/// A path as `A→B→C` routine displays.
fn path_str(root: &str, path: &[Value]) -> String {
    let mut s = root.to_string();
    for hop in path {
        s.push('→');
        s.push_str(hop["toDisplay"].as_str().unwrap_or("?"));
    }
    s
}

fn path_strs(root: &str, eff: &Value) -> Vec<String> {
    via_paths(eff).iter().map(|p| path_str(root, p)).collect()
}

/// The source lines of a path's hops (each hop's call-site anchor).
fn hop_lines(path: &[Value]) -> Vec<u64> {
    path.iter()
        .filter_map(|h| h["anchor"]["line"].as_u64())
        .collect()
}

fn evidence_kind(eff: &Value) -> &str {
    eff["evidence"]["sourceKind"].as_str().unwrap_or("")
}

fn summary(e: &Value) -> String {
    db_effects(e)
        .iter()
        .map(|x| {
            format!(
                "{}:{}",
                x["type"].as_str().unwrap_or("?"),
                temp_state(x).unwrap_or("-")
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

// ---------------------------------------------------------------------------
// #65 — the original contract (A4: authority/subset), kept.
// ---------------------------------------------------------------------------

/// #65 values plus A4 (spec "Values come from `query effects`' store"): every
/// digest value is one `query effects` reports for the same routine, in the
/// same mode. Mutation: render "unknown" for every row in `for_effect` →
/// "TempOnly: digest says unknown".
#[test]
fn digest_db_effects_carry_query_effects_temp_state() {
    check_mode(true);
    check_mode(false);
}

fn collect_temp_states(v: &Value, out: &mut BTreeSet<String>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                if k == "tempState"
                    && let Some(s) = x.as_str()
                {
                    out.insert(s.to_string());
                }
                collect_temp_states(x, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| collect_temp_states(x, out)),
        _ => {}
    }
}

fn check_mode(single_app: bool) {
    let doc = &docs()[if single_app { 0 } else { 1 }].1;
    assert_eq!(doc["schemaVersion"], "1.5.0");

    // Every DB_INSERT in the routine has the expected value.
    let expect_inserts = |name: &str, want: &str| {
        let e = entry(doc, name);
        let inserts = of_type(e, "DB_INSERT");
        assert!(
            !inserts.is_empty(),
            "{name}: no DB_INSERT in {}",
            summary(e)
        );
        for x in inserts {
            assert_eq!(temp_state(x), Some(want), "{name}: {}", summary(e));
        }
    };
    expect_inserts("TempOnly", "known(true)");
    expect_inserts("ViaHelperTemp", "known(true)");
    expect_inserts("ViaRecursionTemp", "known(true)");
    expect_inserts("RealWrite", "known(false)");
    expect_inserts("ViaHelperReal", "known(false)");
    expect_inserts("ViaRecursionReal", "known(false)");
    expect_inserts("Helper", "parameter-dependent(0)");
    expect_inserts("Forward", "parameter-dependent(0)");

    // Cross-command contract: every digest value is one `query effects` reports
    // for the same routine.
    let tmp = write_workspace();
    let ws = tmp.path();
    let model_id = compute_gate_model_instance_id(ws).expect("model id");
    let substrate = QuerySubstrate::build(ws, &model_id, single_app).expect("query substrate");
    let mut failures = Vec::new();
    for e in doc["payload"]["entries"].as_array().expect("entries") {
        let name = e["routine"]["display"].as_str().unwrap();
        let db = db_effects(e);
        if db.is_empty() {
            continue;
        }
        let q = render_query_effects(&substrate, &format!("Probe Writer.{name}"), "t", true);
        let qdoc: Value = serde_json::from_str(&q.json_text).expect("query json");
        let mut query_states = BTreeSet::new();
        collect_temp_states(&qdoc, &mut query_states);
        for x in db {
            for part in temp_state(x).iter().flat_map(|s| s.split(',')) {
                if !query_states.contains(part) {
                    failures.push(format!(
                        "{name}: digest says {part}, query effects says {query_states:?}"
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// ---------------------------------------------------------------------------
// A1 — the ordering filter, `detail.tempState` and the cone agree.
// ---------------------------------------------------------------------------

/// A1b. No `temp-state-incoherent` diagnostic anywhere in the document.
/// Mutation: put producers back into `fact_cone_for_scc` for recursive SCCs →
/// "RecUiTemp: temp-state-incoherent class-absent".
///
/// GREEN before #69: the diagnostic kind does not exist yet, so this test cannot
/// be red on the base; it pins the post-fix state (D4) and is the named red of
/// the mutation above.
#[test]
fn a1b_no_temp_state_incoherence() {
    each_mode(|doc, f| {
        for e in doc["payload"]["entries"].as_array().expect("entries") {
            let root = e["routine"]["display"].as_str().unwrap();
            for d in e["entryDiagnostics"].as_array().into_iter().flatten() {
                if d["kind"] == "temp-state-incoherent" {
                    f.push(format!(
                        "{root}: temp-state-incoherent {}",
                        d["reason"].as_str().unwrap_or("?")
                    ));
                }
            }
        }
    });
}

/// The snapshot's write facts per routine display:
/// display -> [(op, rendered temp state, provenance)].
fn snapshot_writes(snap: &CapabilitySnapshot) -> BTreeMap<String, Vec<(String, String, String)>> {
    let display: BTreeMap<&str, &str> = snap
        .identities
        .stable_ids
        .iter()
        .map(String::as_str)
        .zip(snap.identities.display_names.iter().map(String::as_str))
        .collect();
    let mut out: BTreeMap<String, Vec<(String, String, String)>> = BTreeMap::new();
    for fact in &snap.capability_facts {
        let Some(SnapCapabilityExtra::Table { temp_state, .. }) = &fact.extra else {
            continue;
        };
        let ts = match temp_state {
            Some(SnapTempState::Known { value }) => format!("known/{value}"),
            Some(SnapTempState::ParameterDependent { parameter_index }) => {
                format!("PD({parameter_index})")
            }
            Some(SnapTempState::Unknown) => "unknown".to_string(),
            None => "none".to_string(),
        };
        let name = display
            .get(fact.subject.as_str())
            .copied()
            .unwrap_or(fact.subject.as_str());
        // A display may be object-qualified; keep the routine part.
        let name = name.rsplit('.').next().unwrap_or(name).to_string();
        out.entry(name)
            .or_default()
            .push((fact.op.clone(), ts, fact.provenance.clone()));
    }
    out
}

/// A1c. Cone values are frame-correct. Stated limit: asserted on
/// `compose_snapshot`, the production builder `run_digest_pipeline` calls (over
/// the single-app model, which equals the cross-app one here: no dependency),
/// and NOT claimed as a pipeline pin. Mutation: give solve entries the
/// producer's raw PD value instead of `t` → "ViaHelperReal: PD(0) != known/false".
#[test]
fn a1c_cone_values_are_frame_correct() {
    let tmp = write_workspace();
    let ws = tmp.path();
    let model_id = compute_gate_model_instance_id(ws).expect("model id");
    let model = assemble_and_resolve_workspace_program(ws, &model_id, false).expect("model");
    let snap = compose_snapshot(&model);
    let writes = snapshot_writes(&snap);
    let mut f = Vec::new();
    let inserts = |name: &str| -> Vec<String> {
        writes
            .get(name)
            .map(|v| {
                v.iter()
                    .filter(|(op, _, _)| op == "insert")
                    .map(|(_, ts, _)| ts.clone())
                    .collect()
            })
            .unwrap_or_default()
    };
    for (name, want) in [
        ("ViaHelperReal", "known/false"),
        ("ViaRecursionReal", "known/false"),
        ("ViaRecursionTemp", "known/true"),
        ("Forward", "PD(0)"),
    ] {
        let got = inserts(name);
        if got.is_empty() {
            f.push(format!("{name}: no insert fact ({writes:?})"));
        }
        for ts in got {
            if ts != want {
                f.push(format!("{name}: {ts} != {want}"));
            }
        }
    }
    let perm: BTreeSet<String> = inserts("PermUi").into_iter().collect();
    for want in ["PD(0)", "PD(1)"] {
        if !perm.contains(want) {
            f.push(format!("PermUi: no {want} in its own output {perm:?}"));
        }
    }
    assert!(f.is_empty(), "{}", f.join("\n"));
}

// ---------------------------------------------------------------------------
// A2 — the witness of a write that becomes temporary at a call site.
// ---------------------------------------------------------------------------

/// A2a. A temporary call site keeps its witness, for Insert/Modify/Delete:
/// source evidence at the producer's excerpt, the exact hop list, and no
/// ORDERING_UNKNOWN. Mutation: skip depth-0 proof enumeration →
/// "HelperUiTemp: evidence unavailable".
#[test]
fn a2a_temp_call_site_keeps_its_witness() {
    each_mode(|doc, f| {
        for (root, ty, excerpt, hops) in [
            (
                "HelperUiTemp",
                "DB_INSERT",
                "R.Insert",
                "HelperUiTemp→HelperUi",
            ),
            (
                "HelperModUiTemp",
                "DB_MODIFY",
                "R.Modify",
                "HelperModUiTemp→HelperModUi",
            ),
            (
                "HelperDelUiTemp",
                "DB_DELETE",
                "R.Delete",
                "HelperDelUiTemp→HelperDelUi",
            ),
            (
                "ViaForwardUiTemp",
                "DB_INSERT",
                "R.Insert",
                "ViaForwardUiTemp→ForwardUi→HelperUi",
            ),
        ] {
            let e = entry(doc, root);
            let kt: Vec<_> = of_type(e, ty).into_iter().filter(|x| is_kt(x)).collect();
            if kt.is_empty() {
                f.push(format!("{root}: no known(true) {ty} ({})", summary(e)));
                continue;
            }
            for x in kt {
                if evidence_kind(x) != "source" {
                    f.push(format!("{root}: evidence unavailable"));
                } else if x["evidence"]["excerpt"] != excerpt {
                    f.push(format!(
                        "{root}: evidence excerpt {} != {excerpt}",
                        x["evidence"]["excerpt"]
                    ));
                }
                let got = path_strs(root, x);
                if got != [hops] {
                    f.push(format!("{root}: viaPaths {got:?} != [{hops}]"));
                }
            }
            if has_guarantee(e, "ORDERING_UNKNOWN") {
                f.push(format!("{root}: ORDERING_UNKNOWN present"));
            }
        }
    });
}

/// A2h. `alsem fingerprint` takes the same proofs digest does: through the
/// fingerprint indexes built from the snapshot, `HelperUiTemp`'s inherited
/// known-temporary insert has one witness path ending at `HelperUi`'s insert.
/// Mutations: `build_fingerprint_indexes_pub` hands over an empty solve
/// (`pd_solution`), or an empty edge / fact table (`typed_edges` /
/// `capability_facts`) → "no witness path" / index-out-of-bounds panic.
#[test]
fn a2h_fingerprint_witnesses_use_the_proofs() {
    let tmp = write_workspace();
    let ws = tmp.path();
    let model_id = compute_gate_model_instance_id(ws).expect("model id");
    let model = assemble_and_resolve_workspace_program(ws, &model_id, false).expect("model");
    let snap = compose_snapshot(&model);
    let idx = build_fingerprint_indexes_pub(&snap);
    let display: BTreeMap<&str, &str> = snap
        .identities
        .stable_ids
        .iter()
        .map(String::as_str)
        .zip(snap.identities.display_names.iter().map(String::as_str))
        .collect();
    let fact = snap
        .capability_facts
        .iter()
        .find(|f| {
            let name = display.get(f.subject.as_str()).copied().unwrap_or("");
            name.rsplit('.').next() == Some("HelperUiTemp")
                && f.op == "insert"
                && matches!(
                    &f.extra,
                    Some(SnapCapabilityExtra::Table {
                        temp_state: Some(SnapTempState::Known { value: true }),
                        ..
                    })
                )
        })
        .expect("HelperUiTemp's known-temporary insert fact");
    let out = reconstruct_witness_paths_pub(&fact.subject, fact, &idx, 256);
    let diags: Vec<&str> = out.diagnostics.iter().map(|d| d.kind.as_str()).collect();
    assert_eq!(out.paths.len(), 1, "no witness path ({diags:?})");
    assert_eq!(out.paths[0].query_hops.len(), 1, "HelperUiTemp→HelperUi");
    assert!(
        out.paths[0].terminal_hop.is_some(),
        "no terminal ({diags:?})"
    );
    assert!(!diags.contains(&"terminal-not-found"), "{diags:?}");
}

fn the_kt_insert<'a>(e: &'a Value, root: &str, f: &mut Vec<String>) -> Option<&'a Value> {
    let kt: Vec<_> = of_type(e, "DB_INSERT")
        .into_iter()
        .filter(|x| is_kt(x))
        .collect();
    if kt.len() != 1 {
        f.push(format!(
            "{root}: want exactly one known(true) DB_INSERT ({})",
            summary(e)
        ));
    }
    kt.first().copied()
}

/// A2d. All paths and conditionality are kept. `ViaCondTwiceTemp`: 2 viaPaths
/// (both `CondTwice` call sites), unconditional-on-success. `HelperTwoTemp`:
/// one cone fact, built by the INSERT branch alone; exactly ONE viaPath and
/// unconditional-on-success (p1 conditional and p2 unconditional are distinct
/// raw paths with equal projections). `CondOnlyTemp`: conditional, and
/// `viaPathsTruncated == false`. Mutations: keep one link per state →
/// "ViaCondTwiceTemp: expected 2 viaPaths"; dedup raw paths by edge sequence
/// only / keep the first conditionality on an equal projected merge →
/// "HelperTwoTemp: conditionality conditional"; skip the insert-branch
/// normalization → "HelperTwoTemp: expected 1 viaPath, got 2"; fold the BFS's
/// own `terminal-not-found` into `had_truncation` → "CondOnlyTemp: conditionality unknown".
#[test]
fn a2d_all_paths_and_conditionality_kept() {
    let c1 = line_of("condtwice-1");
    let c2 = line_of("condtwice-2");
    each_mode(|doc, f| {
        let e = entry(doc, "ViaCondTwiceTemp");
        if let Some(x) = the_kt_insert(e, "ViaCondTwiceTemp", f) {
            let paths = via_paths(x);
            if paths.len() != 2 {
                f.push(format!(
                    "ViaCondTwiceTemp: expected 2 viaPaths, got {} ({:?})",
                    paths.len(),
                    path_strs("ViaCondTwiceTemp", x)
                ));
            }
            let last: BTreeSet<u64> = paths
                .iter()
                .filter_map(|p| hop_lines(p).last().copied())
                .collect();
            if last != BTreeSet::from([c1, c2]) {
                f.push(format!(
                    "ViaCondTwiceTemp: CondTwice call sites {last:?} != {{{c1}, {c2}}}"
                ));
            }
            if x["conditionality"] != "unconditional-on-success" {
                f.push(format!(
                    "ViaCondTwiceTemp: conditionality {}",
                    x["conditionality"].as_str().unwrap_or("?")
                ));
            }
        }

        let e = entry(doc, "HelperTwoTemp");
        if let Some(x) = the_kt_insert(e, "HelperTwoTemp", f) {
            let n = via_paths(x).len();
            if n != 1 {
                f.push(format!("HelperTwoTemp: expected 1 viaPath, got {n}"));
            }
            if x["conditionality"] != "unconditional-on-success" {
                f.push(format!(
                    "HelperTwoTemp: conditionality {}",
                    x["conditionality"].as_str().unwrap_or("?")
                ));
            }
        }

        let e = entry(doc, "CondOnlyTemp");
        if let Some(x) = the_kt_insert(e, "CondOnlyTemp", f) {
            if x["conditionality"] != "conditional" {
                f.push(format!(
                    "CondOnlyTemp: conditionality {}",
                    x["conditionality"].as_str().unwrap_or("?")
                ));
            }
            if x["viaPathsTruncated"] != false {
                f.push("CondOnlyTemp: viaPathsTruncated true".into());
            }
        }
    });
}

/// A2g. Enumeration incompleteness reaches the consumers: `IncRoot` has one
/// short conditional proof and one longer than `MAX_DEPTH` under ONE root first
/// hop; the kt effect has `viaPathsTruncated == true` and conditionality
/// `unknown`. Mutation: drop the `incomplete` OR into `had_truncation` →
/// "IncRoot: conditionality conditional".
#[test]
fn a2g_incompleteness_reaches_consumers() {
    each_mode(|doc, f| {
        let e = entry(doc, "IncRoot");
        if let Some(x) = the_kt_insert(e, "IncRoot", f) {
            if x["conditionality"] != "unknown" {
                f.push(format!(
                    "IncRoot: conditionality {}",
                    x["conditionality"].as_str().unwrap_or("?")
                ));
            }
            if x["viaPathsTruncated"] != true {
                f.push("IncRoot: viaPathsTruncated false".into());
            }
        }
    });
}

/// A2f. Degraded witnesses never share an occurrence: `DeepRoot` (68 hops, past
/// `MAX_DEPTH`) has a kt and an nt DB_INSERT, both with unavailable evidence,
/// with different `factId`s. Mutation: drop the `|kt` canonical suffix →
/// "DeepRoot: factIds equal".
#[test]
fn a2f_degraded_witnesses_never_share_an_occurrence() {
    each_mode(|doc, f| {
        let e = entry(doc, "DeepRoot");
        let ins = of_type(e, "DB_INSERT");
        let kt: Vec<_> = ins.iter().filter(|x| is_kt(x)).collect();
        let nt: Vec<_> = ins.iter().filter(|x| is_nt(x)).collect();
        if ins.len() == 1 && kt.is_empty() && temp_state(ins[0]).is_some_and(|s| s.contains(',')) {
            // Both classes in ONE effect: the kt and the nt write share an
            // occurrence (one factId) — the defect itself.
            f.push(format!(
                "DeepRoot: factIds equal (kt and nt share one effect: {})",
                summary(e)
            ));
            return;
        }
        if kt.len() != 1 || nt.len() != 1 {
            f.push(format!(
                "DeepRoot: want one kt and one nt DB_INSERT ({})",
                summary(e)
            ));
            return;
        }
        for x in [kt[0], nt[0]] {
            if evidence_kind(x) != "unavailable" {
                f.push(format!(
                    "DeepRoot: precondition: evidence {} (want unavailable)",
                    evidence_kind(x)
                ));
            }
        }
        if kt[0]["factId"] == nt[0]["factId"] {
            f.push("DeepRoot: factIds equal".into());
        }
    });
}

// ---------------------------------------------------------------------------
// A3 — Validate / Copy / TransferFields are not DB writes (D5).
// ---------------------------------------------------------------------------

/// A3. State-only ops are not DB writes, on temporary and real records alike.
/// Replaces the #65 block that pinned "ValidateTemp emits a DB_MODIFY" (the
/// pinned behaviour changes, see CHANGELOG). Mutation: put each op back in
/// `modify`, one at a time → "<op routine>: DB_MODIFY present".
#[test]
fn a3_state_only_ops_are_not_db_writes() {
    each_mode(|doc, f| {
        for name in [
            "ValidateTemp",
            "ValidateReal",
            "CopyTemp",
            "CopyShareTemp",
            "CopyReal",
            "TransferTemp",
            "TransferReal",
            "ValidateRealUi",
            "CopyRealUi",
        ] {
            let e = entry(doc, name);
            for x in db_effects(e) {
                f.push(format!(
                    "{name}: {} present",
                    x["type"].as_str().unwrap_or("?")
                ));
            }
        }
        for name in ["ValidateRealUi", "CopyRealUi"] {
            if has_label(entry(doc, name)) {
                f.push(format!("{name}: label present"));
            }
        }
        f.sort();
        f.dedup();
    });
}
