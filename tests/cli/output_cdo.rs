//! Output invariants on the CDO baseline (MCP output plan
//! `docs/superpowers/plans/2026-11-04-mcp-output-fixes.md`, item T): the checks of
//! `output_invariants` on three real routines, the first CDO-gated member of the `cli`
//! umbrella. Skipped unless `CDO_WS` names the workspace (`crate::cdo`); run by
//! `scripts/cdo-gate`, never by the debug suites.
//!
//! The routine set (owner decision 4): `CDO Issue Document::IssueReminder` (the routine
//! of the review), `CDO E-Mail Handler::RunReportID` (a hub, the stage-5 and W
//! measurements' routine) and `CDO Telemetry::UpdateLastUpload` (small and plain: one
//! `DOSetup.Modify`, no dependency call, coverage complete; chosen in wave 0).
//!
//! Every answer comes from ONE `alsem mcp` server (one resident build of CDO), so the
//! checks also read what an agent reads: the envelope, and the stdout byte for byte
//! (inline or the spilled file). The answers are collected once per test process.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::{Value, json};

use crate::mcp_server::{Server, cli_bytes, envelope};
use crate::output_invariants::{Anchor, check_anchors, dump_dependency_source, human_anchors};

/// `(label, explain/prove/digest selector, query selector, touches table, writes table)`.
pub(crate) const ROUTINES: &[(&str, &str, &str, &str, &str)] = &[
    (
        "IssueReminder",
        "CDO Issue Document::IssueReminder",
        "CDO Issue Document.IssueReminder",
        "CDO File",
        "Issued Reminder Header",
    ),
    (
        "RunReportID",
        "CDO E-Mail Handler::RunReportID",
        "CDO E-Mail Handler.RunReportID",
        "CDO File",
        "CDO E-Mail Log",
    ),
    (
        "UpdateLastUpload",
        "CDO Telemetry::UpdateLastUpload",
        "CDO Telemetry.UpdateLastUpload",
        "CDO Setup",
        "CDO Setup",
    ),
];

/// One tool call's answer: the serialized `tools/call` result (as the client
/// receives it), its envelope and the CLI stdout it carries.
pub(crate) struct Answer {
    pub result_text: String,
    pub envelope: Value,
    pub stdout: String,
}

pub(crate) struct CdoAnswers {
    pub ws: PathBuf,
    pub dump: tempfile::TempDir,
    /// `"<routine> <call>"` -> answer, in call order.
    pub answers: BTreeMap<String, Answer>,
}

/// The calls made for each routine: `(call label, tool, arguments)`.
fn calls(
    sel: &str,
    qsel: &str,
    table: &str,
    wtable: &str,
) -> Vec<(&'static str, &'static str, Value)> {
    vec![
        ("explain human", "alsem_explain", json!({"routines": [sel]})),
        (
            "explain json",
            "alsem_explain",
            json!({"routines": [sel], "format": "json"}),
        ),
        (
            "prove may-commit human",
            "alsem_prove",
            json!({"routine": sel, "question": "may-commit", "format": "human", "deterministic": true}),
        ),
        (
            "prove may-commit json",
            "alsem_prove",
            json!({"routine": sel, "question": "may-commit", "format": "json", "deterministic": true}),
        ),
        (
            "prove writes-table json",
            "alsem_prove",
            json!({"routine": sel, "question": format!("writes-table:{wtable}"), "format": "json", "deterministic": true}),
        ),
        (
            "digest json",
            "alsem_digest",
            json!({"routines": [sel], "deterministic": true}),
        ),
        (
            "digest human",
            "alsem_digest",
            json!({"routines": [sel], "format": "human", "deterministic": true}),
        ),
        (
            "query effects human",
            "alsem_query_effects",
            json!({"routine": qsel, "deterministic": true}),
        ),
        (
            "query effects json",
            "alsem_query_effects",
            json!({"routine": qsel, "format": "json", "deterministic": true}),
        ),
        (
            "query touches human",
            "alsem_query_touches",
            json!({"table": table, "from": qsel, "deterministic": true}),
        ),
        // Wave 3a: the `CDO Log Attachment` case (design §1 item 9, §12).
        (
            "prove writes-table log-attachment json",
            "alsem_prove",
            json!({"routine": sel, "question": format!("writes-table:{LOG_ATTACHMENT}"), "format": "json", "deterministic": true}),
        ),
    ]
}

/// The table of design §1 item 9: query lists `Modify` operations on it the
/// capability cone rules out under literal arguments.
const LOG_ATTACHMENT: &str = "CDO Log Attachment";

/// IssueReminder's three `Modify` operations on [`LOG_ATTACHMENT`] (design §1
/// item 9): `CDO E-Mail Log::DeleteLogAndAttachmentData`
/// (`CDOEMailLog.Table.al:566`), `CDO Log Attachment::MoveToFilesystem` (`:110`)
/// and `::MoveToAzureBlob` (`:132`). Every route from IssueReminder passes
/// `OnInsert = true` into `MoveEMailToFile`, which exits before them; the
/// cone drops them, query's per-operation rows (L4, no literal pass) list
/// them.
const LOG_ATTACHMENT_MODIFIES: &[&str] = &[
    "98e44272062b13c6/a3c756ee42ea8c1596505540f26e96e75c254da4a710a10e874e7d8436202861/op2",
    "98e44272062b13c6/ad4dd41f5631121196e1bf00583864d95520cfdfc2b166858cba1c696af4369b/op1",
    "98e44272062b13c6/fe793a463cda533b7d5b791521249214b63f6696b25b1b5dd00cdf0b4519c6f3/op1",
];

/// MCP output wave 3a, the cross-tool agreement's query leg on CDO (design
/// §7 as decided in §12): for each routine, every table the cone's physical
/// class names (explain's own rows and its rows beyond an entry point) is in
/// query effects' writes (its per-operation rows or the cone's entry
/// points); query's writes beyond entry points are explain's, table for
/// table; and today no table query's per-operation rows write may persist
/// is outside the cone (measured 2026-11-04 on `bc3ccb18`: IR 15 own, 131
/// beyond; RR 13 own, 396 beyond). A future extra must name its substrate
/// difference here. The leg is per table; per operation kind query may
/// list more, and the `CDO Log Attachment` case pins the one stated
/// difference (literal-dead in the cone) by its exact operation ids:
/// query's IR rows hold exactly [`LOG_ATTACHMENT_MODIFIES`] as its `Modify`
/// rows on the table, explain names no `DB_MODIFY` on it (own or beyond),
/// the table is still a physical write of both (its `DeleteAll` and
/// `Insert`), and prove answers `yes` with no `DB_MODIFY` evidence.
#[test]
fn cdo_query_effects_writes_agree_with_the_cone() {
    let Some(a) = cdo_answers() else {
        return;
    };
    let doc = |label: &str, call: &str| -> Value {
        serde_json::from_str(&a.answers[&format!("{label} {call}")].stdout).expect("JSON")
    };
    for (label, ..) in ROUTINES {
        let explain = doc(label, "explain json");
        let query = doc(label, "query effects json");
        assert_eq!(
            query["payload"]["summary"]["scope"],
            "operations-one-routine-deep; writes-beyond-entry-points-full-cone",
            "{label}: the cone was read"
        );
        let (e_own, e_beyond) =
            crate::output_invariants::explain_physical_tables(&explain["cards"][0]);
        let (q_own, q_beyond) = crate::output_invariants::query_write_tables(&query);
        assert_eq!(q_beyond, e_beyond, "{label}: query's beyond is explain's");
        let missing: Vec<&String> = e_own
            .iter()
            .chain(&e_beyond)
            .filter(|t| !q_own.contains(*t) && !q_beyond.contains(*t))
            .collect();
        assert!(
            missing.is_empty(),
            "{label}: cone tables query does not name: {missing:?}"
        );
        let extra: Vec<&String> = q_own
            .iter()
            .filter(|t| !e_own.contains(*t) && !e_beyond.contains(*t))
            .collect();
        assert!(
            extra.is_empty(),
            "{label}: query tables the cone does not name: {extra:?}"
        );
        // The human section is explain's own lines.
        let block = crate::output_invariants::beyond_block(
            &a.answers[&format!("{label} query effects human")].stdout,
        );
        let e_block = crate::output_invariants::beyond_block(
            &a.answers[&format!("{label} explain human")].stdout,
        );
        assert_eq!(block, e_block, "{label}: the beyond section");
    }
    // The CDO Log Attachment case (IssueReminder).
    let query = doc("IssueReminder", "query effects json");
    let explain = doc("IssueReminder", "explain json");
    let mut modifies: Vec<&str> = query["payload"]["effects"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["tableName"] == LOG_ATTACHMENT && e["op"] == "Modify")
        .filter_map(|e| e["operationId"].as_str())
        .collect();
    modifies.sort_unstable();
    assert_eq!(modifies, LOG_ATTACHMENT_MODIFIES, "query's Modify rows");
    let mut explain_types: Vec<&str> = Vec::new();
    for w in explain["cards"][0]["effects"]["writes"].as_array().unwrap() {
        let names_it = w["table"] == LOG_ATTACHMENT
            || w["resources"]
                .as_array()
                .is_some_and(|rs| rs.iter().any(|r| r["display"] == LOG_ATTACHMENT));
        if names_it && w["class"] != "temporary" && w["literalUnreachable"] != true {
            explain_types.push(w["op"].as_str().unwrap());
        }
    }
    explain_types.sort_unstable();
    assert_eq!(
        explain_types,
        ["DB_DELETE", "DB_INSERT"],
        "explain's rows on the table"
    );
    let prove = doc("IssueReminder", "prove writes-table log-attachment json");
    let r = &prove["payload"]["result"];
    assert_eq!(r["answer"], "yes", "{r}");
    let mut types: Vec<&str> = r["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["type"].as_str())
        .collect();
    types.sort_unstable();
    assert_eq!(types, ["DB_DELETE", "DB_INSERT"], "prove's evidence");
}

/// Every routine's answers off one server, once per process; `None` without CDO.
pub(crate) fn cdo_answers() -> Option<&'static CdoAnswers> {
    static ANSWERS: OnceLock<Option<CdoAnswers>> = OnceLock::new();
    ANSWERS
        .get_or_init(|| {
            let ws = crate::cdo::cdo_ws_or_enforce()?;
            let dump = tempfile::tempdir().unwrap();
            dump_dependency_source(&ws, dump.path());
            let mut server = Server::ready(&ws, None);
            let mut answers = BTreeMap::new();
            let mut id = 1000;
            for (label, sel, qsel, table, wtable) in ROUTINES {
                for (call, tool, args) in calls(sel, qsel, table, wtable) {
                    id += 1;
                    let result = server.call(id, tool, args.clone());
                    let env = envelope(&result);
                    let (stdout, stderr, exit) = cli_bytes(&env);
                    assert_eq!(
                        exit,
                        0,
                        "{label} {call}: {}",
                        String::from_utf8_lossy(&stderr)
                    );
                    answers.insert(
                        format!("{label} {call}"),
                        Answer {
                            result_text: result.to_string(),
                            envelope: env,
                            stdout: String::from_utf8(stdout).expect("UTF-8 stdout"),
                        },
                    );
                }
            }
            assert!(server.finish().success(), "the server exits cleanly");
            Some(CdoAnswers { ws, dump, answers })
        })
        .as_ref()
}

/// Item T, anchor source-check on CDO (catches B8): a deterministic sample of 50
/// verifiable anchors from the three routines' human outputs (explain, prove,
/// digest, query effects, query touches) opens to the line each describes; `dep:`
/// anchors resolve against `aldump --dump-dependency-source`. An anchor whose file
/// is not in the dump is skipped and counted; more than 10% skipped fails.
///
/// Discrimination (2026-11-04, cdo profile): printing the 0-based row again in
/// explain's `anchor_human`, digest's `ev_line` and query's `origin_human` fails this
/// test with 49 of the 50 sampled anchors one line early; restored, it passes.
#[test]
fn cdo_anchor_source_check() {
    let Some(a) = cdo_answers() else { return };
    let mut anchors: Vec<Anchor> = a
        .answers
        .iter()
        .filter(|(k, _)| k.ends_with(" human"))
        .flat_map(|(_, ans)| human_anchors(&ans.stdout))
        .filter(|x| !x.tokens.is_empty())
        .collect();
    anchors.sort_by(|x, y| (&x.file, x.line, &x.printed).cmp(&(&y.file, y.line, &y.printed)));
    anchors.dedup_by(|x, y| x.file == y.file && x.line == y.line && x.printed == y.printed);
    assert!(
        anchors.len() >= 50,
        "precondition: at least 50 verifiable anchors, got {}",
        anchors.len()
    );
    let step = anchors.len() / 50;
    let sample: Vec<Anchor> = anchors.into_iter().step_by(step).take(50).collect();
    let deps = sample.iter().filter(|x| x.file.starts_with("dep:")).count();
    let c = check_anchors(&sample, &a.ws, a.dump.path());
    assert!(
        c.failures.is_empty(),
        "anchors that do not open to what they print:\n{}",
        c.failures.join("\n")
    );
    assert!(
        c.missing_file * 10 <= sample.len(),
        "{} of {} sampled anchors name a file the dump does not have",
        c.missing_file,
        sample.len()
    );
    assert!(
        deps >= 1 && c.matched >= 45,
        "precondition: the sample checks dependency code too ({deps} dep anchors, {} matched)",
        c.matched
    );
}

/// Item T, `lineBase` on CDO (catches E5): every JSON document the three routines'
/// answers carry, and every envelope, says `lineBase: 1`.
#[test]
fn cdo_documents_and_envelopes_state_line_base_1() {
    let Some(a) = cdo_answers() else { return };
    let mut failures = Vec::new();
    for (key, ans) in &a.answers {
        if ans.envelope["lineBase"] != 1 {
            failures.push(format!(
                "{key}: envelope lineBase {}",
                ans.envelope["lineBase"]
            ));
        }
        if key.ends_with(" json") {
            let doc: Value = serde_json::from_str(&ans.stdout).unwrap();
            let base = if doc.get("schema").is_some() {
                &doc["lineBase"]
            } else {
                &doc["payload"]["lineBase"]
            };
            if base != 1 {
                failures.push(format!("{key}: document lineBase {base}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Item T, size budgets for the digest summary on CDO (catches D1, D3, E3; plan
/// 1b): each routine's digest answer (JSON and human) carries the renderer's
/// summary, at most 8 KiB, listing its one routine in full (not degraded to
/// counts), and the whole serialized `tools/call` result stays under 40,000
/// characters, however large the digest file is (5.6 MB for `IssueReminder`).
///
/// Discrimination: re-embedding the unresolved call sites in the entry summary
/// (the D1 defect) makes `IssueReminder`'s summary exceed the cap, which drops
/// its routine (`routinesListed` 0): see `digest_cli`'s
/// `digest_summary_counts_and_names_without_the_unresolved_items` for the unit
/// proof of the same break.
#[test]
fn cdo_digest_summary_is_small_and_lists_the_routine() {
    let Some(a) = cdo_answers() else { return };
    let mut failures = Vec::new();
    for (label, ..) in ROUTINES {
        for format in ["json", "human"] {
            let key = format!("{label} digest {format}");
            let ans = &a.answers[&key];
            let s = &ans.envelope["summary"];
            let bytes = s.to_string().len();
            if s.is_null() || bytes > al_sem::engine::batch::output::SUMMARY_CAP_BYTES {
                failures.push(format!("{key}: summary of {bytes} bytes: {s}"));
            }
            if s["routinesListed"] != 1 || s["rootsResolved"] != 1 || s.get("countsOnly").is_some()
            {
                failures.push(format!("{key}: the one routine is not listed in full: {s}"));
            }
            if ans.result_text.len() > 40_000 {
                failures.push(format!(
                    "{key}: tools/call result of {} characters",
                    ans.result_text.len()
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Item T, "MCP result against the client's real limit" and "one copy" on CDO
/// (plan 1c; E4, P3): Claude Code files a result whose `structuredContent` is
/// over 50,000 characters (measured 2026-11-04: it reads that field alone), so
/// with the default inline cap every one of the 30 answers keeps
/// `structuredContent` under it, and holds the stdout at most once (`output`
/// XOR `stdoutText`).
///
/// Discrimination (2026-11-04): restoring the triple copy (pre-1c) is what
/// this catches: `RunReportID`'s writes-table JSON answer measured 103,764
/// characters of `structuredContent` then (hands-on run). A STATED LIMIT:
/// raising the default cap back to 64 KiB does NOT fail it (cdo profile run:
/// pass), because none of the 30 answers has a stdout between 40 and 64 KiB
/// (a JSON one is re-serialized compactly as `output`, smaller than its text).
/// The 40 KiB default rests on arithmetic (stdout escaped plus a ~1 KB
/// envelope under 50,000 characters), not on this test.
#[test]
fn cdo_every_answer_fits_the_client_limit() {
    let Some(a) = cdo_answers() else { return };
    let mut failures = Vec::new();
    for (key, ans) in &a.answers {
        let result: Value = serde_json::from_str(&ans.result_text).unwrap();
        let sc = &result["structuredContent"];
        let chars = sc.to_string().chars().count();
        if chars > 50_000 {
            failures.push(format!("{key}: structuredContent of {chars} characters"));
        }
        if sc.get("output").is_some() && sc.get("stdoutText").is_some() {
            failures.push(format!("{key}: two copies"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Item T, boundary groups on CDO (plan 1d; catches B2, D4, P2): in every
/// routine's digest JSON and prove JSON, each group names its boundary
/// owner-qualified, `resourceCount` equals `resources[]`'s length, and no
/// resource-less group (a COMMIT, an error) claims resources or an unresolved
/// one. The same checks as `output_invariants::group_failures`, whose
/// fixture test carries the discrimination proofs.
#[test]
fn cdo_boundary_groups_name_their_owner_and_every_resource() {
    let Some(a) = cdo_answers() else { return };
    let mut failures = Vec::new();
    let mut groups = 0;
    for (key, ans) in &a.answers {
        let doc: Value = match key.as_str() {
            k if k.ends_with("digest json") || k.contains("prove") && k.ends_with(" json") => {
                serde_json::from_str(&ans.stdout).unwrap()
            }
            _ => continue,
        };
        let effects: Vec<Value> = match doc["payload"]["entries"].as_array() {
            Some(entries) => entries
                .iter()
                .flat_map(|e| e["effects"].as_array().cloned().unwrap_or_default())
                .collect(),
            None => doc["payload"]["result"]["evidence"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        };
        groups += effects
            .iter()
            .filter(|e| e["detail"]["boundary"].is_string())
            .count();
        failures.extend(crate::output_invariants::group_failures(key, &effects));
    }
    assert!(
        groups >= 50,
        "precondition: CDO's boundary groups ({groups})"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Item T, "a summary on every spill" on CDO (plan 1e, E2): every one of the 30
/// answers that went to a file (IssueReminder's 80 KB explain card, its query
/// answers, RunReportID's 194 KB card, ...) carries the renderer's summary, at
/// most 8 KiB, in a `tools/call` result under 40,000 characters. The fixture
/// test `output_invariants::every_spilled_answer_carries_a_summary` carries the
/// discrimination proof.
#[test]
fn cdo_every_spilled_answer_carries_a_small_summary() {
    let Some(a) = cdo_answers() else { return };
    let mut failures = Vec::new();
    let mut spilled = 0;
    for (key, ans) in &a.answers {
        if !ans.envelope["stdoutFile"].is_object() {
            continue;
        }
        spilled += 1;
        let s = &ans.envelope["summary"];
        let bytes = s.to_string().len();
        if !s.is_object() || bytes > al_sem::engine::batch::output::SUMMARY_CAP_BYTES {
            failures.push(format!("{key}: summary of {bytes} bytes: {s}"));
        }
        if ans.result_text.len() > 40_000 {
            failures.push(format!("{key}: result of {} chars", ans.result_text.len()));
        }
    }
    assert!(
        spilled >= 10,
        "precondition: CDO's large answers spill ({spilled})"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// T-W2-15, the digest size ratchet (owner decision 2026-11-04, spec §9 q1):
/// the default 3.x digest JSON (pretty) of IssueReminder and RunReportID is at
/// most 1,250,000 bytes whole and at most 720,000 bytes as its core (the
/// document without `eventsByPublisher`, every event name kept by O2),
/// pinned to the CDO baseline `bc3ccb18`. A change that grows either past its
/// budget fails here; one that shrinks it far may lower the budget.
#[test]
fn cdo_digest_json_fits_its_size_budgets() {
    let Some(a) = cdo_answers() else { return };
    const WHOLE: usize = 1_250_000;
    const CORE: usize = 720_000;
    fn strip(v: &mut Value) {
        match v {
            Value::Object(m) => {
                m.remove("eventsByPublisher");
                m.values_mut().for_each(strip);
            }
            Value::Array(xs) => xs.iter_mut().for_each(strip),
            _ => {}
        }
    }
    for label in ["IssueReminder", "RunReportID"] {
        let doc = &a.answers[&format!("{label} digest json")].stdout;
        let mut v: Value = serde_json::from_str(doc).expect("digest JSON");
        assert_eq!(v["schemaVersion"], "3.0.0", "{label}");
        strip(&mut v);
        let core = serde_json::to_string_pretty(&v).unwrap().len() + 1;
        eprintln!(
            "{label}: digest JSON {} bytes (budget {WHOLE}), core {core} bytes (budget {CORE})",
            doc.len()
        );
        assert!(doc.len() <= WHOLE, "{label}: {} > {WHOLE}", doc.len());
        assert!(core <= CORE, "{label}: core {core} > {CORE}");
    }
}

/// W2-2 gap (a), T-W2-16 leg 2 on CDO: explain's and digest's
/// `--writes-table` give prove's own answer (`answer`, `blockedBy`,
/// `assumptions`) for IssueReminder and RunReportID, over the first 12
/// tables explain's card lists and 12 tables of the program no output of the
/// routine names (a fixed list of Base Application master tables, kept only
/// where neither the explain nor the digest document mentions them).
#[test]
fn cdo_explain_digest_and_prove_give_one_table_answer() {
    let Some(ws) = crate::cdo::cdo_ws_or_enforce() else {
        return;
    };
    const ABSENT: &[&str] = &[
        "Country/Region",
        "Currency",
        "Payment Terms",
        "Salesperson/Purchaser",
        "Unit of Measure",
        "Shipment Method",
        "Language",
        "Post Code",
        "Bank Account",
        "Location",
        "Vendor",
        "Item",
        "Fixed Asset",
        "Employee",
        "Resource",
        "Territory",
    ];
    let mut server = Server::ready(&ws, None);
    let mut id = 5000;
    let mut ask = |tool: &str, args: Value| -> Value {
        id += 1;
        let result = server.call(id, tool, args);
        let (stdout, stderr, exit) = cli_bytes(&envelope(&result));
        assert_eq!(exit, 0, "{tool}: {}", String::from_utf8_lossy(&stderr));
        serde_json::from_slice(&stdout).expect("JSON")
    };
    let key = |v: &Value| {
        (
            v["answer"].clone(),
            v.get("blockedBy").cloned().unwrap_or(json!([])),
            v.get("assumptions").cloned().unwrap_or(Value::Null),
        )
    };
    for (label, sel, ..) in &ROUTINES[..2] {
        let e = ask(
            "alsem_explain",
            json!({"routines": [sel], "format": "json"}),
        );
        let card = &e["cards"][0];
        let mut named: Vec<String> = Vec::new();
        for w in card["effects"]["writes"].as_array().unwrap() {
            let names: Vec<String> = match w["resources"].as_array() {
                Some(rs) => rs
                    .iter()
                    .filter_map(|r| r["display"].as_str().map(String::from))
                    .collect(),
                None => w["table"].as_str().map(String::from).into_iter().collect(),
            };
            for n in names {
                if !n.is_empty() && !named.contains(&n) && named.len() < 12 {
                    named.push(n);
                }
            }
        }
        let d = ask(
            "alsem_digest",
            json!({"routines": [sel], "deterministic": true}),
        );
        let (et, dt) = (card.to_string(), d.to_string());
        let absent: Vec<String> = ABSENT
            .iter()
            .filter(|t| !et.contains(&format!("\"{t}\"")) && !dt.contains(&format!("\"{t}\"")))
            .take(12)
            .map(|t| (*t).to_string())
            .collect();
        assert_eq!(named.len(), 12, "{label}: {named:?}");
        assert!(absent.len() >= 8, "{label}: {absent:?}");
        let tables: Vec<String> = named.into_iter().chain(absent).collect();
        let ea = ask(
            "alsem_explain",
            json!({"routines": [sel], "format": "json", "writesTables": tables}),
        );
        let da = ask(
            "alsem_digest",
            json!({"routines": [sel], "deterministic": true, "writesTables": tables}),
        );
        let ea = ea["cards"][0]["tableAnswers"].as_array().unwrap().clone();
        let da = da["payload"]["entries"][0]["tableAnswers"]
            .as_array()
            .unwrap()
            .clone();
        for (i, t) in tables.iter().enumerate() {
            let p = ask(
                "alsem_prove",
                json!({"routine": sel, "question": format!("writes-table:{t}"), "format": "json", "deterministic": true}),
            );
            let pr = &p["payload"]["result"];
            assert_eq!(key(&ea[i]), key(pr), "{label} explain vs prove: {t}");
            assert_eq!(key(&da[i]), key(pr), "{label} digest vs prove: {t}");
        }
    }
    assert!(server.finish().success());
}
