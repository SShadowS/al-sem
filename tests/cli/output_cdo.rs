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
    /// The dependency source dump (`crate::process_scratch`): lives for the whole process.
    pub dump: PathBuf,
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
        // Item T: the CDO rendering goldens read the human writes-table answer.
        (
            "prove writes-table human",
            "alsem_prove",
            json!({"routine": sel, "question": format!("writes-table:{wtable}"), "format": "human", "deterministic": true}),
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
        // Final CDO review, item 1: an `unknown` with hundreds of blockers.
        (
            "prove writes-table data-exch human",
            "alsem_prove",
            json!({"routine": sel, "question": format!("writes-table:{DATA_EXCH}"), "format": "human", "deterministic": true}),
        ),
        // Final CDO review, item 3: every entry point's line and sites.
        (
            "explain detail",
            "alsem_explain",
            json!({"routines": [sel], "detail": true}),
        ),
    ]
}

/// Final CDO review, item 1: IssueReminder's `writes-table:Data Exch.` is
/// `unknown` with 287 blockers (2026-11-05, `bc3ccb18`); one line each was
/// 294 lines and 47,875 bytes, over the 40 KiB inline cap.
const DATA_EXCH: &str = "Data Exch.";

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
            // A `static` keeps the dump for the anchor check: never a `TempDir` here
            // (see `process_scratch`; ~12,000 files leaked per run before).
            let dump = crate::process_scratch("output-cdo-dependency-source");
            dump_dependency_source(&ws, &dump);
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
    let c = check_anchors(&sample, &a.ws, &a.dump);
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
/// with the default inline cap every answer keeps `structuredContent` under it
/// and the whole result under the wire budget (T-W2-23's two budgets), and
/// holds the stdout at most once (`output` XOR `stdoutText`).
///
/// Discrimination (2026-11-04): restoring the triple copy (pre-1c) is what
/// this catches: `RunReportID`'s writes-table JSON answer measured 103,764
/// characters of `structuredContent` then (hands-on run). A STATED LIMIT:
/// raising the default cap back to 64 KiB does NOT fail it (cdo profile run:
/// pass), because none of the then 30 answers has a stdout between 40 and 64
/// KiB (a JSON one is re-serialized compactly as `output`, smaller than its
/// text). Since item T the pool bounds the escaped answer itself
/// (`tools::MCP_CLIENT_BUDGET_CHARS`), proven on the fixture by
/// `output_invariants_w3::every_answer_fits_the_client_and_the_wire`.
#[test]
fn cdo_every_answer_fits_the_client_limit() {
    let Some(a) = cdo_answers() else { return };
    let mut failures = Vec::new();
    for (key, ans) in &a.answers {
        let result: Value = serde_json::from_str(&ans.result_text).unwrap();
        let sc = &result["structuredContent"];
        // T-W2-23's two budgets: what the client reads, and the whole wire.
        failures.extend(crate::output_invariants_w3::budget_failures(key, &result));
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

/// Item T, "a summary on every spill" on CDO (plan 1e, E2): every answer that
/// went to a file (IssueReminder's 80 KB explain card, its query
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
/// most 1,310,000 bytes whole and at most 760,000 bytes as its core (the
/// document without `eventsByPublisher`, every event name kept by O2),
/// pinned to the CDO baseline `bc3ccb18`. A change that grows either past its
/// budget fails here; one that shrinks it far may lower the budget.
///
/// Raised 2026-11-06 (F1, record operations raise their table's platform
/// events) from 1,250,000 / 720,000: IssueReminder's cone now holds the
/// subscribers of the table events it raises, its digest measured 1,293,230
/// bytes whole and 744,546 core (+3.5%). The owner's ratchet: confirmed by
/// the owner on 2026-11-06.
///
/// Raised again 2026-11-06 (output validation V3, digest 3.1.0: routine rows
/// gain `signature`, `member`, `selector`, `handle`, and every display is the
/// one routine display) from 1,310,000 / 760,000: IssueReminder measured
/// 1,347,484 bytes whole and 798,800 core (+4.2% / +7.3%). The owner's
/// ratchet: accepted by the owner on 2026-11-06 as the new budget.
#[test]
fn cdo_digest_json_fits_its_size_budgets() {
    let Some(a) = cdo_answers() else { return };
    const WHOLE: usize = 1_380_000;
    const CORE: usize = 820_000;
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
        assert_eq!(v["schemaVersion"], "3.3.0", "{label}");
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
    // The location `role` (output validation F78) labels the document it is
    // printed in (prove and explain, not digest); the answer is the rest.
    fn without_roles(v: &mut Value) {
        match v {
            Value::Object(m) => {
                m.remove("role");
                m.values_mut().for_each(without_roles);
            }
            Value::Array(a) => a.iter_mut().for_each(without_roles),
            _ => {}
        }
    }
    let key = |v: &Value| {
        let mut blocked = v.get("blockedBy").cloned().unwrap_or(json!([]));
        without_roles(&mut blocked);
        (
            v["answer"].clone(),
            blocked,
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

// ---------------------------------------------------------------------------
// Item T on CDO (spec §8.3): the rows wave 3 adds, and the rendering goldens.
// ---------------------------------------------------------------------------

/// The CDO answers the rendering goldens and the size ratchets read: `(call
/// label, golden file suffix)`. The digest's is its envelope summary.
const RENDERED: &[(&str, &str)] = &[
    ("explain human", "explain.human.txt"),
    ("query effects human", "query-effects.human.txt"),
    ("query touches human", "query-touches.human.txt"),
    ("prove may-commit human", "prove-may-commit.human.txt"),
    ("prove writes-table human", "prove-writes-table.human.txt"),
    (
        "prove writes-table data-exch human",
        "prove-writes-table-data-exch.human.txt",
    ),
    ("digest human", "digest.human.txt"),
];

/// Per routine and call, the size ratchet (T-W2-15): lines and bytes of the
/// whole human output, measured 2026-11-05 on `bc3ccb18` with about 10%
/// headroom (explain IR 127 lines / 32,433 B, RR 139 / 37,671; query
/// effects IR 62 / 20,901, RR 70 / 24,677; touches 11 / 2.2 KB; digest
/// human IR 113 / 30,687, RR 123 / 35,392). A number may go down freely; up
/// only by editing it here with the reason. The plan's targets hold under
/// every one: explain ≤ 150 lines, query effects ≤ 100, touches ≤ 20.
///
/// Raised 2026-11-06 by output validation V3 (spec 2026-11-06 §2.1: every
/// occurrence of an overloaded name prints its full parameter types, names
/// are AL-quoted, dependency objects carry their number): measured explain
/// IR 136 lines / 39,169 B, RR 145 / 45,058; digest human IR 120 / 36,991,
/// RR 126 / 42,077 (of RR's explain growth, 5,295 B are parameter lists,
/// ~700 B dependency numbers, ~670 B quotes). Lines did not grow.
fn ratchet(label: &str, call: &str) -> Budget {
    let (lines, bytes) = match (label, call) {
        ("IssueReminder", "explain human") => (140, 43_000),
        ("RunReportID", "explain human") => (150, 49_500),
        (_, "explain human") => (30, 2_500),
        ("IssueReminder", "query effects human") => (70, 23_000),
        ("RunReportID", "query effects human") => (80, 27_000),
        (_, "query effects human") => (20, 2_000),
        // Callers through dependency code (design 2026-11-06 r2): RunReportID's
        // touches now lists its 2 code callers through Document Sending
        // Profile with folded witnesses, and the configured count (16 lines,
        // 3,683 B on 2026-11-06; before 11 / 2.2 KB).
        // Output validation V2: the tier words (`[via-dependency]`,
        // `[via-configuration]` and why a configured caller is a candidate)
        // add ~100 B (4,201 B on 2026-11-06).
        ("RunReportID", "query touches human") => (20, 4_400),
        // Output validation V4 (§5.2): the four header lines (~450 B) where
        // the removed boilerplate was shorter: IssueReminder's touches 3,030
        // B, RunReportID's may-commit 2,179 B on 2026-11-06.
        (_, "query touches human") => (20, 3_300),
        (_, "prove may-commit human") => (20, 2_400),
        ("RunReportID", "prove writes-table human") => (20, 5_000),
        (_, "prove writes-table human") => (20, 2_500),
        // Final CDO review, item 1: IssueReminder's 287 blockers fold by
        // kind (22 lines, 2,061 B on 2026-11-05; before 294 / 47,875).
        // RunReportID answers `yes` with 5 evidence rows and their paths
        // (17 lines, 7,366 B).
        ("RunReportID", "prove writes-table data-exch human") => (20, 8_200),
        (_, "prove writes-table data-exch human") => (30, 3_000),
        ("IssueReminder", "digest human") => (125, 40_700),
        ("RunReportID", "digest human") => (135, 46_300),
        (_, "digest human") => (30, 2_500),
        _ => (150, 40 * 1024),
    };
    Budget { lines, bytes }
}

use crate::output_invariants_w3::{
    Budget, crossing_failures, evidence_failures, hygiene_failures, kind_failures, size_failures,
};

/// The CDO inputs a rendering golden was minted from: `(git sha, dirty,
/// dependency closure)`, the stamps every CDO golden carries (CLAUDE.md).
fn cdo_stamp(ws: &std::path::Path) -> Value {
    let git = |args: &[&str]| -> Option<String> {
        let o = crate::git::reader(ws).args(args).output().ok()?;
        o.status
            .success()
            .then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    json!({
        "workspace_git_sha": git(&["rev-parse", "HEAD"]),
        "workspace_dirty": git(&["status", "--porcelain"]).map(|s| !s.is_empty()),
        "dependency_closure_sha256":
            al_sem::program::resolve::semantic_golden::dependency_closure_digest(ws).ok(),
    })
}

fn goldens_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/cli-output-goldens/cdo")
}

/// Whether the CDO workspace is the one the goldens and ratchets were
/// minted from. Under `REGEN_TEMP_GOLDENS=1` the stamp is (re)written; on
/// drift `ENFORCE_CDO_WS=1` fails and an ungated run warns and skips.
fn stamp_matches(ws: &std::path::Path) -> bool {
    let path = goldens_dir().join("STAMP.json");
    let now = cdo_stamp(ws);
    if crate::regen::regen_mode() {
        std::fs::create_dir_all(goldens_dir()).unwrap();
        crate::regen::write_golden(&path, serde_json::to_string_pretty(&now).unwrap() + "\n")
            .unwrap();
        return true;
    }
    let minted: Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);
    if minted == now {
        return true;
    }
    let why = format!(
        "the CDO workspace drifted from the rendering goldens' stamp: minted {minted}, now {now}"
    );
    assert!(
        std::env::var("ENFORCE_CDO_WS").as_deref() != Ok("1"),
        "{why}"
    );
    eprintln!("warning: {why}; skipping");
    false
}

/// Item T rows 1, 2 and 6 and T-W2-15 on CDO: each routine's human answers
/// (explain, query effects, touches, prove may-commit and writes-table,
/// digest human) print no blank name, `(no anchor)` row, escaped space or
/// two identical lines, and each stays within its ratchet ([`ratchet`]:
/// lines AND bytes of the whole output; every line ≤ 2,000 characters),
/// pinned to the CDO input stamps. Formatting checks: no discrimination
/// proof (the fixture twin is `output_invariants_w3::
/// human_outputs_are_clean_and_within_budget`).
#[test]
fn cdo_human_outputs_are_clean_and_within_their_ratchets() {
    let Some(a) = cdo_answers() else { return };
    if !stamp_matches(&a.ws) {
        return;
    }
    let mut failures = Vec::new();
    for (label, ..) in ROUTINES {
        for (call, _) in RENDERED {
            let key = format!("{label} {call}");
            let text = &a.answers[&key].stdout;
            eprintln!(
                "{key}: {} lines, {} bytes",
                text.lines().count(),
                text.len()
            );
            failures.extend(hygiene_failures(&key, text));
            failures.extend(size_failures(&key, text, Some(ratchet(label, call))));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Output validation V3 (§2.1, §12.1): every CDO workspace routine's
/// `selector` and display, and every `selector` in the CDO rendering
/// goldens, resolve to their own stable id (`cli_selector_round_trip`).
#[test]
fn cdo_selectors_round_trip() {
    let Some(ws) = crate::cdo::cdo_ws_or_enforce() else {
        return;
    };
    crate::cli_selector_round_trip::cdo_round_trip(&ws);
}

/// The inline cap's margin (MCP output plan item T): IssueReminder's and
/// RunReportID's default explain card must arrive inline, under the 40 KiB
/// cap with at least 1 KiB to spare, and under the client's character
/// budget once escaped. RunReportID's is the largest (37,671 bytes on
/// 2026-11-05, a 3,289-byte margin; IssueReminder 32,433, 8,527). A card
/// that grows into the margin fails here: shorten the card, do not raise
/// the cap without the owner.
///
/// The cap is TEMPORARILY 48 KiB (owner decision 2026-11-06): output
/// validation V3's one routine display grew RunReportID's card to 45,058
/// bytes (IssueReminder 39,169). It goes back to 40 KiB in V5 (spec
/// 2026-11-06 §7.5/§7.7, path dedup).
#[test]
fn cdo_explain_cards_stay_inline_with_a_margin() {
    let Some(a) = cdo_answers() else { return };
    const MARGIN: usize = 1024;
    let cap = al_sem::mcp::tools::MCP_DEFAULT_INLINE_BYTES as usize;
    let mut failures = Vec::new();
    for label in ["IssueReminder", "RunReportID"] {
        let ans = &a.answers[&format!("{label} explain human")];
        let bytes = ans.stdout.len();
        eprintln!(
            "{label}: explain card {bytes} bytes, margin {}",
            cap - bytes
        );
        if !ans.envelope["stdoutText"].is_string() {
            failures.push(format!("{label}: the card went to a file ({bytes} bytes)"));
        }
        if bytes + MARGIN > cap {
            failures.push(format!(
                "{label}: {bytes} bytes leaves less than {MARGIN} of the {cap}-byte cap"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Item T row 8 on CDO (M2): every live effect type each routine's digest
/// reports is on its line of explain's card. The fixture test
/// `output_invariants_w3::every_effect_kind_digest_reports_is_in_the_card`
/// carries the discrimination proof.
#[test]
fn cdo_every_effect_kind_is_in_the_card() {
    let Some(a) = cdo_answers() else { return };
    let mut failures = Vec::new();
    for (label, ..) in ROUTINES {
        let d: Value =
            serde_json::from_str(&a.answers[&format!("{label} digest json")].stdout).unwrap();
        let effects = d["payload"]["entries"][0]["effects"].as_array().unwrap();
        let card = &a.answers[&format!("{label} explain human")].stdout;
        failures.extend(kind_failures(label, effects, card));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// T-W2-17 and T-W2-18 on CDO: every prove evidence row names its place or
/// says `unavailable`; every digest crossing prints its target's qualified
/// display or names every target, and every row's crossing is listed. The
/// fixture tests carry the proofs. Red before item T's fix (2026-11-05, cdo
/// profile): five crossings per root entered an overload
/// (`CTS-CDN Network Doc. Mgt.::SendNetworkDocument` twice, `Email
/// Message::AddAttachment`, `CTS-SYS Telemetry::LogMessage`, `Regex::Match`)
/// and printed the bare name while the routine table printed the
/// parameters, so two entry points read alike; the crossing's name now reads
/// its targets' qualified displays (`RoutineRollup::build`).
#[test]
fn cdo_evidence_and_crossings_name_their_places_and_targets() {
    let Some(a) = cdo_answers() else { return };
    let mut failures = Vec::new();
    let mut crossings = 0;
    for (key, ans) in &a.answers {
        if key.contains("prove") && key.ends_with(" json") {
            failures.extend(evidence_failures(
                key,
                &serde_json::from_str(&ans.stdout).unwrap(),
            ));
        }
        if key.ends_with("digest json") {
            let d: Value = serde_json::from_str(&ans.stdout).unwrap();
            crossings += d["payload"]["entries"][0]["crossings"]
                .as_object()
                .map_or(0, |m| m.len());
            failures.extend(crossing_failures(key, &d));
        }
    }
    assert!(
        crossings >= 20,
        "precondition: CDO's crossings ({crossings})"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// T-W2-3 on CDO: every folded list (`… +N <unit>`) in a human answer also
/// states its full count with the same unit on that line (`(M <unit>)` or
/// `M <unit>` with M > N). Before wave 3 three lines did not: the fields an
/// owner assigns, events by publisher, top dependency callees.
///
/// Discrimination (2026-11-05, cdo profile): `Folded::count_note` returning
/// "" fails at "IssueReminder digest human: … +2 publishers with no full
/// count" (and the callees lines); restored, passes. Before the fix this
/// test also failed on the fields lines (`… +4 fields`), which now state
/// `(not operation-specific; 10 fields)`.
#[test]
fn cdo_folded_lines_state_their_full_count() {
    let Some(a) = cdo_answers() else { return };
    let mut failures = Vec::new();
    let mut folds = 0;
    for (key, ans) in a.answers.iter().filter(|(k, _)| k.ends_with("human")) {
        for line in ans.stdout.lines() {
            let mut rest = line;
            while let Some(i) = rest.find("… +") {
                let after = &rest[i + "… +".len()..];
                let n: String = after.chars().take_while(char::is_ascii_digit).collect();
                let unit: String = after[n.len()..]
                    .trim_start()
                    .chars()
                    .take_while(|c| c.is_alphabetic())
                    .collect();
                let n: usize = n.parse().unwrap_or(0);
                folds += 1;
                let total = line.match_indices(&format!(" {unit}")).any(|(j, _)| {
                    let digits: String = line[..j]
                        .chars()
                        .rev()
                        .take_while(|c| c.is_ascii_digit() || *c == ',')
                        .collect::<String>()
                        .chars()
                        .rev()
                        .filter(char::is_ascii_digit)
                        .collect();
                    digits.parse::<usize>().is_ok_and(|m| m > n)
                });
                if !total {
                    failures.push(format!(
                        "{key}: … +{n} {unit} with no full count: {}",
                        line.trim()
                    ));
                }
                rest = after;
            }
        }
    }
    assert!(folds >= 10, "precondition: CDO's folded lines ({folds})");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// T-W2-14 on CDO: RunReportID is reached from hundreds of page actions;
/// its card gives them at most 3 lines (the count on the `callers:` line,
/// `by caption`, `most actions`), and the grouped counts add up to the
/// actions the JSON lists. The fixture test
/// `output_invariants_w3::page_actions_fold_to_three_lines` carries the proof.
#[test]
fn cdo_page_actions_take_three_lines() {
    let Some(a) = cdo_answers() else { return };
    let card = &a.answers["RunReportID explain human"].stdout;
    let lines: Vec<&str> = card
        .lines()
        .map(str::trim)
        .filter(|l| {
            l.starts_with("reached from page action")
                || l.starts_with("by caption")
                || l.starts_with("most actions")
        })
        .collect();
    assert!(lines.len() < 3, "{lines:?}");
    let j: Value = serde_json::from_str(&a.answers["RunReportID explain json"].stdout).unwrap();
    let s = &j["cards"][0]["reachedFromSummary"];
    let actions = s["actions"].as_u64().unwrap();
    assert!(actions > 100, "precondition: a hub ({actions})");
    let sum = |k: &str| -> u64 {
        s[k].as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| x["count"].as_u64())
            .sum()
    };
    assert_eq!(
        (sum("byCaption"), sum("byObject")),
        (actions, actions),
        "{s}"
    );
    assert_eq!(
        j["cards"][0]["reachedFrom"].as_array().map_or(0, Vec::len) as u64,
        actions
    );
    assert!(
        card.contains(&format!(
            "reached from {} page actions",
            al_sem::engine::l5::routine_rollup::thousands(actions as usize)
        )),
        "the count on the callers line"
    );
}

/// Item T, the CDO rendering goldens (plan item T; spec §8.2): for each
/// routine, the explain card, query effects, query touches, prove may-commit
/// and writes-table (human) and the digest's summary are byte-compared with
/// `tests/cli-output-goldens/cdo/<routine>.<call>`, pinned to the CDO input
/// stamps (`STAMP.json`). `REGEN_TEMP_GOLDENS=1` (with `CDO_WS`) rewrites
/// them: the next blank name, duplicate line or lost section shows up as a
/// golden diff someone must read.
#[test]
fn cdo_rendering_goldens() {
    let Some(a) = cdo_answers() else { return };
    if !stamp_matches(&a.ws) {
        return;
    }
    let regen = crate::regen::regen_mode();
    let mut failures = Vec::new();
    for (label, ..) in ROUTINES {
        let mut files: Vec<(String, String)> = RENDERED
            .iter()
            .filter(|(call, _)| *call != "digest human")
            .map(|(call, suffix)| {
                (
                    format!("{label}.{suffix}"),
                    a.answers[&format!("{label} {call}")].stdout.clone(),
                )
            })
            .collect();
        let mut summary = a.answers[&format!("{label} digest json")].envelope["summary"].clone();
        // The result index (V4 §5.2) lives in the server's output folder,
        // a new one per run, and names the answer file there: the golden
        // keeps its row count.
        if summary["index"]["path"].is_string() {
            summary["index"]["path"] = json!("<the server's output folder>");
            summary["index"]["sha256"] = json!("<per run>");
        }
        files.push((
            format!("{label}.digest-summary.json"),
            serde_json::to_string_pretty(&summary).unwrap() + "\n",
        ));
        for (name, text) in files {
            let path = goldens_dir().join(&name);
            if regen {
                crate::regen::write_golden(&path, &text).unwrap();
                continue;
            }
            match std::fs::read_to_string(&path) {
                Ok(g) if g == text => {}
                Ok(_) => failures.push(format!(
                    "{name}: differs (REGEN_TEMP_GOLDENS=1 with CDO_WS rewrites it; read the diff)"
                )),
                Err(e) => failures.push(format!("{name}: {e}")),
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Final CDO review, item 2 on CDO: every table record under `writes in
/// workspace code` in each routine's explain card and digest (human) has a
/// `path:` line or `no path kept: <reason>`. And IssueReminder's `CDO Queue
/// Entry` Insert (`CDODocumentTemplateLine.Table.al:1713`), whose three
/// listed paths all pass `RunReportID → QueueMail`, which IssueReminder's
/// literal arguments rule out, carries the searched witness: a real path
/// that re-enters workspace code beyond `Codeunit Reminder-Issue::OnRun`.
///
/// Discrimination (2026-11-05, cdo profile): restoring the old record
/// lines in `routine_rollup::workspace_write_lines` (the anchor, then
/// `path_lines` only) fails at "IssueReminder explain: no path and no
/// reason under CDO Queue Entry: Insert 1 — known persistent" (and its
/// digest twin); restored, passes.
#[test]
fn cdo_write_lines_have_a_path_or_a_reason() {
    let Some(a) = cdo_answers() else { return };
    let mut failures = Vec::new();
    let mut checked = 0;
    for (label, ..) in ROUTINES {
        for call in ["explain human", "digest human"] {
            let key = format!("{label} {call}");
            let (f, n) =
                crate::output_invariants_w3::write_path_failures(&key, &a.answers[&key].stdout);
            failures.extend(f);
            checked += n;
        }
    }
    assert!(
        checked >= 40,
        "precondition: write records checked ({checked})"
    );
    let card = &a.answers["IssueReminder explain human"].stdout;
    let lines: Vec<&str> = card.lines().collect();
    let at = lines
        .iter()
        .position(|l| l.trim_start().starts_with("CDO Queue Entry: Insert"))
        .expect("precondition: IssueReminder writes CDO Queue Entry in workspace code");
    let block = lines[at + 1..]
        .iter()
        .take_while(|l| l.starts_with("      "))
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    if !block
        .contains("path (re-enters workspace code beyond Codeunit \"Reminder-Issue\" (393)::OnRun")
        || !block.contains("→ QueueMailCombined")
    {
        failures.push(format!(
            "IssueReminder's CDO Queue Entry has no searched witness:\n{block}"
        ));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Final CDO review, item 3 on CDO: for each routine, the scope line's
/// entry points are the `writes beyond` section's plus the temporary-only
/// ones, its sites the union of both sets' sites (read off the `--detail`
/// card), in explain and digest (human); query effects states the same
/// section counts. Measured 2026-11-05 on `bc3ccb18`: IssueReminder 27
/// entry points at 31 sites (12 at 14 with may-persist writes, 15 at 17
/// temporary-only); before, the scope line said "14 entry points at 14
/// sites" (the writing crossings, their sites summed).
///
/// Discrimination (2026-11-05, cdo profile): restoring the old totals
/// (`entry_points` = the writing crossings, `crossing_sites` their summed
/// sites) fails at "IssueReminder: scope says 14 entry points, the sections
/// 12 + 15"; restored, passes.
#[test]
fn cdo_entry_point_counts_agree() {
    let Some(a) = cdo_answers() else { return };
    let mut failures = Vec::new();
    for (label, ..) in ROUTINES {
        let get = |call: &str| a.answers[&format!("{label} {call}")].stdout.as_str();
        let detail = get("explain detail");
        failures.extend(crate::output_invariants_w3::entry_point_failures(
            label,
            get("explain human"),
            detail,
            Some(get("query effects human")),
        ));
        failures.extend(crate::output_invariants_w3::entry_point_failures(
            &format!("{label} digest"),
            get("digest human"),
            detail,
            None,
        ));
    }
    let ir = crate::output_invariants_w3::entry_point_lines(
        &a.answers["IssueReminder explain human"].stdout,
    );
    assert!(
        ir.other.is_some_and(|o| o.0 > 0),
        "precondition: IssueReminder has temporary-only entry points: {ir:?}"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Agent-tasks C-9: `alsem tests` on the pinned baseline
/// (`cli_tests_selection::cdo_selection_ratchets`), here so `cdo-gate` runs
/// it.
#[test]
fn cdo_test_selection_ratchets() {
    crate::cli_tests_selection::cdo_selection_ratchets();
}

/// Agent-tasks B1's CDO acceptance (`cli_impact::cdo_impact_ratchets`),
/// here so `cdo-gate` runs it.
#[test]
fn cdo_impact_ratchets() {
    crate::cli_impact::cdo_impact_ratchets();
}

/// Agent-tasks B2's CDO acceptance (`cli_impact_base::cdo_impact_base_ratchets`).
#[test]
fn cdo_impact_base_ratchets() {
    crate::cli_impact_base::cdo_impact_base_ratchets();
}
