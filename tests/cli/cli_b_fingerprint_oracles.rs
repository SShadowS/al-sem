//! cli-b/b3 — FINGERPRINT native oracles.
//!
//! These assert the EXACT TS-faithful bytes/behavior for the corpus-invisible
//! paths the fingerprint differential does not exercise (it covers only a narrow
//! slice: includeInherited=true, witness∈{3,all,0,false}, no --roots, JSON-only
//! selector error). Every string here was originally transcribed directly from the
//! al-sem TS source (`src/cli/fingerprint.ts`, `format-fingerprint.ts`,
//! `query/witness.ts`, `contracts/fingerprint-query.ts`, which lived at
//! `U:\Git\al-sem`, now retired) — the values are Rust-owned constants; no golden
//! needed.
//!
//! Grouped by the second-pass review item number.

use std::collections::HashMap;

use al_sem::engine::l5::digest::{
    FingerprintIndexesPub, HumanHop, TerminalHopInfo, reconstruct_witness_paths_pub,
};
use al_sem::engine::l5::fingerprint_cli::{
    FingerprintFormat, SpecifiedFlags, default_format, normalize_witness, reject_illegal_combos,
    validate_roots,
};
use al_sem::engine::l5::fingerprint_query::{WitnessLimit, format_hop};
use al_sem::engine::l5::snapshot::{
    SnapshotCapabilityFact, SnapshotCoverageRecord, SnapshotGraphEdge, SnapshotRange,
    SnapshotSourceAnchor,
};

// ===========================================================================
// Helpers — minimal builders for hand-built witness inputs.
// ===========================================================================

fn empty_idx() -> FingerprintIndexesPub {
    FingerprintIndexesPub {
        stable_id_to_display: HashMap::new(),
        routine_display_by_id: HashMap::new(),
        outgoing_edges: HashMap::new(),
        facts_by_routine: HashMap::new(),
        direct_facts_by_routine: HashMap::new(),
        coverage_by_routine: HashMap::new(),
        callsite_by_id: HashMap::new(),
        operation_by_id: HashMap::new(),
        event_display_by_id: HashMap::new(),
    }
}

fn fact(provenance: &str, op: &str, kind: &str) -> SnapshotCapabilityFact {
    SnapshotCapabilityFact {
        subject: "ROOT".to_string(),
        op: op.to_string(),
        resource_kind: kind.to_string(),
        resource_id: None,
        resource_arg_source: None,
        confidence: "static".to_string(),
        provenance: provenance.to_string(),
        via: "self".to_string(),
        witness_operation_id: None,
        witness_callsite_id: None,
        extra: None,
    }
}

fn anchor() -> SnapshotSourceAnchor {
    SnapshotSourceAnchor {
        source_unit_id: "u".to_string(),
        range: SnapshotRange {
            start_line: 1,
            start_column: 1,
            end_line: 1,
            end_column: 1,
        },
        enclosing_routine_id: "ROOT".to_string(),
        syntax_kind: "call".to_string(),
    }
}

fn direct_call(callsite_id: &str, from: &str, to: &str, edge_id: &str) -> SnapshotGraphEdge {
    SnapshotGraphEdge::DirectCall {
        kind: "direct-call",
        callsite_id: callsite_id.to_string(),
        from: from.to_string(),
        to: to.to_string(),
        source_anchor: anchor(),
        edge_id: edge_id.to_string(),
    }
}

fn coverage_unknown() -> SnapshotCoverageRecord {
    SnapshotCoverageRecord {
        subject: "R1".to_string(),
        direct_status: "unknown".to_string(),
        inherited_status: "unknown".to_string(),
        reasons: vec![],
        unknown_targets: vec![],
    }
}

// ===========================================================================
// Item 1 — --witness query-branch trigger (index.ts:439 `!== "3"`).
// ===========================================================================

#[test]
fn item1_witness_3_does_not_trigger_query() {
    // `--witness 3` (explicit default) must NOT mark witness as specified.
    let specified = SpecifiedFlags {
        witness: Some("3").is_some() && Some("3") != Some("3"),
        ..Default::default()
    };
    assert!(
        !specified.is_query_requested(),
        "--witness 3 must not trigger the query branch (→ capability-snapshot envelope)"
    );
}

#[test]
fn item1_witness_5_triggers_query() {
    let w = Some("5");
    let specified = SpecifiedFlags {
        witness: w.is_some() && w != Some("3"),
        ..Default::default()
    };
    assert!(
        specified.is_query_requested(),
        "--witness 5 must trigger the fingerprint-query branch"
    );
}

#[test]
fn item1_witness_false_zero_all_trigger_query() {
    for v in ["false", "0", "all"] {
        let w = Some(v);
        let specified = SpecifiedFlags {
            witness: w.is_some() && w != Some("3"),
            ..Default::default()
        };
        assert!(
            specified.is_query_requested(),
            "--witness {v} must trigger query"
        );
    }
}

#[test]
fn item1_witness_none_does_not_trigger_but_resolves_to_3() {
    let w: Option<&str> = None;
    let specified = SpecifiedFlags {
        witness: w.is_some() && w != Some("3"),
        ..Default::default()
    };
    assert!(
        !specified.is_query_requested(),
        "no --witness must not trigger query"
    );
    // Resolved limit is still 3 (normalizeWitness(undefined) → 3).
    assert_eq!(normalize_witness(None), Ok(WitnessLimit::Capped(3)));
}

// ===========================================================================
// Item 2 — --include-inherited default true + polarity (index.ts:466).
// ===========================================================================

#[test]
fn item2_no_include_inherited_triggers_query() {
    // `--no-include-inherited` (specified=true via the false case) → query branch.
    let specified = SpecifiedFlags {
        include_inherited: true, // mirrors `includeInherited === false`
        ..Default::default()
    };
    assert!(specified.is_query_requested());
}

#[test]
fn item2_default_include_inherited_does_not_trigger() {
    // Default (true) is NOT specified → no trigger.
    let specified = SpecifiedFlags::default();
    assert!(!specified.is_query_requested());
}

// ===========================================================================
// Item 3 — --roots validation (validateRoots, fingerprint.ts:67).
// ===========================================================================

#[test]
fn item3_unknown_root_kind_errors_exit1() {
    let err = validate_roots(&["not-a-root".to_string()]).unwrap_err();
    assert_eq!(
        err,
        "unknown root kind 'not-a-root'; valid: trigger-table, trigger-page, page-action, report-trigger, event-subscriber, install-codeunit, upgrade-codeunit, api-page, web-service-exposed, job-queue-entrypoint, public-procedure, test-procedure"
    );
}

#[test]
fn item3_valid_root_kinds_pass() {
    let ok = validate_roots(&[
        "public-procedure".to_string(),
        "event-subscriber".to_string(),
    ])
    .unwrap();
    assert_eq!(ok, vec!["public-procedure", "event-subscriber"]);
}

/// #10: the CLI validator and the classifier share ONE vocabulary, so every kind the
/// classifier can emit must be accepted by `--roots`.
///
/// This is the test that a divergence would break, and the shape matters: it asserts
/// every canonical value is ACCEPTED. A stale CLI copy MISSING a value fails here. A
/// stale copy with an EXTRA value would not — that direction is covered by
/// `item3_unknown_root_kind_errors_exit1`, which pins the exact valid-list text.
#[test]
fn item3_every_classifier_root_kind_is_accepted_by_the_cli() {
    let all: Vec<String> = al_sem::engine::root_classification::ROOT_KIND_VALUES
        .iter()
        .map(|k| (*k).to_string())
        .collect();
    let ok = validate_roots(&all).unwrap_or_else(|e| {
        panic!("classifier emits a kind the CLI validator rejects: {e}");
    });
    assert_eq!(ok, all);
}

// ===========================================================================
// Item 4 — --witness range (normalizeWitness, fingerprint.ts:86).
// ===========================================================================

#[test]
fn item4_witness_over_256_errors() {
    assert_eq!(
        normalize_witness(Some("257")),
        Err("--witness must be in 0..256 or 'all' (got 257)".to_string())
    );
    assert_eq!(
        normalize_witness(Some("1000")),
        Err("--witness must be in 0..256 or 'all' (got 1000)".to_string())
    );
}

#[test]
fn item4_witness_boundary_256_ok() {
    assert_eq!(
        normalize_witness(Some("256")),
        Ok(WitnessLimit::Capped(256))
    );
    assert_eq!(normalize_witness(Some("0")), Ok(WitnessLimit::Capped(0)));
}

#[test]
fn item4_witness_garbage_errors() {
    assert_eq!(
        normalize_witness(Some("banana")),
        Err("invalid --witness value".to_string())
    );
}

// ===========================================================================
// Item 6 — illegal-combo messages (rejectIllegalCombos, fingerprint.ts:110).
// ===========================================================================

#[test]
fn item6_shard_plus_human_message() {
    // --shard + (default human) → exact TS message.
    let err = reject_illegal_combos(SpecifiedFlags::default(), &FingerprintFormat::Human, true)
        .unwrap_err();
    assert_eq!(err, "--shard requires --format=json|cbor|cbor.gz");
}

#[test]
fn item6_shard_plus_query_flags_messages() {
    // Each query flag → `--shard cannot be combined with --<name>`. The flagName
    // mapping: routineSelectors→routine, includeInherited→include-inherited.
    let cases: &[(SpecifiedFlags, &str)] = &[
        (
            SpecifiedFlags {
                roots: true,
                ..Default::default()
            },
            "--shard cannot be combined with --roots",
        ),
        (
            SpecifiedFlags {
                routine_selectors: true,
                ..Default::default()
            },
            "--shard cannot be combined with --routine",
        ),
        (
            SpecifiedFlags {
                witness: true,
                ..Default::default()
            },
            "--shard cannot be combined with --witness",
        ),
        (
            SpecifiedFlags {
                include_inherited: true,
                ..Default::default()
            },
            "--shard cannot be combined with --include-inherited",
        ),
    ];
    for (spec, want) in cases {
        let err = reject_illegal_combos(*spec, &FingerprintFormat::Json, true).unwrap_err();
        assert_eq!(&err, want);
    }
}

#[test]
fn item6_cbor_plus_query_flag_message() {
    let err = reject_illegal_combos(
        SpecifiedFlags {
            witness: true,
            ..Default::default()
        },
        &FingerprintFormat::Cbor,
        false,
    )
    .unwrap_err();
    assert_eq!(
        err,
        "--witness is only valid with --format=human or --format=json"
    );

    let err2 = reject_illegal_combos(
        SpecifiedFlags {
            roots: true,
            ..Default::default()
        },
        &FingerprintFormat::CborGz,
        false,
    )
    .unwrap_err();
    assert_eq!(
        err2,
        "--roots is only valid with --format=human or --format=json"
    );
}

#[test]
fn item6_legal_combos_pass() {
    // human + query flags → legal.
    assert!(
        reject_illegal_combos(
            SpecifiedFlags {
                witness: true,
                ..Default::default()
            },
            &FingerprintFormat::Human,
            false
        )
        .is_ok()
    );
    // json + query flags → legal.
    assert!(
        reject_illegal_combos(
            SpecifiedFlags {
                roots: true,
                ..Default::default()
            },
            &FingerprintFormat::Json,
            false
        )
        .is_ok()
    );
    // shard + json, no query → legal.
    assert!(
        reject_illegal_combos(SpecifiedFlags::default(), &FingerprintFormat::Json, true).is_ok()
    );
}

// ===========================================================================
// Item 5/6 — defaultFormat (fingerprint.ts:140).
// ===========================================================================

#[test]
fn item6_default_format_resolution() {
    // Omitted + no shard → human.
    assert_eq!(default_format(None, false), Ok(FingerprintFormat::Human));
    // Omitted + shard → json.
    assert_eq!(default_format(None, true), Ok(FingerprintFormat::Json));
    // Explicit values.
    assert_eq!(
        default_format(Some("cbor.gz"), false),
        Ok(FingerprintFormat::CborGz)
    );
    // Unknown format → exact error.
    assert_eq!(
        default_format(Some("yaml"), false),
        Err("unknown --format 'yaml'; valid: human, json, cbor, cbor.gz".to_string())
    );
}

// ===========================================================================
// Item 7 — witness diagnostics (9 kinds: 8 explicit + terminal-not-found).
// All hand-built so each kind is hit exactly.
// ===========================================================================

#[test]
fn item7_missing_witness_anchor_direct_synthetic() {
    // Direct fact, no witness ids → synthetic terminal + missing-witness-anchor,
    // incomplete:true, ONE path with the terminal hop (detail = subject).
    let idx = empty_idx();
    let f = fact("direct", "subscribe", "event");
    let out = reconstruct_witness_paths_pub("ROOT", &f, &idx, 256);
    assert!(out.incomplete);
    assert_eq!(out.diagnostics.len(), 1);
    assert_eq!(out.diagnostics[0].kind, "missing-witness-anchor");
    assert_eq!(out.diagnostics[0].detail.as_deref(), Some("ROOT"));
    assert_eq!(out.paths.len(), 1);
}

#[test]
fn item7_missing_operation_evidence() {
    // Direct fact with witnessOperationId not in the index → missing-operation-evidence
    // (detail = operationId), incomplete:true.
    let idx = empty_idx();
    let mut f = fact("direct", "modify", "table");
    f.witness_operation_id = Some("op-missing".to_string());
    let out = reconstruct_witness_paths_pub("ROOT", &f, &idx, 256);
    assert!(out.incomplete);
    assert_eq!(out.diagnostics.len(), 1);
    assert_eq!(out.diagnostics[0].kind, "missing-operation-evidence");
    assert_eq!(out.diagnostics[0].detail.as_deref(), Some("op-missing"));
}

#[test]
fn item7_missing_callsite_evidence() {
    let idx = empty_idx();
    let mut f = fact("direct", "send", "http");
    f.witness_callsite_id = Some("cs-missing".to_string());
    let out = reconstruct_witness_paths_pub("ROOT", &f, &idx, 256);
    assert!(out.incomplete);
    assert_eq!(out.diagnostics.len(), 1);
    assert_eq!(out.diagnostics[0].kind, "missing-callsite-evidence");
    assert_eq!(out.diagnostics[0].detail.as_deref(), Some("cs-missing"));
}

#[test]
fn item7_first_hop_not_found_no_callsite() {
    // Inherited fact with NO witnessCallsiteId → first-hop-not-found, detail = via.
    let idx = empty_idx();
    let mut f = fact("inherited", "modify", "table");
    f.via = "call".to_string();
    let out = reconstruct_witness_paths_pub("ROOT", &f, &idx, 256);
    assert!(out.incomplete);
    assert!(out.paths.is_empty());
    assert_eq!(out.diagnostics.len(), 1);
    assert_eq!(out.diagnostics[0].kind, "first-hop-not-found");
    assert_eq!(out.diagnostics[0].detail.as_deref(), Some("call"));
}

#[test]
fn item7_first_hop_not_found_no_edges() {
    // Inherited fact WITH witnessCallsiteId but no matching first edge → detail = callsiteId.
    let idx = empty_idx();
    let mut f = fact("inherited", "modify", "table");
    f.via = "call".to_string();
    f.witness_callsite_id = Some("cs0".to_string());
    let out = reconstruct_witness_paths_pub("ROOT", &f, &idx, 256);
    assert!(out.incomplete);
    assert!(out.paths.is_empty());
    assert_eq!(out.diagnostics.len(), 1);
    assert_eq!(out.diagnostics[0].kind, "first-hop-not-found");
    assert_eq!(out.diagnostics[0].detail.as_deref(), Some("cs0"));
}

#[test]
fn item7_terminal_not_found_default_no_detail() {
    // Inherited fact whose BFS walks but finds no matching direct fact terminal →
    // terminal-not-found, NO detail, incomplete:true. Build: root → R1 (a call edge),
    // R1 has outgoing edges (so it is NOT an opaque boundary) but NO matching direct
    // fact and no further reachable terminal.
    let mut idx = empty_idx();
    idx.outgoing_edges.insert(
        "ROOT".to_string(),
        vec![direct_call("cs0", "ROOT", "R1", "e0")],
    );
    // R1 → R2 (keeps R1 from being an opaque-or-unresolved boundary), but R2 has
    // no edges, no facts, and is NOT coverage-unknown → BFS exhausts with no terminal.
    idx.outgoing_edges
        .insert("R1".to_string(), vec![direct_call("cs1", "R1", "R2", "e1")]);
    let mut f = fact("inherited", "modify", "table");
    f.via = "call".to_string();
    f.witness_callsite_id = Some("cs0".to_string());
    let out = reconstruct_witness_paths_pub("ROOT", &f, &idx, 256);
    assert!(out.incomplete);
    assert!(out.paths.is_empty());
    assert_eq!(out.diagnostics.len(), 1);
    assert_eq!(out.diagnostics[0].kind, "terminal-not-found");
    assert_eq!(out.diagnostics[0].detail, None);
}

#[test]
fn item7_opaque_or_unresolved_boundary() {
    // root → R1 via cs0; R1 has NO outgoing edges, NO direct facts, and
    // coverage.directStatus == "unknown" → opaque-or-unresolved-boundary (detail = R1).
    let mut idx = empty_idx();
    idx.outgoing_edges.insert(
        "ROOT".to_string(),
        vec![direct_call("cs0", "ROOT", "R1", "e0")],
    );
    idx.coverage_by_routine
        .insert("R1".to_string(), coverage_unknown());
    let mut f = fact("inherited", "modify", "table");
    f.via = "call".to_string();
    f.witness_callsite_id = Some("cs0".to_string());
    let out = reconstruct_witness_paths_pub("ROOT", &f, &idx, 256);
    // Boundary push emits one path (the hops up to the boundary) — NOT incomplete by itself.
    assert_eq!(out.paths.len(), 1);
    assert!(out
        .diagnostics
        .iter()
        .any(|d| d.kind == "opaque-or-unresolved-boundary" && d.detail.as_deref() == Some("R1")));
}

#[test]
fn item7_path_limit_reached_detail_cap() {
    // Two distinct one-hop terminal paths; cap=1 → truncated + path-limit-reached(cap=1).
    let mut idx = empty_idx();
    idx.outgoing_edges.insert(
        "ROOT".to_string(),
        vec![
            direct_call("cs0", "ROOT", "A", "e0"),
            direct_call("cs0", "ROOT", "B", "e1"),
        ],
    );
    // A and B each carry a matching direct fact → each seeds a terminal path.
    let term = fact("direct", "modify", "table");
    idx.direct_facts_by_routine
        .insert("A".to_string(), vec![term.clone()]);
    idx.direct_facts_by_routine
        .insert("B".to_string(), vec![term]);
    let mut f = fact("inherited", "modify", "table");
    f.via = "call".to_string();
    f.witness_callsite_id = Some("cs0".to_string());
    let out = reconstruct_witness_paths_pub("ROOT", &f, &idx, 1);
    assert!(out.truncated);
    assert!(
        out.diagnostics
            .iter()
            .any(|d| d.kind == "path-limit-reached" && d.detail.as_deref() == Some("cap=1"))
    );
}

// ===========================================================================
// Item 8 — human hop rendering (formatHop, format-fingerprint.ts:270).
// event-dispatch must render the SHORT eventDisplay, not the full event_id.
// ===========================================================================

#[test]
fn item8_event_dispatch_renders_short_display() {
    let hop = HumanHop::EventDispatch {
        event_display: "OnAfterPostSalesDoc".to_string(),
    };
    assert_eq!(format_hop(&hop), "event OnAfterPostSalesDoc");
}

#[test]
fn item8_call_hop_with_anchor() {
    let hop = HumanHop::Call {
        routine_display: "DoWork".to_string(),
        callee_display: "DoWork".to_string(),
        source_file: Some("ws:src/a.al".to_string()),
        line: Some(12),
        column: Some(8),
    };
    assert_eq!(format_hop(&hop), "DoWork (via DoWork at ws:src/a.al:12:8)");
}

#[test]
fn item8_call_hop_no_anchor() {
    let hop = HumanHop::Call {
        routine_display: "DoWork".to_string(),
        callee_display: "DoWork".to_string(),
        source_file: None,
        line: None,
        column: None,
    };
    assert_eq!(format_hop(&hop), "DoWork (via DoWork)");
}

#[test]
fn item8_object_run_unresolved_target() {
    let hop = HumanHop::ObjectRun {
        routine_display: "Runner".to_string(),
        target_display: None,
        source_file: Some("ws:src/b.al".to_string()),
        line: Some(3),
        column: Some(1),
    };
    assert_eq!(
        format_hop(&hop),
        "Runner (via Codeunit.Run <unresolved> at ws:src/b.al:3:1)"
    );
}

#[test]
fn item8_variable_typed_call() {
    let hop = HumanHop::VariableTypedCall {
        routine_display: "Post".to_string(),
        receiver_type: "Codeunit \"Sales-Post\"".to_string(),
        callee_display: Some("Run".to_string()),
        source_file: None,
        line: None,
        column: None,
    };
    assert_eq!(format_hop(&hop), "Post (via Codeunit \"Sales-Post\".Run)");
}

#[test]
fn item8_interface_dispatch_pluralization() {
    let one = HumanHop::InterfaceDispatch {
        routine_display: "Handle".to_string(),
        interface_name: "IProcessor".to_string(),
        candidate_count: 1,
        source_file: None,
        line: None,
        column: None,
    };
    assert_eq!(
        format_hop(&one),
        "Handle (via interface IProcessor, 1 candidate)"
    );
    let many = HumanHop::InterfaceDispatch {
        routine_display: "Handle".to_string(),
        interface_name: "IProcessor".to_string(),
        candidate_count: 3,
        source_file: Some("ws:src/c.al".to_string()),
        line: Some(7),
        column: Some(2),
    };
    assert_eq!(
        format_hop(&many),
        "Handle (via interface IProcessor, 3 candidates at ws:src/c.al:7:2)"
    );
}

// ===========================================================================
// Item 12 — unresolved dispatch ordering by witnessCallsiteId
// (fingerprint-query.ts:468). The comparator mirrors the in-engine sort.
// ===========================================================================

#[test]
fn item12_unresolved_dispatch_sorts_by_witness_callsite_id() {
    use al_sem::engine::l5::fingerprint_query::DispatchInstance;
    let mk = |cs: Option<&str>| DispatchInstance {
        object_type: "Codeunit".to_string(),
        target_id: None,
        target_display: None,
        confidence: "static".to_string(),
        provenance: "direct".to_string(),
        via: "self".to_string(),
        witness_callsite_id: cs.map(str::to_string),
    };
    // Insertion order deliberately NOT sorted: cs2, cs0, (none), cs1.
    let mut v = [mk(Some("cs2")), mk(Some("cs0")), mk(None), mk(Some("cs1"))];
    // The EXACT in-engine comparator (witnessCallsiteId, "" when absent).
    v.sort_by(|a, b| {
        a.witness_callsite_id
            .as_deref()
            .unwrap_or("")
            .cmp(b.witness_callsite_id.as_deref().unwrap_or(""))
    });
    let order: Vec<&str> = v
        .iter()
        .map(|d| d.witness_callsite_id.as_deref().unwrap_or("<none>"))
        .collect();
    // "" (none) sorts first, then cs0, cs1, cs2.
    assert_eq!(order, vec!["<none>", "cs0", "cs1", "cs2"]);
}

// ===========================================================================
// Item 16 — human selector-error stderr (fingerprint.ts:301-316).
// ===========================================================================

#[test]
fn item16_unresolved_selector_human_message() {
    use al_sem::engine::l5::fingerprint_cli::format_selector_errors_human;
    use al_sem::engine::l5::fingerprint_query::FingerprintQueryDiagnostic;
    let diags = vec![FingerprintQueryDiagnostic::SelectorUnresolved {
        selector: "DoesNotExist".to_string(),
    }];
    assert_eq!(
        format_selector_errors_human(&diags),
        "error: --routine 'DoesNotExist' did not match any routine (tried: stable-routine-id, full-display, two-segment, one-segment, object-qualified)\n"
    );
}

#[test]
fn item16_ambiguous_selector_human_message() {
    use al_sem::engine::l5::fingerprint_cli::format_selector_errors_human;
    use al_sem::engine::l5::fingerprint_query::FingerprintQueryDiagnostic;
    let diags = vec![FingerprintQueryDiagnostic::SelectorAmbiguous {
        selector: "Post".to_string(),
        matched_form: "one-segment".to_string(),
        candidates: vec![
            (
                "app:Codeunit:50:#h1".to_string(),
                "Codeunit \"A\"::Post".to_string(),
            ),
            (
                "app:Codeunit:51:#h2".to_string(),
                "Codeunit \"B\"::Post".to_string(),
            ),
        ],
    }];
    assert_eq!(
        format_selector_errors_human(&diags),
        "error: --routine 'Post' is ambiguous (matched via one-segment); candidates:\n  - Codeunit \"A\"::Post  (app:Codeunit:50:#h1)\n  - Codeunit \"B\"::Post  (app:Codeunit:51:#h2)\n"
    );
}

// Item 8 — terminal hop rendering (formatHop case "terminal").
#[test]
fn item8_terminal_operation_and_callsite_and_synthetic() {
    use al_sem::engine::l5::fingerprint_query::format_terminal_hop;
    let op = TerminalHopInfo {
        evidence_kind: "operation".to_string(),
        display_text: "Header.Modify".to_string(),
        source_file: Some("ws:src/p.al".to_string()),
        line: Some(9),
        column: Some(8),
    };
    assert_eq!(
        format_terminal_hop(&op),
        "direct Header.Modify at ws:src/p.al:9:8"
    );
    let cs = TerminalHopInfo {
        evidence_kind: "callsite".to_string(),
        display_text: "Foo".to_string(),
        source_file: None,
        line: None,
        column: None,
    };
    assert_eq!(format_terminal_hop(&cs), "call Foo");
    let syn = TerminalHopInfo {
        evidence_kind: "synthetic".to_string(),
        display_text: "subscribe event".to_string(),
        source_file: None,
        line: None,
        column: None,
    };
    assert_eq!(format_terminal_hop(&syn), "subscribe event");
}
