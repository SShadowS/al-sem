//! Phase 0: span-based site matcher fixture matrix (no env needed).
//!
//! Exercises [`match_sites`] — the cascade-resistance spine of the
//! L3-INDEPENDENT site matcher.  All tests construct synthetic edges via
//! [`canonical_call_edge_for_test`] so no real workspace is required.
//!
//! 1B.3b Task 3: the four live dual-run "fresh vs L3" comparison gates that
//! used to live here (`run_harness`/`run_site_harness`/
//! `run_resolution_harness`/`run_member_resolution_harness`/
//! `run_implicit_trigger_harness`/`run_event_flow_gate`, and their
//! `DiffReport`/`ResolutionReport`/`MemberResolutionReport`/
//! `ImplicitTriggerResolutionReport`/`EventFlowGateReport` report types) were
//! DELETED — replaced by the frozen semantic/trigger/event goldens + the
//! ported fan-out applicability teeth (Tests 14-20 below) and the
//! L3-INDEPENDENT fixture tests (`event_fixture_two_stage_join`,
//! `implicit_trigger_fixture_resolves_exact_target_set`).

use al_sem::program::node::{AppRegistry, ObjKey, ObjectKind, ObjectNodeId, RoutineNodeId};
use al_sem::program::resolve::differential::{
    SiteMatch, canonical_call_edge_for_test, match_sites, project_fresh_event_rows,
    verify_event_subscriber_route,
};
use al_sem::snapshot::{AppId, ParsedFile, ParsedUnit, Provenance, TrustTier};

// Task T0.2: the CDO_WS/ENFORCE_CDO_WS gating helper is shared with
// `program_graph.rs` and `snapshot_robustness.rs` via `#[path]` inclusion —
// see `tests/common/cdo.rs` for why (separate test-binary crates can't
// `use` each other's `mod`s).
#[path = "common/cdo.rs"]
mod cdo;
use cdo::cdo_ws_or_enforce;

#[path = "common/regen.rs"]
mod regen;

/// Shared CDO substrate (2026-07-15 shared-substrate spec): ONE snapshot →
/// graph → parse → full-resolve build, consumed by every read-only CDO test.
/// The engine's own determinism test (two independent builds, below) licenses
/// the sharing; that test and the #[ignore]d dumps deliberately do NOT use
/// this. Panics if the one build fails (every consumer would have failed
/// identically). `None` == the usual CDO_WS skip.
struct CdoShared {
    ctx: al_sem::program::resolve::full::ProgramContext,
    report: al_sem::program::resolve::full::ProgramReport,
}

static CDO_SHARED: std::sync::OnceLock<Option<CdoShared>> = std::sync::OnceLock::new();

fn cdo_shared() -> Option<&'static CdoShared> {
    CDO_SHARED
        .get_or_init(|| {
            let ws = cdo_ws_or_enforce()?;
            let ctx = al_sem::program::resolve::full::build_context(&ws)
                .expect("shared CDO substrate: build_context must succeed on CDO_WS");
            let report = al_sem::program::resolve::full::resolve_full_program_with(&ctx);
            Some(CdoShared { ctx, report })
        })
        .as_ref()
}

// ---------------------------------------------------------------------------
// Test 1 (from brief): one missing L3 site must NOT cascade
// ---------------------------------------------------------------------------

/// Verifies the core cascade-resistance guarantee: when the L3 oracle is
/// missing exactly one site that the fresh side emits, that site becomes a
/// single `FreshOnly` and all other pairings are undisturbed.
#[test]
fn one_missing_site_does_not_cascade() {
    // Build 5 fresh sites at increasing spans; L3 has the same 5 minus the 2nd.
    let mk = |start: u32, fp: u64| canonical_call_edge_for_test("cu:c:run", start, fp);
    let fresh = vec![mk(10, 1), mk(20, 2), mk(30, 3), mk(40, 4), mk(50, 5)];
    let l3 = vec![mk(10, 1), mk(30, 3), mk(40, 4), mk(50, 5)];
    let matches = match_sites(&fresh, &l3);
    let paired = matches
        .iter()
        .filter(|m| matches!(m, SiteMatch::Paired(_, _)))
        .count();
    let fresh_only = matches
        .iter()
        .filter(|m| matches!(m, SiteMatch::FreshOnly(_)))
        .count();
    let l3_only = matches
        .iter()
        .filter(|m| matches!(m, SiteMatch::L3Only(_)))
        .count();
    let unaligned = matches
        .iter()
        .filter(|m| matches!(m, SiteMatch::Unaligned(_, _)))
        .count();
    // 4 clean pairs; the 2nd fresh site is the single FreshOnly; NO cascade on 3/4/5.
    assert_eq!(paired, 4, "matches: {matches:?}");
    assert_eq!(fresh_only, 1);
    assert_eq!(
        matches.len(),
        5,
        "every site must be in exactly one bucket: {matches:?}"
    );
    assert_eq!(l3_only, 0, "no L3-only sites in this test");
    assert_eq!(unaligned, 0, "no unaligned duplicates in this test");
}

// ---------------------------------------------------------------------------
// Test 2: duplicate calls on the same line pair cleanly
// ---------------------------------------------------------------------------

/// When two fresh sites and two L3 sites share the same strong key
/// `(unit, start_line, callee_fp)` (e.g. identical back-to-back calls on one
/// line), the matcher pairs them positionally — 2 `Paired`, no `Unaligned`.
#[test]
fn duplicate_calls_on_same_line_pair_cleanly() {
    let mk = |start: u32, fp: u64| canonical_call_edge_for_test("cu:c:run", start, fp);
    // Two identical sites in both fresh and L3.
    let fresh = vec![mk(10, 1), mk(10, 1)];
    let l3 = vec![mk(10, 1), mk(10, 1)];
    let matches = match_sites(&fresh, &l3);
    let paired = matches
        .iter()
        .filter(|m| matches!(m, SiteMatch::Paired(_, _)))
        .count();
    let unaligned = matches
        .iter()
        .filter(|m| matches!(m, SiteMatch::Unaligned(_, _)))
        .count();
    assert_eq!(paired, 2, "matches: {matches:?}");
    assert_eq!(
        unaligned, 0,
        "equal-count duplicates must not produce Unaligned"
    );
}

// ---------------------------------------------------------------------------
// Test 3: FreshOnly in a different (from,kind) group does not cascade
// ---------------------------------------------------------------------------

/// A fresh site whose caller has NO L3 peer at all (different `from` key →
/// different partition) is emitted as `FreshOnly`.  The two other sites from
/// the first caller still pair cleanly — proving that one partition's
/// mismatch is invisible to another partition.
#[test]
fn fresh_only_different_caller_does_not_cascade() {
    let mk = |caller: &str, start: u32, fp: u64| canonical_call_edge_for_test(caller, start, fp);
    let fresh = vec![
        mk("cu:c:run", 10, 1),
        mk("cu:c:run", 20, 2),
        mk("cu:c:post", 10, 1), // different caller — no L3 peer
    ];
    let l3 = vec![mk("cu:c:run", 10, 1), mk("cu:c:run", 20, 2)];
    let matches = match_sites(&fresh, &l3);
    let paired = matches
        .iter()
        .filter(|m| matches!(m, SiteMatch::Paired(_, _)))
        .count();
    let fresh_only = matches
        .iter()
        .filter(|m| matches!(m, SiteMatch::FreshOnly(_)))
        .count();
    let l3_only = matches
        .iter()
        .filter(|m| matches!(m, SiteMatch::L3Only(_)))
        .count();
    // 2 clean pairs in "cu:c:run"; 1 FreshOnly in "cu:c:post"; no L3Only.
    assert_eq!(paired, 2, "matches: {matches:?}");
    assert_eq!(fresh_only, 1, "the cu:c:post site has no L3 peer");
    assert_eq!(l3_only, 0);
}
// ---------------------------------------------------------------------------
// Test 10 (Phase 4b Task 4; converted 1B.3b Task 1 Step 4): Fixture —
// L3-INDEPENDENT EventFlow target-set baseline
// ---------------------------------------------------------------------------

/// Verifies the fresh resolver's OWN EventFlow resolution against a frozen,
/// hand-reviewed baseline over the embedded fixture in `tests/fixtures/events/`.
///
/// 1B.3b Task 1: this test used to call `run_event_flow_gate` (a LIVE L3
/// comparison, even on this small synthetic fixture). It now calls
/// [`project_fresh_event_rows`] — L3-INDEPENDENT, no `engine::l3` build at
/// all — and asserts the EXACT resolved publisher→subscriber pair set
/// against a baseline frozen below. 1B.3b Task 3 deleted the old live,
/// CDO-gated `run_event_flow_gate` dual-run gate entirely; the zero-tolerance
/// EventFlow gate is now this fixture plus `cdo_event_audit_frozen_load`
/// (the frozen event golden) and the ported event-route teeth.
///
/// The fixture has ONE app with:
///   • codeunit 50100 EventPublisher  — two overloads of OnAfterPost (0- and
///     1-param), OnBeforePost (BusinessEvent), OnInternalEvent (InternalEvent).
///   • codeunit 50200 ManualSub       — subscribes to OnAfterPost with 0 params,
///     EventSubscriberInstance=Manual.
///   • codeunit 50201 SkipLicenseSub  — subscribes to OnBeforePost,
///     SkipOnMissingLicense=true.
///   • codeunit 50202 MultiAttrSub    — two [EventSubscriber] attrs (OnAfterPost
///     + OnBeforePost on the same procedure) — fresh reads BOTH (no
///     first-attr-only limitation; that was an L3 quirk, not a fresh one).
///   • codeunit 50203 InternalSub     — subscribes to OnInternalEvent.
///
/// Fresh resolves exactly 5 publisher→subscriber rows (verified by inspecting
/// this exact baseline before committing it):
///   1. OnAfterPost (0-param overload)  <- ManualSub.HandleOnAfterPost
///   2. OnAfterPost (0-param overload)  <- MultiAttrSub.HandleBoth (first attr)
///   3. OnBeforePost                    <- SkipLicenseSub.HandleOnBeforePost
///   4. OnBeforePost                    <- MultiAttrSub.HandleBoth (second attr)
///   5. OnInternalEvent                 <- InternalSub.HandleOnInternalEvent
///
/// Fresh correctly disambiguates the 0-param OnAfterPost overload (no
/// subscriber lands on the 1-param overload) — that disambiguation was
/// previously visible only as `l3_false_positive_arity_mismatch` on the L3
/// comparison; here it is a direct, positive assertion about fresh's own
/// arity-aware overload pick.
#[test]
fn event_fixture_two_stage_join() {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/events");

    let rows = project_fresh_event_rows(&fixture);
    let actual: Vec<(String, String, usize, String, String)> = rows
        .iter()
        .map(|r| {
            (
                r.publisher.object_lc.clone(),
                r.event_name_lc.clone(),
                r.publisher_arity.unwrap_or(usize::MAX),
                r.subscriber.object_lc.clone(),
                r.subscriber.routine_lc.clone(),
            )
        })
        .collect();
    eprintln!("event fixture fresh rows: {actual:#?}");

    let mut expected: Vec<(String, String, usize, String, String)> = vec![
        (
            "50100".into(),
            "onafterpost".into(),
            0,
            "50200".into(),
            "handleonafterpost".into(),
        ),
        (
            "50100".into(),
            "onafterpost".into(),
            0,
            "50202".into(),
            "handleboth".into(),
        ),
        (
            "50100".into(),
            "onbeforepost".into(),
            0,
            "50201".into(),
            "handleonbeforepost".into(),
        ),
        (
            "50100".into(),
            "onbeforepost".into(),
            0,
            "50202".into(),
            "handleboth".into(),
        ),
        (
            "50100".into(),
            "oninternalevent".into(),
            0,
            "50203".into(),
            "handleoninternalevent".into(),
        ),
    ];
    expected.sort();
    let mut actual_sorted = actual.clone();
    actual_sorted.sort();

    assert_eq!(
        actual_sorted, expected,
        "fresh EventFlow resolution over tests/fixtures/events diverged from the \
         frozen baseline.\nActual:\n{actual:#?}"
    );

    // No subscriber lands on the 1-param OnAfterPost overload — fresh's
    // arity-aware overload pick (was the L3 comparison's
    // `l3_false_positive_arity_mismatch` signal; now a direct assertion).
    assert!(
        rows.iter()
            .filter(|r| r.event_name_lc == "onafterpost")
            .all(|r| r.publisher_arity == Some(0)),
        "no subscriber may resolve to the 1-param OnAfterPost overload: {rows:#?}"
    );

    // Determinism
    let rows2 = project_fresh_event_rows(&fixture);
    assert_eq!(
        rows, rows2,
        "project_fresh_event_rows must be deterministic"
    );
}

// ---------------------------------------------------------------------------
// Task 5: Independent event-route teeth (unit tests — no CDO env required)
// ---------------------------------------------------------------------------

/// Build a minimal `ParsedUnit` from AL source for a given app GUID.
fn make_teeth_unit(guid: &str, name: &str, src: &str) -> (AppId, ParsedUnit) {
    let app_id = AppId {
        guid: guid.to_string(),
        name: name.to_string(),
        publisher: "Test".to_string(),
        version: "1.0.0.0".to_string(),
    };
    let provenance = Provenance {
        app: app_id.clone(),
        tier: TrustTier::Workspace,
        content_hash: String::new(),
    };
    let unit = ParsedUnit {
        app: app_id.clone(),
        files: vec![ParsedFile {
            virtual_path: "Sub.al".to_string(),
            file: std::sync::Arc::new(al_syntax::parse(src)),
            provenance,
            text: src.into(),
        }],
    };
    (app_id, unit)
}

/// Build a `(AppRegistry, RoutineNodeId)` for a codeunit-scoped procedure.
fn make_sub_rid(
    app_id: &AppId,
    obj_num: i64,
    routine_name_lc: &str,
    params: usize,
) -> (AppRegistry, RoutineNodeId) {
    let mut apps = AppRegistry::default();
    let app_ref = apps.intern(app_id);
    let rid = RoutineNodeId {
        object: ObjectNodeId {
            app: app_ref,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(obj_num),
        },
        name_lc: routine_name_lc.to_string(),
        enclosing_member_lc: None,
        params_count: params,
        sig_fp: 0,
    };
    (apps, rid)
}

/// (c) Correct subscriber with a matching raw `[EventSubscriber]` attribute → PASSES.
#[test]
fn event_teeth_correct_subscriber_passes() {
    let src = r#"codeunit 50100 "EvtSub"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"EvtPub", 'OnAfterX', '', false, false)]
    local procedure OnAfterXHandler()
    begin
    end;
}"#;
    let (app_id, unit) = make_teeth_unit("guid-teeth-c", "TeethApp", src);
    let (apps, sub_rid) = make_sub_rid(&app_id, 50100, "onafterxhandler", 0);
    assert!(
        verify_event_subscriber_route(
            &sub_rid,
            "codeunit",
            "evtpub",
            "onafterx",
            0,
            Some(false),
            &[unit],
            &apps,
        ),
        "correct subscriber must PASS the teeth check"
    );
}

/// (a) Subscriber raw attribute names a DIFFERENT publisher → FAILS.
#[test]
fn event_teeth_wrong_publisher_fails() {
    let src = r#"codeunit 50101 "EvtSubWrongPub"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"EvtPub", 'OnAfterX', '', false, false)]
    local procedure OnAfterXHandler()
    begin
    end;
}"#;
    let (app_id, unit) = make_teeth_unit("guid-teeth-a", "TeethApp", src);
    let (apps, sub_rid) = make_sub_rid(&app_id, 50101, "onafterxhandler", 0);
    assert!(
        !verify_event_subscriber_route(
            &sub_rid,
            "codeunit",
            "evtpub_other", // WRONG publisher name
            "onafterx",
            0,
            Some(false),
            &[unit],
            &apps,
        ),
        "wrong publisher name must FAIL the teeth check"
    );
}

/// (b) Subscriber `params_count` exceeds publisher params → FAILS (parameter prefix check).
#[test]
fn event_teeth_excess_params_fails() {
    // Subscriber procedure has 2 params; publisher event has 0.
    let src = r#"codeunit 50102 "EvtSubManyParams"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"EvtPub", 'OnAfterX', '', false, false)]
    local procedure OnAfterXHandler(Sender: Codeunit "EvtPub"; var IsHandled: Boolean)
    begin
    end;
}"#;
    let (app_id, unit) = make_teeth_unit("guid-teeth-b", "TeethApp", src);
    let (apps, sub_rid) = make_sub_rid(&app_id, 50102, "onafterxhandler", 2);
    assert!(
        !verify_event_subscriber_route(
            &sub_rid,
            "codeunit",
            "evtpub",
            "onafterx",
            0, // publisher has 0 params; subscriber has 2
            Some(false),
            &[unit],
            &apps,
        ),
        "subscriber with more params than publisher must FAIL the teeth check"
    );
}

// ---------------------------------------------------------------------------
// Tests 12–16: ABI ingestion-integrity + Histogram taxonomy split
// ---------------------------------------------------------------------------

use al_sem::engine::deps::symbol_reference::{
    AbiEventKind as SrAbiEventKind, AbiObject, AbiParameter, AbiRoutine, SubtypeTag,
    SymbolReferenceAbi,
};
use al_sem::program::node::AppRef;
use al_sem::program::resolve::abi_check::{
    AbiIntegrityReport, RawAbiIndex, abi_ingestion_integrity, run_abi_integrity_check_on,
};
use al_sem::program::resolve::edge::{
    AbiEventKind, AbiRoutineKey, AbiRoutineKind, BuiltinId, CanonicalSpan, DispatchShape, Edge,
    EdgeKind, Evidence, Histogram, Route, RouteTarget, SetCompleteness, SiteId, SourcePos,
    UnknownReason, Witness,
};

/// Build a minimal dep abi with Codeunit 50100 "Dep Pub":
///   - DoDepWork(x: Integer) — procedure, 1 param
///   - OnDepEvent(p1, p2)   — event-publisher (Integration), 2 params
fn dep_pub_abi() -> SymbolReferenceAbi {
    SymbolReferenceAbi {
        objects: vec![AbiObject {
            object_type: "Codeunit".into(),
            object_number: 50100,
            name: "Dep Pub".into(),
            routines: vec![
                AbiRoutine {
                    name: "DoDepWork".into(),
                    kind: "procedure".into(),
                    event_kind: SrAbiEventKind::Unknown,
                    parameters: vec![AbiParameter {
                        name: "x".into(),
                        type_text: "Integer".into(),
                        is_var: false,
                        is_temporary: false,
                        subtype_id: None,
                        subtype_raw_name: None,
                        subtype_tag: SubtypeTag::NoSubtype,
                    }],
                    return_type_text: None,
                    return_type_id: None,
                    is_local: false,
                    is_internal: false,
                    is_protected: false,
                    parameters_known: true,
                    attributes: vec![],
                    attributes_parsed: vec![],
                },
                AbiRoutine {
                    name: "OnDepEvent".into(),
                    kind: "event-publisher".into(),
                    event_kind: SrAbiEventKind::Integration,
                    parameters: vec![
                        AbiParameter {
                            name: "p1".into(),
                            type_text: "Integer".into(),
                            is_var: false,
                            is_temporary: false,
                            subtype_id: None,
                            subtype_raw_name: None,
                            subtype_tag: SubtypeTag::NoSubtype,
                        },
                        AbiParameter {
                            name: "p2".into(),
                            type_text: "Text".into(),
                            is_var: false,
                            is_temporary: false,
                            subtype_id: None,
                            subtype_raw_name: None,
                            subtype_tag: SubtypeTag::NoSubtype,
                        },
                    ],
                    return_type_text: None,
                    return_type_id: None,
                    is_local: false,
                    is_internal: false,
                    is_protected: false,
                    parameters_known: true,
                    attributes: vec![],
                    attributes_parsed: vec![],
                },
            ],
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// Build a minimal `RoutineNodeId` for use in synthetic edges.
fn test_rid(app: u32, obj_kind: ObjectKind, obj_num: i64, name: &str) -> RoutineNodeId {
    RoutineNodeId {
        object: ObjectNodeId {
            app: AppRef(app),
            kind: obj_kind,
            key: ObjKey::Id(obj_num),
        },
        name_lc: name.to_string(),
        enclosing_member_lc: None,
        params_count: 0,
        sig_fp: 0,
    }
}

/// Build a minimal synthetic `Edge` with a single route.
fn single_route_edge(from_rid: RoutineNodeId, route: Route) -> Edge {
    Edge {
        from: from_rid.clone(),
        site: SiteId {
            caller: from_rid,
            span: CanonicalSpan {
                unit: "Test.al".into(),
                start: SourcePos { line: 1, col: 1 },
                end: SourcePos { line: 1, col: 20 },
            },
            callee_fingerprint: 42,
        },
        kind: EdgeKind::Call,
        shape: DispatchShape::Exact,
        completeness: SetCompleteness::Complete,
        routes: vec![route],
    }
}

/// Build the `AbiRoutineKey` that `resolver.rs::make_routine_route` would emit
/// for `DoDepWork` on Codeunit 50100 in app `AppRef(1)`.
fn dodepwork_key() -> AbiRoutineKey {
    AbiRoutineKey {
        app: AppRef(1),
        // object_type is format!("{:?}", ObjectKind::Codeunit).to_ascii_lowercase()
        object_type: "codeunit".into(),
        object_number: 50100,
        object_name_lc: String::new(), // empty when object_number != 0
        routine_name_lc: "dodepwork".into(),
        params_count: 1,
        param_type_fp: 0, // not checked by the index
        routine_kind: AbiRoutineKind::Procedure,
        event_kind: AbiEventKind::None,
    }
}

/// Build the `AbiRoutineKey` for `OnDepEvent` (event-publisher/Integration).
fn ondepevent_key() -> AbiRoutineKey {
    AbiRoutineKey {
        app: AppRef(1),
        object_type: "codeunit".into(),
        object_number: 50100,
        object_name_lc: String::new(),
        routine_name_lc: "ondepevent".into(),
        params_count: 2,
        param_type_fp: 0,
        routine_kind: AbiRoutineKind::EventPublisher,
        event_kind: AbiEventKind::Integration,
    }
}

/// Test 12: a mapped `AbiSymbol` route → `abi_mapped=1, abi_unmapped=0`.
#[test]
fn abi_integrity_maps_known_routine() {
    let abi = dep_pub_abi();
    let index = RawAbiIndex::build([(AppRef(1), &abi)]);

    let caller = test_rid(0, ObjectKind::Codeunit, 99, "caller");
    let edge = single_route_edge(
        caller,
        Route {
            target: RouteTarget::AbiSymbol {
                key: dodepwork_key(),
            },
            evidence: Evidence::Opaque,
            conditions: vec![],
            witness: Witness::AbiSymbol {
                key: dodepwork_key(),
            },
            receiver_tier: None,
        },
    );

    let report = abi_ingestion_integrity(&[edge], &index);
    assert_eq!(
        report,
        AbiIntegrityReport {
            abi_routes_total: 1,
            abi_mapped: 1,
            abi_unmapped: 0,
            abi_unmapped_sites: vec![],
        },
        "DoDepWork must map back to the raw ABI"
    );
}

/// Test 13: a fabricated `AbiSymbol` key naming a NON-existent routine →
/// `abi_unmapped=1`.
#[test]
fn abi_integrity_catches_unmapped_route() {
    let abi = dep_pub_abi();
    let index = RawAbiIndex::build([(AppRef(1), &abi)]);

    let bogus_key = AbiRoutineKey {
        app: AppRef(1),
        object_type: "codeunit".into(),
        object_number: 50100,
        object_name_lc: String::new(),
        routine_name_lc: "nonexistentproc".into(),
        params_count: 0,
        param_type_fp: 0,
        routine_kind: AbiRoutineKind::Procedure,
        event_kind: AbiEventKind::None,
    };

    let caller = test_rid(0, ObjectKind::Codeunit, 99, "caller");
    let edge = single_route_edge(
        caller,
        Route {
            target: RouteTarget::AbiSymbol {
                key: bogus_key.clone(),
            },
            evidence: Evidence::Opaque,
            conditions: vec![],
            witness: Witness::AbiSymbol {
                key: bogus_key.clone(),
            },
            receiver_tier: None,
        },
    );

    let report = abi_ingestion_integrity(&[edge], &index);
    assert_eq!(report.abi_routes_total, 1);
    assert_eq!(
        report.abi_unmapped, 1,
        "a key naming a non-existent routine must be caught as unmapped"
    );
    assert_eq!(
        report.abi_unmapped_sites[0].key.routine_name_lc,
        "nonexistentproc"
    );
}

/// Test 13b (beyond-1B.3b Task 5.5): an implicit ENTRY-TRIGGER boundary key
/// (the `resolve_object_run` Opaque fallback's synthesized key — object
/// exists, but the trigger name is never listed in the raw ABI `Methods`
/// array by AL/ABI-schema construction) must be treated as MAPPED even
/// though the raw index genuinely has no entry for it. `dep_pub_abi()`
/// carries a Codeunit 50100 with ZERO methods named `onrun` — this proves
/// the exemption, not a coincidental raw-index hit.
#[test]
fn abi_integrity_exempts_entry_trigger_boundary_key() {
    let abi = dep_pub_abi();
    let index = RawAbiIndex::build([(AppRef(1), &abi)]);

    let entry_trigger_key = AbiRoutineKey {
        app: AppRef(1),
        object_type: "codeunit".into(),
        object_number: 50100,
        object_name_lc: String::new(),
        routine_name_lc: "onrun".into(),
        params_count: 0,
        param_type_fp: 0,
        routine_kind: AbiRoutineKind::Procedure,
        event_kind: AbiEventKind::None,
    };

    let caller = test_rid(0, ObjectKind::Codeunit, 99, "caller");
    let edge = single_route_edge(
        caller,
        Route {
            target: RouteTarget::AbiSymbol {
                key: entry_trigger_key.clone(),
            },
            evidence: Evidence::Opaque,
            conditions: vec![],
            witness: Witness::AbiSymbol {
                key: entry_trigger_key,
            },
            receiver_tier: None,
        },
    );

    let report = abi_ingestion_integrity(&[edge], &index);
    assert_eq!(
        report,
        AbiIntegrityReport {
            abi_routes_total: 1,
            abi_mapped: 1,
            abi_unmapped: 0,
            abi_unmapped_sites: vec![],
        },
        "an entry-trigger boundary key (onrun/onopenpage/onprereport, Procedure/None, \
         0 params) must be exempt from the raw-ABI-Methods lookup — it asserts object \
         existence, not Methods-list membership, which entry triggers can never satisfy"
    );

    // Sanity: the SAME object/params/kind shape but a DIFFERENT (non-entry-trigger)
    // routine name must NOT be exempt — still genuinely unmapped.
    let non_trigger_key = AbiRoutineKey {
        app: AppRef(1),
        object_type: "codeunit".into(),
        object_number: 50100,
        object_name_lc: String::new(),
        routine_name_lc: "notatrigger".into(),
        params_count: 0,
        param_type_fp: 0,
        routine_kind: AbiRoutineKind::Procedure,
        event_kind: AbiEventKind::None,
    };
    let edge2 = single_route_edge(
        test_rid(0, ObjectKind::Codeunit, 99, "caller"),
        Route {
            target: RouteTarget::AbiSymbol {
                key: non_trigger_key.clone(),
            },
            evidence: Evidence::Opaque,
            conditions: vec![],
            witness: Witness::AbiSymbol {
                key: non_trigger_key,
            },
            receiver_tier: None,
        },
    );
    let report2 = abi_ingestion_integrity(&[edge2], &index);
    assert_eq!(
        report2.abi_unmapped, 1,
        "a non-entry-trigger name must still be caught as genuinely unmapped"
    );
}

/// Test 14: an event-publisher-target route whose key says `EventPublisher /
/// Integration` → maps to the event-publisher ABI entry (Task-1 fix verified).
/// A key with the WRONG `routine_kind` (Procedure) must be caught as unmapped.
#[test]
fn abi_integrity_event_publisher_kind_checked() {
    let abi = dep_pub_abi();
    let index = RawAbiIndex::build([(AppRef(1), &abi)]);

    // Correct key (EventPublisher / Integration) → must map.
    let caller = test_rid(0, ObjectKind::Codeunit, 99, "caller");
    let correct_edge = single_route_edge(
        caller.clone(),
        Route {
            target: RouteTarget::AbiSymbol {
                key: ondepevent_key(),
            },
            evidence: Evidence::Opaque,
            conditions: vec![],
            witness: Witness::AbiSymbol {
                key: ondepevent_key(),
            },
            receiver_tier: None,
        },
    );
    let ok = abi_ingestion_integrity(&[correct_edge], &index);
    assert_eq!(ok.abi_mapped, 1, "EventPublisher key must map");
    assert_eq!(ok.abi_unmapped, 0);

    // Wrong routine_kind (Procedure instead of EventPublisher) → must be caught.
    let mut wrong_key = ondepevent_key();
    wrong_key.routine_kind = AbiRoutineKind::Procedure;
    let wrong_edge = single_route_edge(
        caller,
        Route {
            target: RouteTarget::AbiSymbol {
                key: wrong_key.clone(),
            },
            evidence: Evidence::Opaque,
            conditions: vec![],
            witness: Witness::AbiSymbol { key: wrong_key },
            receiver_tier: None,
        },
    );
    let bad = abi_ingestion_integrity(&[wrong_edge], &index);
    assert_eq!(
        bad.abi_unmapped, 1,
        "mangled routine_kind (Procedure instead of EventPublisher) must be unmapped"
    );
}

/// Test 15: Histogram taxonomy split.
///
/// • Source route  → `resolved_source` increments, NOT `resolved_catalog/abi_external`.
/// • Catalog route → `resolved_catalog` increments.
/// • AbiSymbol/Opaque route → `resolved_abi_external` increments.
/// • Unknown/empty → `unknown`.
/// • `real_unknown_rate` stays = unknown / total.
#[test]
fn histogram_taxonomy_split() {
    let ws_rid = test_rid(0, ObjectKind::Codeunit, 1, "caller");

    // Source-resolved edge.
    let src_edge = single_route_edge(
        ws_rid.clone(),
        Route {
            target: RouteTarget::Routine(test_rid(0, ObjectKind::Codeunit, 2, "target")),
            evidence: Evidence::Source,
            conditions: vec![],
            witness: Witness::SourceSpan {
                file: "f.al".into(),
                span: (0, 10),
            },
            receiver_tier: None,
        },
    );

    // Catalog-resolved edge.
    let catalog_edge = single_route_edge(
        ws_rid.clone(),
        Route {
            target: RouteTarget::Builtin(BuiltinId("message".into())),
            evidence: Evidence::Catalog,
            conditions: vec![],
            witness: Witness::CatalogEntry {
                id: BuiltinId("message".into()),
                catalog_version: "v1".into(),
            },
            receiver_tier: None,
        },
    );

    // ABI-external edge.
    let abi_edge = single_route_edge(
        ws_rid.clone(),
        Route {
            target: RouteTarget::AbiSymbol {
                key: dodepwork_key(),
            },
            evidence: Evidence::Opaque,
            conditions: vec![],
            witness: Witness::AbiSymbol {
                key: dodepwork_key(),
            },
            receiver_tier: None,
        },
    );

    // Unknown (unresolved) edge.
    let unknown_edge = single_route_edge(
        ws_rid,
        Route {
            target: RouteTarget::Unresolved,
            evidence: Evidence::Unknown(UnknownReason::MemberNotFound),
            conditions: vec![],
            witness: Witness::None,
            receiver_tier: None,
        },
    );

    let edges = [src_edge, catalog_edge, abi_edge, unknown_edge];
    let h = Histogram::of_edges(&edges);

    assert_eq!(h.total, 4);
    assert_eq!(h.resolved_source, 1, "Source route → resolved_source");
    assert_eq!(h.resolved_catalog, 1, "Catalog route → resolved_catalog");
    assert_eq!(
        h.resolved_abi_external, 1,
        "AbiSymbol/Opaque route → resolved_abi_external"
    );
    assert_eq!(h.unknown, 1, "Unresolved/Unknown → unknown");
    assert_eq!(h.conditional_resolved, 0);
    assert_eq!(h.honest_dynamic, 0);
    assert_eq!(h.honest_empty, 0);

    // real_unknown_rate = 1/4 = 0.25
    let rate = h.real_unknown_rate();
    assert!(
        (rate - 0.25).abs() < 1e-9,
        "real_unknown_rate must be 0.25, got {rate}"
    );
}

/// Test 16 (CDO, env-gated): `abi_ingestion_integrity` over the full edge set →
/// `abi_unmapped == 0`.  Prints the taxonomy'd histogram + ABI coverage counts.
/// A miss = an ingestion/key-derivation bug — investigate and fix, do NOT relax.
#[test]
fn abi_ingestion_integrity_cdo_gate() {
    let Some(shared) = cdo_shared() else {
        return;
    };

    let report = run_abi_integrity_check_on(&shared.ctx);

    eprintln!(
        "AbiIntegrityReport: abi_routes_total={} abi_mapped={} abi_unmapped={}",
        report.abi_routes_total, report.abi_mapped, report.abi_unmapped,
    );
    // When abi_routes_total == 0, abi_unmapped == 0 holds vacuously: the
    // workspace's deps all ship EmbeddedSource/ShowMyCode, so they resolve to
    // Source routes rather than AbiSymbol.  The 2 true SymbolOnly deps in CDO
    // are trivial (permissionset/translation apps) with no public routines.
    // ABI ingestion-path correctness is validated by the in-repo fixture tests
    // (Tests 12-14), NOT by this CDO run.  This note exists so a maintainer
    // reading a passing test output does not mistake "vacuous pass" for
    // "ABI coverage exercised on CDO".  When a workspace with SymbolOnly
    // public-routine deps is used, this gate WILL exercise the ABI path.
    if report.abi_routes_total == 0 {
        eprintln!(
            "NOTE: this CDO workspace has no SymbolOnly-dep routines (its deps ship \
             EmbeddedSource/ShowMyCode \u{2192} resolve to Source routes, not AbiSymbol). \
             The ABI ingestion path is validated by the in-repo fixtures (Tests 12-14), \
             NOT by this CDO run. abi_unmapped==0 holds trivially here."
        );
    }
    if !report.abi_unmapped_sites.is_empty() {
        eprintln!("UNMAPPED SITES (first 10):");
        for site in report.abi_unmapped_sites.iter().take(10) {
            eprintln!(
                "  app={:?} obj_type={} obj_num={} obj_name_lc={} \
                 routine={} params={} kind={:?} event={:?}",
                site.key.app,
                site.key.object_type,
                site.key.object_number,
                site.key.object_name_lc,
                site.key.routine_name_lc,
                site.key.params_count,
                site.key.routine_kind,
                site.key.event_kind,
            );
        }
    }

    // Also compute and print the histogram split.
    {
        use al_sem::program::resolve::stub::resolve_program;

        {
            let graph = shared.ctx.graph();
            let parsed = shared.ctx.parsed();
            let edges = resolve_program(graph, parsed);
            let h = Histogram::of_edges(&edges);
            eprintln!(
                "Histogram: total={} resolved_source={} resolved_catalog={} \
                 resolved_abi_external={} conditional_resolved={} \
                 honest_dynamic={} honest_empty={} unknown={} \
                 real_unknown_rate={:.4}",
                h.total,
                h.resolved_source,
                h.resolved_catalog,
                h.resolved_abi_external,
                h.conditional_resolved,
                h.honest_dynamic,
                h.honest_empty,
                h.unknown,
                h.real_unknown_rate(),
            );

            // Task 1 (round-1 I3) diagnostic: enumerate SymbolOnly OBJECTS
            // with >=1 surviving routine (Public/Protected; `local`/
            // `internal` are still dropped at ingestion — unaffected by this
            // task). Task 1's fix is EMPIRICALLY CDO-neutral because CDO's
            // true-SymbolOnly surface is empty (all real deps ship
            // EmbeddedSource/ShowMyCode), NOT because the selection-logic
            // change is inert in general — this diagnostic makes that
            // emptiness explicit so any metric movement on a DIFFERENT
            // workspace (one with real SymbolOnly public-routine deps) is
            // immediately attributable to this task's per-candidate
            // visibility fix, rather than mistaken for a regression.
            let symbolonly_objects_with_routines: Vec<_> = graph
                .objects
                .iter()
                .filter(|o| o.tier == TrustTier::SymbolOnly)
                .filter(|o| graph.routines.iter().any(|r| r.id.object == o.id))
                .collect();
            eprintln!(
                "Task1 SymbolOnly-object diagnostic: {} SymbolOnly object(s) carry >=1 \
                 surviving routine (expected 0 on CDO — its true-SymbolOnly surface is empty)",
                symbolonly_objects_with_routines.len()
            );
            for o in symbolonly_objects_with_routines.iter().take(20) {
                eprintln!(
                    "  non-empty SymbolOnly object: {:?} name={:?}",
                    o.id, o.name
                );
            }
        }
    }

    assert_eq!(
        report.abi_unmapped, 0,
        "every AbiSymbol route must map back to the raw ABI — a miss is an \
         ingestion/key-derivation bug; investigate and fix: {report:?}"
    );

    // Determinism: two consecutive runs (over the same shared substrate) must
    // produce identical output.
    assert_eq!(
        report,
        run_abi_integrity_check_on(&shared.ctx),
        "run_abi_integrity_check_on must be deterministic"
    );
}

/// Non-circularity demonstration.
///
/// Proves that `verify_event_subscriber_route` reads from the raw `ParsedUnit` IR,
/// NOT from any cached `RoutineNode.event_subscribers` field:
///
/// 1. With a correct `ParsedUnit` (raw `[EventSubscriber]` attribute present) → PASSES.
/// 2. With a modified `ParsedUnit` where the attribute is absent (simulating what
///    would happen if the function read corrupt raw IR instead of the cached value)
///    → FAILS.
///
/// If the function used the cached `RoutineNode.event_subscribers` (which still says
/// "subscribes to EvtPub"), both cases would return PASS.  The FAIL in case 2 is the
/// proof: the function observably reads from the raw `ParsedUnit` IR.
#[test]
fn event_teeth_non_circularity_reads_raw_ir() {
    // ── Case 1: correct ParsedUnit (attribute present) → PASSES ────────────
    let src_with_attr = r#"codeunit 50103 "EvtSubNC"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"EvtPub", 'OnAfterX', '', false, false)]
    local procedure OnAfterXHandler()
    begin
    end;
}"#;
    let (app_id, unit_with_attr) = make_teeth_unit("guid-teeth-nc", "TeethNC", src_with_attr);
    let (apps, sub_rid) = make_sub_rid(&app_id, 50103, "onafterxhandler", 0);

    assert!(
        verify_event_subscriber_route(
            &sub_rid,
            "codeunit",
            "evtpub",
            "onafterx",
            0,
            Some(false),
            &[unit_with_attr],
            &apps,
        ),
        "correct raw IR must PASS"
    );

    // ── Case 2: ParsedUnit with attribute absent → FAILS ───────────────────
    // The `sub_rid` (RoutineNodeId) is unchanged — it represents the same routine
    // in the index's view.  If the function used a cached `RoutineNode.event_subscribers`
    // (built from the ORIGINAL correct source), it would still return PASS here.
    // The FAIL proves it actually re-parses the raw `ParsedUnit` IR.
    let src_no_attr = r#"codeunit 50103 "EvtSubNC"
{
    local procedure OnAfterXHandler()
    begin
    end;
}"#;
    let (_, unit_no_attr) = make_teeth_unit("guid-teeth-nc", "TeethNC", src_no_attr);

    assert!(
        !verify_event_subscriber_route(
            &sub_rid,
            "codeunit",
            "evtpub",
            "onafterx",
            0,
            Some(false),
            &[unit_no_attr],
            &apps,
        ),
        "absent attribute in raw IR must FAIL — proves the check reads raw ParsedUnit IR, \
         not the index's cached event_subscribers"
    );
}

// ---------------------------------------------------------------------------
// Tests 11+: 1B.3a Task 3 — obligation coverage + resolve_full_program
// ---------------------------------------------------------------------------

use al_sem::program::resolve::full::{
    ClassifiedEdge, Coverage, ObligationId, ProgramReport, coverage_holds, is_primary_scope,
    resolve_full_program,
};

// ---------------------------------------------------------------------------
// Test 11 (unit fixture): coverage holds; histogram buckets are correct
// ---------------------------------------------------------------------------

/// Runs `resolve_full_program` over the small `full_program_fixture` workspace.
///
/// The fixture contains one codeunit with:
///   - Caller(): 3 call obligations (KnownProc → resolved_source; UnknownXYZ →
///     Unknown; Codeunit.Run(Dyn) → HonestDynamic)
///   - OnMyEvent(): publisher obligation → HonestEmpty EventFlow edge
///   - KnownProc(): 0 call obligations (body empty)
///
/// Assertions:
///   1. `coverage_holds` — every obligation maps to exactly one edge.
///   2. Histogram buckets count correctly.
///   3. `real_unknown_rate` is consistent with Unknown count / total.
#[test]
fn full_program_fixture_coverage_holds_and_histogram_is_correct() {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/full_program_fixture");

    let report = resolve_full_program(&fixture).expect("fixture must parse successfully");

    // ── Coverage contract (distinct-id SET equality) ─────────────────────────
    assert!(
        coverage_holds(&report.coverage),
        "coverage contract violated: missing={:?}, extra={:?}",
        report.coverage.missing,
        report.coverage.extra,
    );

    // The fixture has 3 call sites (Caller body) + 1 publisher (OnMyEvent).
    // KnownProc body is empty, so no call sites there.
    assert_eq!(
        report.coverage.parsed_obligations, 4,
        "expected 3 call sites + 1 publisher obligation = 4 total"
    );
    assert_eq!(
        report.coverage.classified_edges, 4,
        "classified_edges must equal parsed_obligations"
    );

    // ── Histogram buckets ────────────────────────────────────────────────────
    // resolved_source=1 (KnownProc), unknown=1 (UnknownXYZ),
    // honest_dynamic=1 (Codeunit.Run(Dyn)), honest_empty=1 (OnMyEvent event).
    assert_eq!(
        report.histogram.resolved_source, 1,
        "KnownProc() must resolve via Source evidence"
    );
    assert_eq!(
        report.histogram.unknown, 1,
        "UnknownXYZ() must classify as Unknown"
    );
    assert_eq!(
        report.histogram.honest_dynamic, 1,
        "Codeunit.Run(Dyn) must classify as HonestDynamic"
    );
    assert_eq!(
        report.histogram.honest_empty, 1,
        "OnMyEvent publisher with zero subscribers must be HonestEmpty"
    );
    // Nothing should be in catalog or abi_external for this fixture.
    assert_eq!(report.histogram.resolved_catalog, 0);
    assert_eq!(report.histogram.resolved_abi_external, 0);

    // ── real_unknown_rate ────────────────────────────────────────────────────
    // 1 Unknown out of 4 total = 0.25
    let rate = report.histogram.real_unknown_rate();
    assert!(
        (rate - 0.25).abs() < 1e-9,
        "real_unknown_rate must be 0.25 for this fixture; got {rate}"
    );
}

/// Shared-substrate refactor (2026-07-15 spec): the context-taking core and
/// the path wrapper must produce identical reports — the wrapper IS
/// `build_context` + `resolve_full_program_with`, so any divergence means the
/// split leaked behavior.
#[test]
fn resolve_full_program_with_matches_path_wrapper_on_fixture() {
    use al_sem::program::resolve::full::{build_context, resolve_full_program_with};

    let fixture =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/semantic-golden");
    let via_path = resolve_full_program(&fixture).expect("wrapper must succeed on fixture");
    let ctx = build_context(&fixture).expect("build_context must succeed on fixture");
    let via_ctx = resolve_full_program_with(&ctx);

    assert_eq!(
        via_path.histogram, via_ctx.histogram,
        "histogram must match"
    );
    assert_eq!(
        via_path.primary_histogram, via_ctx.primary_histogram,
        "primary histogram must match"
    );
    assert_eq!(
        via_path.edges.len(),
        via_ctx.edges.len(),
        "edge count must match"
    );
}

// ---------------------------------------------------------------------------
// Task 2 (mirrors I1): end-to-end call-graph fixture — object-typed declared
// var shape preservation (`ParsedType::Object` → `ObjectRef`).
// ---------------------------------------------------------------------------

/// Runs `resolve_full_program` over `tests/r0-corpus/ws-object-name-shape/` —
/// see that directory's `PROOF.md` for the full write-up.
///
/// `codeunit 80 RealById` (no `P()`) and `codeunit 50100 "80"` (a codeunit
/// literally NAMED `80`, declares `P()`) coexist; `Caller.Trigger` declares
/// `C: Codeunit "80"` (a QUOTED name reference) and calls `C.P()`. Pre-fix,
/// `resolve_object_name_lc` re-parsed the already-unquoted string `"80"` as
/// the numeric id `80`, silently misrouting the receiver to `RealById` —
/// which has no `P()` — producing a false `Unknown` edge instead of the
/// correct `Source` edge to `Named80.P`. This is the exact `ParsedType::Object`
/// sibling of the I1 `ParsedType::Record` shape-loss fix.
#[test]
fn object_name_shape_quoted_digit_name_resolves_to_named_object_not_numeric_id() {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-object-name-shape");

    let report = resolve_full_program(&fixture).expect("fixture must parse successfully");

    assert!(
        coverage_holds(&report.coverage),
        "coverage contract violated: missing={:?}, extra={:?}",
        report.coverage.missing,
        report.coverage.extra,
    );

    // Locate the Caller.Trigger call site's edge (the only call site in the
    // fixture — `C.P()`).
    let call_edge = report
        .edges
        .iter()
        .find(|ce| {
            ce.edge.from.name_lc == "trigger" && ce.edge.from.object.key == ObjKey::Id(50101)
        })
        .expect("Caller.Trigger's C.P() call site must produce a classified edge");

    assert_eq!(
        call_edge.edge.routes.len(),
        1,
        "expected exactly one route: {:?}",
        call_edge.edge.routes
    );
    let route = &call_edge.edge.routes[0];
    assert_eq!(
        route.evidence,
        Evidence::Source,
        "C.P() must resolve via Source evidence (a same-app declared-var member \
         call), not {:?} — route: {route:?}",
        route.evidence
    );

    let RouteTarget::Routine(ref rid) = route.target else {
        panic!(
            "expected RouteTarget::Routine (a resolved procedure), got {:?}",
            route.target
        );
    };
    assert_eq!(
        rid.object.key,
        ObjKey::Id(50100),
        "C.P() must target Named80 (codeunit 50100, literally named \"80\"), \
         NEVER RealById (codeunit id 80) — a wrong id here is exactly the I1-class \
         shape-loss bug this fixture proves fixed. Got target object key: {:?}",
        rid.object.key
    );
    assert_eq!(rid.name_lc, "p", "target routine must be Named80.P");
}

// ---------------------------------------------------------------------------
// Test 12 (unit): dropped obligation → coverage_holds returns false
// ---------------------------------------------------------------------------

/// The coverage contract catches a silently-dropped obligation.
///
/// Verifies that if we manually construct a `Coverage` where one obligation ID
/// is missing from the edges, `coverage_holds` returns `false`.  This ensures
/// the contract check is active and not vacuously true.
#[test]
fn dropped_obligation_is_caught_by_coverage_contract() {
    use al_sem::program::node::{AppRef, ObjKey, ObjectKind, ObjectNodeId, RoutineNodeId};
    use al_sem::program::resolve::edge::{CanonicalSpan, SourcePos};

    fn make_rid(name: &str) -> RoutineNodeId {
        RoutineNodeId {
            object: ObjectNodeId {
                app: AppRef(0),
                kind: ObjectKind::Codeunit,
                key: ObjKey::Id(1),
            },
            name_lc: name.to_string(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        }
    }

    fn make_span(line: u32) -> CanonicalSpan {
        CanonicalSpan {
            unit: "Test.al".into(),
            start: SourcePos { line, col: 0 },
            end: SourcePos { line, col: 10 },
        }
    }

    let id_a = ObligationId::CallSite {
        caller: make_rid("caller"),
        span: make_span(10),
        callee_fp: 1,
    };
    let id_b = ObligationId::CallSite {
        caller: make_rid("caller"),
        span: make_span(20),
        callee_fp: 2,
    };

    // Coverage where obligation B is missing from edges — simulates a resolver
    // that silently dropped obligation B.
    let missing_coverage = Coverage {
        parsed_obligations: 2,
        classified_edges: 1,
        missing: vec![id_b.clone()],
        extra: vec![],
    };
    assert!(
        !coverage_holds(&missing_coverage),
        "a coverage with missing obligations must NOT hold"
    );

    // Coverage where both obligations are classified — contract must hold.
    let full_coverage = Coverage {
        parsed_obligations: 2,
        classified_edges: 2,
        missing: vec![],
        extra: vec![],
    };
    assert!(
        coverage_holds(&full_coverage),
        "a complete coverage must hold"
    );

    // Extra edge (no obligation): must also fail.
    let extra_coverage = Coverage {
        parsed_obligations: 1,
        classified_edges: 2,
        missing: vec![],
        extra: vec![id_a],
    };
    assert!(
        !coverage_holds(&extra_coverage),
        "a coverage with extra (obligation-less) edges must NOT hold"
    );
}

// ---------------------------------------------------------------------------
// Test 13 (CDO env-gated): coverage holds; evidence_overclaim=0; self-reported
//          metric prints + deterministic; rate ≤ recorded ceiling.
// ---------------------------------------------------------------------------

/// Full-program obligation coverage + self-reported north-star metric over CDO.
///
/// Guards: requires `CDO_WS` env var pointing at a real BC workspace.
///
/// Assertions (all required):
///   - `coverage_holds` (distinct-id SET equality — no obligation silently dropped)
///   - `abi_unmapped == 0` (ABI ingestion integrity)
///   - Taxonomy'd histogram + real_unknown_rate prints cleanly
///   - Deterministic (two consecutive runs produce identical histogram)
///   - `real_unknown_rate` ≤ recorded ceiling (regression guard)
#[test]
fn cdo_full_program_coverage_and_self_reported_metric() {
    let Some(ws) = cdo_ws_or_enforce() else {
        return;
    };
    let Some(shared) = cdo_shared() else {
        return;
    };

    let report = &shared.report;

    // ── Coverage contract ────────────────────────────────────────────────────
    assert!(
        coverage_holds(&report.coverage),
        "coverage contract violated on CDO — no obligation may be silently dropped.\n\
         missing={} ids, extra={} ids",
        report.coverage.missing.len(),
        report.coverage.extra.len(),
    );

    // ── ABI ingestion integrity ──────────────────────────────────────────────
    assert_eq!(
        report.abi_integrity.abi_unmapped, 0,
        "ABI ingestion integrity: {} route key(s) not found in raw SymbolReference\n{:#?}",
        report.abi_integrity.abi_unmapped, report.abi_integrity.abi_unmapped_sites
    );

    // ── ParseStatus::Recovered diagnostic (Task 3, preproc foundations
    // plan) ───────────────────────────────────────────────────────────────
    // MEASURED 2026-07-04: NOT empty at introduction — this new diagnostic
    // immediately proved its worth by surfacing TWO real, previously-
    // invisible tree-sitter-al grammar defects, both confined to DEPENDENCY
    // (embedded ShowMyCode) source, NEVER the CDO primary workspace itself:
    //
    // 1. `OptionMembers = TableData,...` (bare, unquoted, case-insensitive
    //    `tabledata`) as the FIRST member immediately after `=` collided
    //    with the `tabledata` keyword that also starts the
    //    `tabledata_permission_list` alternative of `_property_value` —
    //    tree-sitter dropped the first token as an ERROR and resumed
    //    `option_member_list` from the second member onward. Reproduced
    //    minimally: `OptionMembers = TableData,Table,Report;` errored;
    //    `OptionMembers = Foo,TableData,Bar;` (same keyword, non-first
    //    position) parsed clean — confirmed this was a first-position-only
    //    ambiguity, not a general keyword-as-option-member gap (the grammar
    //    already special-cased several OTHER keywords, e.g. `local`/
    //    `internal`/`protected`, as valid `option_member`s). Hit the
    //    Microsoft `System` app's `Object.Table.al` (field `Type: Option`),
    //    `NAVAppObjectPrerequisites.Table.al` (same shape), and
    //    `DatabaseLocks.Table.al` (same shape) — all three are variants of
    //    the platform's standard NAV-object-type option list.
    // 2. `# pragma warning disable LC0088` (a SPACE between `#` and
    //    `pragma`) was not recognized as a pragma directive at all — only
    //    `#pragma` (no space) was. Hit Continia System Application's
    //    `Http.Codeunit.al` (one stray `# pragma` amid otherwise-consistent
    //    `#pragma` usage in the same file).
    //
    // FIXED 2026-07-04 (grammar-defects-and-repin plan, Task 1) — both were
    // genuine `tree-sitter-al` grammar gaps (this project owns that
    // grammar); fixed at the grammar source and repinned (submodule
    // `f150581` -> `6d87aee`, engine tag v3.1.0):
    //   1. A hidden `_tabledata_keyword` rule referenced both by
    //      `tabledata_permission` (unchanged shape) and a new
    //      `alias($._tabledata_keyword, $.identifier)` arm in
    //      `option_member` (mirrors the existing `table_keyword`-via-
    //      `keyword_as_identifier` precedent for the same keyword-as-
    //      option-member class). No new visible node kind — `gen-syntax`
    //      re-run confirmed a byte-for-byte identical RawKind vocabulary
    //      (388 named kinds / 73 fields / 388 typed structs / 13 union
    //      enums); after the spaced-if revert `node-types.json` is
    //      byte-identical to the pre-plan baseline, so the whole generated
    //      directory (hash included) is unchanged.
    //   2. `pragma`/`preproc_region`/`preproc_endregion` tightened from zero-
    //      tolerance / `\s*` to `[ \t]*` (horizontal-only — `\s*` would let
    //      the extras token span a newline and silently swallow the next
    //      line's real source; confirmed pre-fix via a scratch fixture that
    //      `preproc_region` already had this exact cross-line hazard, closed
    //      in the same pass).
    // A THIRD, preventive fix — `# if`/`# elif` (single space) as literal
    // variants mirroring the existing `# else`/`# endif` precedent — was
    // drafted (zero corpus instances) but DROPPED after review found it
    // introduced genuine GLR non-determinism (the pre-existing
    // `preproc_split_if_then_begin_else_shared` construct, given a spaced
    // open, produced two mutually-exclusive stable parses across process
    // states for byte-identical input — `tree-sitter test`'s own pass count
    // flapped 1453<->1463 with zero source change) plus a silent shape defect
    // under `#if`-nesting (the literal-variant token doesn't participate in
    // the scanner's depth counter, so a nested spaced `# if` undercounted
    // depth and lost its enclosing `begin_keyword`/`end_keyword` naming).
    // Current, intentional behavior: a spaced `# if`/`# elif` is NOT
    // recognized — the file Recovers (this diagnostic's designed detection
    // path) rather than parsing silently wrong or non-deterministically. See
    // `tree-sitter-al`'s CHANGELOG.md ("Not supported") and
    // `test/corpus/preproc_if_elif_whitespace_not_recognized_test.txt`.
    //
    // RESOLVED 2026-07-05 (roadmap-closure plan, Task 4, grammar v3.2.0,
    // submodule `307dc39` -> `14bd55c`) — the scanner-exclusive route this
    // comment's "future fix" pointer called for has landed:
    // `PREPROC_OPEN`/`PREPROC_CLOSE` now consume optional horizontal
    // whitespace between `#` and the keyword as PART OF the token (never
    // skipped, never a bare `\s`/`isspace()` that could span a newline), so a
    // spaced `# if`/`# endif` participates in the scanner's depth counter
    // exactly like the unspaced form. Critically this is NOT the reverted
    // literal-variant approach — `preproc_if`/`preproc_endif` carry ONLY
    // `$.preproc_open`/`$.preproc_close` (every grammar-literal alternative,
    // spaced and unspaced, is REMOVED), so there is exactly one route to
    // either token and nothing for GLR to fork on; the non-determinism class
    // described above does not reproduce (proven via 5x clean-cache
    // `tree-sitter test` runs plus 5x clean-cache re-parses of the exact
    // `preproc_split_if_then_begin_else_shared`-with-spaced-open repro, all
    // identical). `preproc_elif` (no scanner token, no depth interaction)
    // separately gained spaced literal variants mirroring the pre-existing,
    // safe `preproc_else` pattern. CDO has zero spaced-preproc source, so
    // this is inert here (`recovered_files` stays empty, confirmed below) —
    // see `tree-sitter-al`'s CHANGELOG.md `[3.2.0]` entry and
    // `test/corpus/preproc_if_elif_whitespace_tolerance_test.txt` (renamed
    // from `..._not_recognized_test.txt`) for the full fix and fixtures.
    //
    // Both `tools/tree-harness.sh` (CDO source, 551 files; BC.History,
    // 16,898 files — CORRECTED 2026-07-05, Task 5: this checkout's actual
    // BC.History corpus size at the time Task 4 ran the harness, per its own
    // report; "15,358" was a stale figure inherited from this repo's
    // CLAUDE.md, which predates the corpus's growth and was never
    // re-measured before being quoted here) confirm byte-identical parse
    // trees before/after outside the 4 previously-Recovered dependency
    // files — zero blast radius beyond the targeted constructs. Per this
    // diagnostic's own documented
    // invariant (see `recovered_file_paths`'s doc), these 4 files' `#if`
    // union-reads are no longer silently incomplete — any PRIOR
    // absence/`ProvenAbsent`-shaped caveat about `System::Object`/
    // `NAVAppObjectPrerequisites`/`DatabaseLocks` or `Continia System
    // Application::Http` is retired along with the ERROR recovery that
    // motivated it. `genuine_wrong=0` unaffected throughout (measured below).
    //
    // Each distinct file previously appeared TWICE — a PRE-EXISTING,
    // unrelated artifact: `parse_snapshot` parses per `AppUnit`, and
    // `build_program_graph`'s own Step-4 dedup comment already documents
    // that the same app can appear as both a `.alpackages` entry and an
    // implicitly-injected MS-tier dependency in one snapshot (`build.rs`'s
    // "Same app can appear as both a workspace source and an embedded dep"
    // comment) — `recovered_file_paths` runs over the PRE-dedup
    // `ParsedUnit`s. That duplication artifact is now moot (both copies of
    // all 4 files parse clean).
    //
    // RETIGHTENED 2026-07-04 (grammar-defects-and-repin plan, Task 1):
    // 8 -> 0 (empty). Measured EMPTY on the repinned grammar (`6d87aee`) —
    // pinned exact as a threshold alert (any FUTURE non-empty value means
    // either the grammar regressed or a NEW recoverable construct appeared
    // in a CDO dependency; investigate, don't silently re-pin without
    // understanding why).
    let mut recovered = report.recovered_files.clone();
    recovered.sort();
    let expected_recovered: Vec<String> = vec![];
    assert_eq!(
        recovered, expected_recovered,
        "recovered_files moved from the pinned CDO baseline (0 entries, \
         post grammar-defects-and-repin Task 1 — the 2 known \
         tree-sitter-al grammar gaps are fixed at the grammar source; see \
         the comment above) — investigate before re-pinning"
    );

    // ── Self-reported taxonomy'd histogram (print for record) ────────────────
    let h = &report.histogram;
    let ph = &report.primary_histogram;
    eprintln!(
        "\n\
         ═══════════════════════════════════════════════════════════════\n\
         1B.3a Task 3 — Self-reported north-star metric (no L3 oracle)\n\
         ═══════════════════════════════════════════════════════════════\n\
         \n\
         Whole-program (all source-bearing routines + all publishers):\n\
           total={} resolved_source={} resolved_catalog={} resolved_abi_external={}\n\
           conditional_resolved={} honest_dynamic={} honest_empty={} unknown={}\n\
           real_unknown_rate={:.4} ({:.2}%)\n\
         \n\
         Primary-scoped (workspace edges only — mirrors --l3-call-graph-stats-cross-app):\n\
           total={} resolved_source={} resolved_catalog={} resolved_abi_external={}\n\
           conditional_resolved={} honest_dynamic={} honest_empty={} unknown={}\n\
           real_unknown_rate={:.4} ({:.2}%)\n\
         \n\
         Coverage: parsed_obligations={} classified_edges={}\n\
         ABI integrity: abi_routes_total={} abi_mapped={} abi_unmapped={}\n\
         ═══════════════════════════════════════════════════════════════",
        h.total,
        h.resolved_source,
        h.resolved_catalog,
        h.resolved_abi_external,
        h.conditional_resolved,
        h.honest_dynamic,
        h.honest_empty,
        h.unknown,
        h.real_unknown_rate(),
        h.real_unknown_rate() * 100.0,
        ph.total,
        ph.resolved_source,
        ph.resolved_catalog,
        ph.resolved_abi_external,
        ph.conditional_resolved,
        ph.honest_dynamic,
        ph.honest_empty,
        ph.unknown,
        ph.real_unknown_rate(),
        ph.real_unknown_rate() * 100.0,
        report.coverage.parsed_obligations,
        report.coverage.classified_edges,
        report.abi_integrity.abi_routes_total,
        report.abi_integrity.abi_mapped,
        report.abi_integrity.abi_unmapped,
    );

    // ── Stratification invariant: sum(unknownByReason) == unknown count ──────
    // (soundness completion plan v2.1, Task 4 Step 2). The fixture-scoped test
    // `unknown_reason_breakdown_over_real_fixtures_sums_and_spans_reasons`
    // already pins this over 6 curated `ws-*` corpora; this asserts the SAME
    // exhaustive-stratification invariant over the REAL CDO corpus (the
    // production input `aldump --program-call-graph-stats`'s `unknownByReason`
    // serves), so a future decline site that produces `ObligationOutcome::
    // Unknown` without tagging an `Evidence::Unknown(UnknownReason)` route
    // (e.g. an empty-routes non-fanout edge, or a `RouteTarget::Unresolved`
    // route carrying non-`Unknown` evidence) cannot silently understate the
    // breakdown while `unknown` itself climbs undetected. Gated here (not just
    // fixtures) because CDO is the only corpus large/diverse enough to have
    // caught the real +10 Task-1.5 soundness correction in the first place.
    use al_sem::program::resolve::edge::unknown_reason_breakdown;
    let whole_by_reason = unknown_reason_breakdown(report.edges.iter().map(|ce| &ce.edge));
    let whole_by_reason_sum: usize = whole_by_reason.values().sum();
    assert_eq!(
        whole_by_reason_sum, h.unknown,
        "whole-program: sum(unknownByReason)={whole_by_reason_sum} must equal the \
         Unknown-obligation count={} — a mismatch means a decline site is \
         reaching ObligationOutcome::Unknown without tagging an UnknownReason; \
         breakdown={whole_by_reason:?}",
        h.unknown,
    );
    let primary_by_reason = unknown_reason_breakdown(
        report
            .edges
            .iter()
            .filter(|ce| is_primary_scope(ce, report.primary_app_ref))
            .map(|ce| &ce.edge),
    );
    let primary_by_reason_sum: usize = primary_by_reason.values().sum();
    assert_eq!(
        primary_by_reason_sum, ph.unknown,
        "primary-scoped: sum(unknownByReason)={primary_by_reason_sum} must equal \
         the Unknown-obligation count={} — a mismatch means a decline site is \
         reaching ObligationOutcome::Unknown without tagging an UnknownReason; \
         breakdown={primary_by_reason:?}",
        ph.unknown,
    );
    eprintln!(
        "unknownByReason (primary)={primary_by_reason:?}\nunknownByReason (whole)={whole_by_reason:?}"
    );

    // ── Regression guard: primary real_unknown_rate ≤ recorded ceiling ───────
    // Ceiling history: 6.46% (2026-06-30, 1B.3a) → 0.07 (~8% headroom) →
    // 0.030 (beyond-1B.3b Tasks 1-7 + 5.5, recorded 2.81%) → 0.022 (follow-up
    // plan v2.1 Task 3, recorded 1.91%) → 0.021 (follow-up plan v2.1 Task 4,
    // FINAL arc capstone, RE-CONFIRMED 1.91% by an independent re-run on
    // 2026-07-01: primary unknown=346/18104; whole-program 0.81%,
    // unknown=346/42843 — byte-identical, no drift).
    //
    // RAISED 2026-07-02 (uniform-access-and-compound-receiver plan, Task 1):
    // 0.021 → 0.023, measured 0.0225 (2.25%). A SOUNDNESS CORRECTION, not a
    // regression — "soundness beats the metric" (plan v2.1 Task 1's own
    // charter). Task 1 makes `resolve_in_object` PER-CANDIDATE access-aware
    // (it previously did ZERO access filtering): the `ReceiverType::Object`
    // arm (`resolve_member`, gap D) and both `Interface`-impl fan-out
    // delegates (gap F/G) could false-resolve a cross-app `internal` member
    // to `Source`. This was a TRANSIENT over-decline, not the final honest
    // rate — see the next entry.
    //
    // TIGHTENED 2026-07-02 (uniform-access-and-compound-receiver plan,
    // Task 1.5, inserted immediately after Task 1): 0.023 → 0.020, measured
    // 0.0188 (1.88%) — BELOW every prior recorded floor, including the
    // pre-Task-1 1.91%. The combined Task-1+1.5 story: Task 1 correctly
    // fails closed on cross-app `internal` (no exceptions modeled yet); Task
    // 1.5 models AL's `InternalsVisibleTo` friend-app exception, so a
    // cross-app `internal` member is visible when the declaring app's own
    // manifest lists the caller's app as a friend, in ADDITION to the
    // same-app case. Measuring CDO's `InternalNotVisible` bucket proved
    // 100% of it (every site Task 1 declined, cross-app-internal-wise) was
    // CDO calling `internal` members of its CTS-CDN dependency, whose
    // manifest explicitly names CDO a friend — i.e. every one of those calls
    // is AL-LEGAL, and Task 1 alone was an OVER-DECLINE, not a soundness
    // ceiling. Task 1.5 restores them to `Source`, and — as a documented
    // side effect of `object_has_visible_member_candidate` being the SAME
    // shared helper `resolve_bare`'s Step 2 (extension-base) already used
    // pre-Task-1.5 — also sidesteps a known `resolve_bare` reason-overwrite
    // imprecision for a further 7 sites that were mislabeled
    // `ReceiverOutOfClosure` instead of `InternalNotVisible` (see the
    // `unknown` COUNT ceiling below). Net result: the TRUE honest rate for
    // this codebase is LOWER than any prior measurement, because the
    // over-decline was never real soundness — declining an AL-LEGAL friend
    // call was itself the bug.
    //
    // TIGHTENED 2026-07-02 (uniform-access-and-compound-receiver plan, Task
    // 4): 0.020 → 0.019, measured 0.0182 (1.82%). Task 4 resolves the
    // `<Framework>.<Prop|Method()>` compound-receiver subset of the
    // `CompoundReceiver` bucket via a versioned, per-entry-provenanced
    // return-type table (`src/program/resolve/framework_returns.rs`) plus
    // `this.<rest>` self-scoped stripping (`infer_this_member`, object-globals-
    // only per AL's documented `this.` semantics). Measured CDO delta:
    // `CompoundReceiver` 167→156 (-11), every other bucket BYTE-IDENTICAL
    // (`UntrackedReceiver`=91, `OverloadAmbiguous`=56,
    // `BuiltinPrecedenceCollision`=1, `MemberNotFound`=25 — unchanged). All 11
    // newly-`Catalog` sites were EXHAUSTIVELY hand-adjudicated against real
    // CDO source (not a sample — see `.superpowers/sdd/task-4-report.md`): 2
    // `this.DialogWindow.Open`/`.Close()` sites in `Page 6175313 "CDO
    // eDocuments Setup Wizard"` (a genuine object-level `Dialog` global,
    // confirmed by reading its `var` section) resolving to the `Dialog`
    // catalog, and 9 `<JsonToken var>.AsValue().AsText()`/`.AsInteger()`
    // chains across `Codeunit 6175274`/`6175322`/`6175347`, `Page 6175389`
    // (×3), and `Table 6175273` (×3) resolving to the `JsonValue` catalog —
    // every base variable's declared type and every leaf member independently
    // confirmed against the real source. A round-2 self-review during this
    // adjudication caught and fixed a quote-parity bug (a QUOTED field name
    // that merely unquotes to text starting with a framework keyword, e.g.
    // Table "CDO File"'s own `"File Blob"` Blob field colliding with the
    // `File` framework type) BEFORE it could land as a false `Catalog` route —
    // see `infer_receiver_type_for_expr`'s doc and the
    // `quoted_identifier_never_collides_with_framework_keyword_via_recursion`
    // regression test. 0.019 gave a small deterministic margin above the
    // measured 0.0182.
    //
    // TIGHTENED 2026-07-02 (plan v2.1 Task 3): 0.019→0.0182, measured 0.0181.
    // Task 3 resolves the `Var.Method().X()` cross-object call-result chain
    // subset of `CompoundReceiver` via a PURE `resolve_member` type-query on
    // the base's static type (`infer_cross_object_chain_receiver`, `src/
    // program/resolve/receiver.rs`) — `CompoundReceiver` 156→154, every
    // other bucket byte-identical. Both newly-resolved sites EXHAUSTIVELY
    // hand-adjudicated correct against real System Application embedded
    // source (see `.superpowers/sdd/task-3-report.md`): `Codeunit 6175364
    // "CDO Universign E-Seal Service"`'s `ProcessSealResponse`, lines
    // 165/168, `Response.GetContent().AsText()`/`.AsBlob()` where `Response:
    // Codeunit "Http Response Message"` (System App id 2356) declares
    // `GetContent(): Codeunit "Http Content"` (id 2354), which declares
    // `AsText(): Text`/`AsBlob(): Codeunit "Temp Blob"` — confirmed by
    // reading the System Application `.app`'s embedded source directly
    // (`src/Rest Client/src/HttpResponseMessage.Codeunit.al`/
    // `HttpContent.Codeunit.al`). `genuine_wrong` stays 0.
    //
    // TIGHTENED 2026-07-02 (chain-tables plan, Task 4): 0.0182→0.0176,
    // measured 0.0175 (1.75%). Task 4 resolves the Xml framework chain
    // subset of `CompoundReceiver` (new entries in `framework_return_kind`,
    // `src/program/resolve/framework_returns.rs`) plus a NEW,
    // distinct-family typed-return table for the `RecordRef`/`FieldRef`/
    // `KeyRef` handle family (`recordref_family_return_kind`, `src/
    // program/resolve/recordref_returns.rs`) + the matching
    // `ReceiverType::{RecordRef,FieldRef,KeyRef}` arm in
    // `infer_compound_member_receiver` — `CompoundReceiver` 154→144, every
    // other bucket byte-identical. All 10 newly-resolved sites EXHAUSTIVELY
    // hand-adjudicated correct by diffing the full edge dump before/after
    // (not a sample — see `tests/r0-corpus/ws-chain-tables/PROOF.md`): 4
    // `RecordRef.Field(n).<Leaf>()` chains (`Codeunit 6175309 "CDO Legacy
    // eDoc Dispatcher"` line 148, `Codeunit 6175372 "CDO eDocs Send Code
    // Migration"` lines 296-298), 1 `RecordRef.KeyIndex(1).FieldIndex(1)`
    // chain (`Codeunit 6175399 "CDO Data Delete Handler"` line 216), and 5
    // `Node.AsXmlElement().<Add|GetChildNodes>()` chains (`Codeunit 6175324
    // "CDO Xml Node"` lines 89/93/120/131/141). `genuine_wrong` stays 0.
    // This task ALSO found and fixed a genuine PRE-EXISTING fail-open bug in
    // Step 4 (`classify_type_text`'s `starts_with("xml")` catch-all firing
    // on a COMPOUND receiver text, not just a bare identifier — see
    // `receiver.rs`'s Step-4 doc and `PROOF.md`'s "Step-4 bare-identifier
    // guard fix"); the fix is why several `XmlElement.Create(...).
    // AsXmlNode()`-shaped sites do NOT additionally appear in the diffed
    // "newly resolved" set above despite being genuinely exercised by the
    // new `create`/`asxmlnode` table entries — they were ALREADY (wrongly)
    // resolving pre-fix via the bug, and the new validated table entries
    // are what keeps them CORRECTLY resolving post-fix instead of
    // regressing to `Unknown`.
    //
    // TIGHTENED 2026-07-02 (plan v2.1 Task 5, FINAL — arc capstone): 0.0176→
    // 0.01751, RE-CONFIRMED 1.75% (exact raw value 317/18104=0.017509942…) by
    // an independent single-threaded re-run under `ENFORCE_CDO_WS=1`
    // (byte-identical to Task 4's own measurement, no drift — Task 5 makes no
    // resolver changes, only closes the plan). Ceiling pinned to a tiny
    // deterministic margin (0.01751, five decimal places) above the exact
    // measured raw rate — `0.0175` alone would sit BELOW the true value
    // (317/18104 rounds to "1.75%" at 2 decimals but is not exactly 0.0175)
    // and spuriously trip; this is the plan's FINAL floor: 1.82%→1.75% over
    // the whole T1-T4 arc (T1/T2 CDO-neutral soundness+plumbing, T3
    // cross-object chains -2 edges, T4 Xml/RecordRef tables -10 edges). See
    // CHANGELOG.md for the full arc summary.
    //
    // TIGHTENED 2026-07-03 (applicability-param-subtype-recfield plan v2.1,
    // Task 3): 0.01751→0.01293, measured 1.29% (exact raw value
    // 234/18104=0.012925…). Task 3 adds the table-field type index
    // (`FieldNode` on `ObjectNode` + `ResolveIndex::field_in_table`,
    // visibility-scoped, unique-or-decline) and the non-method
    // `Member{object, member}` record-field arm in
    // `infer_compound_member_receiver` (`Rec."Field".X()` and every other
    // `Var."Field".X()` member-qualified record-field chain — the arm keys
    // on the base typing `Record{table: Some}`, not on the receiver being
    // literally `Rec`), plus the EnumType-as-chain-base entry
    // (`enum_chain_return_kind`: `Ordinals()`/`Names()` → `Framework(List)`).
    // Measured CDO delta: `CompoundReceiver` 144→61 (−83), every other
    // bucket BYTE-IDENTICAL (`UntrackedReceiver`=91, `OverloadAmbiguous`=56,
    // `BuiltinPrecedenceCollision`=1, `MemberNotFound`=25). All 83
    // newly-resolved edges EXHAUSTIVELY adjudicated via a full before/after
    // edge-dump diff (83 added / 83 removed — the SAME 83 sites flipping
    // `Unknown(CompoundReceiver)`→`Catalog`, zero collateral changes): 68
    // Blob-catalog edges (every field verified `Blob` in its declaring
    // table's real source — `"File Blob"`/`"File Blob Password Protected"`
    // on Table 6175301, `"Error Message"`/`"E-Mail"` on Table 6175273,
    // `"PDF Sign Certificate"` on 6175283, `"Statement PDF"` on 6175287,
    // `Blob` on 6175296 "CDO Temp Blob", `Template` on 6175330), 7
    // `Enum::asinteger` (5 distinct verified Enum fields on 6175283/6175284),
    // 1 `Enum::ordinals` + 1 `List::count` (the multi-level
    // `Rec."eSeal Service".Ordinals().Count()` on Page 6175455, field
    // verified `Enum CDOESealService` on Table 6175329 "CDO eSeal Setup"),
    // 5 `Media::hasvalue` (`"Media Reference"; Media` on the PLATFORM ABI
    // table "Media Resources", verified from the Microsoft System .app's
    // SymbolReference.json — proves the ABI-tier field index live and
    // classify-strict: a Media field routes to the MEDIA catalog, never
    // falsely Blob), and 1 `Text::contains` (`"Additional Information";
    // Text[250]` on Base App "Error Message", verified from embedded
    // source). `genuine_wrong` stays 0 (companion audit gate).
    //
    // TIGHTENED 2026-07-03 (applicability-param-subtype-recfield plan v2.1,
    // Task 4): 0.01293→0.00995, measured 0.99% (exact raw value
    // 180/18104=0.009943…). Task 4 adds `infer_receiver_type`'s Step 3a —
    // bare implicit-Rec QUOTED-field receivers (`"Field".X()` with NO
    // `Rec.` prefix, inside a Table/TableExtension's own procedure means
    // exactly `Rec."Field".X()`; mirrors `resolve_bare`'s Step-3
    // implicit-Rec precedent, `WithState::NoWithProven`-gated) — plus a
    // Step-2 quote-parity fix (a QUOTED identifier naming a real local var
    // now correctly matches Step 2's var lookup, which always wins over a
    // field — was previously silently unmatched since `VarDecl` names are
    // stored unquoted while the top-level `receiver_lc` retains quotes) and
    // a round-2 soundness correction (`ResolveIndex::table_scope_has_
    // routine`: AL's parens are optional on a zero-argument call, so a bare
    // `Member`/quoted-bare-receiver shape is ambiguous between a field and
    // a parens-less procedure-call chain — a same-named routine anywhere in
    // the visibility-scoped table surface now blocks field-typing, applied
    // to BOTH the new Step 3a arm and Task 3's existing `Rec."Field".X()`
    // arm). Measured CDO delta: `UntrackedReceiver` 91→37 (−54), every
    // other bucket BYTE-IDENTICAL (`CompoundReceiver`=61,
    // `OverloadAmbiguous`=56, `BuiltinPrecedenceCollision`=1,
    // `MemberNotFound`=25 — confirming the routine-shadow guard caused ZERO
    // CDO delta to Task 3's already-landed arm, as predicted). All 54
    // newly-resolved edges EXHAUSTIVELY adjudicated via a full before/after
    // edge-dump diff (54 added / 54 removed, IDENTICAL `(from, span)` key
    // sets — a pure re-resolution of the same 54 sites, zero site
    // additions/removals/collateral changes): 53 Blob-catalog edges
    // (`Blob::createinstream`/`createoutstream`/`hasvalue`, every field
    // spot-verified `Blob` in its declaring table's real source across 11
    // distinct tables — e.g. `"To BLOB"`/`"Cc BLOB"`/`"Bcc BLOB"` on Table
    // 6175273, `"HTML E-Mail Template"`/`"Plain Text E-Mail Template"`/
    // `"Request Page (xml)"` on Table 6175284, `"Request Page (XML)"` on
    // Table 6175282) and 1 `Text::trim` (Table 6175281 "CDO Setup",
    // field(203; "Azure Blob Private Endpoint URL"; Text[250])'s own
    // `OnValidate` trigger — this ONE site was also `genuine_wrong` against
    // the frozen L3 golden until adjudicated: L3's golden misattributes
    // this callee_fp to an UNRELATED procedure `CheckAzureContainerPerCompany`
    // called from a DIFFERENT field's `OnValidate` trigger 8-31 lines away
    // — the SAME L3 line/routine-key misattribution bug already documented
    // for the sibling `CopyStr`/`MaxStrLen` calls on this exact line
    // [`known-genuine-divergences.json` entries 39-40]; `Text::trim`
    // independently verified a genuine catalog member and the field
    // genuinely `Text[250]`, so entry 52 is `l3_error_intrinsic` — see
    // `cdo_l3_semantic_audit_no_fresh_wrong`). `genuine_wrong` stays 0
    // (companion audit gate). Quote-parity's OWN independent CDO yield is
    // MEASURED ZERO (no site in the diff flipped via the var-lookup path
    // alone — every one of the 54 flips is Step 3a's field arm) — framed
    // honestly as defensive/soundness plumbing (like Task 2's ABI fix),
    // proven correct by dedicated unit + r0-corpus fixtures instead.
    //
    // RE-CONFIRMED 2026-07-03 (applicability-param-subtype-recfield plan
    // v2.1, Task 5, FINAL — arc capstone): byte-identical 0.99%
    // (180/18104=0.009943…) by an independent single-threaded re-run under
    // `ENFORCE_CDO_WS=1` (`unknownByReason`={CompoundReceiver: 61,
    // UntrackedReceiver: 37, OverloadAmbiguous: 56,
    // BuiltinPrecedenceCollision: 1, MemberNotFound: 25}, sum==180). Task 5
    // makes no resolver changes — this ceiling is already at the plan's
    // FINAL floor, no further tightening. Net across the whole T1-T4 arc:
    // 1.75% (317) → 0.99% (180), sub-1% for the first time.
    //
    // TIGHTENED 2026-07-03 (dataitem-receivers plan, Task 1): 0.00995→0.00879,
    // measured 0.88% (exact raw value 159/18104=0.008782…). Task 1 adds
    // report-dataitem receivers: `infer_receiver_type`'s new Step 2b
    // (dataitem-NAME receiver lookup), the routine-contextual Report/
    // ReportExtension arm of `infer_implicit_rec` (`RoutineDecl.
    // dataitem_source_table` threaded from the lowerer), the centralized
    // quote-aware `is_atomic_receiver_token` guard (replaces the naive
    // dot-substring check that mislabeled a dot-bearing quoted dataitem name
    // `CompoundReceiver`), and the additive `modify()` lowerer fix + its
    // resolve-time dataset-context fallback. Measured CDO delta:
    // `UntrackedReceiver` 37→17 (−20), `CompoundReceiver` 61→60 (−1), every
    // other bucket BYTE-IDENTICAL (`OverloadAmbiguous`=56,
    // `BuiltinPrecedenceCollision`=1, `MemberNotFound`=25 — confirming this
    // task's changes are surgically scoped to the receiver/dataitem paths,
    // zero collateral effect). `genuine_wrong` stays 0 and `fresh_wrong`
    // stays 149 (companion audit gate, byte-identical) — the drop is smaller
    // than the plan's "~27" dataitem-named-site estimate because several of
    // those sites correctly stay `Unknown` under the fail-closed collision/
    // requestpage-isolation/var-shadow guards, per design (see
    // `.superpowers/sdd/task-1-report.md`).
    //
    // TIGHTENED 2026-07-03 (Task 1 review fix, 5b1bb94): 0.00879→0.008341,
    // measured 0.83% (151/18104=0.0083407 — 6 decimals needed; 0.00834 sits
    // BELOW the raw value). The review-fix restored 8 real
    // quoted-paren Blob-field sites the naive `contains('(')` pre-check had
    // regressed (Catalog→Unknown, masked by bucket netting) + 1 diagnostic
    // relabel; ALL 18,586 routes diffed — exactly 9 changes. NOTE: the
    // paragraph above's "zero collateral" and "correctly stay Unknown under
    // guards" claims were WRONG (see the CHANGELOG correction) — the true
    // accounting: 19 pre-fix dataitem UntrackedReceiver sites, all 29 real
    // dataitem uses resolve, the residual contains zero dataitem sites.
    //
    // UNCHANGED 2026-07-03 (dataitem-depscope-reason-split plan, Task 2 —
    // DIAGNOSTIC-ONLY reason-split): rate/count BYTE-IDENTICAL — Task 2 adds
    // NO new resolution, only new `UnknownReason` diagnostic labels
    // (`ArityMismatch`/`AbiCollapsedOverload`/`AccessFilteredOverload` split
    // out of `OverloadAmbiguous`; `ObjectNotInGraph` split out of
    // `MemberNotFound`; the additive nullable `receiver_tier` field on
    // `MemberNotFound` routes). Measured on CDO (release, single-threaded):
    // `unknownByReason` (both primary AND whole-program) =
    // {CompoundReceiver: 51, UntrackedReceiver: 18, OverloadAmbiguous: 56,
    // BuiltinPrecedenceCollision: 1, MemberNotFound: 25} — ZERO edges moved
    // into any of the four new reason buckets. This is a real, measured
    // finding, not an implementation gap: CDO's current 151-site residual
    // happens to be homogeneous per shape family — every `OverloadAmbiguous`
    // site is a GENUINE >1-visible-candidate ambiguity (none is a bare
    // arity-zero-match, an ABI-collapse-marked survivor [the collapse-marker
    // guard is dormant on CDO by construction — 0 `abi_overload_collapsed`
    // routines], or an access-narrowed-to-1 decline), and every
    // `MemberNotFound` site is a genuine member-absent-on-a-resolved-surface
    // (none is a `resolve_object_run`/`resolve_member`-Object-arm absent-
    // RECEIVER-OBJECT case) — verified via 6 new collision-free unit
    // fixtures in `resolver.rs`/`edge.rs` that DO exercise each new bucket
    // independently of CDO's specific population (`resolve_in_object`'s
    // source-tier same-arity-different-type overloads alias one
    // `RoutineNodeId` — see
    // `resolve_member_object_two_distinct_sig_fp_overloads_access_narrowed_
    // to_one_declines`'s doc for why the `AccessFilteredOverload` fixture
    // manually constructs distinct `sig_fp`s rather than reusing AL source
    // text). The `receiver_tier` diagnostic (new, additive) stratifies the 25
    // `MemberNotFound` sites further — see `aldump --program-call-graph-
    // stats`'s new `unknownReceiverTier` key for the live breakdown; not
    // re-pinned here (diagnostic-only, no gate). `genuine_wrong` stays 0
    // (companion audit gate); every applicability gate green; per-site
    // bijection holds by construction (only `Evidence::Unknown`'s reason
    // payload / `Route::receiver_tier` differ — `Evidence::kind()`, route
    // targets, and every non-`Unknown` bucket are untouched by this task).
    // TIGHTENED 2026-07-03 (sigfp-and-ambiguous-reclassification plan, Task
    // 4 — the metric-definition change): 0.008341 → 0.005248, measured
    // 0.0052475 (95/18104). The full 56-site same-object `OverloadAmbiguous`
    // population (100% of it — Step 0 partitioned all 56 to
    // `resolve_in_object`'s own genuine-ambiguity arm via 3 non-nested,
    // same-object call sites: `resolve_member`'s `Object` receiver 41,
    // `resolve_bare`'s Step 1 own-object 13, `resolve_bare`'s Step 3
    // implicit-Rec single-winning-table-scope-object 2 — ZERO from the
    // cross-object table-scope `Ambiguous` outcome, ZERO from the interface
    // per-implementer `matching!=1` arm, ZERO nested under an interface
    // delegate) reclassifies `ObligationOutcome::AmbiguousResolved` — a
    // proven-closed candidate set, not a resolution gap; see the
    // `unknownByReason` comment below and the charter §5/§8 metric-definition
    // addendum. `ambiguousResolved` 0→56 (both scopes). This is a
    // metric-DEFINITION change, not a resolver soundness fix — THIS IS WHY
    // `Histogram::legacy_unknown_rate_including_ambiguous()` exists: computed
    // on the SAME run, it reads UNCHANGED at 0.008341 (151/18104,
    // byte-identical to this ratchet's pre-Task-4 value), proving the
    // reclassification is a pure relabeling, never a stat-juke. CDO semantic
    // audit (informational, not gated by this ratchet): `genuine_wrong`
    // stays 0 (HARD GATE, unchanged); `fresh_wrong`/`fresh_ahead_dispatch`
    // stays byte-identical at 149/149 and `fresh_missing` stays 3 — ALL 56
    // reclassified sites landed `fresh_extra` (4968→5024, +56) because the
    // frozen L3 golden was EMPTY for every one of them (acceptance-matrix
    // rule 1 — ungated); `matches` 6257→6201 (-56, the mirror movement);
    // digest moved (fresh's projected non-empty targets for these 56 sites
    // are new content). `FRESH_WRONG_CEILING`/`FRESH_MISSING_CEILING` need NO
    // motion (see `cdo_l3_semantic_audit_no_fresh_wrong`, unchanged). Full
    // 160-test CDO-gated harness green (`--test-threads=1`).
    //
    // TIGHTENED 2026-07-03 (argtype-dispatch-and-page-catalog plan, Task 1):
    // 0.005248 → 0.004254, measured 0.43% (77/18104=0.0042532). The Page/Report
    // instance-catalog completion (SetTableView/SetRecord/GetRecord/
    // SetSelectionFilter on Page vars; SetTableView on Report vars; SaveRecord
    // stays CurrPage-only) resolved 18 MemberNotFound sites (25→7); the 7
    // remaining are the eCandidates verified-real absences (honest Unknown).
    //
    // UNCHANGED 2026-07-03 (argtype-dispatch-and-page-catalog plan v2.1, Task
    // 2 — fail-closed arg-type dispatch): byte-identical 0.43%
    // [77/18104=0.0042532] — Task 2 disambiguates `AmbiguousResolved`
    // candidate sets (56→12, later 56→13 after the 2026-07-04 review fix —
    // see the `ambiguous_resolved` ratchet below), a DIFFERENT histogram
    // bucket entirely; it never touches `Unknown`. This ceiling's
    // re-measurement on the identical CDO snapshot is the direct
    // confirmation of that non-interaction (re-confirmed again after the
    // review fix).
    //
    // TIGHTENED 2026-07-04 (receiver-closure-and-arg-increments plan, Task 1
    // — CurrPage UserControl / direct-var ControlAddIn closed-if-known
    // gating): 0.43% → 0.22%, measured 0.22% [40/18104=0.0022095].
    // `CompoundReceiver` 51→14 (-37 — all 37 `CurrPage.<usercontrol>.
    // Method(...)`/`CurrPage.<usercontrol>.SetContent(...)` sites: 30 on
    // source-declared `CDO.Editor`/`CDO.PrintService` resolve via the
    // closed-if-known declared-procedure+arity gate, 7 on platform
    // `WebPageViewer` [no reachable declaration under its bare, unqualified
    // AL reference — see `TRUE_PLATFORM_CONTROL_ADDINS`'s doc] via the
    // `TruePlatform` open-accept). Every one of the 30 declared-addin calls
    // was exhaustively hand-verified against the real `.al` declarations
    // (`ControlAddin/HTMLEditor/HTML Editor.al`,
    // `ControlAddin/PrintService/Print Service.al`) to call a REAL declared
    // procedure at the REAL declared arity — zero Typos, zero arity
    // mismatches, zero events called — so all 37 sites landed `Resolved`
    // (Catalog), none in `MemberNotFound`/`UntrackedReceiver` (both
    // byte-identical: `UntrackedReceiver`=18, `MemberNotFound`=7,
    // `BuiltinPrecedenceCollision`=1 — unchanged). Direct-var `var X:
    // ControlAddIn "Foo"` retrofit: ZERO CDO impact (grepped — no direct-var
    // `ControlAddIn`-typed declaration anywhere in the real CDO corpus), a
    // pure soundness fix validated by unit tests only.
    // `unknownByReason`={CompoundReceiver: 14, UntrackedReceiver: 18,
    // BuiltinPrecedenceCollision: 1, MemberNotFound: 7}, sum==40.
    // `genuine_wrong` stays 0 (companion audit gate); `ambiguous_resolved`
    // stays 13 (untouched bucket).
    //
    // TIGHTENED 2026-07-04 (receiver-closure-and-arg-increments plan, Task 2
    // — parens-optional zero-arg framework members + ErrorInfo.CustomDimensions
    // rows): 0.22% → 0.15%, measured 0.15% [27/18104=0.0014914].
    // `CompoundReceiver` 14→1 (-13): 9 parens-less zero-arg framework-member
    // chain sites (all in `Codeunit 6175322 "CDO Http Management"` —
    // `HttpClient.DefaultRequestHeaders.{Contains,Remove,Add}` ×4 and
    // `Response/ResponseMsg.Content.ReadAs` ×5, the zero-arg getter written
    // WITHOUT parens, legal AL — resolved via the new `zero_arg_aware_lookup`
    // property-row→zero-arg-method-row fallback in `receiver.rs`) + 4
    // `ErrorInfo.CustomDimensions.{ContainsKey,Get}` sites (Codeunits
    // 6175309/6175376 — the new `(ErrorInfo, "customdimensions", true, 0) →
    // Dictionary` row in `framework_returns.rs`; the Dictionary VALUE-type
    // generic is untracked but irrelevant — the leaf `ContainsKey`/`Get`
    // calls are the edges, dispatched by `member_catalog`'s DICTIONARY set).
    // The remaining `CompoundReceiver`=1 is the (D) enum-chain site deferred
    // to Task 4. Every other bucket byte-identical (`UntrackedReceiver`=18,
    // `MemberNotFound`=7, `BuiltinPrecedenceCollision`=1). L3 semantic
    // audit: matches 6158→6145 (-13) / fresh_extra 5069→5082 (+13) — the
    // frozen L3 golden held these 13 leaf sites unresolved too, so fresh is
    // now AHEAD of the retired reference (fresh_extra_verified per-site, see
    // `.superpowers/sdd/task-2-report.md`); fresh_missing stays 1,
    // fresh_wrong stays 149/149 adjudicated, `genuine_wrong` stays 0;
    // `ambiguous_resolved` stays 13 (untouched bucket).
    //
    // TIGHTENED 2026-07-04 (receiver-closure-and-arg-increments plan, Task 3
    // — named-return-value bindings + implicit-self table fields): 0.15% →
    // 0.072%, measured 0.0718% [13/18104=0.0007181]. `UntrackedReceiver`
    // 18→4 (-14): 11 named-return-binding receiver sites (a bare reference
    // to a routine's own `procedure X() Ret: Type` binding, e.g. `Codeunit
    // 6175279 "CDO Module Manager"`'s `GetAvailableModules() ModuleList:
    // List of [Enum "CDO Module Type"]` body's `ModuleList.Add(...)`) + 3
    // implicit-self Table/TableExtension field sites (the SAME `field_in_
    // table` machinery Step 3a already used for quoted names, widened to
    // also accept unquoted bare field names — `table_scope_has_routine`
    // routine-shadow guard unchanged) now resolve via the new `caller_scope_
    // symbol` shared helper's named-return tier / the widened Step 3a arm.
    // The residual 4 are ALL confirmed out-of-scope-for-this-task via
    // `task3_dump_untracked_receiver_sites_on_cdo` (a one-off `#[ignore]`d
    // diagnostic, kept alongside the sibling Task-2 dump) + direct source
    // read: 2 are `Enum::"Type".
    // Ordinals()` static enum-type calls (category F, `Codeunit 6175279`
    // line 28 + `Codeunit 6175317 "CDO Core Event Handler"` line 26), 1 is a
    // bare enum-type-name receiver `"CDO Send on Posting".FromInteger(...)`
    // (category G, `Codeunit 6175288 "CDO Data Upgrade"` line 625) — both
    // categories explicitly deferred to Task 4 (enum-shape receivers) — and
    // 1 is a Page's own `CalcFields`+bare-quoted-field reference to its
    // SourceTable's field (`"View (Blob)".CreateInStream(...)` in
    // `.dependencies/CDO/Page/CDOPageDefaultFilters.Page.al` line 88) —
    // Step 3a is Table/TableExtension-only BY DESIGN (module doc, unchanged
    // by this task); a Page's implicit-Rec bare-field shorthand is a
    // separately-tracked, explicitly out-of-scope gap ("Page-with-
    // SourceTable NOT in scope" — plan v2.1), not a regression. `Compound
    // Receiver` stays 1 (the (D) enum-value-chain site, also deferred to
    // Task 4); `BuiltinPrecedenceCollision` stays 1; `MemberNotFound` stays
    // 7 (the honest eCandidates absences). `unknownByReason`={CompoundReceiver:
    // 1, UntrackedReceiver: 4, BuiltinPrecedenceCollision: 1,
    // MemberNotFound: 7}, sum==13. `genuine_wrong` stays 0
    // (`cdo_genuine_wrong_is_precedence_adjudicated`, unchanged); the L3
    // semantic audit stays clean (`cdo_l3_semantic_audit_no_fresh_wrong`,
    // unchanged — its own ceilings needed no motion, all 14 newly-resolved
    // sites' frozen L3 golden was already empty for them, so fresh is simply
    // further ahead of the retired reference). See `ambiguous_resolved`'s
    // ratchet below for the SIBLING #9/#10 flip (13→11) this same task
    // produced in a disjoint histogram bucket.
    //
    // TIGHTENED 2026-07-04 (receiver-closure-and-arg-increments plan, Task 4
    // — enum-shape receivers + member-field arg dispatch + comment-aware
    // with scan): 0.072% → 0.05%, measured 0.0497% [9/18104=0.0004971].
    // `CompoundReceiver` 1→0 (the (D) enum-value-literal-chain site,
    // `EMailLog."Linked to Table"::Customer.AsInteger()` in `Codeunit
    // 6175403 "CDO Auto-Send Log Runner"` line 38, now resolves via the new
    // `QualifiedEnum` receiver arm's VALUE-instance branch). `UntrackedReceiver`
    // 4→1 (-3): the 2 `Enum::"Type".Ordinals()` static enum-type-reference
    // sites (category F, `Codeunit 6175279 "CDO Module Manager"` line 28 +
    // `Codeunit 6175317 "CDO Core Event Handler"` line 26) resolve via the
    // new `QualifiedEnum` receiver arm's TYPE-static branch
    // (`ReceiverType::EnumTypeStatic`, existence-checked against the real
    // `Enum "CDO Module Type"` object); the bare enum-type-name receiver
    // `"CDO Send on Posting".FromInteger(...)` (category G, `Codeunit
    // 6175288 "CDO Data Upgrade"` line 625) resolves via the new Step 4b
    // bare-enum-type-name gate (unique `Enum "CDO Send on Posting"` match,
    // zero same-name non-Enum objects anywhere, no routine/value-symbol
    // shadow). The residual `UntrackedReceiver`=1 is the HONEST Page-gap
    // (`"View (Blob)".CreateInStream(...)` in
    // `.dependencies/CDO/Page/CDOPageDefaultFilters.Page.al` line 88 —
    // Step 3a is Table/TableExtension-only by design, explicitly out of this
    // plan's scope). `BuiltinPrecedenceCollision` stays 1;
    // `MemberNotFound` stays 7 (the honest eCandidates absences).
    // `unknownByReason`={UntrackedReceiver: 1, BuiltinPrecedenceCollision: 1,
    // MemberNotFound: 7}, sum==9 (`CompoundReceiver` no longer appears —
    // zero sites). `genuine_wrong` stays 0
    // (`cdo_genuine_wrong_is_precedence_adjudicated`, re-run and green); the
    // L3 semantic audit stays clean (`cdo_l3_semantic_audit_no_fresh_wrong`,
    // re-run and green — all 4 newly-resolved sites' frozen L3 golden was
    // already empty for them). See `ambiguous_resolved`'s ratchet below for
    // the 4 SIBLING member-field-arg + with-scan flips (11→7) this same task
    // produced in a disjoint histogram bucket, each individually adjudicated
    // compiler-correct against real CDO field declarations (see that
    // ratchet's comment).
    //
    // TIGHTENED 2026-07-04 (pageext-merge-and-final-residual plan, Task 2):
    // 0.000111 → 0.0, measured 0.0000% (0/18104). Both Task-2 targets closed:
    // (a) the Page implicit-Rec field arm (Step 3a widened Table|TableExtension
    // -> also Page|PageExtension via `resolver::implicit_rec_table_id`) resolved
    // the honest Page-gap `"View (Blob)".CreateInStream(...)` (`.dependencies/
    // CDO/Page/CDOPageDefaultFilters.Page.al:88`) to `Framework(Blob)`'s Catalog
    // route via the page's SourceTable's own field, closing `UntrackedReceiver`
    // 1->0; (b) the GLOBAL compiler-grounded suppression
    // (`resolver::INSTANCE_ONLY_NEVER_BARE` — 19 `PAGE_INSTANCE` names, MS
    // Learn-cited, proven to have NO bare-call form anywhere in AL) removed
    // `run` from the bare-call builtin candidate set, so the real CDO site
    // (`CDOEMailJobs.Page.al:125`'s bare `Run()` vs `CDOEMailJob.Table.al:192`'s
    // `procedure Run()`) resolves to the table's own procedure instead of
    // colliding, closing `BuiltinPrecedenceCollision` 1->0. `unknownByReason`=
    // {} (EMPTY on both primary and whole-program — first time ANY `Unknown`
    // reason bucket exists at all). CDO real-unknown rate reaches the FLOOR:
    // 0.0000% (0/18104). `genuine_wrong` stays 0 (the manifest's stale
    // `.dependencies/CDO/Page/CDOEMailJobs.Page.al:124` entry — pre-dating the
    // collision-detection code, adjudicated when fresh's OWN disposition there
    // WAS `Catalog`/Builtin(`run`) — was CORRECTED IN PLACE to the new
    // `SameAppSourceProcedure` shape, not removed; see
    // `known-genuine-divergences.json`'s entry-9 description and
    // `cdo_l3_semantic_audit_no_fresh_wrong`'s doc comment for the full
    // adjudication). `ambiguousResolved` stays 7 (unaffected — a disjoint
    // histogram bucket).
    //
    // TIGHTENED 2026-07-04 (pageext-merge-and-final-residual plan, Task 1):
    // 0.000498 → 0.000111, measured 0.011% (2/18104=0.0001105). The
    // Page/PageExtension routine merge (the missing resolve_in_table_scope
    // analog) resolved all 7 MemberNotFound sites — CDO's own pageextension
    // declares the members (the "verified-real absences" premise falsified);
    // residual 2 = UntrackedReceiver 1 (the honest Page-gap) +
    // BuiltinPrecedenceCollision 1 (both Task-2 targets).
    //
    // RE-CONFIRMED 2026-07-04 (pageext-merge-and-final-residual plan, Task 4,
    // FINAL — arc capstone): byte-identical 0.0000% (0/18108) by an
    // independent single-threaded re-run under `ENFORCE_CDO_WS=1`.
    // `unknownByReason`={} (empty, both scopes) — unchanged since Task 2.
    // `total` grew 18104→18108 (Task 3's `in_expression` lowerer completeness
    // fix; the numerator stays 0, so the ceiling needs no motion). Task 4
    // makes no resolver changes — this is already THE FLOOR: every
    // statically-resolvable call obligation on CDO resolves. Full honest-
    // taxonomy composition and the companion-gate re-confirmation (`genuine_
    // wrong`=0, `fresh_missing`=0, `fresh_wrong`=149 all `fresh_ahead_
    // dispatch`, applicability 0 violations/non-vacuous, `recoveredFiles`=8)
    // are recorded in CHANGELOG.md's arc-capstone entry and
    // `.superpowers/sdd/task-4-report-close.md`. See the `ambiguous_resolved`
    // ratchet below for the SAME arc's other half — `ambiguousResolved`
    // reaching 0 (Task 3) — confirmed unchanged by this task's re-run.
    let primary_rate = ph.real_unknown_rate();
    assert!(
        primary_rate <= 0.0,
        "primary real_unknown_rate {primary_rate:.6} exceeds ceiling 0.0 \
         (recorded 2026-07-04 post pageext-merge-and-final-residual Task 2: \
         0.0000% [0/18104], the Page implicit-Rec field arm + the GLOBAL \
         compiler-grounded bare-call suppression; was 0.011% \
         [2/18104=0.0001105] post Task 1, the PageExtension routine merge; \
         was \
         0.0497% post receiver-closure-and-arg-increments plan \
         Task 4: 0.0497% [9/18104=0.0004971], enum-shape receivers + \
         member-field arg dispatch + comment-aware with scan; was 0.072% \
         [13/18104=0.0007181] post Task 3, named-return-value bindings + \
         implicit-self table fields; was 0.15% [27/18104=0.0014914] post \
         Task 2, parens-optional zero-arg framework members + \
         ErrorInfo.CustomDimensions rows; was 0.22% \
         [40/18104=0.0022095] post Task 1, CurrPage UserControl / \
         direct-var ControlAddIn closed-if-known gating; was 0.43% \
         [77/18104=0.0042532] post argtype-dispatch-and-page-catalog Task 1, \
         Page/Report instance-catalog completion; was 0.52% post \
         sigfp-and-ambiguous-reclassification plan Task 4 — the \
         metric-definition change: 0.52% [95/18104=0.0052475], \
         the full 56-site same-object OverloadAmbiguous population \
         reclassified AmbiguousResolved; was 0.83% \
         [151/18104=0.008341] post dataitem-receivers Task 1 review fix, \
         0.88% post dataitem-receivers Task 1, 0.99% post \
         applicability-param-subtype-recfield Task 4/5, 1.29% post Task 3, \
         1.75% post plan v2.1 Task 5 FINAL, 1.81% post plan v2.1 Task 3, \
         1.82% post uniform-access-and-compound-receiver Task 4, 1.88% \
         post-Task-1.5, 2.25% post-Task-1-only [a transient over-decline], \
         1.91% pre-Task-1, 2.81% pre-follow-up, 6.46% pre-beyond-1B.3b) — \
         engine regressed; investigate before raising the ceiling. NOTE: use \
         `Histogram::legacy_unknown_rate_including_ambiguous()` (should stay \
         0.008341) to distinguish a genuine regression from a producer that \
         stopped emitting AmbiguousResolved."
    );

    // ── Regression guard: primary real-`unknown` COUNT ceiling ───────────────
    // A ratio ceiling alone can hide a regression if `total` also shifts (a
    // denominator change masking a numerator increase) — pin the absolute
    // `unknown` COUNT too. Re-confirmed 2026-07-01 (follow-up plan v2.1
    // Task 4, arc capstone): primary `unknown`=346, which (empirically, for
    // CDO — not an architectural guarantee) equals whole-program
    // `unknown`=346: every current `Unknown` route happens to originate from
    // a workspace (primary) routine; a dependency-internal `Unknown` would
    // inflate whole-program above primary without this count catching it,
    // hence the separate whole-program ceiling below.
    //
    // RAISED 2026-07-01 (soundness completion plan v2.1, Task 1.5): 346→356
    // (+10), a SOUNDNESS CORRECTION, not a regression — the ratchet must not
    // block a false-`Source`→honest-`Unknown` fix (plan: "soundness beats the
    // metric"). Task 1.5 access-filters `resolve_bare`'s Step 2
    // ("extension base") the same way Task 1 filtered `resolve_in_table_scope`;
    // pre-fix, Step 2 had ZERO access filtering. All +10 were spot-check
    // VERIFIED on CDO: every one is a bare call from
    // `Al/Extensions/eCandidates/CDOConnecteCandidates.PageExt.al`
    // (PageExtension 6175296, app "Continia Document Output") to
    // `internal procedure` `GetIsSingleConnect`/`GeteCandidatesFiltered`/
    // `GetIsVendor`, all declared on the base Page `"CTS-CDN Connect
    // eCandidates"` (id 6252183) in app "Continia Delivery Network"
    // (GUID `0745e76d-...`, a genuinely DIFFERENT app from CDO's
    // `f4b69b55-...`, per `app.json`'s `dependencies`) — confirmed by
    // extracting that dependency's embedded ShowMyCode source directly (the
    // 3 procedures ARE `internal`, cross-app, was false `Source` pre-fix).
    //
    // RAISED AGAIN 2026-07-02 (uniform-access-and-compound-receiver plan,
    // Task 1): 356→407 (+51). At the time this was recorded as "ANOTHER
    // SOUNDNESS CORRECTION" — that framing was INCOMPLETE (corrected below):
    // Task 1 closed a real gap (`resolve_in_object` did zero access
    // filtering for the `ReceiverType::Object` arm and the `Interface`-impl
    // fan-out), but ALL +51 landed in `InternalNotVisible` for calls that
    // turned out to be AL-LEGAL friend calls (see Task 1.5 immediately
    // below) — so the +51 was a TRANSIENT OVER-DECLINE, not a durable
    // soundness floor. Every other `unknownByReason` bucket
    // (`CompoundReceiver`=167, `UntrackedReceiver`=91, `OverloadAmbiguous`=56,
    // `BuiltinPrecedenceCollision`=1, `MemberNotFound`=25) was BYTE-IDENTICAL
    // before/after Task 1, confirming that fix itself was surgically scoped
    // to cross-app `internal` access exclusion with zero collateral effect
    // — the OVER-decline was specifically in the `internal`-access rule
    // being too strict (same-app-only), not in the per-candidate filtering
    // mechanism itself.
    //
    // TIGHTENED 2026-07-02 (uniform-access-and-compound-receiver plan,
    // Task 1.5, inserted immediately after Task 1): 407→348. Task 1.5 models
    // AL's `InternalsVisibleTo` friend-app exception (`internal_visible_
    // across` in `src/program/resolve/resolver.rs`): a cross-app `internal`
    // member is visible when the declaring app's manifest lists the
    // caller's app as a friend, not ONLY same-app. Measured CDO delta:
    // primary/whole `unknown` 407→340 (a drop of 67, not merely the 60
    // `InternalNotVisible` sites originally measured) — the `InternalNotVisible`
    // bucket dropped to EXACTLY 0 (every real CDO cross-app-internal site was
    // friend-authorized, none was a true stranger), AND, as a documented side
    // effect, `ReceiverOutOfClosure` also dropped from 7 to 0 (CORRECTED
    // 2026-07-02, Task 5: this comment previously said "10 to 0" — the
    // arithmetic only works out to the measured 67-site total drop as
    // 60 (`InternalNotVisible`) + 7 (`ReceiverOutOfClosure`); "10" would
    // over-count the drop by 3). Those 7 sites are the SAME bare
    // `GetIsSingleConnect`/`GeteCandidatesFiltered`/
    // `GetIsVendor` calls from `CDOConnecteCandidates.PageExt.al` (extending
    // base Page `"CTS-CDN Connect eCandidates"`, id 6252183, all 3 procedures
    // declared `internal`) that `resolve_bare`'s Step 2 (extension-base) now
    // resolves directly via the SAME `object_has_visible_member_candidate`
    // helper Task 1.5 extended — previously Step 2 declined (access-excluded)
    // and execution fell through to Step 3's implicit-Rec fallback, which
    // ALSO failed and OVERWROTE the more-specific `InternalNotVisible` reason
    // with the generic `ReceiverOutOfClosure` (a known, documented
    // reason-overwrite imprecision — see the plan's "Out of scope" list); now
    // that Step 2 succeeds outright, that overwrite path is never reached for
    // these 7 sites. Spot-check VERIFIED against real CDO/CTS-CDN source
    // (both `.app`s extracted directly): `CTSCDNConnecteCandidates.Page.al`
    // (page 6252183) declares `internal procedure GetIsSingleConnect`/
    // `GeteCandidatesFiltered`/`GetIsVendor`; `IPrePostValidator.Validate`'s
    // TWO implementers (`CTS-CDN Default PrePost Valid.` id 6225611 and
    // `CTS-CDN Legacy PrePost Valid.` id 6225586) both declare `internal
    // procedure Validate`; CTS-CDN's `NavxManifest.xml` `<InternalsVisibleTo>`
    // explicitly lists `<Module Id="f4b69b55-..." Name="Continia Document
    // Output" .../>` — every restored edge targets the CORRECT, real
    // `internal` member its declaring app explicitly authorized CDO to call.
    // `genuine_wrong` stays 0 (companion gate). The combined Task-1+1.5
    // story: Task 1 declines cross-app `internal` (fail closed, no exception
    // modeled yet); Task 1.5 restores the subset that AL itself declares
    // legal via `InternalsVisibleTo`; only a TRUE stranger (no friend
    // declaration in either direction) stays `Unknown`.
    //
    // UNCHANGED 2026-07-02 (uniform-access-and-compound-receiver plan, Task
    // 3): implemented `Func().Method()` compound-receiver resolution (a bare
    // SAME-OBJECT function call's result typed via `resolve_bare` + the new
    // `RoutineNode.return_type`, see `infer_call_result_receiver` in
    // `src/program/resolve/receiver.rs`) — 12 new fixture tests over
    // `ws-compound-call-result` prove it end-to-end (positive Record/
    // Codeunit/Interface-return shapes + 9 fail-closed negatives: overloaded/
    // arity-mismatched/absent prefix, scalar return, Rec/builtin-shadow
    // collision, local-var-shadow, cross-app-ambiguous return, the deferred
    // cross-object-chain and string-literal-dot-arg guards). Measured on CDO:
    // BYTE-IDENTICAL to the pre-Task-3 baseline — `unknown`=340/340 (primary/
    // whole), `unknownByReason`={CompoundReceiver: 167, UntrackedReceiver: 91,
    // OverloadAmbiguous: 56, BuiltinPrecedenceCollision: 1, MemberNotFound:
    // 25} on BOTH sides, zero newly-`Resolved` call-result edges to
    // adjudicate. Root cause (exhaustively grepped, not sampled): CDO's
    // source tree contains ZERO occurrences of a BARE (non-member-qualified)
    // `Func().Method()` chain anywhere. Every real chained-call-result idiom
    // found is `Var.Method().Method()` — a MEMBER-qualified prefix, i.e.
    // Task 4's scope, not Task 3's bare-function shape.
    //
    // TIGHTENED 2026-07-02 (uniform-access-and-compound-receiver plan, Task
    // 4): 348→337, measured 329/329 (primary/whole). Task 4 resolves the
    // `<Framework>.<Prop|Method()>` subset of `CompoundReceiver` via a
    // versioned return-type table (`framework_returns.rs`) + `this.<rest>`
    // stripping — `CompoundReceiver` 167→156, every other bucket
    // byte-identical. All 11 newly-resolved sites EXHAUSTIVELY hand-
    // adjudicated (see the rate-ceiling comment above and
    // `.superpowers/sdd/task-4-report.md`); `genuine_wrong` stays 0. 337
    // gave a small deterministic margin above the measured 329.
    //
    // TIGHTENED 2026-07-02 (plan v2.1 Task 3): 337→330, measured 327/327
    // (primary/whole). See the rate-ceiling comment above for the full
    // adjudication of the 2 newly-resolved sites.
    //
    // TIGHTENED 2026-07-02 (chain-tables plan, Task 4): 330→320, measured
    // 317/317 (primary/whole). See the rate-ceiling comment above for the
    // full exhaustive-diff adjudication of the 10 newly-resolved sites (4
    // `RecordRef.Field(n)` chains, 1 `RecordRef.KeyIndex(1).FieldIndex(1)`
    // chain, 5 Xml `AsXmlElement()`→`Add`/`GetChildNodes` chains) and the
    // Step-4 bare-identifier bug fix this task also made.
    //
    // TIGHTENED 2026-07-02 (plan v2.1 Task 5, FINAL — arc capstone): 320→317,
    // RE-CONFIRMED 317/317 (primary/whole) by an independent single-threaded
    // re-run under `ENFORCE_CDO_WS=1`, byte-identical to Task 4's own
    // measurement (`unknownByReason`={CompoundReceiver: 144,
    // UntrackedReceiver: 91, OverloadAmbiguous: 56,
    // BuiltinPrecedenceCollision: 1, MemberNotFound: 25}, sum==317). Ceiling
    // pinned to the exact measured value — the plan's FINAL floor.
    //
    // TIGHTENED 2026-07-03 (applicability-param-subtype-recfield plan v2.1,
    // Task 3): 317→234, measured 234/234 (primary/whole) — the record-field
    // chain arm (see the rate-ceiling comment above for the full delta and
    // the exhaustive 83-edge adjudication). `unknownByReason`=
    // {CompoundReceiver: 61, UntrackedReceiver: 91, OverloadAmbiguous: 56,
    // BuiltinPrecedenceCollision: 1, MemberNotFound: 25}, sum==234.
    //
    // TIGHTENED 2026-07-03 (applicability-param-subtype-recfield plan v2.1,
    // Task 4): 234→180, measured 180/180 (primary/whole) — bare
    // implicit-Rec quoted-field receivers + the routine-shadow guard (see
    // the rate-ceiling comment above for the full delta and the exhaustive
    // 54-edge adjudication). `unknownByReason`={CompoundReceiver: 61,
    // UntrackedReceiver: 37, OverloadAmbiguous: 56,
    // BuiltinPrecedenceCollision: 1, MemberNotFound: 25}, sum==180.
    //
    // RE-CONFIRMED 2026-07-03 (applicability-param-subtype-recfield plan
    // v2.1, Task 5, FINAL — arc capstone): byte-identical 180/180
    // (primary/whole), no resolver changes this task — already at the
    // plan's FINAL floor.
    //
    // TIGHTENED 2026-07-03 (dataitem-receivers plan, Task 1): 180→159,
    // measured 159/159 (primary/whole) — report-dataitem receivers (see the
    // rate-ceiling comment above for the full delta and adjudication
    // summary). `unknownByReason`={CompoundReceiver: 60, UntrackedReceiver:
    // 17, OverloadAmbiguous: 56, BuiltinPrecedenceCollision: 1,
    // MemberNotFound: 25}, sum==159.
    // TIGHTENED 2026-07-03 (Task 1 review fix, 5b1bb94): 159→151 — the
    // quoted-paren guard fix restored 8 Catalog sites (+1 relabel);
    // `unknownByReason`={CompoundReceiver: 51, UntrackedReceiver: 18,
    // OverloadAmbiguous: 56, BuiltinPrecedenceCollision: 1,
    // MemberNotFound: 25}, sum==151.
    //
    // UNCHANGED 2026-07-03 (dataitem-depscope-reason-split plan, Task 2):
    // count stays 151 — diagnostic-only reason-split, see the rate-ceiling
    // comment above for the full measured (zero-movement) breakdown.
    //
    // TIGHTENED 2026-07-03 (sigfp-and-ambiguous-reclassification plan, Task
    // 4 — the metric-definition change): 151→95, measured 95/95
    // (primary/whole). See the rate-ceiling comment above for the full
    // partition + adjudication (all 56 `OverloadAmbiguous` sites
    // reclassified `AmbiguousResolved`; `unknownByReason`={CompoundReceiver:
    // 51, UntrackedReceiver: 18, BuiltinPrecedenceCollision: 1,
    // MemberNotFound: 25}, sum==95 — `OverloadAmbiguous` no longer appears).
    // TIGHTENED 2026-07-03 (argtype-dispatch-and-page-catalog Task 1): 95→77 —
    // the instance-catalog completion; `unknownByReason`={CompoundReceiver:
    // 51, UntrackedReceiver: 18, BuiltinPrecedenceCollision: 1,
    // MemberNotFound: 7}, sum==77.
    // UNCHANGED 2026-07-03 (argtype-dispatch-and-page-catalog plan v2.1, Task
    // 2): byte-identical 77 — the arg-type dispatch pick moves
    // `AmbiguousResolved` sites to `Resolved`, never touches `Unknown`.
    //
    // TIGHTENED 2026-07-04 (receiver-closure-and-arg-increments plan, Task 1):
    // 77→40 — see the rate-ceiling comment above for the full delta and
    // adjudication (all 37 `CurrPage.<usercontrol>` sites resolved,
    // `unknownByReason`={CompoundReceiver: 14, UntrackedReceiver: 18,
    // BuiltinPrecedenceCollision: 1, MemberNotFound: 7}, sum==40).
    //
    // TIGHTENED 2026-07-04 (receiver-closure-and-arg-increments plan, Task 2):
    // 40→27 — see the rate-ceiling comment above for the full delta and
    // adjudication (9 parens-less framework-member sites + 4
    // ErrorInfo.CustomDimensions sites resolved, `unknownByReason`=
    // {CompoundReceiver: 1, UntrackedReceiver: 18,
    // BuiltinPrecedenceCollision: 1, MemberNotFound: 7}, sum==27).
    //
    // TIGHTENED 2026-07-04 (receiver-closure-and-arg-increments plan, Task 4
    // — enum-shape receivers): 27→9 — see the rate-ceiling comment above for
    // the full delta and adjudication (the (D)/(F)/(G) enum-shape sites all
    // resolve, `unknownByReason`={UntrackedReceiver: 1,
    // BuiltinPrecedenceCollision: 1, MemberNotFound: 7}, sum==9 —
    // `CompoundReceiver` no longer appears).
    // TIGHTENED 2026-07-04 (pageext-merge-and-final-residual, Task 1): 9→2 —
    // the PageExtension routine merge closed all 7 MemberNotFound sites;
    // `unknownByReason`={UntrackedReceiver: 1, BuiltinPrecedenceCollision: 1},
    // sum==2 — `MemberNotFound` no longer appears.
    // TIGHTENED 2026-07-04 (pageext-merge-and-final-residual, Task 2): 2→0 —
    // the Page implicit-Rec field arm + the GLOBAL compiler-grounded bare-call
    // suppression closed BOTH residual sites; `unknownByReason`={} (EMPTY) —
    // the FLOOR. See the rate-ceiling comment above for the full adjudication.
    //
    // RE-CONFIRMED 2026-07-04 (pageext-merge-and-final-residual, Task 4,
    // FINAL — arc capstone): byte-identical 0 (`total`=18108, grown from
    // 18104 by Task 3's `in_expression` fix; numerator unaffected). No
    // resolver changes this task — already the floor.
    assert!(
        ph.unknown == 0,
        "primary unknown count {} exceeds ceiling 0 (recorded 2026-07-04 \
         post pageext-merge-and-final-residual Task 2: 0, the Page \
         implicit-Rec field arm + the GLOBAL bare-call suppression; was 2 \
         post Task 1, the PageExtension routine merge; was 9 post \
         receiver-closure-and-arg-increments plan Task 4: 9, \
         enum-shape receivers + member-field arg dispatch + comment-aware \
         with scan; was 27 post Task 2, parens-optional zero-arg framework \
         members + ErrorInfo.CustomDimensions rows; was 40 post Task 1, \
         CurrPage UserControl / direct-var ControlAddIn closed-if-known \
         gating; was 77 post argtype-dispatch-and-page-catalog Task 1, the \
         Page/Report instance-catalog completion; was 95 post \
         sigfp-and-ambiguous-reclassification plan Task 4 — the \
         metric-definition change: 95, the full 56-site same-object \
         OverloadAmbiguous population reclassified AmbiguousResolved; was \
         151 post dataitem-receivers Task 1 + review fix, 159 post Task 1 \
         alone, 180 post applicability-param-subtype-recfield Task 4/5, 234 \
         post Task 3, 317 post plan v2.1 Task 5 FINAL, 327 post plan v2.1 \
         Task 3, 329 post uniform-access-and-compound-receiver Task 4, 340 \
         post Task 1.5/3, 407 post Task 1 alone [transient over-decline], \
         356 post soundness completion plan v2.1 Task 1.5, 346 post \
         follow-up plan v2.1 Task 4, 508 pre-follow-up) — engine regressed; \
         investigate before raising the ceiling",
        ph.unknown,
    );
    // Defense-in-depth companion: whole-program `unknown` COUNT, in case a
    // future regression lands in a dependency-internal (non-primary) routine
    // — the primary-scoped count above would not catch that on its own.
    // TIGHTENED 2026-07-02 alongside the primary ceiling above (same plan
    // v2.1 Task 3 fix; whole-program `unknown`=327, same value as primary
    // then).
    //
    // TIGHTENED 2026-07-02 (chain-tables plan, Task 4): 330→320, alongside
    // the primary ceiling above; whole-program `unknown`=317, same value as
    // primary today.
    //
    // TIGHTENED 2026-07-02 (plan v2.1 Task 5, FINAL — arc capstone): 320→317,
    // alongside the primary ceiling above; whole-program `unknown`=317,
    // byte-identical re-confirm, same value as primary today.
    //
    // TIGHTENED 2026-07-03 (applicability-param-subtype-recfield plan v2.1,
    // Task 3): 317→234, alongside the primary ceiling above; whole-program
    // `unknown`=234, same value as primary today.
    //
    // TIGHTENED 2026-07-03 (applicability-param-subtype-recfield plan v2.1,
    // Task 4): 234→180, alongside the primary ceiling above; whole-program
    // `unknown`=180, same value as primary today.
    //
    // RE-CONFIRMED 2026-07-03 (applicability-param-subtype-recfield plan
    // v2.1, Task 5, FINAL — arc capstone): byte-identical 180, no resolver
    // changes this task.
    //
    // TIGHTENED 2026-07-03 (dataitem-receivers plan, Task 1): 180→159,
    // alongside the primary ceiling above; whole-program `unknown`=159,
    // same value as primary today.
    //
    // TIGHTENED 2026-07-03 (Task 1 review fix, 5b1bb94): 159→151, alongside
    // the primary ceiling above (quoted-paren restoration).
    //
    // UNCHANGED 2026-07-03 (dataitem-depscope-reason-split plan, Task 2):
    // count stays 151, alongside the primary ceiling above (diagnostic-only
    // reason-split, zero-movement measured breakdown).
    //
    // TIGHTENED 2026-07-03 (sigfp-and-ambiguous-reclassification plan, Task
    // 4 — the metric-definition change): 151→95, alongside the primary
    // ceiling above; whole-program `unknown`=95, same value as primary
    // today (every reclassified site originates in the workspace/primary
    // scope).
    // TIGHTENED 2026-07-03 (argtype-dispatch-and-page-catalog Task 1): 95→77,
    // alongside the primary ceiling above.
    // UNCHANGED 2026-07-03 (argtype-dispatch-and-page-catalog plan v2.1, Task
    // 2): byte-identical 77, alongside the primary ceiling above.
    //
    // TIGHTENED 2026-07-04 (receiver-closure-and-arg-increments plan, Task
    // 1): 77→40, alongside the primary ceiling above; whole-program
    // `unknown`=40, same value as primary today.
    //
    // TIGHTENED 2026-07-04 (receiver-closure-and-arg-increments plan, Task
    // 2): 40→27, alongside the primary ceiling above; whole-program
    // `unknown`=27, same value as primary today.
    //
    // TIGHTENED 2026-07-04 (receiver-closure-and-arg-increments plan, Task
    // 4): 27→9, alongside the primary ceiling above; whole-program
    // `unknown`=9, same value as primary today.
    //
    // TIGHTENED 2026-07-04 (pageext-merge-and-final-residual, Task 1): 9→2,
    // alongside the primary ceiling above; whole-program `unknown`=2, same
    // value as primary today.
    //
    // TIGHTENED 2026-07-04 (pageext-merge-and-final-residual, Task 2): 2→0,
    // alongside the primary ceiling above; whole-program `unknown`=0, the
    // FLOOR (same value as primary today — every closed site originates in
    // the workspace/primary scope).
    //
    // RE-CONFIRMED 2026-07-04 (pageext-merge-and-final-residual, Task 4,
    // FINAL — arc capstone): byte-identical 0 (`total`=43408, grown from
    // 43404 by Task 3's `in_expression` fix). No resolver changes this task.
    assert!(
        h.unknown == 0,
        "whole-program unknown count {} exceeds ceiling 0 (recorded \
         2026-07-04 post pageext-merge-and-final-residual Task 2: 0, the \
         Page implicit-Rec field arm + the GLOBAL bare-call suppression; was \
         2 post Task 1, the PageExtension routine merge; was 9 post \
         receiver-closure-and-arg-increments plan Task 4: 9, \
         enum-shape receivers + member-field arg dispatch + comment-aware \
         with scan; was 27 post Task 2, parens-optional zero-arg framework \
         members + ErrorInfo.CustomDimensions rows; was 40 post Task 1, \
         CurrPage UserControl / direct-var ControlAddIn closed-if-known \
         gating; was 77 post argtype-dispatch-and-page-catalog Task 1; was 95 \
         post sigfp-and-ambiguous-reclassification plan Task 4 — see the \
         primary-scoped ceiling comment above for the full history and \
         adjudication) — engine regressed; investigate before raising the \
         ceiling",
        h.unknown,
    );

    // ── `ambiguous_resolved` ratchet (round-1 addendum: "ambiguous_resolved
    // gets its own histogram count + ratchet") ───────────────────────────────
    // NEW 2026-07-03 (sigfp-and-ambiguous-reclassification plan, Task 4): 56,
    // the full measured CDO same-object same-arity ambiguity population.
    //
    // TIGHTENED 2026-07-03 (argtype-dispatch-and-page-catalog plan v2.1,
    // Task 2 — fail-closed arg-type dispatch): 56 -> 12 (44 flips to
    // `Resolved`). This is now a DOWNWARD-moving count as arg-type dispatch
    // coverage improves — no longer a pure "producer regressed" floor the
    // way it was under Task 4's metric-definition change alone. A further
    // DROP is expected as future increments (Enum::Value / `Rec.Field` /
    // call-result argument typing — see the plan's roadmap) disambiguate
    // more of the residual 12; investigate before RAISING this pin (a rise
    // means the pick regressed — a producer bug silently degrading a
    // formerly-picked site back to ambiguous, or fewer candidates
    // recognized) or before an UNEXPECTED further drop not attributable to a
    // known increment (verify `genuine_wrong` stays 0 first).
    //
    // 44-flip per-site adjudication (see `.superpowers/sdd/task-2-report.md`
    // for the full table; `task2_dump_argtype_dispatch_flips_on_cdo`, an
    // `#[ignore]`d one-off diagnostic below, reproduces the raw dump):
    // overwhelmingly Object/Record EXACT-IDENTITY eliminations (a `Record
    // "CDO Send Code"`-typed local can never bind a `Code[20]` parameter, or
    // vice versa — two disjoint AL runtime representations, the SOUNDEST
    // elimination category in the whole design) — e.g. `CheckAndSetHandled`
    // (2 Record-typed overloads, 5 call sites), `PrintPDFFile` (2 Record-
    // typed overloads, 4 sites), `RunPrePostValidation` (`Record "Sales
    // Header"` vs `Record "Service Header"`, 2 sites), the obsoleted
    // `SendElectronicDocument` shim family (4 arities all funnel through a
    // local `Record "CDO Send Code"` var into the one Record-typed 7-arg
    // overload, elimination against the sibling `Code[20]`-typed overload).
    // A smaller set are cross-family Base eliminations (Text vs Integer/
    // InStream/JsonObject — e.g. `GetJsonAttribute`'s 3-overload family,
    // hand-traced: a `var returnValue: Text` argument eliminates BOTH the
    // `Text`-first-param overload [position 0 mismatch] AND the `var
    // Integer`-typed overload [position 2 mismatch], leaving the
    // `(JsonObject, Text, var Text)` overload as the sole survivor). Every
    // hand-traced site is a case an AL compiler would resolve identically —
    // and NONE touch the "undecided" text-ish/numeric soft-family gate (no
    // Text-vs-Code or Integer-vs-Decimal pick fired on CDO). `genuine_wrong`
    // stays 0 (`cdo_genuine_wrong_is_precedence_adjudicated`, unchanged) and
    // the L3 semantic audit stays clean (`cdo_l3_semantic_audit_no_fresh_
    // wrong`, unchanged) — both HARD gates, both re-run and green on this
    // exact CDO snapshot.
    //
    // TIGHTENED AGAIN 2026-07-04 (Task 2 REVIEW FIX, Finding 1 — `with`-scope
    // gate for bare-identifier arg typing): 12 -> 13 (one of the 44 Task-2
    // picks reverts to `AmbiguousResolved`). INVESTIGATED per this ratchet's
    // own "a rise means the pick regressed — verify before updating"
    // instruction, using a diff of `task2_dump_argtype_dispatch_flips_on_cdo`
    // run before vs. after the review fix (isolated via `git stash` against
    // the SAME warm CDO snapshot, not two separate measurements): exactly ONE
    // site dropped, `UseContiniaAuthorization` (`Codeunit 6175322 "CDO Http
    // Management"`, calling `ContiniaOnlineAuth.Authorize(HttpClient,
    // false)`) — no site was added. Root cause: this routine's own body
    // contains NO real `with` block (the AST-depth signal is correctly 0),
    // but its LEADING COMMENT ("`// Callers running inside a TryFunction MUST
    // share the same "CDO Http Management" instance with an outside-
    // TryFunction warmup call...`") contains a standalone word "with" —
    // `extract::routine_has_with_token`'s raw-text scan is BY DESIGN
    // comment-blind (see that fn's doc: "a `with` token inside a ... comment
    // still trips this ... over-skip is always safe, a false negative is
    // fatal"), so the two with-detection signals DISAGREE and
    // `WithCtx::state()` resolves to `WithState::Unknown` for this ENTIRE
    // routine. This is NOT a new heuristic gap introduced by the review fix —
    // `resolve_bare`'s pre-existing Step 3 (implicit-Rec) has ALWAYS skipped
    // on this exact `Unknown` signal for any bare call in this routine; the
    // review fix's `arg_dispatch::type_one_arg` with-scope gate (module doc)
    // deliberately mirrors that SAME established, already-tested, and already
    // load-bearing precedent for arg typing too — the finding explicitly
    // required "mirror Step 3's existing gate exactly," and this single-site
    // count movement is the direct, EXPECTED, and CORRECT consequence of
    // doing so faithfully rather than adding a narrower (and inconsistent)
    // gate that special-cases this call site. `unknown`/`real_unknown_rate`
    // stay BYTE-IDENTICAL (a disjoint histogram bucket — this call site was
    // already `Source`-resolved via a DIFFERENT route before Task 2 ever
    // existed; it merely stops being a Task-2 CONFIDENT PICK, falling back to
    // the pre-Task-2 honest `AmbiguousResolved` shape for its 2-candidate
    // `Authorize` overload set) — re-confirmed byte-identical below.
    //
    // TIGHTENED 2026-07-04 (receiver-closure-and-arg-increments plan, Task 3
    // — the #9/#10 named-return-binding arg-typing flip): 13 -> 11 (2 of the
    // 43 undecided sites become confident picks, 43->45 — reproduced via a
    // diff of `task2_dump_argtype_dispatch_flips_on_cdo`'s dump before/after
    // this task on the SAME warm CDO snapshot). BOTH flips are in `Page
    // 6175389 "CDO Local Print Service Part"`'s 3-overload `GetJsonAttribute`
    // family (`(Text,Text,var Text)` / `(JsonObject,Text,var Text)` /
    // `(JsonObject,Text,var Integer)`):
    //   - `GetErrorMessageFromResponse(response: Text) ReturnValue: Text`
    //     (line 291) calls `GetJsonAttribute(JObject, 'message', ReturnValue)`
    //     (line 296) — `ReturnValue` is this routine's OWN named-return
    //     binding (type `Text`, never a param/local/global before this task
    //     existed a way to type it at all); typing it now exact-matches the
    //     `(JsonObject, Text, var Text)` overload's 3rd `var` param and
    //     provably eliminates the sibling `var Integer` overload (disjoint
    //     canonical types, C5's ByRef-exact rule) — a clean pick.
    //   - `GetStatusCodeFromResponse(response: Text) ReturnValue: Integer`
    //     (line 299) calls `GetJsonAttribute(JObject, 'statusCode',
    //     ReturnValue)` (line 306) — the SAME shape, `ReturnValue: Integer`
    //     (assigned `:= 200` earlier in the body — a live "used-before- AND
    //     after-assignment" real-world confirmation the engine is correctly
    //     flow-INsensitive) — exact-matches the sibling `var Integer`
    //     overload, eliminating `var Text`.
    // Both are cases a real AL compiler resolves identically (a `var`
    // parameter requires the EXACT declared type; a named return value's
    // type is exactly as concrete as an explicit local's) — adjudicated
    // compiler-correct, not a guess. A THIRD `getjsonattribute`-family entry
    // in the same dump (the `(Text,Text,var Text)` overload's own body
    // calling the `(JsonObject,...)` overload via its OWN `var returnValue:
    // Text` PARAMETER, not a named-return binding) was ALREADY one of the
    // pre-Task-3 43 picks — unaffected by this task, confirmed by its
    // presence being independent of `RoutineDecl.return_name`. `unknown`/
    // `real_unknown_rate` are UNCHANGED by this flip (a disjoint histogram
    // bucket, exactly like the Task-2-review-fix precedent above) —
    // `genuine_wrong` stays 0, the L3 semantic audit stays clean (both
    // re-run and green on this exact CDO snapshot).
    //
    // TIGHTENED 2026-07-04 (receiver-closure-and-arg-increments plan, Task 4
    // — member-field arg dispatch + comment-aware with scan): 11 -> 7 (4 of
    // the 45 undecided sites become confident picks, 45->49 — reproduced via
    // a `git stash`-isolated diff of `task2_dump_argtype_dispatch_flips_on_cdo`'s
    // dump before/after this task on the SAME warm CDO snapshot; ZERO
    // previously-picked site regressed — `comm -23`/`comm -13` on the sorted
    // `caller/span/target` keys confirmed 45 held byte-identical, only 4
    // additions). All 4 individually adjudicated against the real CDO field
    // declarations (source-verified, not merely plausible):
    //   - `CreateReportUsingTemplateLineReports` (`Codeunit 6175296 "CDO
    //     E-Mail Template Management"` line 408) calls
    //     `DOFile.CreateDocumentFromReport(EMailTemplLineReport."Output
    //     Format", EMailTemplLineReport."Report-ID", ReportLayout,
    //     EMailTemplLineReport."Report Layout Name",
    //     EMailTemplLineReport."Report Layout AppID", ...)` against a
    //     2-overload family differing ONLY in their first param
    //     (`DocumentType: Integer` vs `OutputFormat: Enum "CDO Report Output
    //     Format"`) — position 0's field `"Output Format"` IS declared `Enum
    //     "CDO Report Output Format"` on `Table 6175307 "CDO E-Mail Templ.
    //     Line Report"` (source-verified, field 253) — the SAME enum object
    //     the candidate param names, exact Object-identity match, provably
    //     eliminating the Integer-first-param sibling (disjoint canonical
    //     families). Positions 1/3/4 (`"Report-ID"`/`"Report Layout
    //     Name"`/`"Report Layout AppID"`, Integer/Text[250]/Guid
    //     respectively, source-verified identical on both overloads) are
    //     non-discriminating but MUST type too (the module doc's "ALL args
    //     typed" call-level degradation) — the new Member arm supplies
    //     exactly that.
    //   - `CreateReportUsingReportSelection` (same codeunit, line 473) calls
    //     the SAME overload family with `DocumentType: Integer` (a bare local
    //     var — pre-existing typing, unaffected) discriminating at position
    //     0, but positions 3/4
    //     (`EMailTemplateLine."Report Layout Name"`/`"Report Layout
    //     AppID"`, Text[250]/Guid on `Table 6175284 "CDO E-Mail Template
    //     Line"`, source-verified identical to the param types) were
    //     PREVIOUSLY untyped member-field args blocking the whole call —
    //     the new Member arm unblocks it.
    //   - `PrintPDFFile`'s own 1-arg overload body (`Page 6175389 "CDO Local
    //     Print Service Part"` line 225) calls `PrintPDFFile(DOPrintDocument,
    //     DOPrintDocument.Printername)` against a 2-overload family
    //     (`DOFile: Record "CDO File"` vs `DOPrintDocument: Record "CDO
    //     Print document"`, BOTH with `PrinterName: Text` at position 1) —
    //     position 0 (`DOPrintDocument`, a declared Record var — pre-existing
    //     typing) already discriminates via Record-object identity;
    //     `Printername` IS declared `Text[250]` on `CDOPrintdocument.Table.al`
    //     (source-verified, field 14) — matches `PrinterName: Text` on BOTH
    //     overloads (non-discriminating, but the new Member arm's typing of
    //     it is what unblocks the previously-degraded whole call).
    //   - `UseContiniaAuthorization` (`Codeunit 6175322 "CDO Http
    //     Management"` line 32) calls `ContiniaOnlineAuth.Authorize(HttpClient,
    //     false)` — this is NOT a member-field flip at all: `HttpClient` is
    //     the codeunit's OWN object-level global (bare identifier, ordinary
    //     pre-existing typing), and this is the RESTORATION the comment-aware
    //     with-scan fixes — the routine's own leading comment ("...share the
    //     same \"CDO Http Management\" instance WITH an outside-TryFunction
    //     warmup call...") contains a standalone "with" that the OLD
    //     comment-blind scan mis-flagged as `scan_hit=true`, forcing
    //     `WithState::Unknown` for the whole routine and blocking the
    //     bare-identifier arg-typing gate; the new comment-aware scan
    //     correctly excludes it, restoring `WithState::NoWithProven` and
    //     un-blocking the (already-typeable) `HttpClient` argument. This is
    //     the SAME site the Task-2-review-fix ratchet (above) originally
    //     documented losing this exact pick to the comment-blind scan —
    //     Task 4 is its intended restoration, not a new mechanism.
    // Every pick is a case a real AL compiler resolves identically —
    // adjudicated compiler-correct, not a guess. `genuine_wrong` stays 0
    // (`cdo_genuine_wrong_is_precedence_adjudicated`, re-run and green); the
    // L3 semantic audit stays clean (`cdo_l3_semantic_audit_no_fresh_wrong`,
    // re-run and green).
    //
    // TIGHTENED 2026-07-04 (pageext-merge-and-final-residual plan, Task 3 —
    // call-result + boolean argument typing): 7 -> 0, a FULL closure — every
    // remaining candidate flipped, not the "~4-5" the plan's grounding
    // predicted (its "3 sites are SymbolOnly-receiver-blocked" premise was
    // FALSIFIED by measurement: CDO's dependencies ship embedded/ShowMyCode
    // source, so their receivers resolve as ordinary `RouteTarget::Routine`
    // candidates, never `AbiSymbol`/SymbolOnly — the tier gate this task's
    // brief expected to block 3 sites never actually applied to any of
    // them). Located via the new `task3_dump_remaining_ambiguous_resolved_
    // sites_on_cdo` diagnostic (a `git stash`-isolated before/after dump on
    // the SAME warm CDO snapshot — 7 sites before, 0 after) and INDIVIDUALLY
    // hand-adjudicated against the real embedded/workspace source (a mix of
    // direct grep over `.alpackages`-embedded `.app` zip source and a
    // Microsoft ALAppExtensions checkout present alongside this workspace):
    //   - 2× `PrintPDFFile` (`Page 6175389 "CDO Local Print Service Part"`,
    //     lines 238/252): `PrintPDFFile(DOTempBlob.ToBase64String(),
    //     PrinterName)` against the 3-overload family (`Record "CDO
    //     File"`/`Record "CDO Print document"`/`(fileAsBase64: Text;
    //     printerName: Text)`). `"CDO Temp Blob".ToBase64String() Base64String:
    //     Text` (source-verified, `Table 6175296` line 55) — a Member-
    //     function call-result, `arg_dispatch::type_one_arg`'s new `Call{
    //     function: Member{..}}` arm — types `Text`, exact-matching the
    //     `(Text, Text)` overload and PROVABLY eliminating both Record-typed
    //     siblings (disjoint `Object` vs `Base` canonical shapes — no soft
    //     family crosses that boundary). The EXACT PrintPDFFile shape this
    //     task's grounding named.
    //   - `SendElectronicDocument` (`Codeunit 6175296 "CDO E-Mail Template
    //     Management"` line 1357): the caller's own `SendCode` local is
    //     declared `Record "CDO Send Code"` (source-verified — used
    //     elsewhere in the SAME call's arg list via `SendCode.EmailEDoc()`/
    //     `SendCode."Fallback to Email"`, both real members of that table) —
    //     exact-matches the `internal` 7-param overload's `SendCode: Record
    //     "CDO Send Code"` and eliminates the public wrapper's `EDocCode:
    //     Code[20]` (disjoint canonical shapes). This flip needed NO new
    //     Task 3 typing arm at all (position 1 is an ordinary declared-var
    //     Member-field/bare-identifier arg, Task 2's own machinery) — it was
    //     blocked purely by an untyped position ELSEWHERE in the same call
    //     before Task 3's Boolean-Binary arm landed (positions 3/4 are
    //     `and`/`or`/`not`-composed Boolean expressions —
    //     `(TempFile.IsEmpty() and not IsForEmailPreview)` and similar —
    //     which the module doc's call-level degradation rule required ALL
    //     positions typed; Task 3's unconditional Eq/Ne/.../And/Or/Xor/In ->
    //     Boolean rule is what unblocks the whole call).
    //   - `LogMessage` (`Codeunit 6225926 "CTS-SYS Telemetry"`, a Continia
    //     dependency — embedded source extracted directly from `.alpackages/
    //     Continia Software_Continia System Application_29.0.0.100291.app`):
    //     `LogMessage('0086', StrSubstNo(...), 'Platform')` against a
    //     2-overload family differing ONLY at position 2 (`var
    //     CustomDimensions: Codeunit "CTS-SYS Telemetry Dictionary"` vs the
    //     `[Obsolete]`-marked `FunctionalArea: Text`). Position 1
    //     (`StrSubstNo(...)`, a bare GLOBAL BUILTIN call-result) types via
    //     the new passive builtin-return catalog (part c: `strsubstno ->
    //     text`, gated on `resolve_bare` positively reporting
    //     `RouteTarget::Builtin` for this exact name) — non-discriminating
    //     (both candidates take `Text` there). Position 2 (`'Platform'`, a
    //     Text literal — pre-existing typing) exact-matches the
    //     `FunctionalArea: Text` overload and is PROVABLY incompatible with
    //     the `var Codeunit` overload (a Text literal can neither bind a
    //     `var` parameter nor an unrelated Object type). The `#if not
    //     CLEAN29`/`[Obsolete]` wrapping on the picked overload is a
    //     versioning annotation, not a resolution-relevant fact — a real AL
    //     compiler resolves this identically regardless of which `#if`
    //     branch a given build target keeps (the OTHER candidate could
    //     never bind a Text literal at position 2 either way).
    //   - 2× `AddUserMessage` (`Codeunit 6175366 "CDO Email Translate Mgt."`,
    //     `AdjustTone`/`Translate`): `AOAIChatMessages.AddUserMessage(ConfigCU.
    //     BuildUserMessageForWording(HTML, Tone))` /
    //     `AddUserMessage(ConfigCU.BuildUserMessage(HTML, Language))` — both
    //     inner calls are declared `BuildUserMessage(HTML: Text; Language:
    //     Text): Text` / `BuildUserMessageForWording(HTML: Text; Tone: Text):
    //     Text` on the WORKSPACE'S OWN `Codeunit 6175367 "CDO Email
    //     Translate Config"` (source-verified, lines 83/96) — a Member-
    //     function call-result (Step-6-mirroring arm,
    //     `type_call_result_arg_member` — `ConfigCU.Method(...)` is a
    //     `Member{object, member}` call, not a bare `Identifier`) types
    //     `Text`. The OUTER receiver `AOAIChatMessages: Codeunit "AOAI Chat Messages"`
    //     is Microsoft's REAL System Application object (id 7763; found as
    //     genuine embedded source in the `.alpackages`-packaged `.app`, NOT
    //     an ABI/SymbolOnly stub) declaring FOUR `AddUserMessage` overloads;
    //     the arity-1 pair is `(NewMessage: Text)` vs `(AOAIUserMessage:
    //     Codeunit "AOAI User Message")` — `Text` exact-matches the FIRST and
    //     is PROVABLY incompatible with the second (an unrelated Object
    //     type, not the `SecretText` this task's grounding speculated —
    //     ground-truth checked directly against the real MS source, both
    //     independently by a research sub-agent and by direct `.app`-zip
    //     extraction).
    //   - `AddAttachment` (`Codeunit 6175335 "CDO Send Mail Management"`
    //     line 180): `EmailMsg.AddAttachment(DOFile.Filename, '', DOTempBlob.
    //     ToBase64String())` — position 2 is the SAME `"CDO Temp Blob".
    //     ToBase64String(): Text` Member-call-result as the PrintPDFFile
    //     sites — exact-matches Microsoft's real `Email Message`
    //     (`Codeunit 8904`) `AddAttachment(AttachmentName: Text[250];
    //     ContentType: Text[250]; AttachmentBase64: Text)` overload and
    //     eliminates the sibling `AttachmentInStream: InStream` overload
    //     (Text vs InStream — disjoint runtime representations).
    // Every one of the 7 is individually hand-traced against REAL source
    // (never guessed, never merely plausible) — a case a real AL compiler
    // resolves identically. `real_unknown_rate`/`unknown` COUNT are
    // UNCHANGED (a disjoint histogram bucket; both stay 0, the floor).
    // `genuine_wrong` stays 0 (`cdo_genuine_wrong_is_precedence_adjudicated`,
    // re-run and green); the L3 semantic audit stays clean
    // (`cdo_l3_semantic_audit_no_fresh_wrong`, re-run and green). Coverage
    // (`total`) grows 18104->18108 primary / 43404->43408 whole-program — an
    // HONEST, ADDITIVE side effect of a genuine `al-syntax` lowerer
    // completeness fix this task ALSO made (`in_expression` — `X in [..]` —
    // now lowers to `ExprKind::Binary{op: BinaryOp::In, ..}` like the other
    // four comparison/logical `RawKind`s, instead of an unreachable
    // `ExprKind::Unknown` whose registered-but-orphaned children a tree
    // walker starting from the statement root could never reach — e.g.
    // `Session.CurrentClientType() in [ClientType::Web, ..]` in `Codeunit
    // 6175362 "CDO Telemetry"` line 478 previously hid its own nested
    // `CurrentClientType()` call obligation entirely); `coverage.holds`
    // stays `true` (no orphaned obligation), confirmed by the determinism
    // re-run below.
    //
    // RE-CONFIRMED 2026-07-04 (pageext-merge-and-final-residual plan, Task 4,
    // FINAL — arc capstone): byte-identical 0 (both scopes) by an
    // independent single-threaded re-run under `ENFORCE_CDO_WS=1`. Task 4
    // makes no resolver changes. Combined with the `real_unknown_rate`/
    // `unknown`-COUNT ceilings above (also confirmed at 0), the
    // **legacy-inclusive** rate (`(unknown + ambiguous_resolved) / total`,
    // the pre-sigfp-reclassification-plan metric definition --
    // `Histogram::legacy_unknown_rate_including_ambiguous()`) was ALSO exactly
    // 0.0000% on the 2026-07-04 workspace.
    //
    // STALE AS WRITTEN, corrected 2026-09-14 (caught by gpt-5.6-sol in review):
    // that sentence sat directly above a ratchet now pinned at ambiguousResolved
    // = 67, which is arithmetically incompatible with it. On the pinned
    // `bc3ccb18` baseline the legacy-inclusive rate is (0 + 67) / 20897 =
    // **0.3206%**, not 0. `real_unknown_rate` alone is still exactly 0.0000% --
    // the two definitions have diverged again, which is precisely the situation
    // the dual-metric reporting exists to keep visible rather than average away.
    // RE-DERIVED 2026-09-14 for the pinned `bc3ccb18` baseline: 0 -> 67.
    //
    // The 0 was measured on 2026-07-04 against a workspace whose mint SHA
    // (`64643a2f`) exists in no checkout any more, so it could only be inherited,
    // never reproduced. This number is RE-DERIVED, not relaxed:
    //
    // - NOT a regression. 67 measures IDENTICALLY on pre-grammar-bump master
    //   (`4f20f2c5`) built against the v4.0.1 grammar, and identically with and
    //   without the Query-catalog fix. No engine change moved it; it is a property
    //   of this workspace.
    // - NOT holes. `ambiguousResolved` is a CLOSED candidate set — the engine names
    //   every candidate and declines to pick one. `realUnknownRate` is still
    //   0.0000%, so nothing became unresolvable.
    // - The shape: 66 of the 67 carry exactly 2 candidates, 1 carries 3. Every pair
    //   is the same routine name at the same arity, separable only by parameter
    //   TYPES (`createedoclogentries/4`, `logmessage/3`, ...), and several callees
    //   live in dependency ABI — the newer 30.0.0.0 Continia symbols expose overload
    //   pairs the previous baseline's package set did not.
    //
    // Driving this back down is real precision work (arg-type dispatch once took an
    // earlier population to 0) and is tracked separately. It is NOT a reason to keep
    // a ratchet pinned to a number nobody can reproduce.
    const CDO_AMBIGUOUS_RESOLVED: usize = 67;
    assert_eq!(
        ph.ambiguous_resolved, CDO_AMBIGUOUS_RESOLVED,
        "primary ambiguousResolved count {} != the re-derived 2026-09-14 value {}          for the pinned bc3ccb18 baseline (was 0 on the unreproducible 2026-07-04          workspace) — these are closed same-arity overload candidate sets, not          resolution holes; investigate before updating this ratchet",
        ph.ambiguous_resolved, CDO_AMBIGUOUS_RESOLVED,
    );
    assert_eq!(
        h.ambiguous_resolved, CDO_AMBIGUOUS_RESOLVED,
        "whole-program ambiguousResolved count {} != the re-derived 2026-09-14          value {} — same value as primary today, as it has always been (every          ambiguous site is workspace-scoped). See the primary assertion above for          why this moved off 0.",
        h.ambiguous_resolved, CDO_AMBIGUOUS_RESOLVED,
    );

    // ── Determinism ──────────────────────────────────────────────────────────
    let report2 = resolve_full_program(&ws).expect("second run must succeed");
    assert_eq!(
        report.histogram, report2.histogram,
        "resolve_full_program must be deterministic (histogram differs between runs)"
    );
    assert_eq!(
        report.primary_histogram, report2.primary_histogram,
        "resolve_full_program must be deterministic (primary_histogram differs)"
    );
    assert_eq!(
        report.coverage.parsed_obligations, report2.coverage.parsed_obligations,
        "resolve_full_program must be deterministic (parsed_obligations differs)"
    );
}

// ---------------------------------------------------------------------------
// Tests 14–16: 1B.3a Task 4 — L3-validated semantic golden + applicability
// ---------------------------------------------------------------------------

use al_sem::program::resolve::semantic_golden::{
    ANON_GOLDEN_SCHEMA_VERSION, AdjudicatedOverride, GoldenSiteKey, SemanticGolden,
    VERDICT_L3_ERROR_INTRINSIC, adjudicated_overrides_path, cdo_anon_golden_path,
    cdo_event_anon_golden_path, cdo_trigger_anon_golden_path, load_adjudicated_overrides,
    load_anon_event_golden, load_anon_golden, mint_fresh_golden_for_kind, mint_l3_validated_golden,
    run_cdo_event_audit_on, run_cdo_semantic_audit_on, run_cdo_semantic_audit_on_raw,
    run_cdo_trigger_audit_on, run_route_applicability, run_route_applicability_on,
    run_semantic_diff, run_unknown_include_sender_plus1_subscribers_preflight_on,
};

// beyond-1B.3b Task 3: the INDEPENDENT adjudication test's inputs — the
// structural builtin catalog (the SAME data the fresh resolver's `builtin`
// classification itself is built on; using it directly here is sanctioned by
// the brief's "structural catalog" independence criterion) and a hasher for
// the `source_sha256` drift check. Deliberately NOT importing
// `resolve_full_program`/`Edge`/`CanonicalEdge` anywhere near the adjudicator
// — see `cdo_genuine_wrong_is_precedence_adjudicated`'s doc comment.
use al_sem::program::resolve::builtins::is_global_builtin;
use al_sem::program::resolve::member_catalog::{MemberCatalogKind, member_builtin};
use al_sem::program::resolve::receiver::{FrameworkKind, ParsedType, classify_type_text};
use sha2::{Digest, Sha256};

// 1B.3b Task 1 originally defined `cdo_ws_or_enforce()` here, scoped to only
// the three frozen-golden audits it added (Tests 16–18). Task T0.2 routes
// EVERY bare CDO_WS gate in this file (and in `program_graph.rs` /
// `snapshot_robustness.rs`) through the same guard, so the definition moved
// to the shared `tests/common/cdo.rs` (imported near the top of this file as
// `cdo::cdo_ws_or_enforce`) — see that file's doc comment for the contract.

// ── The CDO drift handler ───────────────────────────────────────────────────
//
// The engine's audit helpers DETECT workspace drift and decide nothing about
// it — they call whichever `DriftHandler` they were given
// (`semantic_golden::DriftHandler`). This is that handler, and it is here, in
// test code beside `enforce_audit_ran`'s read of the same variable and the
// eight audit call sites it serves, rather than ambiently inside a library
// function.
//
// It belongs conceptually beside `cdo_ws_or_enforce` in `tests/common/cdo.rs`,
// which gates workspace PRESENCE on the same variable. It is NOT there because
// that file is `#[path]`-included verbatim by three test binaries and only this
// one runs the golden audits, so a handler defined there is dead code in the
// other two. `tests/common/regen.rs:36` carries an `allow(dead_code)`
// attribute for exactly that reason; this file reaches the same end by
// defining the item where it is actually used.
//
// Warning-only is exactly how the previous baseline rotted: the goldens were
// minted from a tree whose SHA later existed in no checkout at all, every run
// printed this message, and nobody acted on it for two months — while the
// audits it guards silently paired ZERO sites and still reported a pass. A
// gated run (`scripts/cdo-gate`) is the one context where an unreproducible
// baseline must stop the build rather than narrate at it. Ungated developer
// runs keep the warning, because drift there is ordinary and claims nothing.
//
// The `assert!` PREEMPTS the `eprintln!`: under enforcement the warning never
// prints, and the panic message carries the text instead.
fn drift_handler(msg: &str) {
    assert!(
        std::env::var("ENFORCE_CDO_WS").as_deref() != Ok("1"),
        "{msg}"
    );
    eprintln!("WARNING: {msg}");
}

/// The sentinel the probes below emit. Distinctive so a captured stderr search
/// cannot match anything else the test binary prints.
const DRIFT_SENTINEL: &str = "issue30-sentinel-drift-text";

/// The prefix the handler's ungated warning carries. A5's contract is the
/// ABSENCE of this under enforcement -- any `WARNING:` line, however formatted,
/// not merely one containing [`DRIFT_SENTINEL`] verbatim.
const WARNING_PREFIX: &str = "WARNING:";

/// Set by a driver on the CHILD process it spawns. The two probes below are
/// entry points for those children, not tests in their own right, so in an
/// ordinary run -- where this is unset -- they do nothing and pass.
///
/// They are gated this way rather than marked as ignored tests because the
/// autonomous flow's `check-diff` rejects that attribute outright, and because
/// an env gate keeps the driver's `--exact` filter as the ONLY selection
/// mechanism. The hazard that creates is real and is guarded: see `PROBE_DONE`.
const PROBE_ENV: &str = "ISSUE30_CHILD_PROBE";

/// Printed by a probe as its LAST statement. Every driver requires it, because
/// libtest exits SUCCESSFULLY both when a filter matches zero tests and when a
/// probe returns early -- so "the child exited 0" alone would let a stale probe
/// name, or a driver that forgot `PROBE_ENV`, silently disable the check it is
/// supposed to perform. See `probe_driver_rejects_a_child_that_ran_nothing`.
const PROBE_DONE: &str = "issue30-probe-completed";

/// Counts the library probe's handler calls, so each library check point it drives is
/// pinned on its own: a check point that silently stopped calling the handler
/// would leave the count one short.
static PROBE_DRIFT_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Child entry point: emit the handler's output so a driver can capture it.
/// A no-op unless this process was spawned as a probe child.
#[test]
fn child_probe_drift_handler() {
    if std::env::var_os(PROBE_ENV).is_none() {
        return;
    }
    drift_handler(DRIFT_SENTINEL);
    println!("{PROBE_DONE}");
}

/// Child entry point: call a PUBLIC library entry point with a handler that
/// cannot fail. Whatever `ENFORCE_CDO_WS` says, the library must return.
#[test]
fn child_probe_library_under_enforcement() {
    use std::sync::atomic::Ordering;

    if std::env::var_os(PROBE_ENV).is_none() {
        return;
    }

    fn record(_msg: &str) {
        PROBE_DRIFT_CALLS.fetch_add(1, Ordering::SeqCst);
    }

    // If the driver did not actually set enforcement, this probe proves nothing
    // about the library ignoring it.
    assert_eq!(
        std::env::var("ENFORCE_CDO_WS").as_deref(),
        Ok("1"),
        "precondition: the driver must set ENFORCE_CDO_WS=1 for this probe to mean anything"
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    let report =
        al_sem::program::resolve::semantic_golden::run_cdo_trigger_audit(tmp.path(), record);

    assert!(
        report.golden_loaded,
        "precondition: the committed trigger golden must load, or the drift check \
         point is never reached"
    );
    assert_eq!(
        PROBE_DRIFT_CALLS.load(Ordering::SeqCst),
        1,
        "the trigger audit's drift check point must have fired against a temp dir"
    );

    // The EVENT audit's check point too. Its path wrapper falls through to the
    // check point on an empty directory, so it can be driven here; the semantic
    // audit's returns before its check point on an empty directory, and driving
    // it on the real `semantic-golden` fixture would merge that fixture's sites
    // into the developer's local `cdo-deanon-map.json`, so it is not driven.
    let report = al_sem::program::resolve::semantic_golden::run_cdo_event_audit(tmp.path(), record);
    assert!(
        report.golden_loaded,
        "precondition: the committed event golden must load, or its drift check \
         point is never reached"
    );
    assert_eq!(
        PROBE_DRIFT_CALLS.load(Ordering::SeqCst),
        2,
        "the event audit's drift check point must have fired as well"
    );
    println!("{PROBE_DONE}");
}

/// Re-run this same test binary for exactly one probe, with `ENFORCE_CDO_WS`
/// set to `enforce`. Returns (succeeded, stdout, stderr) -- kept SEPARATE,
/// because merging them would let a `println!` satisfy an assertion about
/// `eprintln!`.
///
/// A child process is what makes the stderr assertions possible at all --
/// libtest captures stderr but never hands it to the test -- and it is also why
/// nothing here calls the `unsafe` `std::env::set_var`, which would race every
/// other test thread in this process.
///
/// `enforce: None` REMOVES the variable rather than setting it to anything.
/// Unset is the state every developer's shell is in, so it must be tested as
/// itself: an earlier version used `"0"` in its place, which let a handler that
/// panics whenever the variable is ABSENT pass every assertion here. `"0"` and
/// other set-but-not-`"1"` values are tested too, alongside it, not instead of
/// it.
fn run_probe_with_enforcement(probe: &str, enforce: Option<&str>) -> (bool, String, String) {
    let exe = std::env::current_exe().expect("current_exe");
    let mut cmd = std::process::Command::new(exe);
    cmd.args([probe, "--exact", "--nocapture", "--test-threads", "1"])
        .env(PROBE_ENV, "1");
    match enforce {
        Some(v) => cmd.env("ENFORCE_CDO_WS", v),
        None => cmd.env_remove("ENFORCE_CDO_WS"),
    };
    let out = cmd.output().expect("re-exec this test binary");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A3 + A4 + A5, against CAPTURED STDERR rather than an inference about
/// statement order.
///
/// An earlier version asserted only that `catch_unwind` returned `Err` and
/// argued the `eprintln!` after the `assert!` was therefore unreachable. That is
/// true of the body as written and proves nothing about a regression: moving the
/// `eprintln!` ABOVE the `assert!` breaks the suppression while keeping that
/// test green, and deleting the `eprintln!` breaks the ungated warning while
/// keeping it green too. A later version merged stdout and stderr, which let a
/// `println!` masquerade as the stderr warning. A third checked only the exact
/// sentinel-bearing line, which let `eprintln!("WARNING: {msg:?}")` through on a
/// quotation mark. Later versions tested the ungated arm in too few states:
/// `"0"` alone let a handler that panics when the variable is ABSENT pass; unset
/// alone let one that panics whenever it is PRESENT pass; and unset plus `"0"`
/// let one that enforces on anything but `"0"` pass. Every mutation named here
/// fails this version.
#[test]
fn drift_handler_warning_is_emitted_ungated_and_suppressed_gated() {
    let warning = format!("WARNING: {DRIFT_SENTINEL}");

    // A3: ungated, the handler warns ON STDERR and execution continues -- in
    // every ungated state. The rule is `== "1"`, so "ungated" is unset AND every
    // set value other than "1". Each state below catches a different wrong
    // handler: unset catches one that panics when the variable is absent; "0"
    // catches one that checks presence; "true" catches one that enforces on any
    // set value except "0". No finite list is exhaustive, but each of these is a
    // shape a real regression has taken.
    for (label, enforce) in [
        ("unset", None),
        ("\"0\"", Some("0")),
        ("\"true\"", Some("true")),
    ] {
        let (ok, out, err) = run_probe_with_enforcement("child_probe_drift_handler", enforce);
        assert!(
            ok,
            "ungated ({label}), drift must warn and continue:\n{err}"
        );
        assert!(
            out.contains(PROBE_DONE),
            "the ungated ({label}) child never reported completing the probe:\n{out}\n{err}"
        );
        assert!(
            err.contains(&warning),
            "ungated ({label}), the WARNING line must be emitted on STDERR \
             (stdout was:\n{out}\nstderr was:\n{err})"
        );
    }

    // A4 + A5: gated, the run fails carrying the message, and the WARNING line
    // is emitted on NEITHER stream.
    let (ok, out, err) = run_probe_with_enforcement("child_probe_drift_handler", Some("1"));
    assert!(
        !ok,
        "under ENFORCE_CDO_WS=1 drift must FAIL the run, not narrate at it:\n{err}"
    );
    assert!(
        err.contains(DRIFT_SENTINEL) || out.contains(DRIFT_SENTINEL),
        "the failure must carry the drift message:\n{out}\n{err}"
    );
    // A5 is "no `WARNING:` line", not "not this exact string". Asserting only
    // the sentinel-bearing form let a fourth counterexample through:
    // `eprintln!("WARNING: {msg:?}")` prints the message QUOTED, breaking the
    // suppression while dodging an exact-substring check. Reject the PREFIX on
    // both streams, which is the contract as written.
    assert!(
        !err.contains(WARNING_PREFIX) && !out.contains(WARNING_PREFIX),
        "under enforcement NO `WARNING:` line may be emitted, but one was \
         (stdout:\n{out}\nstderr:\n{err})"
    );
    // A differently-worded duplicate -- `eprintln!("warning: {msg}")` -- dodges a
    // prefix check. The one thing every duplicate shares is the message itself,
    // so under enforcement it must appear exactly ONCE across both streams. This
    // checks the COUNT, not where the one copy is: a handler that printed the
    // message and then panicked with some other text would still pass it.
    let occurrences = out.matches(DRIFT_SENTINEL).count() + err.matches(DRIFT_SENTINEL).count();
    assert_eq!(
        occurrences, 1,
        "under enforcement the drift message must appear exactly once across both \
         streams, but it appeared {occurrences} times (stdout:\n{out}\nstderr:\n{err})"
    );
    assert!(
        !out.contains(PROBE_DONE),
        "the gated child must have failed INSIDE the handler, before completing:\n{out}"
    );
}

/// A1 + A2 through PUBLIC API: with `ENFORCE_CDO_WS=1` genuinely ambient, a
/// library audit entry point that finds drift must still RETURN. Before this
/// change it panicked from inside `warn_on_workspace_drift`, which is the whole
/// of issue #30.
#[test]
fn library_ignores_enforcement_env() {
    let (ok, out, err) =
        run_probe_with_enforcement("child_probe_library_under_enforcement", Some("1"));
    assert!(
        ok,
        "the library must not act on ENFORCE_CDO_WS -- only the handler its caller \
         supplied may:\n{err}"
    );
    assert!(
        out.contains(PROBE_DONE),
        "the child exited 0 but never reported completing the probe -- libtest also \
         exits 0 when a filter matches nothing, so this assertion is what separates \
         'the library returned' from 'nothing ran':\n{out}\n{err}"
    );
    // The handler in this probe records and prints nothing, so any `WARNING:`
    // here can only have come from the library. A1 is "the library decides
    // nothing", and printing is deciding: a check point that went back to
    // `eprintln!` would pass every other assertion.
    assert!(
        !err.contains(WARNING_PREFIX) && !out.contains(WARNING_PREFIX),
        "the library itself printed a warning; only the caller's handler may \
         (stdout:\n{out}\nstderr:\n{err})"
    );
    // ...and with any other prefix. The drift message always opens with this
    // text, so the library printing it -- `WARNING:`, `warning:`, or no prefix
    // at all -- shows up here. The match is case-sensitive, so a library that
    // lower-cased the message itself would not be caught.
    assert!(
        !err.contains("CDO workspace drifted") && !out.contains("CDO workspace drifted"),
        "the library itself printed the drift message; only the caller's handler \
         may (stdout:\n{out}\nstderr:\n{err})"
    );
}

/// Pins the hazard the completion marker exists for: libtest reports SUCCESS
/// both for a filter that matches no test and for a probe that returned early
/// because its env gate was unset. Without the marker check, either would turn
/// the drivers above into tests that pass while checking nothing.
#[test]
fn probe_driver_rejects_a_child_that_ran_nothing() {
    // (a) a name that matches no test at all
    let (ok, out, _err) = run_probe_with_enforcement("issue30_no_such_probe_exists", Some("1"));
    assert!(
        ok,
        "libtest is expected to exit 0 on a filter that matches nothing -- that is \
         precisely why exit status alone cannot be trusted here"
    );
    assert!(
        !out.contains(PROBE_DONE),
        "a child that ran nothing must not emit the completion marker"
    );

    // (b) a real probe whose env gate was never set: it runs, returns early,
    //     and libtest reports success. The marker is the only difference.
    let exe = std::env::current_exe().expect("current_exe");
    let ungated = std::process::Command::new(exe)
        .args([
            "child_probe_library_under_enforcement",
            "--exact",
            "--nocapture",
            "--test-threads",
            "1",
        ])
        .env_remove(PROBE_ENV)
        .output()
        .expect("re-exec this test binary");
    assert!(
        ungated.status.success(),
        "a probe with its env gate unset must be a silent no-op, not a failure"
    );
    assert!(
        !String::from_utf8_lossy(&ungated.stdout).contains(PROBE_DONE),
        "a probe that returned early must not emit the completion marker"
    );
}

/// 1B.3b Task 1 ENFORCE_CDO_WS guard (part 2 — the audit-ran-and-checked-something
/// check).
///
/// `golden_loaded` stays ENFORCE-gated here (a missing/invalid committed
/// golden is hard-failed only on the gated/internal runner) — but every
/// caller of this function ALSO asserts `audit.golden_loaded` unconditionally
/// in the test body, so a missing golden fails everywhere regardless.
///
/// **1B.3b Task 1 fix (Fix 3): `checked_sites > 0` is now UNCONDITIONAL** —
/// it no longer requires `ENFORCE_CDO_WS=1`. This function is only ever
/// reached after `cdo_ws_or_enforce()` returned `Some` (i.e. `CDO_WS` is
/// present), so "unconditional" here means "whenever CDO_WS is set", which is
/// exactly the scope Fix 3 closes: before this fix, an orphaned anonymization
/// key (mint and audit hashing under different keys, so every `AnonSiteKey`
/// lookup silently misses) was caught ONLY on the gated/internal runner
/// (`ENFORCE_CDO_WS=1`) — the default local dev path (`CDO_WS` set, `ENFORCE`
/// unset) compared nothing and reported success. A golden that loaded but
/// paired zero sites is exactly as broken as one that failed to load; both
/// must fail loudly on every CDO-capable run, not just the gated one.
fn enforce_audit_ran(golden_loaded: bool, checked_sites: usize) {
    if std::env::var("ENFORCE_CDO_WS").as_deref() == Ok("1") {
        assert!(
            golden_loaded,
            "ENFORCE_CDO_WS=1: committed golden missing/invalid"
        );
    }
    assert!(
        checked_sites > 0,
        "checked_sites==0 (audit ran but paired nothing — floor evaporated, e.g. an \
         anon-key mismatch between mint and audit, a renamed golden file, or CDO_WS \
         pointed at the wrong tree). UNCONDITIONAL: this check does not require \
         ENFORCE_CDO_WS=1 (1B.3b Task 1 fix, Fix 3)."
    );
}

// ---------------------------------------------------------------------------
// Test 14 (fixture): fresh edges match the L3-minted semantic golden
// ---------------------------------------------------------------------------

/// Asserts the in-repo L3-validated semantic golden: no `fresh_wrong` and no
/// `fresh_missing` over the `semantic-golden` fixture workspace.
///
/// The golden file (`tests/goldens/semantic-edges/fixture.json`) is minted from
/// L3 and committed.  Regenerate with `REGEN_TEMP_GOLDENS=1 cargo test
/// fixture_semantic_golden_matches_l3`.
///
/// Critical invariants:
///   - `fresh_wrong == 0`: fresh never resolves to a confidently-wrong target.
///   - `fresh_missing == 0`: fresh matches every L3-resolved site.
#[test]
fn fixture_semantic_golden_matches_l3() {
    let fixture =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/semantic-golden");
    let golden_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/goldens/semantic-edges/fixture.json");

    if regen::regen_mode() {
        let golden = mint_l3_validated_golden(&fixture);
        let mut json =
            serde_json::to_string_pretty(&golden).expect("golden must serialize to JSON");
        // Task T0.6 R1 fix: the committed golden carries a trailing newline (the
        // convention every other regen path in this repo follows); this write
        // previously omitted it, so a byte-identical regen was impossible.
        json.push('\n');
        std::fs::create_dir_all(golden_path.parent().unwrap())
            .expect("create goldens/semantic-edges dir");
        std::fs::write(&golden_path, &json).expect("write fixture golden");
        eprintln!(
            "REGEN: wrote {} site(s) to {}",
            golden.entries.len(),
            golden_path.display()
        );
        return;
    }

    let json = std::fs::read_to_string(&golden_path).unwrap_or_else(|_| {
        panic!(
            "golden file missing: {}\n\
             Run `REGEN_TEMP_GOLDENS=1 cargo test fixture_semantic_golden_matches_l3` \
             to mint it from L3.",
            golden_path.display()
        )
    });
    let golden: SemanticGolden = serde_json::from_str(&json).expect("golden JSON must deserialize");

    let diff = run_semantic_diff(&fixture, &golden);

    assert!(
        diff.fresh_wrong.is_empty(),
        "fresh_wrong MUST be empty — fresh resolved to a confidently-wrong target.\n\
         {} violation(s):\n{:#?}",
        diff.fresh_wrong.len(),
        diff.fresh_wrong,
    );
    assert!(
        diff.fresh_missing.is_empty(),
        "fresh_missing MUST be empty — fresh failed to match an L3-resolved site.\n\
         {} gap(s):\n{:#?}",
        diff.fresh_missing.len(),
        diff.fresh_missing,
    );

    eprintln!(
        "Test 14 — semantic golden: paired={} matches={} fresh_extra={} \
         fresh_novel={} golden_missing={}",
        diff.total_paired,
        diff.matches,
        diff.fresh_extra.len(),
        diff.fresh_novel,
        diff.golden_missing,
    );
}

// ---------------------------------------------------------------------------
// Test 14b (1B.3b Task 1 Step 4): fixture — ImplicitTrigger target-set
// ---------------------------------------------------------------------------

/// Synthetic, L3-INDEPENDENT ImplicitTrigger target-set fixture: asserts the
/// fresh resolver resolves the EXACT trigger set for `tests/fixtures/implicit-trigger`
/// (Table 50500 "ITFTable" + TableExtension 50501 "ITFTableExt" + Codeunit
/// 50502 "ITFCaller" — see the fixture's doc comment for the full layout).
///
/// The golden (`tests/goldens/semantic-edges/implicit-trigger-fixture.json`)
/// is minted from FRESH's own resolution (NOT L3 — see
/// [`mint_fresh_golden_for_kind`]) and committed; this is the
/// "frozen/hand-authored expected output" replacement for the
/// `ImplicitTrigger` dispatch-kind coverage that previously depended on a
/// live L3 comparison. Regenerate with `REGEN_TEMP_GOLDENS=1 cargo test
/// implicit_trigger_fixture_resolves_exact_target_set` — ALWAYS manually
/// inspect the diff before committing a regenerated golden (the point of a
/// frozen baseline is catching an UNINTENDED change, not rubber-stamping
/// whatever fresh currently does).
#[test]
fn implicit_trigger_fixture_resolves_exact_target_set() {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/implicit-trigger");
    let golden_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/goldens/semantic-edges/implicit-trigger-fixture.json");

    if regen::regen_mode() {
        let golden = mint_fresh_golden_for_kind(&fixture, EdgeKind::ImplicitTrigger);
        let json = serde_json::to_string_pretty(&golden).expect("golden must serialize to JSON");
        std::fs::create_dir_all(golden_path.parent().unwrap())
            .expect("create goldens/semantic-edges dir");
        std::fs::write(&golden_path, &json).expect("write implicit-trigger fixture golden");
        eprintln!(
            "REGEN: wrote {} site(s) to {}",
            golden.entries.len(),
            golden_path.display()
        );
        return;
    }

    let json = std::fs::read_to_string(&golden_path).unwrap_or_else(|_| {
        panic!(
            "golden file missing: {}\n\
             Run `REGEN_TEMP_GOLDENS=1 cargo test implicit_trigger_fixture_resolves_exact_target_set` \
             to mint it from fresh — then INSPECT the diff before committing.",
            golden_path.display()
        )
    });
    let golden: SemanticGolden = serde_json::from_str(&json).expect("golden JSON must deserialize");

    assert!(
        !golden.entries.is_empty(),
        "the frozen ImplicitTrigger fixture golden must be non-empty — an empty \
         golden would make this test vacuously pass"
    );

    let diff = run_semantic_diff(&fixture, &golden);

    assert!(
        diff.fresh_wrong.is_empty(),
        "fresh_wrong MUST be empty — fresh's ImplicitTrigger resolution changed \
         vs the frozen baseline.\n{} violation(s):\n{:#?}",
        diff.fresh_wrong.len(),
        diff.fresh_wrong,
    );
    assert!(
        diff.fresh_missing.is_empty(),
        "fresh_missing MUST be empty — fresh failed to resolve a site the frozen \
         baseline expects.\n{} gap(s):\n{:#?}",
        diff.fresh_missing.len(),
        diff.fresh_missing,
    );
    assert_eq!(
        diff.total_paired,
        golden.entries.len(),
        "every frozen-baseline site must pair with a fresh site (golden_missing must be 0): {diff:?}"
    );

    eprintln!(
        "Test 14b — ImplicitTrigger fixture: paired={} matches={} fresh_extra={} \
         fresh_novel={} golden_missing={}",
        diff.total_paired,
        diff.matches,
        diff.fresh_extra.len(),
        diff.fresh_novel,
        diff.golden_missing,
    );
}

// ---------------------------------------------------------------------------
// Test 15 (fixture + CDO env-gated): route-applicability contract
// ---------------------------------------------------------------------------

/// Route-applicability structural contract: `witness_contract_violations == 0`
/// and `abi_unmapped == 0` over both the in-repo fixture and (env-gated) CDO.
///
/// The witness↔evidence contract is: Source→SourceSpan, Abi→AbiSymbol,
/// Catalog→CatalogEntry, Opaque→AbiSymbol, Unknown→None+Unresolved.
/// Any violation is a resolver bug — the invariant must be maintained at all
/// times regardless of resolution precision.
#[test]
fn route_applicability_zero_violations() {
    // ── Fixture (no env needed) ───────────────────────────────────────────────
    let fixture =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/semantic-golden");
    let appl = run_route_applicability(&fixture);
    assert!(
        appl.is_clean(),
        "route-applicability contract violated on fixture: witness_violations={} \
         abi_unmapped={} abi_overload_collapsed_lockstep_violations={} \
         interface_applicability_violations={} \
         instance_builtin_violations={} implicit_trigger_violations={} event_violations={} \
         (is_clean() folds ALL seven — printing only the first two would hide which family \
         actually failed; 1B.3b Task 1 observability gap fix; the lockstep counter added by \
         Task 2's review fix, Finding 2)",
        appl.witness_contract_violations,
        appl.abi_unmapped,
        appl.abi_overload_collapsed_lockstep_violations,
        appl.interface_applicability_violations,
        appl.instance_builtin_violations,
        appl.implicit_trigger_violations,
        appl.event_violations,
    );
    eprintln!(
        "Test 15 (fixture) — applicability: total_routes={} violations=0 abi_unmapped=0",
        appl.total_routes,
    );

    // ── CDO (env-gated) ───────────────────────────────────────────────────────
    let Some(shared) = cdo_shared() else {
        return;
    };

    let appl_cdo = run_route_applicability_on(&shared.ctx, &shared.report);
    assert!(
        appl_cdo.is_clean(),
        "route-applicability contract violated on CDO_WS: witness_violations={} \
         abi_unmapped={} abi_overload_collapsed_lockstep_violations={} \
         interface_applicability_violations={} \
         instance_builtin_violations={} implicit_trigger_violations={} event_violations={} \
         (is_clean() folds ALL seven — printing only the first two would hide which family \
         actually failed; 1B.3b Task 1 observability gap fix; the lockstep counter added by \
         Task 2's review fix, Finding 2)",
        appl_cdo.witness_contract_violations,
        appl_cdo.abi_unmapped,
        appl_cdo.abi_overload_collapsed_lockstep_violations,
        appl_cdo.interface_applicability_violations,
        appl_cdo.instance_builtin_violations,
        appl_cdo.implicit_trigger_violations,
        appl_cdo.event_violations,
    );
    eprintln!(
        "Test 15 (CDO) — applicability: total_routes={} violations=0 abi_unmapped=0",
        appl_cdo.total_routes,
    );
}

// ---------------------------------------------------------------------------
// Test 15c (CDO env-gated, IGNORED — one-off diagnostic dump only, no gate):
// argtype-dispatch-and-page-catalog plan, Task 2 — lists every CDO call site
// where the fail-closed arg-type pick fired (a same-name/same-arity SOURCE
// overload set that Task 2's `resolve_in_object`'s `_` arm picked down to a
// single confident `Source` route) — the per-site adjudication basis for the
// Task 2 CDO gate's `ambiguousResolved` 56->13 movement (43 net flips after
// the 2026-07-04 with-scope review fix reverted one). Mirrors
// the "observed via a one-off `--ignored` dump — see git history — then
// pinned here" convention already used elsewhere in this file (see
// `unknown_reason_breakdown_over_real_fixtures_sums_and_spans_reasons`'s
// doc). Run with `CDO_WS=... cargo test --release --test
// program_resolve_harness task2_dump_argtype_dispatch_flips_on_cdo --
// --ignored --nocapture`.
//
// 2026-07-04 (Task 2 review fix, Finding 1): this SAME dump, diffed
// before/after the with-scope arg-typing gate, is what identified the
// single site (`UseContiniaAuthorization`) behind the `ambiguous_resolved`
// ratchet's 12->13 move — see that ratchet's comment for the full root
// cause. The dump now reports 43, not 44.
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn task2_dump_argtype_dispatch_flips_on_cdo() {
    use al_sem::program::abi_ingest::AbiCache;
    use al_sem::program::build::build_program_graph;
    use al_sem::program::resolve::index::ResolveIndex;
    use al_sem::snapshot::SnapshotBuilder;

    let Some(ws) = cdo_ws_or_enforce() else {
        return;
    };

    let report = resolve_full_program(&ws).expect("resolve_full_program must succeed on CDO_WS");
    let snap = (SnapshotBuilder {
        workspace_root: ws.clone(),
        local_providers: vec![],
    })
    .build()
    .expect("snapshot build must succeed on CDO_WS");
    let cache = AbiCache::new();
    let graph = build_program_graph(&snap, &cache);
    let index = ResolveIndex::build(&graph);

    let mut flips: Vec<String> = Vec::new();
    for ce in &report.edges {
        if ce.edge.kind != EdgeKind::Call {
            continue;
        }
        if ce.edge.shape != DispatchShape::Exact || ce.edge.routes.len() != 1 {
            continue;
        }
        let route = &ce.edge.routes[0];
        if route.evidence != Evidence::Source || !route.conditions.is_empty() {
            continue;
        }
        let RouteTarget::Routine(ref rid) = route.target else {
            continue;
        };
        let candidates = index.routines_in_object(&rid.object, &rid.name_lc);
        let matched: usize = candidates
            .iter()
            .filter(|r| r.params_count == rid.params_count)
            .count();
        if matched > 1 {
            let Witness::SourceSpan { ref file, span } = route.witness else {
                continue;
            };
            let src = std::fs::read_to_string(ws.join(file)).unwrap_or_default();
            let decl_text: String = src
                .get(span.0 as usize..span.1 as usize)
                .unwrap_or("<unreadable>")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            flips.push(format!(
                "caller={} span={}:{}:{} target_object={:?} target={}({} params) picked_decl=[{decl_text}]",
                ce.edge.from.name_lc,
                ce.edge.site.span.unit,
                ce.edge.site.span.start.line,
                ce.edge.site.span.start.col,
                rid.object.key,
                rid.name_lc,
                rid.params_count,
            ));
        }
    }
    flips.sort();
    eprintln!(
        "Task 2 CDO flip dump: {} confident picks over a >1-candidate set",
        flips.len()
    );
    for f in &flips {
        eprintln!("{f}");
    }
}

/// Task 3 (receiver-closure-and-arg-increments plan) diagnostic dump — the
/// SAME "dump before/after via a worktree diff" technique
/// `task2_dump_argtype_dispatch_flips_on_cdo` established: every edge whose
/// `ObligationOutcome` is `Unknown` with `UnknownReason::UntrackedReceiver`,
/// span-located (`caller`/`routine`/`unit:line:col`, 0-based). Used to
/// identify the exact 14 sites Task 3 moved out of this bucket (18→4) and
/// confirm the residual 4 are the F/G enum-shape receivers (deferred to Task
/// 4) + one Page-SourceTable implicit-field gap (explicitly out of Task 3's
/// Table/TableExtension-only scope) — see the `unknown` count ceiling's
/// comment above for the full adjudication. `#[ignore]`d (not part of the
/// gated CDO suite; a one-off investigation aid kept for future tasks that
/// touch this bucket, mirroring the sibling dump's precedent).
#[test]
#[ignore]
fn task3_dump_untracked_receiver_sites_on_cdo() {
    let Some(ws) = cdo_ws_or_enforce() else {
        return;
    };
    let report = resolve_full_program(&ws).expect("resolve_full_program must succeed on CDO_WS");
    use al_sem::program::resolve::edge::{ObligationOutcome, classify_obligation};
    let mut lines: Vec<String> = Vec::new();
    for ce in &report.edges {
        if classify_obligation(&ce.edge) != ObligationOutcome::Unknown {
            continue;
        }
        let reason = ce.edge.routes.iter().find_map(|r| match r.evidence {
            Evidence::Unknown(reason) => Some(reason),
            _ => None,
        });
        if reason == Some(UnknownReason::UntrackedReceiver) {
            lines.push(format!(
                "caller={:?} routine={} span={}:{}:{}",
                ce.edge.from.object.key,
                ce.edge.from.name_lc,
                ce.edge.site.span.unit,
                ce.edge.site.span.start.line,
                ce.edge.site.span.start.col,
            ));
        }
    }
    lines.sort();
    lines.dedup();
    eprintln!("UntrackedReceiver sites: {}", lines.len());
    for l in &lines {
        eprintln!("{l}");
    }
}

/// T3 (pageext-merge-and-final-residual plan) diagnostic dump — the
/// "remaining-ambiguous dump" the plan calls for: every CDO edge classified
/// `ObligationOutcome::AmbiguousResolved`, span-located, with EVERY candidate
/// route's target identity AND the raw call-site source text (read via the
/// edge's own `site.span`, line/col based) so a human can visually re-ground
/// each residual site's argument shapes without re-deriving typed
/// `ArgDispatchInfo` — the SAME "dump then diff before/after" technique
/// `task2_dump_argtype_dispatch_flips_on_cdo`/
/// `task3_dump_untracked_receiver_sites_on_cdo` established. `#[ignore]`d —
/// a one-off investigation aid, not part of the gated CDO suite. Run with
/// `CDO_WS=... cargo test --release --test program_resolve_harness
/// task3_dump_remaining_ambiguous_resolved_sites_on_cdo -- --ignored
/// --nocapture`.
#[test]
#[ignore]
fn task3_dump_remaining_ambiguous_resolved_sites_on_cdo() {
    use al_sem::program::resolve::edge::{ObligationOutcome, classify_obligation};

    let Some(ws) = cdo_ws_or_enforce() else {
        return;
    };
    let report = resolve_full_program(&ws).expect("resolve_full_program must succeed on CDO_WS");

    // Cache file contents so re-reading the same unit for multiple candidate
    // routes/multiple sites doesn't re-hit disk per route.
    let mut file_cache: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let mut read_line = |unit: &str, line: u32| -> String {
        let lines = file_cache.entry(unit.to_string()).or_insert_with(|| {
            std::fs::read_to_string(ws.join(unit))
                .unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect()
        });
        lines
            .get(line as usize)
            .cloned()
            .unwrap_or_else(|| "<unreadable line>".to_string())
    };

    let mut sites: Vec<(String, String)> = Vec::new();
    for ce in &report.edges {
        if classify_obligation(&ce.edge) != ObligationOutcome::AmbiguousResolved {
            continue;
        }
        let span = &ce.edge.site.span;
        let header = format!(
            "caller={:?} routine={} span={}:{}:{}-{}:{}",
            ce.edge.from.object.key,
            ce.edge.from.name_lc,
            span.unit,
            span.start.line,
            span.start.col,
            span.end.line,
            span.end.col,
        );
        let site_text = read_line(&span.unit, span.start.line);
        let mut detail = format!("{header}\n    site: {}", site_text.trim());
        for (i, route) in ce.edge.routes.iter().enumerate() {
            let RouteTarget::Routine(ref rid) = route.target else {
                detail.push_str(&format!(
                    "\n    candidate[{i}]: {:?} (non-Routine)",
                    route.target
                ));
                continue;
            };
            let decl_text = match &route.witness {
                Witness::SourceSpan { file, span } => std::fs::read_to_string(ws.join(file))
                    .ok()
                    .and_then(|src| {
                        src.get(span.0 as usize..span.1 as usize)
                            .map(str::to_string)
                    })
                    .unwrap_or_else(|| "<unreadable decl>".to_string()),
                _ => "<no witness>".to_string(),
            };
            detail.push_str(&format!(
                "\n    candidate[{i}]: object={:?} name={} arity={} decl=[{}]",
                rid.object.key,
                rid.name_lc,
                rid.params_count,
                decl_text.split_whitespace().collect::<Vec<_>>().join(" "),
            ));
        }
        sites.push((header, detail));
    }
    sites.sort();
    sites.dedup();
    eprintln!(
        "AmbiguousResolved sites: {} (ambiguousResolved histogram count={})",
        sites.len(),
        report.primary_histogram.ambiguous_resolved
    );
    for (_, detail) in &sites {
        eprintln!("{detail}\n");
    }
}

// ---------------------------------------------------------------------------
// Test 15b (CDO env-gated): unknown-IncludeSender +1-arity preflight
// (Task 1 round-2 addendum, folded in by Task 2)
// ---------------------------------------------------------------------------

/// Counts event-subscriber routines sitting at EXACTLY `publisher_arity + 1`
/// whose resolved publisher's `IncludeSender` is UNKNOWN — the population
/// Task 1's fail-closed "no `+1` tolerance without positive evidence" policy
/// silently declines to wire. T1's commit narrative reported 100%
/// `IncludeSender` coverage on a real Microsoft Base Application probe (zero
/// unknowns among 13,581 publisher-attribute entries) but never landed a
/// CODE diagnostic to confirm that holds on CDO too — this closes that gap.
/// Asserting `0` here confirms the fail-closed unknown-policy choice is not
/// silently orphaning a legitimate wiring population on a real workspace; a
/// nonzero count would not itself be a resolver bug, but is exactly the
/// signal the round-2 addendum asked to surface for adjudication rather than
/// letting the policy discard it silently (see the diagnostic's own doc).
#[test]
fn cdo_unknown_include_sender_plus1_subscribers_preflight_is_zero() {
    let Some(shared) = cdo_shared() else {
        return;
    };

    let count = run_unknown_include_sender_plus1_subscribers_preflight_on(&shared.ctx);
    assert_eq!(
        count, 0,
        "unknown-IncludeSender publishers with +1-arity subscribers found on \
         CDO — the fail-closed unknown-policy choice may be silently orphaning \
         a legitimate wiring population; adjudicate before treating this as \
         expected (see run_unknown_include_sender_plus1_subscribers_preflight's doc)"
    );
    eprintln!("Test 15b (CDO) — unknown-IncludeSender +1-arity preflight: count=0");
}

// ---------------------------------------------------------------------------
// Test 16 (CDO env-gated; load-frozen since 1B.3b Task 1): L3 semantic
// audit — no fresh_wrong
// ---------------------------------------------------------------------------

/// CDO semantic audit: compares the fresh resolver target-set against the
/// COMMITTED, ANONYMIZED, FROZEN L3 verdict (`cdo-anon.json`) over the real
/// CDO workspace.
///
/// 1B.3b Task 1: this no longer mints L3 live — `run_cdo_semantic_audit`
/// LOADS the committed golden. `audit.genuine_wrong_sites` stays PLAINTEXT
/// `GoldenSiteKey` (fresh's OWN identity, recovered from the anonymized
/// fresh-side comparison via the reverse index — see `anon.rs`'s
/// "re-hash-don't-decrypt" principle), so the manifest set-membership check
/// below is UNCHANGED from 1B.3a.
///
/// Guards: requires `CDO_WS` env var pointing at a real BC workspace.
/// `ENFORCE_CDO_WS=1` (the gated/internal runner) hard-fails if `CDO_WS` is
/// missing, the committed golden failed to load, or the audit paired zero
/// sites (`cdo_ws_or_enforce`/`enforce_audit_ran`).
///
/// ## What this test enforces
///
/// The `fresh_wrong` bucket (sites where both L3 and fresh resolved but to
/// different targets) is split into two adjudicated classes:
///
/// - **`fresh_ahead_dispatch`** (ALLOWED): fresh's targets REFINE L3's —
///   either L3's target is a subset of fresh's, or L3 resolved to an interface
///   and fresh resolved to concrete implementors. Phase-4 Interface/Polymorphic
///   fan-out. Not a bug.
///
/// - **`genuine_wrong`** (HARD GATE): fresh confidently resolved to a target
///   DISJOINT from L3's — a different object or procedure with no refinement
///   relationship. This is a real resolver bug. Every `genuine_wrong` site's
///   `(unit, line, callee_fp)` key MUST be present in the committed manifest
///   `tests/goldens/semantic-edges/known-genuine-divergences.json`. A site NOT
///   in the manifest = a NEW confidently-wrong edge → test FAILS immediately
///   with the offending site(s) printed. A count-only gate is insufficient: a
///   swap (fix one adjudicated site + introduce one new disjoint site) holds
///   the count constant and passes silently, defeating the gate entirely.
///
/// `fresh_missing` (L3 resolved but fresh didn't) is informational — tracked
/// over time. The known deferred buckets total 163; anything beyond is a new gap.
#[test]
fn cdo_l3_semantic_audit_no_fresh_wrong() {
    let Some(ws) = cdo_ws_or_enforce() else {
        return;
    };
    let Some(shared) = cdo_shared() else {
        return;
    };

    let audit = run_cdo_semantic_audit_on(&shared.ctx, &shared.report, &ws, drift_handler);
    enforce_audit_ran(audit.golden_loaded, audit.paired);
    assert!(
        audit.golden_loaded,
        "cdo-anon.json missing/invalid at {}; run the dev-mint tool \
         (`cargo run --bin mint-goldens`) with CDO_WS set",
        cdo_anon_golden_path().display(),
    );

    eprintln!(
        "\n\
         ═══════════════════════════════════════════════════════════════\n\
         1B.3a Task 4 — CDO L3 semantic audit\n\
         ═══════════════════════════════════════════════════════════════\n\
         l3_total={} fresh_total={}\n\
         paired={} matches={} ({}%)\n\
         fresh_wrong={} [fresh_ahead_dispatch={} genuine_wrong={}]\n\
         fresh_missing={} fresh_extra={}\n\
         fresh_novel={} golden_missing={}\n\
         digest={}\n\
         ═══════════════════════════════════════════════════════════════",
        audit.l3_total,
        audit.fresh_total,
        audit.paired,
        audit
            .paired
            .saturating_sub(audit.fresh_wrong_count)
            .saturating_sub(audit.fresh_missing_count)
            .saturating_sub(audit.fresh_extra_count),
        audit
            .paired
            .saturating_sub(audit.fresh_wrong_count)
            .saturating_sub(audit.fresh_missing_count)
            .saturating_sub(audit.fresh_extra_count)
            .saturating_mul(100)
            .checked_div(audit.paired)
            .unwrap_or(0),
        audit.fresh_wrong_count,
        audit.fresh_ahead_dispatch_count,
        audit.genuine_wrong_count,
        audit.fresh_missing_count,
        audit.fresh_extra_count,
        audit.fresh_novel,
        audit.golden_missing,
        audit.digest,
    );

    // ── HARD GATE: genuine_wrong SET MEMBERSHIP against adjudicated manifest ──
    // genuine_wrong sites are real resolver bugs (Cat-D different-object or
    // wrong overload pick). They are enumerated in the committed manifest:
    //   tests/goldens/semantic-edges/known-genuine-divergences.json
    // Every genuine_wrong site's (unit, line, callee_fp) key MUST be in the
    // manifest set. A COUNT-only gate is insufficient: a swap (fix one adjudicated
    // site while introducing one new disjoint site) keeps the count at 42 and
    // passes silently — hiding the new bug. Set membership catches swaps.
    let manifest_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/goldens/semantic-edges/known-genuine-divergences.json");
    let manifest_json = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|_| panic!("manifest missing: {}", manifest_path.display()));
    let manifest: serde_json::Value =
        serde_json::from_str(&manifest_json).expect("manifest must be valid JSON");
    let manifest_entries = manifest
        .get("entries")
        .and_then(|e| e.as_array())
        .expect("manifest must have 'entries' array");
    let manifest_keys: std::collections::HashSet<(String, u32, u64)> = manifest_entries
        .iter()
        .map(|entry| {
            let unit = entry["unit"]
                .as_str()
                .expect("manifest entry missing 'unit'")
                .to_string();
            let line = entry["line"]
                .as_u64()
                .expect("manifest entry missing 'line'") as u32;
            let callee_fp = entry["callee_fp"]
                .as_u64()
                .expect("manifest entry missing 'callee_fp'");
            (unit, line, callee_fp)
        })
        .collect();

    // SET MEMBERSHIP: every genuine_wrong site must be in the manifest.
    let new_genuine_wrong: Vec<&GoldenSiteKey> = audit
        .genuine_wrong_sites
        .iter()
        .filter(|site| !manifest_keys.contains(&(site.unit.clone(), site.line, site.callee_fp)))
        .collect();
    assert!(
        new_genuine_wrong.is_empty(),
        "genuine_wrong gate FAILED: {} site(s) NOT in the adjudicated manifest \
         (tests/goldens/semantic-edges/known-genuine-divergences.json).\n\
         A NEW confidently-wrong edge appeared — investigate and either fix the \
         resolver or extend the manifest with a root-cause explanation.\n\
         Offending sites:\n{:#?}",
        new_genuine_wrong.len(),
        new_genuine_wrong,
    );
    // Secondary sanity: count must not exceed the manifest (a decrease is a win).
    assert!(
        audit.genuine_wrong_count <= manifest_keys.len(),
        "genuine_wrong_count {} exceeds manifest size {} — all sites passed \
         membership but count exceeds manifest length (logic error?)",
        audit.genuine_wrong_count,
        manifest_keys.len(),
    );
    // beyond-1B.3b Task 3 (grown to 52 entries by record-field chains plan
    // Task 4, then to 54 by argtype-dispatch-and-page-catalog plan Task 1's
    // 2 `PageInstanceVar` duplicate-trigger-name sites): ALL manifest
    // entries are now adjudicated `l3_error_intrinsic` and overlaid
    // (`run_cdo_semantic_audit` applies `adjudicated-overrides.json` in-memory
    // before diffing) — fresh is compared against the ADJUDICATED target for
    // these sites, which fresh matches by construction (that agreement is
    // what the independent adjudication in
    // `cdo_genuine_wrong_is_precedence_adjudicated` confirms). So
    // `genuine_wrong_count` must now be EXACTLY 0: a nonzero count means
    // either the overlay failed to apply (a wiring bug) or a genuinely NEW
    // disjoint divergence appeared that is not one of the 54 known/adjudicated
    // sites — both are real bugs, not "still-acceptable known wrongness". The
    // manifest/set-membership checks above stay as defense-in-depth for that
    // second case.
    //
    // pageext-merge-and-final-residual plan, Task 2: entry index 9 (site
    // `.dependencies/CDO/Page/CDOEMailJobs.Page.al:124`, `callee_fp
    // 2876017921644654500`, bare `Run()`) was CORRECTED IN PLACE (the entry
    // COUNT stays 54) from its stale `builtin-catalog-fp-collision`
    // adjudication (`receiver_kind: Global`, `catalog_key: run` — adjudicated
    // when fresh's own disposition for this site WAS `Catalog`/`Builtin`,
    // because `resolve_bare`'s Step 3 either predated this site or its
    // PROBE-THEN-DECIDE guard already fired to `Unknown` at the time, either
    // way never reaching `genuine_wrong`) to the NEW `SameAppSourceProcedure`
    // shape: Task 2's grounded suppression makes fresh now resolve this bare
    // `Run()` to the SourceTable's OWN `procedure Run()`
    // (`Table 6175280 "CDO E-Mail Job"`), so the entry stays
    // `l3_error_intrinsic`, just with a corrected override target.
    //
    // CORRECTED 2026-07-04 (pageext-merge-and-final-residual plan, Task 2
    // REVIEW fix): the sentence this replaces claimed L3's frozen golden
    // "still pairs this site with an unrelated/no target" — FALSE. An
    // independent HMAC-based re-verification of the RAW, un-overridden L3
    // golden (`cdo-anon.json`) for this exact site established that its
    // recorded target has ALWAYS been `Table 6175280 "CDO E-Mail Job".Run` —
    // IDENTICAL to fresh's post-Task-2 resolution. L3 was never wrong here;
    // the actual (now-closed) defect was fresh's OWN pre-Task-2 answer
    // (`Catalog`/`Builtin(run)`), which Task 2 fixed for an unrelated reason
    // (the `INSTANCE_ONLY_NEVER_BARE` grounding). This site is consequently
    // now a plain MATCH between fresh and the raw L3 golden — the
    // `adjudicated-overrides.json` entry for this key is a documented NO-OP
    // (it writes back the exact target the raw golden already supplies).
    // Retained rather than deleted (see the manifest's own description field
    // for the full rationale); entry count is unaffected either way.
    assert_eq!(
        audit.genuine_wrong_count, 0,
        "genuine_wrong_count={} (expected 0): all 54 known-genuine-divergences.json sites \
         are adjudicated l3_error_intrinsic and should have been overlaid to match fresh \
         exactly (see adjudicated-overrides.json / apply_adjudicated_overrides). A nonzero \
         count means either the overlay didn't apply (check for an \
         'Adjudication overlay: N/54' log line above — N should be 54) or a genuinely NEW \
         divergence appeared beyond the 54 adjudicated ones.",
        audit.genuine_wrong_count,
    );

    // fresh_ahead_dispatch is always ALLOWED (printed above for visibility).

    // ── COMPLETENESS FLOOR (1B.3b whole-branch fix): re-instate the deleted
    // `regression_unexplained == 0` leg as a pinned CEILING on `fresh_missing`.
    //
    // `fresh_missing` (L3 resolved a target, fresh emitted nothing) was
    // previously informational-only: a dropped trigger/event/member target at
    // CDO scale could increment this counter silently and the test would
    // still pass. History: 191 (1B.3b, `page_rec=115 + codeunit_implicit_rec=24
    // + trigger=38 + other=14`, CHANGELOG.md 1B.3a Task 4) → beyond-1B.3b
    // Tasks 5–7 drained most of `page_rec` (Task 5, 191→176) and ALL of
    // `codeunit_implicit_rec` (Task 6, 174→150) and `compound_receiver`
    // (Task 7, 150→102) → 102 (beyond-1B.3b Task 8, re-measured 2026-07-01).
    // Task 8's characterization (throwaway diagnostic, not committed — see
    // task-8-report.md) of the 102-site residual: 82/102 were a DIFFERENT
    // object than the caller, source-verified as the SAME root cause across
    // every sampled site — a BARE (unqualified) call inside a Page/Report
    // trigger that falls through to the object's own `SourceTable`'s global
    // procedures (verified: `Page 6175272 "CDO E-Mail Templates"`'s
    // `OnAfterGetRecord` calls bare `GetReportSelection()`/`GetReportName()`,
    // both defined on `SourceTable = "CDO E-Mail Template Header"`, table
    // 6175283) — this was `resolve_bare`'s own documented "Step 3:
    // Implicit-Rec (deferred)" TODO. 12/102 a same-object nested-trigger gap;
    // 8 mixed overload sets.
    //
    // follow-up plan v2.1 Task 3 (`resolve_bare` Step 3 — bare implicit-Rec
    // dispatch, IMPLEMENTED) re-measured 2026-07-01: **4** — beyond the
    // predicted 82-site bucket, the remaining 12+8 residual ALSO drained
    // almost entirely (not individually re-characterized site-by-site this
    // pass — a possible root cause is that `resolve_in_table_scope`'s
    // visibility-scoped search subsumes some of those cases too, since a
    // nested field-trigger's enclosing object is still one of Step 3's four
    // eligible kinds, but this is NOT independently confirmed here). Task 4
    // (FINAL, arc capstone) RE-CONFIRMED the same **4** by an independent
    // re-run on 2026-07-01 (byte-identical to Task 3's own measurement, no
    // drift after the Task-3 fix-pass's 2 additional TableExtension/
    // PageExtension-caller fixtures, which are workspace-fixture-only and do
    // not touch CDO). 10 tightens the ceiling to the new floor with a tiny
    // margin (was 15, was 110 before that — a ratchet never loosens);
    // raising it further requires re-justifying the new value against a real
    // characterization, not just bumping the number.
    //
    // TIGHTENED 2026-07-03 (applicability-param-subtype-recfield plan v2.1,
    // Task 4): 10→5, measured 3 (one of the 4 prior `fresh_missing` sites is
    // newly resolved by Step 3a's bare implicit-Rec quoted-field arm to a
    // target that EXACTLY MATCHES L3's golden — moving it into `matches`
    // rather than `fresh_missing`; `genuine_wrong` stays 0). 5 keeps a small
    // margin above the measured 3 (same "tiny margin, not zero-tolerance"
    // policy this ceiling has always used).
    //
    // CORRECTED 2026-07-04 (receiver-closure-and-arg-increments plan, Task 5
    // nit sweep): the "measured 3" note above went STALE without anyone
    // updating it — the live value is, and was already, **1** by the time
    // Task 2 of this same plan ran (`.superpowers/sdd/task-2-report.md`
    // independently recorded `fresh_missing (1)`, byte-identical
    // before/after Task 2, so the drop predates Task 2 — most likely a side
    // effect of Task 1's CurrPage UserControl trigger-adjacent resolution,
    // not independently re-attributed here). Re-confirmed 1 again at this
    // task's own capstone re-measure.
    //
    // CORRECTED 2026-07-04 (pageext-merge-and-final-residual plan, Task 2
    // REVIEW fix): the "1" above was itself measured against a report-table
    // ledger transcription that turned out to be wrong (see the dated
    // correction section in `.superpowers/sdd/task-2-report.md` §4) — the
    // TRUE post-Task-2 value is **0** (Site B, `.dependencies/CDO/Page/
    // CDOEMailJobs.Page.al:124`'s bare `Run()`, was the sole `fresh_missing`
    // occupant and moved into `matches` once Task 2's grounded suppression
    // let it resolve to the SourceTable's own `procedure Run()` — an
    // HMAC-verified exact match with L3's own frozen target; `matches`
    // itself stayed flat at 6120 because Site A independently moved
    // `matches` -> `fresh_extra` in the same commit, netting zero). Tightened
    // 5 -> 2 (a small margin above the measured 0, same non-zero-tolerance
    // policy this ceiling has always used — never a hard 0, since this is a
    // completeness metric, not the hard-gated `genuine_wrong` invariant).
    const FRESH_MISSING_CEILING: usize = 2;
    assert!(
        audit.fresh_missing_count <= FRESH_MISSING_CEILING,
        "COMPLETENESS REGRESSION: fresh_missing_count={} exceeds the recorded \
         ceiling {} (baseline pinned 2026-07-03 post applicability-param-subtype-recfield \
         Task 4 [bare implicit-Rec quoted-field receivers]: measured 3 at pin time, but \
         ACTUALLY 1 by 2026-07-04 — see the CORRECTED note above; was 4 post follow-up \
         plan v2.1 Task 4, 102 pre-follow-up; see CHANGELOG.md). The fresh resolver \
         lost an L3-resolved target it used to find — investigate before raising the \
         ceiling.",
        audit.fresh_missing_count,
        FRESH_MISSING_CEILING,
    );

    // ── Divergence ratchet: `fresh_wrong` COUNT ceiling ───────────────────────
    // `fresh_wrong` (both L3 and fresh resolved, to DIFFERENT targets) splits
    // into `fresh_ahead_dispatch` (allowed, fresh refines L3) and
    // `genuine_wrong` (hard-gated to exactly 0 above). A count-only ceiling on
    // the SUM is still useful defense-in-depth: `genuine_wrong == 0` alone
    // cannot see a new confidently-wrong edge that happens to also satisfy the
    // (heuristic, non-adjudicated) `fresh_ahead_dispatch` refinement test —
    // pinning the total means any such site still trips a review, even though
    // it would pass the `genuine_wrong` set-membership gate. History: 139
    // (beyond-1B.3b Task 7/8) → follow-up plan v2.1 Task 3 (`resolve_bare`
    // Step 3) newly resolves many former `fresh_missing` sites, and several
    // land in `fresh_ahead_dispatch` rather than an exact `matches` (expected
    // collateral movement from closing a real completeness gap, NOT a
    // regression — `genuine_wrong` stays hard-gated to 0 above regardless).
    // Recorded 2026-07-01: `fresh_wrong_count=149` (all 149 adjudicated
    // `fresh_ahead_dispatch`, 0 `genuine_wrong`). Task 4 (FINAL, arc
    // capstone) RE-CONFIRMED the same 149 by an independent re-run
    // (byte-identical, no drift) and pinned the ceiling to EXACTLY the
    // measured value — zero margin, matching `genuine_wrong`'s own
    // zero-tolerance philosophy — so that even ONE new `fresh_wrong` site
    // (whether a genuine `fresh_ahead_dispatch` refinement or a
    // misclassified `genuine_wrong`) trips this gate for manual review
    // rather than silently passing inside slack; a ratchet never loosens.
    //
    // TIGHTENED 2026-07-02 (uniform-access-and-compound-receiver plan,
    // Task 1): 149→148 — an IMPROVEMENT (not a soundness-forced rise like
    // the `unknown` ceilings above). `resolve_in_object`'s new per-candidate
    // access filter reclassified one former `fresh_wrong` site (fresh
    // resolved to a WRONG target, per the L3 golden) into an honest `Unknown`
    // — which the L3-comparison now counts among `matches` (both sides
    // agree there's no confident target) rather than a mismatch.
    //
    // RAISED 2026-07-02 (uniform-access-and-compound-receiver plan, Task 1.5,
    // inserted after Task 1): 148→149. Task 1.5 models `internalsVisibleTo`
    // friend apps, correctly resolving cross-app `internal` calls the
    // declaring app's manifest explicitly authorizes (CDO→CTS-CDN) to
    // `Source`. The RETIRED al-sem/L3 TS reference — frozen at golden-mint
    // time — never modeled `InternalsVisibleTo` either, so it still emits
    // `Unknown`/no-edge for the SAME 67 sites (60 `InternalNotVisible` +
    // 7 sites that were mislabeled `ReceiverOutOfClosure` by the
    // documented `resolve_bare` reason-overwrite gap, see
    // `cdo_full_program_coverage_and_self_reported_metric`'s comment for the
    // 407→340 unknown-count drop). This is a case of the retired reference
    // being WRONG (a known, accepted divergence per this project's charter:
    // "no byte-to-byte parity with al-sem" — fresh is Rust-owned and
    // intentionally more accurate) — 1 of those 67 now diverges from L3 as
    // `fresh_wrong` (fresh: `Source`; L3: something else) rather than falling
    // into `fresh_missing`/`fresh_extra`/`fresh_novel`; the adjudication
    // overlay classifies it (and all 148 prior sites) as `fresh_ahead_
    // dispatch`, confirmed by `genuine_wrong == 0` above. `fresh_missing`
    // stays unchanged at 4 (verified — see the metric gate). Ratchet raised
    // to the exact measured value (zero margin, per this ceiling's own
    // established zero-tolerance philosophy).
    //
    // RE-CONFIRMED 2026-07-03 (applicability-param-subtype-recfield plan
    // v2.1, Task 4): still EXACTLY 149 (byte-identical) — the ONE new
    // divergence Task 4's bare implicit-Rec quoted-field arm exposed at
    // `Table 6175281 CDO Setup.al:332`'s `.Trim()` call (a genuine L3
    // golden defect, `known-genuine-divergences.json` entry 52) is
    // adjudicated `l3_error_intrinsic` and overlaid IN-MEMORY before this
    // diff runs, so it never surfaces here as a raw `fresh_wrong` count —
    // net movement zero, ceiling unchanged.
    const FRESH_WRONG_CEILING: usize = 149;
    assert!(
        audit.fresh_wrong_count <= FRESH_WRONG_CEILING,
        "DIVERGENCE REGRESSION: fresh_wrong_count={} exceeds the recorded \
         ceiling {} (recorded 2026-07-02 post uniform-access-and-compound-receiver \
         Task 1.5: 149, all fresh_ahead_dispatch, genuine_wrong=0; was 148 post \
         Task 1, 149 post follow-up plan v2.1 Task 4) — a new site diverged \
         from the L3-validated golden; investigate (is it a new \
         fresh_ahead_dispatch refinement, or a genuine_wrong that the \
         adjudication heuristic mis-classified?) before raising the ceiling.",
        audit.fresh_wrong_count,
        FRESH_WRONG_CEILING,
    );

    // ── Determinism: two consecutive runs produce the same digest ─────────────
    let audit2 = run_cdo_semantic_audit_on(&shared.ctx, &shared.report, &ws, drift_handler);
    assert_eq!(
        audit.digest, audit2.digest,
        "CDO semantic audit must be deterministic (digest differs between runs)"
    );
}

// ---------------------------------------------------------------------------
// Test 17 (CDO env-gated, 1B.3b Task 1): ImplicitTrigger frozen-golden audit
// ---------------------------------------------------------------------------

/// CDO ImplicitTrigger audit: compares fresh's `ImplicitTrigger` resolution
/// against the committed, anonymized, frozen L3 verdict
/// (`cdo-trigger-anon.json`). See [`AnonTriggerAuditReport`]'s doc comment
/// (in `semantic_golden.rs`) for this audit's scope — it proves the
/// frozen-load mechanism works for the ImplicitTrigger dispatch kind and
/// backs `ENFORCE_CDO_WS`'s `checked_sites>0` requirement. The zero-tolerance
/// ImplicitTrigger gate is this frozen audit plus the
/// `implicit_trigger_fixture_resolves_exact_target_set` fixture test and the
/// ported applicability teeth (`fan_out_applicability_zero_violations`) — the
/// old live, CDO-gated `run_implicit_trigger_harness` dual-run gate was
/// deleted in 1B.3b Task 3.
#[test]
fn cdo_trigger_audit_frozen_load() {
    let Some(ws) = cdo_ws_or_enforce() else {
        return;
    };
    let Some(shared) = cdo_shared() else {
        return;
    };

    let audit = run_cdo_trigger_audit_on(&shared.ctx, &shared.report, &ws, drift_handler);
    enforce_audit_ran(audit.golden_loaded, audit.total_paired);
    assert!(
        audit.golden_loaded,
        "cdo-trigger-anon.json missing/invalid at {}; run the dev-mint tool \
         (`cargo run --bin mint-goldens`) with CDO_WS set",
        cdo_trigger_anon_golden_path().display(),
    );

    eprintln!(
        "Test 17 — CDO ImplicitTrigger frozen audit: l3_total={} fresh_total={} \
         total_paired={} matches={} fresh_wrong={} fresh_missing={} fresh_extra={} \
         fresh_novel={} golden_missing={} digest={}",
        audit.l3_total,
        audit.fresh_total,
        audit.total_paired,
        audit.matches,
        audit.fresh_wrong_count,
        audit.fresh_missing,
        audit.fresh_extra,
        audit.fresh_novel,
        audit.golden_missing,
        audit.digest,
    );

    // ── COMPLETENESS FLOOR (1B.3b whole-branch fix): re-instate the deleted
    // `regression_unexplained == 0` leg for ImplicitTrigger.
    //
    // Zero tolerance for fresh confidently resolving a paired trigger site to
    // the WRONG target set — this mirrors the old live gate's hard
    // zero-tolerance and currently holds (`fresh_wrong_count == 0`).
    assert_eq!(
        audit.fresh_wrong_count, 0,
        "COMPLETENESS REGRESSION: ImplicitTrigger fresh_wrong_count={} (must be 0) \
         — fresh disagreeing with a frozen, L3-verified trigger target is a real \
         resolver bug, investigate.",
        audit.fresh_wrong_count,
    );
    // `fresh_missing` (L3 resolved a trigger target, fresh emitted nothing) is
    // NOT presently zero: this golden carries a SMALL, STABLE, pre-existing
    // gap of 3 sites that was already present at golden MINT time (1B.3b
    // Task 1, `.superpowers/sdd/task-1-report.md`: "total_paired=188
    // matches=185 fresh_wrong=0 fresh_missing=3") and has been UNCHANGED
    // through every capstone verification since (1B.3b Task 4 capstone:
    // identical `matches=185`; reproduced again on 2026-07-01 for this fix:
    // identical `total_paired=188 matches=185 fresh_missing=3`). It predates
    // the gate-completeness deletion this fix restores, so asserting literal
    // `matches == total_paired` would fail on a KNOWN, already-accepted gap,
    // not a new one. Pin it as a CEILING instead (same pattern as Test 16):
    // any NEW drop (4+) is a real completeness regression and FAILS.
    const FRESH_MISSING_CEILING: usize = 3;
    assert!(
        audit.fresh_missing <= FRESH_MISSING_CEILING,
        "COMPLETENESS REGRESSION: ImplicitTrigger fresh_missing={} exceeds the \
         recorded ceiling {} (stable since the golden's 1B.3b Task 1 mint-time \
         verification — see task-1-report.md). A NEW dropped trigger target. \
         Investigate before raising the ceiling.",
        audit.fresh_missing,
        FRESH_MISSING_CEILING,
    );

    // Determinism.
    let audit2 = run_cdo_trigger_audit_on(&shared.ctx, &shared.report, &ws, drift_handler);
    assert_eq!(
        audit.digest, audit2.digest,
        "CDO trigger audit must be deterministic (digest differs between runs)"
    );
}

// ---------------------------------------------------------------------------
// Test 18 (CDO env-gated, 1B.3b Task 1): EventFlow frozen-golden audit
// ---------------------------------------------------------------------------

/// CDO EventFlow audit: compares fresh's resolved EventFlow
/// publisher→subscriber pairs against the committed, anonymized, frozen L3
/// verdict (`cdo-event-anon.json`). Arity-agnostic pair-set comparison only —
/// see [`AnonEventAuditReport`]'s doc comment. The zero-tolerance EventFlow
/// gate is this frozen audit plus the `event_fixture_two_stage_join` fixture
/// test and the ported event-route teeth — the old live, CDO-gated
/// `run_event_flow_gate` dual-run gate was deleted in 1B.3b Task 3.
#[test]
fn cdo_event_audit_frozen_load() {
    let Some(ws) = cdo_ws_or_enforce() else {
        return;
    };
    let Some(shared) = cdo_shared() else {
        return;
    };

    let audit = run_cdo_event_audit_on(&shared.ctx, &ws, drift_handler);
    enforce_audit_ran(audit.golden_loaded, audit.matched_pairs);
    assert!(
        audit.golden_loaded,
        "cdo-event-anon.json missing/invalid at {}; run the dev-mint tool \
         (`cargo run --bin mint-goldens`) with CDO_WS set",
        cdo_event_anon_golden_path().display(),
    );

    eprintln!(
        "Test 18 — CDO EventFlow frozen audit: l3_total={} fresh_total={} \
         matched_pairs={} pair_l3_only={} pair_fresh_only={} digest={}",
        audit.l3_total,
        audit.fresh_total,
        audit.matched_pairs,
        audit.pair_l3_only,
        audit.pair_fresh_only,
        audit.digest,
    );

    // ── COMPLETENESS FLOOR (1B.3b whole-branch fix): re-instate the deleted
    // `regression_unexplained == 0` leg for EventFlow. Zero tolerance: every
    // frozen L3 publisher→subscriber pair must still be found by fresh.
    assert_eq!(
        audit.pair_l3_only, 0,
        "COMPLETENESS REGRESSION: {} frozen L3 EventFlow pair(s) are missing from \
         fresh (pair_l3_only must be 0) — fresh lost a publisher\u{2192}subscriber \
         pair it used to resolve, investigate.",
        audit.pair_l3_only,
    );
    assert_eq!(
        audit.matched_pairs, audit.l3_total,
        "COMPLETENESS REGRESSION: matched_pairs={} != l3_total={} — every frozen \
         L3 EventFlow pair must be matched by fresh.",
        audit.matched_pairs, audit.l3_total,
    );

    // Determinism.
    let audit2 = run_cdo_event_audit_on(&shared.ctx, &ws, drift_handler);
    assert_eq!(
        audit.digest, audit2.digest,
        "CDO event audit must be deterministic (digest differs between runs)"
    );
}

// ---------------------------------------------------------------------------
// Test 19 (UNCONDITIONAL — no CDO_WS needed, public CI): committed golden
// metadata validation
// ---------------------------------------------------------------------------

/// Public-CI metadata validation (1B.3b Task 1): asserts the THREE committed
/// anonymized goldens exist, parse, carry the current schema version, and
/// have non-trivial per-dispatch-kind coverage — WITHOUT needing `CDO_WS` (no
/// CDO source is required to validate a committed artifact's shape). This is
/// the floor public CI (which never has CDO access) can verify; the per-site
/// diff itself only runs on the gated/internal runner (Tests 16–18).
///
/// Also validates the pre-existing `known-genuine-divergences.json` manifest
/// carries exactly 42 entries (1B.3a's adjudicated genuine_wrong baseline —
/// unrelated to `cdo-anon.json`'s anonymization, but co-located metadata this
/// test is the natural unconditional home for).
#[test]
fn committed_goldens_metadata_is_valid() {
    let golden = load_anon_golden(&cdo_anon_golden_path()).unwrap_or_else(|| {
        panic!(
            "cdo-anon.json missing/invalid at {} — committed goldens must always \
             parse, even without CDO_WS",
            cdo_anon_golden_path().display(),
        )
    });
    assert_eq!(golden.schema_version, ANON_GOLDEN_SCHEMA_VERSION);
    assert!(
        !golden.entries.is_empty(),
        "cdo-anon.json must be non-empty"
    );
    let mut by_edge_kind: std::collections::HashMap<u8, usize> = std::collections::HashMap::new();
    for e in &golden.entries {
        *by_edge_kind.entry(e.site.edge_kind).or_insert(0) += 1;
    }
    eprintln!(
        "cdo-anon.json: {} entries, by edge_kind: {by_edge_kind:?}",
        golden.entries.len()
    );
    // edge_kind 0=Call, 1=Run are the dispatch kinds this golden covers
    // (Member/Interface — see semantic_golden.rs's module docs); at least one
    // of each must be present for the golden to be meaningfully non-trivial.
    assert!(
        by_edge_kind.get(&0).copied().unwrap_or(0) > 0,
        "cdo-anon.json must contain at least one Call-kind (edge_kind=0) entry"
    );

    let trigger_golden = load_anon_golden(&cdo_trigger_anon_golden_path()).unwrap_or_else(|| {
        panic!(
            "cdo-trigger-anon.json missing/invalid at {}",
            cdo_trigger_anon_golden_path().display(),
        )
    });
    assert_eq!(trigger_golden.schema_version, ANON_GOLDEN_SCHEMA_VERSION);
    assert!(
        !trigger_golden.entries.is_empty(),
        "cdo-trigger-anon.json must be non-empty"
    );

    let event_golden = load_anon_event_golden(&cdo_event_anon_golden_path()).unwrap_or_else(|| {
        panic!(
            "cdo-event-anon.json missing/invalid at {}",
            cdo_event_anon_golden_path().display(),
        )
    });
    assert_eq!(event_golden.schema_version, ANON_GOLDEN_SCHEMA_VERSION);
    assert!(
        !event_golden.entries.is_empty(),
        "cdo-event-anon.json must be non-empty"
    );

    eprintln!(
        "Test 19 — committed golden metadata: cdo-anon entries={} trigger entries={} \
         event entries={}",
        golden.entries.len(),
        trigger_golden.entries.len(),
        event_golden.entries.len(),
    );

    // The pre-existing genuine_wrong manifest — co-located metadata, also
    // unconditionally checkable.
    let manifest_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/goldens/semantic-edges/known-genuine-divergences.json");
    let manifest_json = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|_| panic!("manifest missing: {}", manifest_path.display()));
    let manifest: serde_json::Value =
        serde_json::from_str(&manifest_json).expect("manifest must be valid JSON");
    let manifest_entries = manifest
        .get("entries")
        .and_then(|e| e.as_array())
        .expect("manifest must have 'entries' array");

    // ── beyond-1B.3b Task 3: manifest + overlay invariants (replaces the bare
    // `assert_eq!(len, 42)`) ────────────────────────────────────────────────
    //
    // Every `known-genuine-divergences.json` entry now carries an adjudicated
    // `verdict` (Task 3). Split: 42 `l3_error_intrinsic` / 0
    // `fresh_false_builtin` (would mean Tasks 1-2 left a real fresh bug
    // unabsorbed) / 0 `needs_manual_review` (fail-closed — an unresolved
    // dimension is never silently treated as passing).
    let mut manifest_site_keys: std::collections::HashSet<(String, u64, u64)> =
        std::collections::HashSet::new();
    let mut manifest_intrinsic_keys: std::collections::HashSet<(String, u64, u64)> =
        std::collections::HashSet::new();
    for entry in manifest_entries {
        let unit = entry["unit"]
            .as_str()
            .expect("manifest entry missing 'unit'")
            .to_string();
        let line = entry["line"]
            .as_u64()
            .expect("manifest entry missing 'line'");
        let callee_fp = entry["callee_fp"]
            .as_u64()
            .expect("manifest entry missing 'callee_fp'");
        let verdict = entry["verdict"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                panic!("manifest entry {unit}:{line} missing non-empty 'verdict' (Task 3)")
            });
        assert!(
            matches!(
                verdict,
                "l3_error_intrinsic" | "fresh_false_builtin" | "needs_manual_review"
            ),
            "manifest entry {unit}:{line} has unrecognized verdict {verdict:?}"
        );
        let key = (unit.clone(), line, callee_fp);
        assert!(
            manifest_site_keys.insert(key.clone()),
            "duplicate site key in known-genuine-divergences.json: {unit}:{line} fp={callee_fp}"
        );
        if verdict == "l3_error_intrinsic" {
            manifest_intrinsic_keys.insert(key);
        }
    }
    // BASELINE-SCOPED, not universal. This used to pin exactly 54 entries with a
    // long provenance note. That count described ONE workspace — the 2026-07
    // CDO tree, whose recorded mint SHA exists in no checkout any more. When the
    // baseline moved to a pinned, reproducible commit, the population it counted
    // ceased to exist, and a hard-coded 54 would only have forced the next author
    // to invent entries to satisfy it.
    //
    // A COUNT was never the right guard here anyway: this test is unconditional
    // (no CDO_WS), so it can check the manifest's SHAPE but cannot know how many
    // divergences the live workspace actually has. The teeth live in the
    // CDO-gated `cdo_genuine_wrong_is_precedence_adjudicated`, which asserts the
    // overlay's key set EQUALS the raw (pre-overlay) genuine_wrong set — so an
    // empty overlay passes only when there is genuinely nothing to adjudicate,
    // and emptying the file to silence a real divergence fails.
    assert_eq!(
        manifest_intrinsic_keys.len(),
        manifest_entries.len(),
        "every known-genuine-divergences.json entry must be adjudicated          l3_error_intrinsic; a shortfall means a fresh_false_builtin or          needs_manual_review survivor slipped through — investigate before          relying on the overlay"
    );

    // ARTIFACT RATCHET — pins the COMMITTED files, which this test can see.
    //
    // An earlier revision deleted every exact count here, arguing an unconditional
    // test 'cannot know the count'. That conflated two different things (caught by
    // gpt-5.6-sol in review): it cannot derive the LIVE WORKSPACE's divergence
    // count without CDO_WS, but it can absolutely pin what is COMMITTED. Without
    // that, both files could be emptied in lockstep and sail through every machine
    // where CDO_WS is unset -- which is most of them, and all of public CI.
    //
    // 0 is the real committed value for the pinned bc3ccb18 baseline: the raw
    // pre-overlay audit reports no genuine_wrong sites there, so there is nothing
    // to adjudicate. Moving the baseline is a deliberate act; so is updating this.
    const COMMITTED_ADJUDICATED_ENTRIES: usize = 0;
    assert_eq!(
        manifest_entries.len(),
        COMMITTED_ADJUDICATED_ENTRIES,
        "known-genuine-divergences.json must carry exactly {} entries for the \
         committed bc3ccb18 baseline. A change here is either a real baseline move \
         (update this constant deliberately, with the new population triaged) or \
         tampering. Do NOT relax it to make a red gate green.",
        COMMITTED_ADJUDICATED_ENTRIES,
    );

    // The adjudication overlay itself (`adjudicated-overrides.json`) — also
    // unconditionally checkable (pure JSON, no CDO_WS needed to validate its
    // SHAPE; the CDO-gated `cdo_genuine_wrong_is_precedence_adjudicated` test
    // re-verifies its CONTENT against live source).
    let overrides =
        load_adjudicated_overrides(&adjudicated_overrides_path()).unwrap_or_else(|| {
            panic!(
                "adjudicated-overrides.json missing/invalid at {}",
                adjudicated_overrides_path().display(),
            )
        });
    let mut override_site_keys: std::collections::HashSet<(String, u64, u64)> =
        std::collections::HashSet::new();
    for ov in &overrides.entries {
        assert!(!ov.callee_text.is_empty(), "override missing callee_text");
        // `catalog_key` is required for the `builtin-catalog-fp-collision` shape;
        // the `CrossAppSourceProcedure` shape (beyond-1B.3b Task 5.5) and the
        // `SameAppSourceProcedure` shape (pageext-merge-and-final-residual
        // plan Task 2) both carry an empty `catalog_key` and populate
        // `target_*` instead.
        assert!(
            !ov.catalog_key.is_empty()
                || ((ov.receiver_kind == "CrossAppSourceProcedure"
                    || ov.receiver_kind == "SameAppSourceProcedure")
                    && ov.target_kind.is_some()
                    && ov.target_app_guid.is_some()
                    && ov.target_object_lc.is_some()
                    && ov.target_routine_lc.is_some()),
            "override missing catalog_key (and not a fully-populated \
             CrossAppSourceProcedure/SameAppSourceProcedure target)"
        );
        assert!(
            !ov.receiver_kind.is_empty(),
            "override missing receiver_kind"
        );
        assert_eq!(
            ov.source_sha256.len(),
            64,
            "override source_sha256 must be a 64-hex-char SHA-256 digest (unit={})",
            ov.unit
        );
        assert!(
            ov.source_sha256
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "override source_sha256 must be lowercase hex (unit={})",
            ov.unit
        );
        assert!(!ov.verdict.is_empty(), "override missing verdict");
        let key = (ov.unit.clone(), ov.line as u64, ov.callee_fp);
        assert!(
            override_site_keys.insert(key),
            "duplicate site key in adjudicated-overrides.json: {}:{} fp={}",
            ov.unit,
            ov.line,
            ov.callee_fp
        );
    }
    // Every `l3_error_intrinsic` manifest entry must have a matching overlay
    // entry (also verdict `l3_error_intrinsic`) — the overlay is what
    // actually makes `run_cdo_semantic_audit` stop flagging these sites, so a
    // manifest entry without a matching overlay entry would silently keep
    // failing the CDO gate despite claiming to be adjudicated.
    let override_intrinsic_keys: std::collections::HashSet<(String, u64, u64)> = overrides
        .entries
        .iter()
        .filter(|ov| ov.verdict == "l3_error_intrinsic")
        .map(|ov| (ov.unit.clone(), ov.line as u64, ov.callee_fp))
        .collect();
    assert_eq!(
        manifest_intrinsic_keys, override_intrinsic_keys,
        "every known-genuine-divergences.json entry adjudicated l3_error_intrinsic must \
         have a matching adjudicated-overrides.json entry (also l3_error_intrinsic), and \
         vice versa — the two sets diverged"
    );
    assert_eq!(
        overrides.entries.len(),
        manifest_entries.len(),
        "adjudicated-overrides.json must carry exactly one entry per          known-genuine-divergences.json site — the two files are 1:1 by          construction. (This replaced a hard-coded 54: that count described the          2026-07 CDO tree, and this test is unconditional, so it cannot know how          many divergences the live workspace has. The population itself is pinned          where reality is available — cdo_genuine_wrong_is_precedence_adjudicated          asserts the overlay's key set EQUALS the raw pre-overlay genuine_wrong          set.)"
    );

    // ── Non-circularity invariant (testable): overlay entries hold CANONICAL
    // CATALOG KEYS / expected-route FACTS, never a serialized fresh edge id.
    // Parse the raw JSON (not the typed struct, which would silently drop an
    // unexpected field) and assert no entry's key set contains anything
    // shaped like a fresh-computed graph/edge/routine identifier.
    let overrides_json: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(adjudicated_overrides_path())
            .expect("adjudicated-overrides.json must be readable"),
    )
    .expect("adjudicated-overrides.json must be valid JSON");
    const FORBIDDEN_FRESH_EDGE_ID_FIELDS: &[&str] = &[
        "resolved_target",
        "resolved_target_id",
        "fresh_edge_id",
        "fresh_target",
        "edge_id",
        "routine_node_id",
        "object_node_id",
        "target_id",
        "route_target",
    ];
    for ov in overrides_json["entries"]
        .as_array()
        .expect("overrides 'entries' must be an array")
    {
        let obj = ov
            .as_object()
            .expect("override entry must be a JSON object");
        for forbidden in FORBIDDEN_FRESH_EDGE_ID_FIELDS {
            assert!(
                !obj.contains_key(*forbidden),
                "adjudicated-overrides.json entry carries a fresh-edge-id-shaped field \
                 {forbidden:?} — overlay entries must hold only canonical catalog keys \
                 (name+arity+receiver-kind) derived independently of fresh's output, \
                 NEVER a serialized fresh edge/route/graph-node id (non-circularity \
                 invariant)"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Test 19b (CDO env-gated, beyond-1B.3b Task 3): genuine_wrong sites are
// precedence-adjudicated from INDEPENDENT source criteria
// ---------------------------------------------------------------------------

/// Case-insensitive, whole-token scan for a LOCAL `procedure <name>(`
/// declaration anywhere in `unit_content` — the lookup-precedence "does a
/// source competitor shadow the catalog hit" check (Task 1: Source shadows
/// Catalog).
///
/// Pure text search over the SAME live CDO source the test reads — no
/// engine/resolver/graph involvement whatsoever. Deliberately permissive
/// (matches any object member named `name`, not just ones reachable from a
/// specific call site) so it stays conservative: a false POSITIVE here would
/// only push a site toward `fresh_false_builtin`/re-investigation, never
/// toward a false PASS.
fn unit_declares_procedure_named(unit_content: &str, name_lc: &str) -> bool {
    let lc = unit_content.to_ascii_lowercase();
    let bytes = lc.as_bytes();
    let needle = "procedure";
    let mut start = 0usize;
    while let Some(pos) = lc[start..].find(needle) {
        let abs = start + pos;
        let before_ok = abs == 0 || {
            let c = bytes[abs - 1];
            !(c.is_ascii_alphanumeric() || c == b'_')
        };
        let after_idx = abs + needle.len();
        let after_ok = after_idx < bytes.len() && bytes[after_idx].is_ascii_whitespace();
        if before_ok && after_ok {
            let mut i = after_idx;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            let tok_start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let tok = &lc[tok_start..i];
            if tok == name_lc {
                let mut j = i;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < bytes.len() && bytes[j] == b'(' {
                    return true;
                }
            }
        }
        start = abs + needle.len();
    }
    false
}

/// Case-insensitive, quote-agnostic, LINE-based scan for a `<var_name>: Page
/// ...` variable declaration anywhere in `unit_content` — the independent
/// (source-only) confirmation that a bare member-call receiver token really
/// is a Page-typed variable, for the `PageInstanceVar` adjudication shape
/// (argtype-dispatch-and-page-catalog plan, Task 1: general declared
/// Page-typed variables, as opposed to the `CurrPage`/`Page` singleton
/// `PageInstance` shape [`assert_shape_matches_receiver_kind`] already
/// checks).
///
/// AL variable declarations are one per line (`Name: Page "X";` / `Name:
/// Page X;`, optionally quoted on the name side) inside a `var` section, so a
/// per-line `<name> :` prefix match followed immediately by `page` (after
/// stripping an optional quote and whitespace) is sound for the declaration
/// shapes this overlay's sites use. Deliberately whole-unit (not scoped to
/// one routine's `var` section) — same conservative-permissive stance as
/// [`unit_declares_procedure_named`]: a false POSITIVE only pushes a site
/// toward re-investigation via a mismatched independent verdict elsewhere,
/// never toward a false PASS of an otherwise-unverified claim.
fn unit_declares_page_typed_var(unit_content: &str, var_name: &str) -> bool {
    let needle_name = var_name.trim_matches('"').to_ascii_lowercase();
    for line in unit_content.lines() {
        let lc_line = line.trim().trim_start_matches('"').to_ascii_lowercase();
        let Some(rest) = lc_line.strip_prefix(&needle_name) else {
            continue;
        };
        let Some(rest) = rest.trim_start_matches('"').trim_start().strip_prefix(':') else {
            continue;
        };
        if rest.trim_start().starts_with("page") {
            return true;
        }
    }
    false
}

/// Independently re-derive an [`AdjudicatedOverride`]'s verdict from LIVE
/// `unit_content` plus the structural builtin catalog — see
/// `semantic_golden.rs`'s `AdjudicatedOverride` doc comment for the full
/// independence contract this function embodies: it calls ONLY
/// [`is_global_builtin`]/[`member_builtin`] (the structural catalog) and
/// [`unit_declares_procedure_named`] (a plain-text scan of the SAME unit) —
/// never `resolve_full_program`, never a fresh-computed `Edge`.
///
/// `"Framework"` (record-field chains plan Task 4, entry 52): `catalog_key`'s
/// PREFIX (before `::`, e.g. `"Text"`) is run through [`classify_type_text`]
/// — the SAME pure string→shape classifier the resolver itself uses (never a
/// bespoke re-implementation) — and MUST parse to `ParsedType::Framework`; an
/// unrecognized prefix or a non-Framework shape (e.g. `Record`/`Primitive`)
/// fails closed to `needs_manual_review` rather than silently skipping the
/// catalog check. This covers a bare QUOTED FIELD receiver typed by its
/// declared type text (`infer_receiver_type`'s Step 3a / `infer_compound_
/// member_receiver`'s record-field arm) — unlike `PageInstance`/`Record`/
/// `RecordRef`, the receiver is not a fixed keyword, so
/// [`assert_shape_matches_receiver_kind`] does not apply a fixed-token check
/// to it (only the catalog-membership + shadow checks below apply).
///
/// `"PageInstanceVar"` (argtype-dispatch-and-page-catalog plan, Task 1):
/// SAME catalog (`PAGE_INSTANCE` is one shared platform-intrinsic catalog
/// regardless of whether the receiver is the `CurrPage`/`Page` singleton or a
/// general declared `Page`-typed variable — see
/// `resolver::is_metadata_sensitive_instance_method`'s doc), so this arm
/// delegates to the exact same `member_builtin` check as `"PageInstance"`.
/// The two receiver_kinds differ only in what
/// [`assert_shape_matches_receiver_kind`] independently verifies about the
/// receiver TOKEN (a fixed CurrPage/Page keyword vs. a declared-variable
/// name) — the catalog-membership question this function answers is
/// identical either way.
fn derive_verdict(ov: &AdjudicatedOverride, unit_content: &str) -> &'static str {
    let method_lc = ov
        .catalog_key
        .rsplit("::")
        .next()
        .unwrap_or(&ov.catalog_key)
        .to_ascii_lowercase();

    let catalog_match = match ov.receiver_kind.as_str() {
        "Global" => is_global_builtin(&method_lc),
        "PageInstance" | "PageInstanceVar" => member_builtin(
            MemberCatalogKind::Framework(&FrameworkKind::PageInstance),
            &method_lc,
        ),
        "Record" => member_builtin(MemberCatalogKind::Record, &method_lc),
        "RecordRef" => member_builtin(MemberCatalogKind::RecordRef, &method_lc),
        "Framework" => {
            let prefix = ov.catalog_key.split("::").next().unwrap_or("");
            match classify_type_text(prefix) {
                ParsedType::Framework(kind) => {
                    member_builtin(MemberCatalogKind::Framework(&kind), &method_lc)
                }
                _ => return "needs_manual_review", // prefix isn't a recognized Framework type
            }
        }
        _ => return "needs_manual_review", // unrecognized receiver kind — fail closed
    };
    if !catalog_match {
        // The claimed catalog member doesn't actually exist for this
        // receiver kind — fresh's builtin claim would be unsupported.
        return "fresh_false_builtin";
    }
    if unit_declares_procedure_named(unit_content, &method_lc) {
        // A source competitor shadows the catalog hit (Task 1 lookup
        // precedence: Source shadows Catalog) — fresh should have picked the
        // source routine, so a `builtin` claim here would be a fresh bug.
        return "fresh_false_builtin";
    }
    "l3_error_intrinsic"
}

/// The call SHAPE parsed straight from `callee_text`, independent of the
/// overlay's own `receiver_kind`: a bare GLOBAL call (no `.`) or a MEMBER
/// call `<receiver>.<method>`, split on the FINAL `.`. Every `callee_text`
/// in the overlay is a simple `Receiver.Method` token pair — no
/// chained/qualified receivers appear among the 42 adjudicated sites — so a
/// single `rfind('.')` split is sufficient. Deliberately lightweight: this
/// is a syntax check, not a type-inferring parser (see
/// `assert_shape_matches_receiver_kind`'s doc comment for what it does and
/// does not prove).
enum CallShape<'a> {
    Global(&'a str),
    Member { receiver: &'a str, method: &'a str },
}

fn parse_callee_shape(callee_text: &str) -> CallShape<'_> {
    match callee_text.rfind('.') {
        Some(idx) => CallShape::Member {
            receiver: &callee_text[..idx],
            method: &callee_text[idx + 1..],
        },
        None => CallShape::Global(callee_text),
    }
}

/// Review-fix (beyond-1B.3b Task 3 fix pass): independently cross-check
/// `ov.receiver_kind` and `ov.catalog_key`'s method component against the
/// call SHAPE parsed straight from `ov.callee_text`, BEFORE `derive_verdict`
/// is allowed to trust `receiver_kind` as given. Closes the review gap where
/// a mislabeled `receiver_kind` (e.g. `"Global"` recorded for what is
/// actually a member call `X.Method(...)` whose method name also happens to
/// be a valid global builtin) would otherwise sail through `derive_verdict`
/// unchallenged.
///
/// Checks performed (a lightweight SYNTAX check, not full type inference of
/// the receiver variable's declared type — the shadow-absence and
/// catalog-membership checks in `derive_verdict` already bound that; this
/// only needs to catch a Global-vs-member/page-instance MISLABEL):
/// - `Global` receiver_kind ⟺ `callee_text` has no `.`.
/// - `PageInstance`/`PageInstanceVar`/`Record`/`RecordRef` receiver_kind ⟺
///   `callee_text` has a `.` (a member call).
/// - For a member call with `receiver_kind == "PageInstance"`, the receiver
///   token (text before the final `.`) must be `CurrPage` or `Page` — the
///   only page-instance-SINGLETON forms this overlay uses.
/// - For a member call with `receiver_kind == "PageInstanceVar"` (argtype-
///   dispatch-and-page-catalog plan, Task 1): the receiver token must NOT be
///   `CurrPage`/`Page` (that shape is `"PageInstance"`) and `unit_content`
///   (independent source text, the SAME unit the caller already read for the
///   `source_sha256`/callee_text checks — never a fresh-computed value) must
///   declare a `<receiver>: Page ...` variable somewhere in the unit
///   ([`unit_declares_page_typed_var`]) — the source-only confirmation that
///   this really is a Page-typed receiver, not merely a name that happens to
///   end in "Page".
/// - In both shapes, the parsed method token must match `catalog_key`'s
///   method component (the part after `::`, or the whole key for a bare
///   global).
///
/// Panics via `assert!`/`assert_eq!` on any mismatch — a hard, load-bearing
/// check, not advisory.
fn assert_shape_matches_receiver_kind(ov: &AdjudicatedOverride, unit_content: &str) {
    let expected_method_lc = ov
        .catalog_key
        .rsplit("::")
        .next()
        .unwrap_or(&ov.catalog_key)
        .to_ascii_lowercase();
    match parse_callee_shape(&ov.callee_text) {
        CallShape::Global(method) => {
            assert_eq!(
                ov.receiver_kind, "Global",
                "{}:{}: callee_text {:?} is a bare (dot-free) call, but receiver_kind is \
                 {:?} not \"Global\" — shape/receiver_kind mismatch",
                ov.unit, ov.line, ov.callee_text, ov.receiver_kind,
            );
            assert_eq!(
                method.to_ascii_lowercase(),
                expected_method_lc,
                "{}:{}: callee_text {:?} does not match catalog_key {:?}'s method component",
                ov.unit,
                ov.line,
                ov.callee_text,
                ov.catalog_key,
            );
        }
        CallShape::Member { receiver, method } => {
            assert!(
                matches!(
                    ov.receiver_kind.as_str(),
                    "PageInstance" | "PageInstanceVar" | "Record" | "RecordRef" | "Framework"
                ),
                "{}:{}: callee_text {:?} is a member call (`<receiver>.<method>`), but \
                 receiver_kind is {:?} — expected \
                 PageInstance/PageInstanceVar/Record/RecordRef/Framework",
                ov.unit,
                ov.line,
                ov.callee_text,
                ov.receiver_kind,
            );
            if ov.receiver_kind == "PageInstance" {
                assert!(
                    receiver.eq_ignore_ascii_case("CurrPage")
                        || receiver.eq_ignore_ascii_case("Page"),
                    "{}:{}: PageInstance member call {:?} has receiver token {:?}, expected \
                     CurrPage or Page (the page-instance forms this overlay uses)",
                    ov.unit,
                    ov.line,
                    ov.callee_text,
                    receiver,
                );
            }
            if ov.receiver_kind == "PageInstanceVar" {
                assert!(
                    !receiver.eq_ignore_ascii_case("CurrPage")
                        && !receiver.eq_ignore_ascii_case("Page"),
                    "{}:{}: PageInstanceVar member call {:?} has receiver token {:?} — the \
                     CurrPage/Page singleton shape must use receiver_kind \"PageInstance\" \
                     instead",
                    ov.unit,
                    ov.line,
                    ov.callee_text,
                    receiver,
                );
                assert!(
                    unit_declares_page_typed_var(unit_content, receiver),
                    "{}:{}: PageInstanceVar member call {:?} has receiver token {:?}, but no \
                     `{receiver}: Page ...` variable declaration was found anywhere in the \
                     unit — cannot independently verify this is a Page-instance receiver",
                    ov.unit,
                    ov.line,
                    ov.callee_text,
                    receiver,
                );
            }
            assert_eq!(
                method.to_ascii_lowercase(),
                expected_method_lc,
                "{}:{}: callee_text {:?}'s method {:?} does not match catalog_key {:?}'s \
                 method component",
                ov.unit,
                ov.line,
                ov.callee_text,
                method,
                ov.catalog_key,
            );
        }
    }
}

/// Count the top-level (paren/quote-depth-aware) comma-separated arguments
/// of the call to `callee_text` found on `line_text` — an independent arity
/// cross-check against `ov.arity`, so that field is load-bearing rather than
/// vestigial (review-fix, beyond-1B.3b Task 3 fix pass).
///
/// Returns `None` — a deliberate, conservative bail-out, NOT arity 0 — when
/// `callee_text` isn't immediately followed by `(` on this line, or when the
/// argument list doesn't close before line end (e.g. a call whose arguments
/// wrap onto a following line). Robustly counting arguments from source text
/// is not reliable for every call form; callers must treat `None` as
/// "cannot cross-check this site", never as a synthesized answer.
///
/// Quote-aware (both `'...'` string literals and `"..."` quoted
/// identifiers, including the AL doubled-quote escape `''`/`""`) so commas
/// inside string/identifier literals are never miscounted, and
/// paren-depth-aware so a nested call's arguments (e.g. `CopyStr(X, 1,
/// MaxStrLen(X))`) are not double-counted at the outer level.
fn count_call_arity_on_line(line_text: &str, callee_text: &str) -> Option<usize> {
    let lc_line = line_text.to_ascii_lowercase();
    let lc_callee = callee_text.to_ascii_lowercase();
    let start = lc_line.find(&lc_callee)?;
    let after_callee = start + callee_text.len();
    let bytes = line_text.as_bytes();

    let mut i = after_callee;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if bytes.get(i) != Some(&b'(') {
        return None; // not a call at this occurrence — cannot cross-check
    }
    i += 1; // past the opening '('
    let arg_start = i;

    let mut depth = 1i32;
    let mut quote: Option<u8> = None;
    let mut commas_at_top = 0usize;
    let mut close_idx = None;
    while i < bytes.len() {
        let c = bytes[i];
        if let Some(q) = quote {
            if c == q {
                if bytes.get(i + 1) == Some(&q) {
                    i += 2; // doubled-quote escape — stays inside the quote
                    continue;
                }
                quote = None;
            }
            i += 1;
            continue;
        }
        match c {
            b'\'' | b'"' => quote = Some(c),
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    close_idx = Some(i);
                    break;
                }
            }
            b',' if depth == 1 => commas_at_top += 1,
            _ => {}
        }
        i += 1;
    }

    let close_idx = close_idx?; // unbalanced by line end — bail out, don't guess
    let inner = line_text[arg_start..close_idx].trim();
    if inner.is_empty() {
        Some(0)
    } else {
        Some(commas_at_top + 1)
    }
}

/// Whether a source-level top-level comma count is a SOUND oracle for the
/// overlay's recorded `arity` at this call (review-fix, beyond-1B.3b Task 3
/// fix pass).
///
/// It is NOT sound for the "object-run static" dispatch forms —
/// `Page.RunModal` / `Page.Run` / `Report.Run` / `Report.RunModal` /
/// `Codeunit.Run` / `Query.Open` / `XmlPort.*` — whose FIRST syntactic
/// argument is an object DESIGNATOR (`Page::"…"`) rather than a value
/// argument. Whether that designator counts toward "arity" is a convention
/// the committed overlay does NOT fix consistently: the two `Page.RunModal`
/// entries disagree — `Page.RunModal(Page::"User Setup")` records arity 1
/// (counting the designator), while `Page.RunModal(Page::"CDO Field List",
/// Field)` records arity 1 (NOT counting it, i.e. only the record). Because
/// `arity` is descriptive metadata only (it is NOT part of the site key and
/// is never consumed by `apply_adjudicated_overrides`/the audit), this
/// inconsistency is cosmetic; rather than false-fail a valid entry on an
/// ambiguous convention we skip the numeric arity oracle for exactly these
/// forms and document it (the shape/receiver-kind cross-check STILL runs for
/// them). For every OTHER call form — bare globals and `CurrPage`/Record/
/// RecordRef member calls — the parenthesized arguments are all value
/// arguments and the count is a sound oracle.
fn arity_source_count_is_sound(callee_text: &str) -> bool {
    match parse_callee_shape(callee_text) {
        CallShape::Member { receiver, .. } => !matches!(
            receiver.to_ascii_lowercase().as_str(),
            "page" | "report" | "codeunit" | "query" | "xmlport"
        ),
        CallShape::Global(_) => true,
    }
}

// ---------------------------------------------------------------------------
// beyond-1B.3b Task 5.5: independent verification for the `CrossAppSourceProcedure`
// override shape — a REAL procedure declared in a dependency app's own embedded
// (ShowMyCode) source, verified WITHOUT reading any fresh-computed edge.
// ---------------------------------------------------------------------------

/// Find the `.app` file in `ws`'s `.alpackages` whose NavxManifest `App@Id`
/// equals `guid` (case-insensitive). Scans every `.app` present — mirrors
/// `crate::dependencies::load_all_apps`'s "every package found is parsed"
/// discovery, independent of any snapshot/graph the fresh resolver built.
fn find_app_by_guid(ws: &std::path::Path, guid: &str) -> std::path::PathBuf {
    let alpackages = ws.join(".alpackages");
    let entries = std::fs::read_dir(&alpackages)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", alpackages.display()));
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("app") {
            continue;
        }
        if let Ok(pkg) = al_sem::app_package::extract_app_package(&path)
            && pkg.metadata.app_id.eq_ignore_ascii_case(guid)
        {
            return path;
        }
    }
    panic!(
        "no .app in {} carries App@Id={guid:?}",
        alpackages.display()
    );
}

/// Independently confirm that `app_path`'s OWN embedded (ShowMyCode) AL
/// source declares a `procedure <routine_lc>(` inside an object block headed
/// `<object_kind_word> <object_lc> "..."` (or `<object_kind_word> <object_lc>` /
/// unquoted for a name-only key) — a plain-text scan of the TARGET app's real
/// source, structurally identical in spirit to [`unit_declares_procedure_named`]
/// but reading the DEPENDENCY's source, not the CDO-side caller's. Returns the
/// matching source file's virtual path for diagnostics, or `None` if no such
/// declaration is found anywhere in the embedded source.
fn target_app_declares_procedure(
    app_path: &std::path::Path,
    object_lc: &str,
    routine_lc: &str,
) -> Option<String> {
    let files = al_sem::snapshot::embedded::extract_embedded_source(app_path).unwrap_or_else(|e| {
        panic!(
            "cannot extract embedded source from {}: {e}",
            app_path.display()
        )
    });
    let object_header_needle = format!(" {object_lc} ");
    for f in &files {
        let lc = f.text.to_ascii_lowercase();
        if !lc.contains(&object_header_needle) {
            continue;
        }
        if unit_declares_procedure_named(&f.text, routine_lc) {
            return Some(f.virtual_path.clone());
        }
    }
    None
}

/// beyond-1B.3b Task 5.5: independently verify one `CrossAppSourceProcedure`
/// override entry — the counterpart to [`assert_shape_matches_receiver_kind`]
/// and [`derive_verdict`] for the `builtin-catalog-fp-collision` shape, but for
/// a cross-app SOURCE-PROCEDURE target instead of a platform-builtin one.
///
/// Confirms, entirely from LIVE data never touching a fresh-computed edge:
/// 1. `callee_text` is a member call whose method component matches
///    `target_routine_lc` (shape sanity — catches a stale/typo'd override).
/// 2. The claimed target app (`target_app_guid`) has a `.app` present in
///    `ws`'s `.alpackages` ([`find_app_by_guid`]).
/// 3. That app's OWN embedded source really declares
///    `procedure <target_routine_lc>(` on object `target_object_lc`
///    ([`target_app_declares_procedure`]).
///
/// Panics (fail-closed) on any check failure — never silently skipped.
fn verify_cross_app_source_procedure_override(ov: &AdjudicatedOverride, ws: &std::path::Path) {
    let target_kind = ov.target_kind.unwrap_or_else(|| {
        panic!(
            "{}:{}: CrossAppSourceProcedure override missing target_kind",
            ov.unit, ov.line
        )
    });
    let target_app_guid = ov.target_app_guid.as_deref().unwrap_or_else(|| {
        panic!(
            "{}:{}: CrossAppSourceProcedure override missing target_app_guid",
            ov.unit, ov.line
        )
    });
    let target_object_lc = ov.target_object_lc.as_deref().unwrap_or_else(|| {
        panic!(
            "{}:{}: CrossAppSourceProcedure override missing target_object_lc",
            ov.unit, ov.line
        )
    });
    let target_routine_lc = ov.target_routine_lc.as_deref().unwrap_or_else(|| {
        panic!(
            "{}:{}: CrossAppSourceProcedure override missing target_routine_lc",
            ov.unit, ov.line
        )
    });

    // ── shape sanity: callee_text's method/name matches target_routine_lc ───
    // Two caller-side shapes are admissible: a qualified MEMBER call
    // (`X.Method(...)`, the original Task 5.5 shape) and, since beyond-1B.3b
    // Task 3 (bare implicit-Rec dispatch), a BARE call (`Method(...)`) whose
    // name IS the routine being invoked — AL's implicit-`Rec` fallback for a
    // Page/Table/TableExtension/PageExtension bare call. Both are sound: the
    // callee TEXT unambiguously names the routine either way; only the
    // presence/absence of an explicit receiver differs.
    match parse_callee_shape(&ov.callee_text) {
        CallShape::Member { method, .. } => assert_eq!(
            method.to_ascii_lowercase(),
            target_routine_lc,
            "{}:{}: callee_text {:?}'s method does not match target_routine_lc {:?}",
            ov.unit,
            ov.line,
            ov.callee_text,
            target_routine_lc,
        ),
        CallShape::Global(name) => assert_eq!(
            name.to_ascii_lowercase(),
            target_routine_lc,
            "{}:{}: bare callee_text {:?} does not match target_routine_lc {:?}",
            ov.unit,
            ov.line,
            ov.callee_text,
            target_routine_lc,
        ),
    }

    // ── target app + object/routine really exist in the target's own source ──
    let app_path = find_app_by_guid(ws, target_app_guid);
    let found = target_app_declares_procedure(&app_path, target_object_lc, target_routine_lc);
    assert!(
        found.is_some(),
        "{}:{}: target app {} ({}) has no embedded source declaring `procedure {}(` on \
         object {} — the CrossAppSourceProcedure override target is unverifiable",
        ov.unit,
        ov.line,
        target_app_guid,
        app_path.display(),
        target_routine_lc,
        target_object_lc,
    );
    eprintln!(
        "CrossAppSourceProcedure verified: {}:{} -> target_app={target_app_guid} \
         target_object={target_object_lc} target_routine={target_routine_lc} \
         (found in {})",
        ov.unit,
        ov.line,
        found.unwrap(),
    );
    // `target_kind` itself has no independent source-side representation to
    // cross-check (object-kind words in AL source are unambiguous — a
    // mismatched `target_kind` would only matter for the OVERLAY's applied
    // GoldenTarget shape, checked structurally by `apply_adjudicated_overrides`
    // matching `differential.rs`'s own `object_kind_str_to_tag` encoding).
    let _ = target_kind;
}

/// The `SameAppSourceProcedure` analog of [`verify_cross_app_source_procedure_override`]
/// (pageext-merge-and-final-residual plan, Task 2): the target routine lives
/// in the CALLER'S OWN app, so `target_app_guid` is never looked up via
/// `.alpackages` (a workspace never carries its OWN compiled `.app` as one of
/// its own dependency packages — only genuine dependencies live there,
/// verified: CDO_WS's `.alpackages` contains only its Continia dependency
/// apps, never a copy of Continia Document Output itself). Instead
/// `target_unit` names the target's OWN file directly in the live workspace
/// source tree, read the SAME way `ov.unit` (the caller's file) already is.
fn verify_same_app_source_procedure_override(ov: &AdjudicatedOverride, ws: &std::path::Path) {
    let target_kind = ov.target_kind.unwrap_or_else(|| {
        panic!(
            "{}:{}: SameAppSourceProcedure override missing target_kind",
            ov.unit, ov.line
        )
    });
    let target_app_guid = ov.target_app_guid.as_deref().unwrap_or_else(|| {
        panic!(
            "{}:{}: SameAppSourceProcedure override missing target_app_guid",
            ov.unit, ov.line
        )
    });
    let target_object_lc = ov.target_object_lc.as_deref().unwrap_or_else(|| {
        panic!(
            "{}:{}: SameAppSourceProcedure override missing target_object_lc",
            ov.unit, ov.line
        )
    });
    let target_routine_lc = ov.target_routine_lc.as_deref().unwrap_or_else(|| {
        panic!(
            "{}:{}: SameAppSourceProcedure override missing target_routine_lc",
            ov.unit, ov.line
        )
    });
    let target_unit = ov.target_unit.as_deref().unwrap_or_else(|| {
        panic!(
            "{}:{}: SameAppSourceProcedure override missing target_unit",
            ov.unit, ov.line
        )
    });

    // ── same-app sanity: this shape is ONLY for a target in the caller's own
    // app — a genuine cross-app target belongs in a CrossAppSourceProcedure
    // entry instead, never this one. ──────────────────────────────────────
    assert_eq!(
        target_app_guid, ov.from_app_guid,
        "{}:{}: SameAppSourceProcedure requires target_app_guid == from_app_guid \
         (a cross-app target belongs in a CrossAppSourceProcedure entry instead)",
        ov.unit, ov.line
    );

    // ── shape sanity: callee_text's method/name matches target_routine_lc ───
    // Mirrors `verify_cross_app_source_procedure_override`'s identical check
    // — both a qualified MEMBER call and a BARE call (AL's implicit-Rec
    // fallback) are admissible shapes here.
    match parse_callee_shape(&ov.callee_text) {
        CallShape::Member { method, .. } => assert_eq!(
            method.to_ascii_lowercase(),
            target_routine_lc,
            "{}:{}: callee_text {:?}'s method does not match target_routine_lc {:?}",
            ov.unit,
            ov.line,
            ov.callee_text,
            target_routine_lc,
        ),
        CallShape::Global(name) => assert_eq!(
            name.to_ascii_lowercase(),
            target_routine_lc,
            "{}:{}: bare callee_text {:?} does not match target_routine_lc {:?}",
            ov.unit,
            ov.line,
            ov.callee_text,
            target_routine_lc,
        ),
    }

    // ── target object + routine really exist in the target's OWN unit,
    // read directly from the live workspace (never `.alpackages`) ──────────
    let target_path = ws.join(target_unit);
    let target_content = std::fs::read_to_string(&target_path).unwrap_or_else(|e| {
        panic!(
            "{}:{}: cannot read SameAppSourceProcedure target_unit {}: {e}",
            ov.unit,
            ov.line,
            target_path.display(),
        )
    });
    let target_lc = target_content.to_ascii_lowercase();
    assert!(
        target_lc.contains(&format!(" {target_object_lc} "))
            || target_lc.contains(&format!(" {target_object_lc}\r"))
            || target_lc.contains(&format!(" {target_object_lc}\n")),
        "{}:{}: target_unit {} does not appear to declare object id/name {:?} \
         (object-header sanity check failed)",
        ov.unit,
        ov.line,
        target_unit,
        target_object_lc,
    );
    assert!(
        unit_declares_procedure_named(&target_content, target_routine_lc),
        "{}:{}: target_unit {} has no `procedure {}(` declaration — the \
         SameAppSourceProcedure override target is unverifiable",
        ov.unit,
        ov.line,
        target_unit,
        target_routine_lc,
    );
    eprintln!(
        "SameAppSourceProcedure verified: {}:{} -> target_app={target_app_guid} \
         target_object={target_object_lc} target_routine={target_routine_lc} \
         (found in {target_unit})",
        ov.unit, ov.line,
    );
    // Same non-independently-checkable note as the CrossAppSourceProcedure
    // sibling: `target_kind` has no independent source-side representation.
    let _ = target_kind;
}

/// beyond-1B.3b Task 3: for every entry in the committed adjudication overlay
/// (`adjudicated-overrides.json`), INDEPENDENTLY re-derive/cross-check it
/// from LIVE CDO source + the structural builtin catalog (never from
/// fresh's output, never from this override's own committed fields) and
/// assert agreement. Concretely, for each entry this test:
///
/// 1. Re-hashes the unit at test time and FAILS LOUDLY on any
///    `source_sha256` mismatch (source drift — CDO_WS is a dirty live
///    workspace with uncommitted edits) rather than silently trusting a
///    possibly-stale adjudication.
/// 2. Confirms `callee_text` still appears on the claimed line (line-drift
///    catch).
/// 3. Cross-checks the call SHAPE parsed straight from `callee_text` against
///    `receiver_kind` and `catalog_key`'s method component
///    ([`assert_shape_matches_receiver_kind`]) — BEFORE anything downstream
///    is allowed to trust `receiver_kind` as given. Catches a
///    Global-vs-member (and page-instance) mislabel.
/// 4. Cross-checks `arity` against an independently-counted top-level
///    argument count parsed from the call site
///    ([`count_call_arity_on_line`]), when that count can be determined
///    soundly from the single source line (a conservative bail-out
///    otherwise — see that function's doc comment).
/// 5. Re-derives the `verdict` itself ([`derive_verdict`]) from the
///    structural catalog + a same-unit source-shadow scan, and asserts it
///    matches the committed value.
///
/// This does NOT re-derive every field of [`AdjudicatedOverride`] — the site
/// KEY fields (`from_app_guid`/`from_object_kind`/`from_object_lc`/
/// `from_routine_lc`/`edge_kind`/`unit`/`line`/`callee_fp`) are identity
/// fields used only to locate the site, not independently re-computed facts.
///
/// Fail-closed: ANY `needs_manual_review` or `fresh_false_builtin` survivor
/// is a real bug (a mis-adjudicated site, or a genuine fresh-catalog gap
/// Tasks 1-2 should have absorbed) and fails the test — never auto-passed.
#[test]
fn cdo_genuine_wrong_is_precedence_adjudicated() {
    let Some(ws) = cdo_ws_or_enforce() else {
        return;
    };
    let Some(shared) = cdo_shared() else {
        return;
    };

    let overrides =
        load_adjudicated_overrides(&adjudicated_overrides_path()).unwrap_or_else(|| {
            panic!(
                "adjudicated-overrides.json missing/invalid at {}",
                adjudicated_overrides_path().display(),
            )
        });

    // ── The overlay must correct EXACTLY the divergences that exist ─────────
    //
    // This replaces a bare `assert!(!overrides.entries.is_empty())`. That
    // assertion assumed the adjudicated population would always be non-empty,
    // which stopped being true when the baseline moved to a pinned workspace
    // where the engine produces no genuine divergences at all. Worse, it guarded
    // the wrong thing: it checked the FILE, not its use, so an empty overlay was
    // simply rejected while a full-but-stale one sailed through.
    //
    // The check that actually holds: run the audit RAW (no overlay) to learn
    // which sites genuinely diverge, and require the overlay's key set to EQUAL
    // that set.
    //
    //   O == G      O = overlay's site keys, G = raw genuine_wrong site keys
    //
    // Empty then passes ONLY when G is empty too. Delete a needed override and G
    // still contains the site while O does not — the assertion fires. Keep a
    // stale override for a site that no longer diverges and O exceeds G — it
    // fires again. Neither direction can be silenced by editing the file.
    let raw = run_cdo_semantic_audit_on_raw(&shared.ctx, &shared.report, &ws, drift_handler);
    let raw_genuine: std::collections::BTreeSet<(String, u32, u64)> = raw
        .genuine_wrong_sites
        .iter()
        .map(|k| (k.unit.clone(), k.line, k.callee_fp))
        .collect();
    // O is built from the entries the overlay ACTUALLY APPLIES, i.e. the
    // `l3_error_intrinsic` ones -- `apply_adjudicated_overrides_detailed` skips
    // every other verdict. Mapping ALL entries here was a real hole (found by
    // gemini-3.8-flash in review): an entry with any other verdict would satisfy
    // O == G while never being applied, so the divergence it names would survive
    // into the effective audit unadjudicated. The effective genuine_wrong_count
    // assertion below is the backstop, but the set itself must describe reality.
    let overlay_keys: std::collections::BTreeSet<(String, u32, u64)> = overrides
        .entries
        .iter()
        .filter(|ov| ov.verdict == VERDICT_L3_ERROR_INTRINSIC)
        .map(|ov| (ov.unit.clone(), ov.line, ov.callee_fp))
        .collect();
    // An entry with any other verdict is never applied, so it can only mislead.
    assert!(
        overrides
            .entries
            .iter()
            .all(|ov| ov.verdict == VERDICT_L3_ERROR_INTRINSIC),
        "adjudicated-overrides.json holds an entry whose verdict is not \
         l3_error_intrinsic; such an entry is silently NOT applied by the \
         overlay, so it would claim to adjudicate a divergence it never touches"
    );

    // Non-vacuity FIRST: a raw audit that paired nothing makes `raw_genuine`
    // trivially empty, which would make the equality below meaningless. This is
    // the exact failure that hid a broken baseline for two months — the audit
    // reported `checked_sites == 0` and its genuine_wrong=0 was read as a pass.
    assert!(
        raw.paired > 0,
        "raw semantic audit paired ZERO sites — the golden and the workspace do          not line up, so every downstream count is vacuous. Do NOT read this as          'no divergences'. Check CDO_WS against the golden's mint stamp."
    );

    assert_eq!(
        overlay_keys, raw_genuine,
        "adjudicated-overrides.json must correct exactly the sites that raw          (pre-overlay) auditing reports as genuine_wrong.
  overlay-only (stale          overrides, correcting nothing): {:?}
  raw-only (real divergences with          NO adjudication): {:?}
An empty overlay is legitimate ONLY when the raw          genuine_wrong set is also empty.",
        overlay_keys.difference(&raw_genuine).collect::<Vec<_>>(),
        raw_genuine.difference(&overlay_keys).collect::<Vec<_>>(),
    );

    // Every override must actually CHANGE the oracle. A no-op override — one
    // that rewrites a site with the value it already held — is indistinguishable
    // from a live one in every count, so a stale overlay can look maintained
    // forever. The committed overlay already contained one such entry.
    let effective = run_cdo_semantic_audit_on(&shared.ctx, &shared.report, &ws, drift_handler);
    assert!(
        effective.overlay.no_op_sites.is_empty(),
        "override(s) changed nothing — they rewrite a target the golden already          had, so they correct no divergence and only look like maintenance: {:?}",
        effective.overlay.no_op_sites,
    );
    assert!(
        effective.overlay.unmatched_sites.is_empty(),
        "override(s) name a site the golden does not contain — a stale          adjudication, or a re-mint that moved the site: {:?}",
        effective.overlay.unmatched_sites,
    );
    assert_eq!(
        effective.genuine_wrong_count, 0,
        "genuine_wrong survivors AFTER the adjudicated overlay — the overlay did          not cover every real divergence"
    );

    let mut l3_error_intrinsic = 0usize;
    let mut fresh_false_builtin = 0usize;
    let mut needs_manual_review = 0usize;

    for ov in &overrides.entries {
        let path = ws.join(&ov.unit);
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read adjudicated unit {}: {e}", path.display()));

        // ── source_sha256 drift check — FAIL, never silently skip ──────────
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        let actual_sha: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            actual_sha,
            ov.source_sha256,
            "SOURCE DRIFT at {} ({}:{}): the CDO unit has changed since this adjudication \
             was recorded. Re-verify the site against the CURRENT source, then update \
             adjudicated-overrides.json's source_sha256 (and re-derive the verdict if the \
             call site itself changed).",
            path.display(),
            ov.unit,
            ov.line,
        );

        // ── callee_text sanity: still on the claimed (1-based) line ────────
        let line_1based = ov.line as usize + 1;
        let lines: Vec<&str> = content.lines().collect();
        let line_text = lines.get(line_1based - 1).copied().unwrap_or("");
        assert!(
            line_text
                .to_ascii_lowercase()
                .contains(&ov.callee_text.to_ascii_lowercase()),
            "callee_text {:?} not found on {}:{} (line drifted?) — line reads: {:?}",
            ov.callee_text,
            ov.unit,
            line_1based,
            line_text,
        );

        // ── CrossAppSourceProcedure shape (beyond-1B.3b Task 5.5): a SEPARATE
        // independent-verification path — the target is a real cross-app
        // procedure, not a structural-catalog builtin, so the builtin-shape
        // checks below (shape/receiver_kind, arity, catalog-membership
        // verdict derivation) do not apply. Verify against the TARGET app's
        // own embedded source instead, then move to the next entry.
        if ov.receiver_kind == "CrossAppSourceProcedure" {
            verify_cross_app_source_procedure_override(ov, &ws);
            assert_eq!(
                ov.verdict, VERDICT_L3_ERROR_INTRINSIC,
                "{}:{}: CrossAppSourceProcedure entries must be verdict l3_error_intrinsic",
                ov.unit, ov.line
            );
            l3_error_intrinsic += 1;
            continue;
        }

        // ── SameAppSourceProcedure shape (pageext-merge-and-final-residual
        // plan, Task 2): the same-app analog of CrossAppSourceProcedure — the
        // target routine lives in the CALLER'S OWN app (a compiler-grounded
        // bare-implicit-Rec dispatch that L3's frozen golden mis-paired or
        // missed), so it is verified against the LIVE workspace source tree
        // directly (`target_unit`), never `.alpackages`. ─────────────────────
        if ov.receiver_kind == "SameAppSourceProcedure" {
            verify_same_app_source_procedure_override(ov, &ws);
            assert_eq!(
                ov.verdict, VERDICT_L3_ERROR_INTRINSIC,
                "{}:{}: SameAppSourceProcedure entries must be verdict l3_error_intrinsic",
                ov.unit, ov.line
            );
            l3_error_intrinsic += 1;
            continue;
        }

        // ── shape / receiver_kind cross-check — BEFORE trusting either ──────
        assert_shape_matches_receiver_kind(ov, &content);

        // ── arity cross-check — BEFORE trusting `arity` ─────────────────────
        // Only where source-level comma counting is a sound oracle for the
        // recorded arity (see `arity_source_count_is_sound`: the object-run
        // static forms carry an object-designator first argument whose
        // arity convention the overlay does not fix, so they are skipped).
        if arity_source_count_is_sound(&ov.callee_text) {
            match count_call_arity_on_line(line_text, &ov.callee_text) {
                Some(counted_arity) => {
                    assert_eq!(
                        counted_arity, ov.arity,
                        "{}:{}: counted {counted_arity} top-level argument(s) for {:?} at the \
                         call site, but the committed arity is {} — arity cross-check mismatch \
                         (re-verify the site)",
                        ov.unit, ov.line, ov.callee_text, ov.arity,
                    );
                }
                None => {
                    eprintln!(
                        "NOTE: arity cross-check skipped for {}:{} ({:?}) — could not robustly \
                         parse a single-line balanced argument list (conservative bail-out, not \
                         a failure)",
                        ov.unit, ov.line, ov.callee_text,
                    );
                }
            }
        } else {
            eprintln!(
                "NOTE: arity cross-check skipped for {}:{} ({:?}) — object-run static dispatch \
                 form (object-designator first argument makes source comma count an unsound \
                 arity oracle; shape/receiver-kind still checked)",
                ov.unit, ov.line, ov.callee_text,
            );
        }

        // ── independent verdict re-derivation ───────────────────────────────
        let verdict = derive_verdict(ov, &content);
        assert_eq!(
            verdict, ov.verdict,
            "independently-derived verdict for {}:{} (catalog_key={:?}, receiver_kind={:?}) \
             is {:?}, but the committed adjudication says {:?} — re-investigate before \
             trusting the overlay.",
            ov.unit, ov.line, ov.catalog_key, ov.receiver_kind, verdict, ov.verdict,
        );

        match verdict {
            "l3_error_intrinsic" => l3_error_intrinsic += 1,
            "fresh_false_builtin" => fresh_false_builtin += 1,
            _ => needs_manual_review += 1,
        }
    }

    eprintln!(
        "Test 19b — independent source adjudication: l3_error_intrinsic={l3_error_intrinsic} \
         fresh_false_builtin={fresh_false_builtin} needs_manual_review={needs_manual_review} \
         (total={})",
        overrides.entries.len(),
    );

    assert_eq!(
        needs_manual_review, 0,
        "needs_manual_review must be 0 — any survivor is an unresolved adjudication \
         dimension (fail-closed, never auto-passed)"
    );
    assert_eq!(
        fresh_false_builtin, 0,
        "fresh_false_builtin must be 0 — any survivor is a real fresh-catalog bug that \
         Tasks 1-2 should have absorbed (source shadows catalog, or the claimed catalog \
         member doesn't actually exist)"
    );
}

// ---------------------------------------------------------------------------
// Test 20 (fixture + CDO env-gated): 1B.3b Task 2 — fan-out applicability
// SOUNDNESS teeth (ported into route_applicability)
// ---------------------------------------------------------------------------

/// Asserts the four 1B.3b Task 2 fan-out SOUNDNESS counters
/// (`interface_applicability_violations` / `instance_builtin_violations` /
/// `implicit_trigger_violations` / `event_violations`) are all 0 over the
/// `fanout-applicability` fixture, which genuinely exercises all four
/// dispatch kinds end-to-end through `resolve_full_program` (not
/// hand-constructed edges — see `semantic_golden.rs`'s own unit tests for the
/// hand-built positive/negative predicate-level proof that the teeth bite).
/// Also asserts `resolve_full_program`'s fixture output actually contains a
/// Polymorphic Call edge, a Multicast ImplicitTrigger edge, an EventFlow edge
/// with >=1 Routine route, and a `PageInstance::` Builtin route — so this
/// assertion is not vacuous.
///
/// SOUNDNESS, not completeness: these check that every fan-out route the
/// resolver DID emit is individually well-formed/applicable — NOT that the
/// resolver emitted every route it should have (that's the frozen,
/// L3-validated goldens' job — 1B.3a/1B.3b Task 1). Distinct from
/// `route_applicability_zero_violations` (Test 15)'s structural
/// witness-contract/ABI checks; `ApplicabilityReport::is_clean()` now folds
/// both families together, so Test 15 already covers this on the same
/// fixture/CDO inputs — this test adds the targeted per-kind assertions and
/// the fixture-exercises-every-kind sanity check.
#[test]
fn fan_out_applicability_zero_violations() {
    // ── Fixture (no env needed) ───────────────────────────────────────────────
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/fanout-applicability");

    let program_report =
        resolve_full_program(&fixture).expect("resolve_full_program must succeed on fixture");
    let has_polymorphic_call = program_report
        .edges
        .iter()
        .any(|ce| ce.edge.kind == EdgeKind::Call && ce.edge.shape == DispatchShape::Polymorphic);
    let has_multicast_trigger = program_report.edges.iter().any(|ce| {
        ce.edge.kind == EdgeKind::ImplicitTrigger && ce.edge.shape == DispatchShape::Multicast
    });
    let has_event_flow_route = program_report.edges.iter().any(|ce| {
        ce.edge.kind == EdgeKind::EventFlow
            && ce
                .edge
                .routes
                .iter()
                .any(|r| matches!(r.target, RouteTarget::Routine(_)))
    });
    let has_page_instance_builtin = program_report.edges.iter().any(|ce| {
        ce.edge.routes.iter().any(
            |r| matches!(&r.target, RouteTarget::Builtin(b) if b.0.starts_with("PageInstance::")),
        )
    });
    assert!(
        has_polymorphic_call,
        "fixture must exercise an Interface (Polymorphic) Call edge"
    );
    assert!(
        has_multicast_trigger,
        "fixture must exercise a Multicast ImplicitTrigger edge"
    );
    assert!(
        has_event_flow_route,
        "fixture must exercise an EventFlow edge with a Routine route"
    );
    assert!(
        has_page_instance_builtin,
        "fixture must exercise a PageInstance:: instance-builtin route"
    );

    let appl = run_route_applicability(&fixture);
    assert_eq!(
        appl.interface_applicability_violations, 0,
        "Interface fan-out soundness violated on fixture"
    );
    assert_eq!(
        appl.instance_builtin_violations, 0,
        "instance-builtin/enum-static soundness violated on fixture"
    );
    assert_eq!(
        appl.implicit_trigger_violations, 0,
        "ImplicitTrigger fan-out soundness violated on fixture"
    );
    assert_eq!(
        appl.event_violations, 0,
        "EventFlow soundness violated on fixture"
    );
    assert!(
        appl.is_clean(),
        "route-applicability contract violated on fixture: {appl:?}"
    );
    eprintln!(
        "Test 20 (fixture) — fan-out applicability: interface=0 instance_builtin=0 \
         implicit_trigger=0 event=0 (total_routes={}) \
         routes_checked[interface={} instance_builtin={} implicit_trigger={} event={}]",
        appl.total_routes,
        appl.interface_routes_checked,
        appl.instance_builtin_routes_checked,
        appl.implicit_trigger_routes_checked,
        appl.event_routes_checked,
    );

    // ── CDO (env-gated) ───────────────────────────────────────────────────────
    let Some(shared) = cdo_shared() else {
        return;
    };

    let appl_cdo = run_route_applicability_on(&shared.ctx, &shared.report);
    assert_eq!(
        appl_cdo.interface_applicability_violations, 0,
        "Interface fan-out soundness violated on CDO_WS — a real bug, investigate \
         and fix (do not relax)."
    );
    assert_eq!(
        appl_cdo.instance_builtin_violations, 0,
        "instance-builtin/enum-static soundness violated on CDO_WS — investigate."
    );
    assert_eq!(
        appl_cdo.implicit_trigger_violations, 0,
        "ImplicitTrigger fan-out soundness violated on CDO_WS — investigate."
    );
    assert_eq!(
        appl_cdo.event_violations, 0,
        "EventFlow soundness violated on CDO_WS — investigate."
    );
    // ── Fix 2 (1B.3b whole-branch fix): non-vacuity must be ASSERTED, not just
    // printed. `violations == 0` is meaningless if `routes_checked == 0` — a
    // `build_fan_out_site_context` regression silently dropping context would
    // collapse every denominator to 0 and pass vacuously. Fail closed instead.
    assert!(
        appl_cdo.interface_routes_checked > 0
            && appl_cdo.instance_builtin_routes_checked > 0
            && appl_cdo.implicit_trigger_routes_checked > 0
            && appl_cdo.event_routes_checked > 0,
        "VACUOUS PASS: routes_checked[interface={} instance_builtin={} \
         implicit_trigger={} event={}] must all be NON-TRIVIAL (>0) — a \
         collapse toward 0 with violations==0 signals a build_fan_out_site_context \
         regression silently dropping context, not a genuine clean run.",
        appl_cdo.interface_routes_checked,
        appl_cdo.instance_builtin_routes_checked,
        appl_cdo.implicit_trigger_routes_checked,
        appl_cdo.event_routes_checked,
    );
    eprintln!(
        "Test 20 (CDO) — fan-out applicability: total_routes={} \
         interface_violations={} instance_builtin_violations={} \
         implicit_trigger_violations={} event_violations={} (all must be 0)\n\
         Test 20 (CDO) — non-vacuity routes_checked: interface={} instance_builtin={} \
         implicit_trigger={} event={} (each must be NON-TRIVIAL — a collapse toward 0 \
         with violations==0 would signal a vacuous pass, e.g. a \
         build_fan_out_site_context regression silently dropping context)",
        appl_cdo.total_routes,
        appl_cdo.interface_applicability_violations,
        appl_cdo.instance_builtin_violations,
        appl_cdo.implicit_trigger_violations,
        appl_cdo.event_violations,
        appl_cdo.interface_routes_checked,
        appl_cdo.instance_builtin_routes_checked,
        appl_cdo.implicit_trigger_routes_checked,
        appl_cdo.event_routes_checked,
    );
}

// ---------------------------------------------------------------------------
// Tests 21+: beyond-1B.3b Task 1 — source shadows builtin (lookup precedence)
// + structural builtin-catalog match, end-to-end over `ws-builtin-shadow`.
//
// Root-cause fix: `resolve_member`'s Record arm was catalog-FIRST (a user
// table method whose name+arity matched a genuine Record builtin was
// mis-classified `Catalog` instead of the local `Source`). AL semantics: a
// visible source/ABI routine of matching name+arity SHADOWS a same-named
// intrinsic. This is the exact shape behind the 42 real CDO
// `builtin-catalog-fp-collision` divergences.
// ---------------------------------------------------------------------------

/// Loads `tests/r0-corpus/ws-builtin-shadow` and returns the full
/// `resolve_full_program` report — shared by Tests 21a-21e below.
fn ws_builtin_shadow_report() -> ProgramReport {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-builtin-shadow");
    resolve_full_program(&fixture).expect("resolve_full_program must succeed on ws-builtin-shadow")
}

/// All classified edges whose call-site obligation's caller routine has
/// `name_lc == caller_name_lc` (case-insensitive by construction — callers
/// pass already-lowercased names).
fn edges_for_caller<'a>(
    report: &'a ProgramReport,
    caller_name_lc: &str,
) -> Vec<&'a ClassifiedEdge> {
    report
        .edges
        .iter()
        .filter(|ce| match &ce.obligation_id {
            ObligationId::CallSite { caller, .. } => caller.name_lc == caller_name_lc,
            ObligationId::Publisher(_) => false,
        })
        .collect()
}

/// Test 21a (fixture a): `R.FieldNo('No.')` on a `Record Acme` whose table
/// declares its OWN `FieldNo(FieldName: Text): Integer` (arity 1, matching the
/// call) must resolve to `Acme.FieldNo` with `Evidence::Source` — NOT the
/// `Record::fieldno` catalog builtin (today, pre-fix: catalog-first → false
/// `builtin`).
#[test]
fn ws_builtin_shadow_record_member_source_shadows_catalog() {
    let report = ws_builtin_shadow_report();
    let edges = edges_for_caller(&report, "calla");
    assert_eq!(
        edges.len(),
        1,
        "CallA must have exactly one call obligation"
    );
    let routes = &edges[0].edge.routes;
    assert_eq!(
        routes.len(),
        1,
        "Record member call is single-dispatch (Exact)"
    );
    let route = &routes[0];

    assert_eq!(
        route.evidence,
        Evidence::Source,
        "local Acme.FieldNo must SHADOW the Record::fieldno catalog builtin; got {route:?}"
    );
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected RouteTarget::Routine, got {:?}", route.target);
    };
    assert_eq!(rid.name_lc, "fieldno");
    assert_eq!(rid.object.kind, ObjectKind::Table);
    assert_eq!(rid.params_count, 1);
    assert!(
        matches!(route.witness, Witness::SourceSpan { .. }),
        "witness must be SourceSpan; got {:?}",
        route.witness
    );
}

/// Test 21b (fixture b): bare `Error('boom')` inside a Codeunit that ALSO
/// declares a local `procedure Error(Msg: Text)` must resolve to the LOCAL
/// `Error` (`Evidence::Source`), not the `error` global intrinsic.
///
/// NOTE: `resolve_bare`'s own-object-first precedence (module doc,
/// `resolver.rs:1-12`) already implemented this correctly BEFORE the Task 1
/// fix — the only genuinely catalog-FIRST arm was `resolve_member`'s Record
/// arm (Test 21a). This is kept as the brief-mandated regression-locking
/// fixture, not as a second bug reproduction.
#[test]
fn ws_builtin_shadow_bare_source_shadows_catalog() {
    let report = ws_builtin_shadow_report();
    let edges = edges_for_caller(&report, "callb");
    assert_eq!(
        edges.len(),
        1,
        "CallB must have exactly one call obligation"
    );
    let routes = &edges[0].edge.routes;
    assert_eq!(routes.len(), 1);
    let route = &routes[0];

    assert_eq!(
        route.evidence,
        Evidence::Source,
        "local ShadowCallerB.Error must SHADOW the global `error` intrinsic; got {route:?}"
    );
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected RouteTarget::Routine, got {:?}", route.target);
    };
    assert_eq!(rid.name_lc, "error");
    assert_eq!(rid.object.kind, ObjectKind::Codeunit);
}

/// Test 21c (fixture c): genuine builtins with ZERO source competitors
/// anywhere in the workspace must STAY `Catalog` after the precedence fix —
/// `R.FieldCaption(1)` (Record builtin), bare `Message('hi')` (global
/// builtin), and `J.Add(...)` on a `JsonObject` (framework-member builtin).
///
/// NOTE: deliberately NOT a `record_op_names()` method (`SetRange`/`Insert`/
/// `Modify`/...) — those 28 names are classified `CalleeShape::RecordOp`
/// (`extract.rs`) and resolved via the SEPARATE implicit-trigger path, not
/// `resolve_member`'s Record arm; `FieldCaption` exercises the Record-arm
/// catalog fallback this test targets.
#[test]
fn ws_builtin_shadow_genuine_builtins_stay_catalog() {
    let report = ws_builtin_shadow_report();
    let edges = edges_for_caller(&report, "callc");
    assert_eq!(edges.len(), 3, "CallC has 3 call obligations");

    let mut catalog_ids: Vec<String> = Vec::new();
    for ce in &edges {
        assert_eq!(ce.edge.routes.len(), 1);
        let route = &ce.edge.routes[0];
        assert_eq!(
            route.evidence,
            Evidence::Catalog,
            "genuine builtin with no source competitor must stay Catalog; got {route:?}"
        );
        let RouteTarget::Builtin(BuiltinId(ref id)) = route.target else {
            panic!("expected RouteTarget::Builtin, got {:?}", route.target);
        };
        assert!(
            matches!(route.witness, Witness::CatalogEntry { .. }),
            "witness must be CatalogEntry; got {:?}",
            route.witness
        );
        catalog_ids.push(id.clone());
    }
    catalog_ids.sort();
    assert_eq!(
        catalog_ids,
        vec![
            "JsonObject::add".to_string(),
            "Record::fieldcaption".to_string(),
            "message".to_string(),
        ],
        "all three genuine-builtin call sites must resolve to their expected catalog ids"
    );
}

/// Test 21d (fixture d): a near-miss name (`ZzNotARealBuiltinFp`, not a real
/// catalog member despite being textually adjacent to real builtins) must NOT
/// be classified `builtin` — falls through to honest `Unknown`. Locks in that
/// the catalog match is exact-string (no fingerprint/hash digest — see
/// `builtins.rs`/`member_catalog.rs`), so a fabricated "fingerprint collision"
/// cannot surface as a false `builtin`.
#[test]
fn ws_builtin_shadow_near_miss_name_is_not_classified_builtin() {
    let report = ws_builtin_shadow_report();
    let edges = edges_for_caller(&report, "calld");
    assert_eq!(
        edges.len(),
        1,
        "CallD must have exactly one call obligation"
    );
    let routes = &edges[0].edge.routes;
    assert_eq!(routes.len(), 1);
    let route = &routes[0];

    assert_eq!(
        route.target,
        RouteTarget::Unresolved,
        "near-miss name must NOT resolve to any target; got {:?}",
        route.target
    );
    assert!(
        matches!(route.evidence, Evidence::Unknown(_)),
        "near-miss name must NOT be classified Catalog; got {route:?}"
    );
    assert_eq!(route.witness, Witness::None);
}

/// Test 21e (fixture e): qualified-intrinsic bypass. `CreateGuid()` (bare,
/// inside a Codeunit that ALSO declares a local `procedure CreateGuid(): Guid`)
/// must resolve to the LOCAL Source (shadowing the global `createguid`
/// intrinsic); `System.CreateGuid()` (fully qualified) must STILL bind to the
/// `System::createguid` Catalog entry — the local declaration does NOT shadow
/// a qualified platform call, because `System.*` is dispatched via the
/// `Framework(System)` singleton receiver, which never consults source
/// candidates (a structurally distinct path from the bare-call shadow check;
/// see `ws-builtin-shadow` Step-3 investigation note in the Task 1 report).
#[test]
fn ws_builtin_shadow_qualified_intrinsic_bypasses_local_shadow() {
    let report = ws_builtin_shadow_report();
    let edges = edges_for_caller(&report, "calle");
    assert_eq!(
        edges.len(),
        2,
        "CallE has 2 call obligations (bare + qualified)"
    );

    let mut source_hit = false;
    let mut catalog_hit = false;
    for ce in &edges {
        assert_eq!(ce.edge.routes.len(), 1);
        let route = &ce.edge.routes[0];
        match route.evidence {
            Evidence::Source => {
                source_hit = true;
                let RouteTarget::Routine(ref rid) = route.target else {
                    panic!("expected RouteTarget::Routine, got {:?}", route.target);
                };
                assert_eq!(rid.name_lc, "createguid");
                assert_eq!(rid.object.kind, ObjectKind::Codeunit);
            }
            Evidence::Catalog => {
                catalog_hit = true;
                let RouteTarget::Builtin(BuiltinId(ref id)) = route.target else {
                    panic!("expected RouteTarget::Builtin, got {:?}", route.target);
                };
                assert_eq!(
                    id, "System::createguid",
                    "qualified call must bind to the System Framework catalog entry"
                );
            }
            other => panic!("unexpected evidence {other:?} on CallE route: {route:?}"),
        }
    }
    assert!(source_hit, "bare CreateGuid() must resolve to local Source");
    assert!(
        catalog_hit,
        "System.CreateGuid() must resolve to Catalog despite the local shadow"
    );
}

// ---------------------------------------------------------------------------
// Test 22: beyond-1B.3b Task 1 REVIEW FIX (Finding 1) — base-table wrong-arity
// falls through to a TableExtension's matching-arity overload.
//
// The precedence rewrite (Test 21a) made a real, correct, but previously
// undisclosed secondary behavior change: pre-fix, ANY name match on the base
// table (regardless of arity) short-circuited the Record arm straight to that
// base-table routine (or, in the catalog-first world, to the catalog) without
// ever considering a TableExtension. Post-fix, `object_has_member_candidate`
// requires an EXACT arity match for Source/ABI-tier objects, so a base-table
// name-only match with the wrong arity is no longer a candidate at all — the
// scope walk correctly falls through to a TableExtension that DOES declare
// the matching arity. `tests/r0-corpus/ws-builtin-shadow-arity/`: `BaseTable`
// declares `Foo()` (arity 0); `BaseTableExt` (a TableExtension of it)
// declares `Foo(X: Integer)` (arity 1); the caller does `R.Foo(5)` (arity 1).
// ---------------------------------------------------------------------------

/// Loads `tests/r0-corpus/ws-builtin-shadow-arity` and returns the full
/// `resolve_full_program` report.
fn ws_builtin_shadow_arity_report() -> ProgramReport {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-builtin-shadow-arity");
    resolve_full_program(&fixture)
        .expect("resolve_full_program must succeed on ws-builtin-shadow-arity")
}

/// `R.Foo(5)` (arity 1) must resolve to `BaseTableExt.Foo` (`Evidence::Source`)
/// — the base table's `Foo()` (arity 0) name-matches but arity-mismatches, so
/// it must NOT short-circuit the scope walk; it must NOT resolve to
/// `Unresolved`/`Unknown` either (that would mean the wrong-arity base-table
/// match incorrectly suppressed the extension candidate, or the ambiguity
/// branch incorrectly fired for what is actually a single valid candidate).
///
/// Sanity-checked by reasoning against `object_has_member_candidate`
/// (`resolver.rs`): for a Source-tier object, `candidates.iter().any(|rid|
/// rid.params_count == arity)` — the base table's ONLY `foo` candidate has
/// `params_count == 0 != 1`, so `object_has_member_candidate` returns `false`
/// for the base table and the arity-1 scan advances to the TableExtension,
/// which has exactly one `params_count == 1` match. If the pre-fix
/// any-name-match short-circuit were reintroduced (matching on name alone,
/// ignoring arity), the base table would wrongly become the sole "candidate"
/// found and `resolve_in_object` would either mis-resolve to the arity-0
/// `Foo` or fail to find an arity-1 routine there and fall through to the
/// Record builtin catalog (there is no arity-1 Record builtin named `Foo`),
/// landing on `Unresolved`/`Unknown` — either way NOT this test's asserted
/// `Evidence::Source` + `TableExtension` target, so this test fails under
/// that regression.
#[test]
fn ws_builtin_shadow_arity_base_wrong_arity_falls_through_to_extension() {
    let report = ws_builtin_shadow_arity_report();
    let edges = edges_for_caller(&report, "callfoo");
    assert_eq!(
        edges.len(),
        1,
        "CallFoo must have exactly one call obligation"
    );
    let routes = &edges[0].edge.routes;
    assert_eq!(
        routes.len(),
        1,
        "Record member call is single-dispatch (Exact)"
    );
    let route = &routes[0];

    assert_eq!(
        route.evidence,
        Evidence::Source,
        "arity-1 call must resolve to the TableExtension's Foo(X: Integer), \
         not fall through to Unknown or mis-hit the base table's arity-0 Foo(); \
         got {route:?}"
    );
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!(
            "expected RouteTarget::Routine (the TableExtension's Foo), got {:?}",
            route.target
        );
    };
    assert_eq!(rid.name_lc, "foo");
    assert_eq!(rid.params_count, 1);
    assert_eq!(
        rid.object.kind,
        ObjectKind::TableExtension,
        "must resolve to the TableExtension's overload, not the base table's; got {:?}",
        rid.object.kind
    );
    assert!(
        matches!(route.witness, Witness::SourceSpan { .. }),
        "witness must be SourceSpan; got {:?}",
        route.witness
    );
}

// ---------------------------------------------------------------------------
// Tests 23+: beyond-1B.3b Task 2 — fail-closed same-arity SOURCE-overload
// guard. `resolve_in_object` used to pick the FIRST arity-matched candidate
// with no ambiguity check; worse, two same-name/same-arity SOURCE overloads
// collapse to one `RoutineNodeId` (source `sig_fp` is always `0`), so
// `build_program_graph`'s post-sort dedup could silently drop one of them.
// `tests/r0-corpus/ws-overload-collision/`: `Ambiguous.Codeunit.al` declares
// two `Resolve` overloads (arity 1, differing only by param TYPE); `Caller`
// invokes `Target.Resolve(5)` (member-Object dispatch). A single-overload
// `Control.Codeunit.al` proves the guard does not over-fire.
// ---------------------------------------------------------------------------

/// Loads `tests/r0-corpus/ws-overload-collision` and returns the full
/// `resolve_full_program` report — shared by Tests 23a-23c below.
fn ws_overload_collision_report() -> ProgramReport {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-overload-collision");
    resolve_full_program(&fixture)
        .expect("resolve_full_program must succeed on ws-overload-collision")
}

/// Test 23a: `Target.Resolve(5)` against a same-name/same-arity SOURCE
/// overload pair (`Resolve(X: Integer)` / `Resolve(X: Code[20])`).
/// REBASELINED (argtype-dispatch-and-page-catalog plan v2.1, Task 2 —
/// "correctness over backwards compatibility": Task 4 of the sigfp-and-
/// ambiguous-reclassification plan is superseded here for THIS specific
/// call site, not the general AmbiguousResolved mechanism, which stays live
/// for every OTHER unresolved same-arity collision).
///
/// The literal argument `5` is an INTEGER literal — the compiler-proven
/// exemplar named directly in the plan's C6 addendum: an Integer literal
/// structurally CANNOT bind a `Code[20]` parameter (an integer constant is
/// not textual data at all, not even via implicit conversion — AL has no
/// Integer->Code conversion), so eliminating the `Code[20]` overload is a
/// SOUND, compiler-correct elimination, leaving `Resolve(X: Integer)` as the
/// unique, exact-canonical-match survivor. This is no longer honest
/// `AmbiguousResolved` for this call site — the fail-closed arg-type pick
/// (`resolve_in_object`'s `_` arm, `arg_dispatch::pick_candidate`) resolves
/// it to a confident single `Source` route, `DispatchShape::Exact`.
#[test]
fn ws_overload_collision_ambiguous_call_becomes_resolved_to_the_integer_overload() {
    use al_sem::program::resolve::edge::{ObligationOutcome, classify_obligation};

    let report = ws_overload_collision_report();
    let edges = edges_for_caller(&report, "callambiguous");
    assert_eq!(
        edges.len(),
        1,
        "CallAmbiguous must have exactly one call obligation"
    );
    let edge = &edges[0].edge;
    assert_eq!(
        edge.shape,
        DispatchShape::Exact,
        "the Integer literal `5` fail-closed-picks the Integer overload — \
         single-route Exact, not AmbiguousOverload; got {:?}",
        edge.shape
    );
    let routes = &edge.routes;
    assert_eq!(
        routes.len(),
        1,
        "a confident pick emits exactly one route; got {routes:?}"
    );
    let route = &routes[0];
    assert_eq!(route.evidence, Evidence::Source, "got {route:?}");
    assert!(
        route.conditions.is_empty(),
        "a fail-closed pick is a plain Source route — no AmbiguousDispatch \
         condition; got {route:?}"
    );
    assert!(
        route.fires_by_default(),
        "a confident pick must fire by default; got {route:?}"
    );
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected a Routine target; got {route:?}");
    };
    assert_eq!(rid.params_count, 1);
    let Witness::SourceSpan { ref file, span } = route.witness else {
        panic!("expected SourceSpan witness; got {route:?}");
    };
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-overload-collision");
    let src = std::fs::read_to_string(fixture.join(file))
        .unwrap_or_else(|e| panic!("cannot read witness file {file}: {e}"));
    let decl_text = &src[span.0 as usize..span.1 as usize];
    assert!(
        decl_text.to_ascii_lowercase().contains("integer"),
        "the picked routine's own declaration text must be the Integer \
         overload; got {decl_text:?}"
    );
    assert!(
        !decl_text.to_ascii_lowercase().contains("code"),
        "the picked routine must NOT be the Code[20] overload; got {decl_text:?}"
    );

    assert_eq!(
        classify_obligation(edge),
        ObligationOutcome::Resolved,
        "a fail-closed single Source route to a proven-compatible overload \
         is Resolved, not AmbiguousResolved"
    );
}

/// Test 23a-control: `CallAmbiguousUntyped` — CORRECTED (pageext-merge-and-
/// final-residual plan, Task 3: a documented rebaseline, not a regression;
/// renamed per this codebase's M1 convention "rename, don't just flip"):
/// `Target.Resolve(GetValue())` — `GetValue()` is now a same-object
/// bare-call-result arg, `arg_dispatch::type_one_arg`'s new `Call` arm's
/// home turf (mirrors Step 5's guards; `GetValue` is unshadowed and
/// unambiguous, resolving via a SINGLE `resolve_bare` route to a source
/// routine returning `Integer`). `Integer` exact-matches `Resolve(X:
/// Integer)` and PROVABLY eliminates `Resolve(X: Code[20])` (disjoint
/// canonical families) — exactly what a real AL compiler resolves,
/// unambiguously. This fixture pre-dates the increment (comment
/// previously: "an untyped argument... stays AmbiguousResolved"); the
/// increment landing is the mechanism, this pick is the expected,
/// adjudicated consequence.
#[test]
fn ws_overload_collision_untyped_arg_call_picks_integer_overload_via_call_result_arg() {
    use al_sem::program::resolve::edge::{ObligationOutcome, classify_obligation};

    let report = ws_overload_collision_report();
    let edges = edges_for_caller(&report, "callambiguousuntyped");
    assert_eq!(
        edges.len(),
        2,
        "CallAmbiguousUntyped has two call obligations: the outer \
         Resolve(...) call and the nested GetValue() argument call"
    );
    // The outer `Resolve(GetValue())` call's span strictly CONTAINS the
    // nested `GetValue()` call's span — select the widest.
    let edge = &edges
        .iter()
        .max_by_key(|ce| ce.edge.site.span.end.col as i64 - ce.edge.site.span.start.col as i64)
        .expect("at least one call obligation")
        .edge;
    assert_eq!(
        edge.shape,
        DispatchShape::Exact,
        "the call-result's Integer return type fail-closed-picks the Integer \
         overload — single-route Exact, not AmbiguousOverload; got {:?}",
        edge.shape
    );
    let routes = &edge.routes;
    assert_eq!(
        routes.len(),
        1,
        "a confident pick emits exactly one route; got {routes:?}"
    );
    let route = &routes[0];
    assert_eq!(route.evidence, Evidence::Source, "got {route:?}");
    assert!(
        route.conditions.is_empty(),
        "a fail-closed pick is a plain Source route — no AmbiguousDispatch \
         condition; got {route:?}"
    );
    assert!(
        route.fires_by_default(),
        "a confident pick must fire by default; got {route:?}"
    );
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected a Routine target; got {route:?}");
    };
    assert_eq!(rid.params_count, 1);
    let Witness::SourceSpan { ref file, span } = route.witness else {
        panic!("expected SourceSpan witness; got {route:?}");
    };
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-overload-collision");
    let src = std::fs::read_to_string(fixture.join(file))
        .unwrap_or_else(|e| panic!("cannot read witness file {file}: {e}"));
    let decl_text = &src[span.0 as usize..span.1 as usize];
    assert!(
        decl_text.to_ascii_lowercase().contains("integer"),
        "the picked routine's own declaration text must be the Integer \
         overload; got {decl_text:?}"
    );
    assert!(
        !decl_text.to_ascii_lowercase().contains("code"),
        "the picked routine must NOT be the Code[20] overload; got {decl_text:?}"
    );

    assert_eq!(
        classify_obligation(edge),
        ObligationOutcome::Resolved,
        "a fail-closed single Source route to a proven-compatible overload \
         is Resolved, not AmbiguousResolved"
    );
}

/// Test 23b: the graph must not silently DROP one of the two colliding
/// overloads. Builds the `ProgramGraph` directly (bypassing
/// `resolve_full_program`'s obligation layer) and asserts BOTH raw `Resolve`
/// entries survive `build_program_graph`'s post-sort dedup for the
/// `Ambiguous.Codeunit.al` object — the collision is marked/preserved, never
/// silently collapsed to one entry with no record.
#[test]
fn ws_overload_collision_graph_preserves_both_overloads() {
    use al_sem::program::abi_ingest::AbiCache;
    use al_sem::program::build::build_program_graph;
    use al_sem::snapshot::SnapshotBuilder;

    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-overload-collision");
    let snap = (SnapshotBuilder {
        workspace_root: fixture,
        local_providers: vec![],
    })
    .build()
    .expect("snapshot must build for ws-overload-collision");
    let cache = AbiCache::new();
    let graph = build_program_graph(&snap, &cache);

    let resolve_entries: Vec<_> = graph
        .routines
        .iter()
        .filter(|r| r.id.name_lc == "resolve")
        .collect();
    assert_eq!(
        resolve_entries.len(),
        2,
        "both `Resolve` overloads must survive the graph build — one must \
         NEVER be silently dropped by the post-sort dedup; got {} entries: {:?}",
        resolve_entries.len(),
        resolve_entries.iter().map(|r| &r.name).collect::<Vec<_>>()
    );
    assert!(
        resolve_entries.iter().all(|r| r.id.params_count == 1),
        "both overloads share arity 1 (the genuine collision shape); got {:?}",
        resolve_entries
            .iter()
            .map(|r| r.id.params_count)
            .collect::<Vec<_>>()
    );

    // Sanity: the single-overload control target must NOT be duplicated —
    // proves the preservation logic is collision-specific, not a blanket
    // "never dedup" change.
    let solo_entries: Vec<_> = graph
        .routines
        .iter()
        .filter(|r| r.id.name_lc == "solo")
        .collect();
    assert_eq!(
        solo_entries.len(),
        1,
        "the single-overload control routine must be exactly one entry \
         (no spurious ambiguity); got {} entries",
        solo_entries.len()
    );
}

/// Test 23c (control): a single-overload target (`Control.Solo`) must still
/// resolve cleanly to `Evidence::Source` — the ambiguity guard must not
/// over-fire on an ordinary, unambiguous procedure.
#[test]
fn ws_overload_collision_control_single_overload_resolves_cleanly() {
    let report = ws_overload_collision_report();
    let edges = edges_for_caller(&report, "callcontrol");
    assert_eq!(
        edges.len(),
        1,
        "CallControl must have exactly one call obligation"
    );
    let routes = &edges[0].edge.routes;
    assert_eq!(routes.len(), 1);
    let route = &routes[0];

    assert_eq!(
        route.evidence,
        Evidence::Source,
        "the single-overload control target must resolve cleanly; got {route:?}"
    );
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected RouteTarget::Routine, got {:?}", route.target);
    };
    assert_eq!(rid.name_lc, "solo");
    assert_eq!(rid.params_count, 1);
    assert_eq!(rid.object.kind, ObjectKind::Codeunit);
    assert!(
        matches!(route.witness, Witness::SourceSpan { .. }),
        "witness must be SourceSpan; got {:?}",
        route.witness
    );
}

// ---------------------------------------------------------------------------
// Tests 23f+: argtype-dispatch-and-page-catalog plan (v2.1), Task 2 —
// fail-closed arg-type overload dispatch. Wires the pre-authored ORPHANED
// fixture banks `ws-overload-arg-type` / `ws-overload-arg-pos2` /
// `ws-overload-negatives` (commit `b4ff081`) plus the deferred-increment
// guard banks `-enum-discriminator` / `-field-discriminator` /
// `-callexpr-discriminator`.
// ---------------------------------------------------------------------------

fn ws_overload_argtype_report() -> ProgramReport {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-overload-arg-type");
    resolve_full_program(&fixture)
        .expect("resolve_full_program must succeed on ws-overload-arg-type")
}

/// The OUTERMOST call obligation for `caller_name_lc` — selects the WIDEST
/// span, so a call whose own argument list contains a NESTED call
/// obligation (e.g. `T.P(GetCount())`, where `GetCount()` is itself a
/// separate obligation `collect_calls_v2` recurses into) always returns the
/// outer call, never the inner one. Trivially correct when there is only one
/// call obligation for `caller_name_lc` (mirrors the widest-span selection
/// pattern used throughout this file for compound-receiver fixtures).
fn outer_call_edge<'a>(report: &'a ProgramReport, caller_name_lc: &str) -> &'a ClassifiedEdge {
    edges_for_caller(report, caller_name_lc)
        .into_iter()
        .filter(|ce| ce.edge.kind == EdgeKind::Call)
        .max_by_key(|ce| ce.edge.site.span.end.col as i64 - ce.edge.site.span.start.col as i64)
        .expect("at least one Call-kind edge")
}

/// Test 23f: `RunImport(InStr: InStream; Setup: Record "Probe Ovl Setup")`
/// calls `Importer.ImportToFileArchive(InStr, Setup)` — position 0's
/// declared `InStream` var EXACTLY matches only the `(InStream, Record)`
/// overload (`(Text, Record)` is eliminated — `InStream` and `Text` are
/// fully disjoint runtime representations, no AL conversion bridges them);
/// position 1 (`Record "Probe Ovl Setup"`) resolves to the SAME table on
/// both candidates (non-discriminating, but not blocking). A confident
/// single `Source` pick.
#[test]
fn ws_overload_argtype_run_import_picks_instream_overload() {
    let report = ws_overload_argtype_report();
    let edge = &outer_call_edge(&report, "runimport").edge;
    assert_eq!(edge.shape, DispatchShape::Exact, "got {:?}", edge.shape);
    assert_eq!(edge.routes.len(), 1, "got {:?}", edge.routes);
    let route = &edge.routes[0];
    assert_eq!(route.evidence, Evidence::Source, "got {route:?}");
    assert!(route.fires_by_default(), "got {route:?}");
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected a Routine target; got {route:?}");
    };
    assert_eq!(rid.params_count, 2);
    let Witness::SourceSpan { ref file, span } = route.witness else {
        panic!("expected SourceSpan witness; got {route:?}");
    };
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-overload-arg-type");
    let src = std::fs::read_to_string(fixture.join(file)).expect("read witness file");
    let decl_text = src[span.0 as usize..span.1 as usize].to_ascii_lowercase();
    assert!(
        decl_text.contains("instream"),
        "picked routine must be the InStream overload; got {decl_text:?}"
    );
}

/// Test 23g: `RunAmbiguous(Setup)` calls `Importer.ImportToFileArchive
/// (Anything, Setup)` where `Anything: Variant` — the FIRST candidate
/// param (`Text`) genuinely could not be eliminated by the naive
/// text-identity model, but under exact-canonical-match neither `Text` nor
/// `InStream` equals `Variant` — 0 exact matches, never a fabricated pick.
/// Must stay `AmbiguousResolved` — this is the fixture's own documented
/// "must not fabricate a resolution here" guard.
#[test]
fn ws_overload_argtype_run_ambiguous_stays_ambiguous_resolved() {
    use al_sem::program::resolve::edge::{ObligationOutcome, classify_obligation};

    let report = ws_overload_argtype_report();
    let edge = &outer_call_edge(&report, "runambiguous").edge;
    assert_eq!(
        edge.shape,
        DispatchShape::AmbiguousOverload,
        "got {:?}",
        edge.shape
    );
    assert_eq!(edge.routes.len(), 2, "got {:?}", edge.routes);
    assert_eq!(
        classify_obligation(edge),
        ObligationOutcome::AmbiguousResolved,
        "a Variant-typed argument proves nothing about either candidate — \
         no arg-type evidence exists to disambiguate"
    );
}

fn ws_overload_argpos2_report() -> ProgramReport {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-overload-arg-pos2");
    resolve_full_program(&fixture)
        .expect("resolve_full_program must succeed on ws-overload-arg-pos2")
}

/// Test 23h: `RunWithPlainArg(InStr, Acc)` calls `Sha.M(Tok, InStr, Acc)` —
/// the DISCRIMINATING position is position 1 (`Tok: Integer` at position 0
/// is identical on both `M` overloads, non-discriminating; `Acc: Code[20]`
/// at position 2 is ALSO identical on both). Position 1's declared
/// `InStream` var picks `M(Tok: Integer; S: InStream; Acc: Code[20])`.
#[test]
fn ws_overload_argpos2_run_with_plain_arg_picks_instream_overload() {
    let report = ws_overload_argpos2_report();
    let edge = &outer_call_edge(&report, "runwithplainarg").edge;
    assert_eq!(edge.shape, DispatchShape::Exact, "got {:?}", edge.shape);
    assert_eq!(edge.routes.len(), 1, "got {:?}", edge.routes);
    let route = &edge.routes[0];
    assert_eq!(route.evidence, Evidence::Source, "got {route:?}");
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected a Routine target; got {route:?}");
    };
    assert_eq!(rid.name_lc, "m");
    assert_eq!(rid.params_count, 3);
    let Witness::SourceSpan { ref file, span } = route.witness else {
        panic!("expected SourceSpan witness; got {route:?}");
    };
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-overload-arg-pos2");
    let src = std::fs::read_to_string(fixture.join(file)).expect("read witness file");
    let decl_text = src[span.0 as usize..span.1 as usize].to_ascii_lowercase();
    assert!(
        decl_text.contains("instream"),
        "picked routine must be the InStream overload; got {decl_text:?}"
    );
}

/// Test 23i — CORRECTED (pageext-merge-and-final-residual plan, Task 3: a
/// documented rebaseline, not a regression; renamed per this codebase's M1
/// convention "rename, don't just flip"): `RunWithCallExprArg(InStr, Acc)`
/// calls `Sha.InsertWithSha1(GetLog(), InStr, Acc)` — position 0
/// (`GetLog(): Interface "Probe ILog"`) is now typed by `arg_dispatch::
/// type_one_arg`'s new `Call` arm (unshadowed, unambiguous bare call);
/// both `InsertWithSha1` candidates share the IDENTICAL `Interface "Probe
/// ILog"` parameter at position 0 (non-discriminating — typing it does NOT
/// block the pick). Position 1's declared `InStr: InStream` var — ALREADY
/// discriminating before this task — now drives a clean single pick, since
/// position 0 no longer degrades the whole call. This fixture pre-dates
/// the increment (comment previously: "position 0... is UNTYPED... degrades
/// the WHOLE call"); the increment landing is the mechanism, this pick is
/// the expected, adjudicated consequence.
#[test]
fn ws_overload_argpos2_run_with_call_expr_arg_picks_instream_overload() {
    let report = ws_overload_argpos2_report();
    let edge = &outer_call_edge(&report, "runwithcallexprarg").edge;
    assert_eq!(edge.shape, DispatchShape::Exact, "got {:?}", edge.shape);
    assert_eq!(edge.routes.len(), 1, "got {:?}", edge.routes);
    let route = &edge.routes[0];
    assert_eq!(route.evidence, Evidence::Source, "got {route:?}");
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected a Routine target; got {route:?}");
    };
    assert_eq!(rid.name_lc, "insertwithsha1");
    assert_eq!(rid.params_count, 3);
    let Witness::SourceSpan { ref file, span } = route.witness else {
        panic!("expected SourceSpan witness; got {route:?}");
    };
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-overload-arg-pos2");
    let src = std::fs::read_to_string(fixture.join(file)).expect("read witness file");
    let decl_text = src[span.0 as usize..span.1 as usize].to_ascii_lowercase();
    assert!(
        decl_text.contains("instream"),
        "picked routine must be the InStream overload; got {decl_text:?}"
    );
}

fn ws_overload_negatives_report() -> ProgramReport {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-overload-negatives");
    resolve_full_program(&fixture)
        .expect("resolve_full_program must succeed on ws-overload-negatives")
}

/// Shared assertion for every `ws-overload-negatives` case: the call must
/// stay honest `AmbiguousResolved` — two concrete `Source` routes, each
/// carrying `Condition::AmbiguousDispatch`, never a guessed pick.
fn assert_stays_ambiguous_resolved(report: &ProgramReport, caller_name_lc: &str) {
    use al_sem::program::resolve::edge::{Condition, ObligationOutcome, classify_obligation};

    let edge = &outer_call_edge(report, caller_name_lc).edge;
    assert_eq!(
        edge.shape,
        DispatchShape::AmbiguousOverload,
        "{caller_name_lc}: got {:?}",
        edge.shape
    );
    let routes = &edge.routes;
    assert_eq!(routes.len(), 2, "{caller_name_lc}: got {routes:?}");
    for route in routes {
        assert_eq!(
            route.evidence,
            Evidence::Source,
            "{caller_name_lc}: got {route:?}"
        );
        assert!(
            route.conditions.contains(&Condition::AmbiguousDispatch),
            "{caller_name_lc}: got {route:?}"
        );
        assert!(!route.fires_by_default(), "{caller_name_lc}: got {route:?}");
    }
    assert_eq!(
        classify_obligation(edge),
        ObligationOutcome::AmbiguousResolved,
        "{caller_name_lc}: must stay AmbiguousResolved, never a guessed pick"
    );
}

/// Test 23j: `CallVariant(S: InStream)` calls `T.V(S)` where `V` overloads
/// on `(Variant)` / `(Integer)` — the Round-1 addendum's Variant-wildcard
/// rule (I5): a Variant candidate at a discriminating position degrades the
/// WHOLE call, even though a naive exclusion-style matcher would have
/// eliminated `Integer` (InStream vs Integer are disjoint) and left Variant
/// as an UNPROVEN "sole survivor" — that is not a confident pick.
#[test]
fn ws_overload_negatives_call_variant_stays_ambiguous_resolved() {
    assert_stays_ambiguous_resolved(&ws_overload_negatives_report(), "callvariant");
}

/// Test 23k: `CallIndistinct(A: Integer; B: Text)` calls `T.I(A, B)` where
/// `I` overloads on `(Integer, Text)` / `(Integer, Code[20])` — position 0
/// is identical on both (non-discriminating); position 1's declared `Text`
/// arg EXACTLY matches the `Text` candidate, but `Code[20]` is NOT
/// eliminated (Text and Code are the SAME "text-ish" soft family — AL's own
/// Text<->Code conversions mean a declared Text var is not PROVEN
/// incompatible with a `Code[20]` parameter). The undecided `Code[20]`
/// candidate blocks the pick.
#[test]
fn ws_overload_negatives_call_indistinct_stays_ambiguous_resolved() {
    assert_stays_ambiguous_resolved(&ws_overload_negatives_report(), "callindistinct");
}

/// Test 23l: `CallObject(L: Codeunit "Neg Target")` calls `T.O(L)` where `O`
/// overloads on `(Interface "Neg ILog")` / `(Text)` — `L`'s canonical type
/// (`Codeunit "Neg Target"`, which does NOT implement `"Neg ILog"`) exactly
/// matches NEITHER candidate — 0 exact matches, never a fabricated pick.
#[test]
fn ws_overload_negatives_call_object_stays_ambiguous_resolved() {
    assert_stays_ambiguous_resolved(&ws_overload_negatives_report(), "callobject");
}

/// Test 23m (deferred-increment guard): `ws-overload-enum-discriminator`'s
/// `Run()` calls `T.P("Probe Kind"::Open)` — a qualified-enum-value
/// argument (`ExprKind::QualifiedEnum`) is NOT a bare identifier/param/local/
/// global reference NOR a literal; this increment's `type_one_arg` leaves it
/// untyped (deferred — Enum::Value arg typing is a documented future
/// increment). Must stay `AmbiguousResolved`, proving the deferral is
/// honored rather than silently mistyped.
#[test]
fn ws_overload_enum_discriminator_stays_ambiguous_resolved() {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-overload-enum-discriminator");
    let report = resolve_full_program(&fixture)
        .expect("resolve_full_program must succeed on ws-overload-enum-discriminator");
    assert_stays_ambiguous_resolved(&report, "run");
}

/// Test 23n (deferred-increment guard): `ws-overload-field-discriminator`'s
/// CORRECTED (Task 4, receiver-closure-and-arg-increments plan — a
/// documented rebaseline, not a regression; renamed per this codebase's M1
/// convention "rename, don't just flip"): `Run(var Rec)` calls
/// `T.P(Rec.Amount)` — `Rec.Amount` is now a `Rec.Field` member-expression
/// arg-typing increment's home turf (`arg_dispatch::type_one_arg`'s new
/// `Member` arm): `"Amount"` is a real `Integer`-typed field on `"FD Rec"`,
/// exact-matching `P(N: Integer)` and PROVABLY eliminating `P(S: Text)`
/// (disjoint canonical families, `same_soft_family` returns false for
/// integer/text) — exactly what a real AL compiler resolves, unambiguously
/// (no implicit conversion needed; an exact-type overload always wins). This
/// fixture pre-dates the increment (comment previously: "stays untyped —
/// deferred"); the increment landing is the mechanism, this pick is the
/// expected, adjudicated consequence.
#[test]
fn ws_overload_field_discriminator_picks_integer_overload_via_member_field_arg() {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-overload-field-discriminator");
    let report = resolve_full_program(&fixture)
        .expect("resolve_full_program must succeed on ws-overload-field-discriminator");
    let edge = &outer_call_edge(&report, "run").edge;
    assert_eq!(edge.shape, DispatchShape::Exact, "got {:?}", edge.shape);
    assert_eq!(edge.routes.len(), 1, "got {:?}", edge.routes);
    let route = &edge.routes[0];
    assert_eq!(route.evidence, Evidence::Source, "got {route:?}");
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected a Routine target; got {route:?}");
    };
    assert_eq!(rid.name_lc, "p");
    assert_eq!(rid.params_count, 1);
    let Witness::SourceSpan { ref file, span } = route.witness else {
        panic!("expected SourceSpan witness; got {route:?}");
    };
    let src = std::fs::read_to_string(fixture.join(file)).expect("read witness file");
    let decl_text = src[span.0 as usize..span.1 as usize].to_ascii_lowercase();
    assert!(
        decl_text.contains("integer"),
        "picked routine must be the Integer overload (Rec.Amount is Integer); \
         got {decl_text:?}"
    );
}

/// Test 23o (CORRECTED — pageext-merge-and-final-residual plan, Task 3: a
/// documented rebaseline, not a regression; renamed per this codebase's M1
/// convention "rename, don't just flip"): `ws-overload-callexpr-
/// discriminator`'s `Run()` calls `T.P(GetCount())` — `GetCount()` is a
/// same-object bare-call-result arg, now `arg_dispatch::type_one_arg`'s new
/// `Call` arm's home turf (mirrors Step 5's guards: no shadowing local, a
/// SINGLE-route `resolve_bare` query resolving `GetCount` to a source
/// routine returning `Integer`). `Integer` exact-matches `P(N: Integer)` and
/// PROVABLY eliminates `P(S: Text)` (disjoint canonical families) — exactly
/// what a real AL compiler resolves, unambiguously. This fixture pre-dates
/// the increment (comment previously: "stays untyped — deferred"); the
/// increment landing is the mechanism, this pick is the expected, adjudicated
/// consequence.
#[test]
fn ws_overload_callexpr_discriminator_picks_integer_overload_via_call_result_arg() {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-overload-callexpr-discriminator");
    let report = resolve_full_program(&fixture)
        .expect("resolve_full_program must succeed on ws-overload-callexpr-discriminator");
    let edge = &outer_call_edge(&report, "run").edge;
    assert_eq!(edge.shape, DispatchShape::Exact, "got {:?}", edge.shape);
    assert_eq!(edge.routes.len(), 1, "got {:?}", edge.routes);
    let route = &edge.routes[0];
    assert_eq!(route.evidence, Evidence::Source, "got {route:?}");
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected a Routine target; got {route:?}");
    };
    assert_eq!(rid.name_lc, "p");
    assert_eq!(rid.params_count, 1);
    let Witness::SourceSpan { ref file, span } = route.witness else {
        panic!("expected SourceSpan witness; got {route:?}");
    };
    let src = std::fs::read_to_string(fixture.join(file)).expect("read witness file");
    let decl_text = src[span.0 as usize..span.1 as usize].to_ascii_lowercase();
    assert!(
        decl_text.contains("integer"),
        "picked routine must be the Integer overload (GetCount() returns Integer); \
         got {decl_text:?}"
    );
}

/// Test 35a (T3, pageext-merge-and-final-residual plan): `ws-overload-
/// membercall-discriminator` — the PrintPDFFile shape. `T.P(Rec.
/// ToBase64String())`: the arg is a MEMBER-function call-result
/// (`arg_dispatch::type_one_arg`'s new `Call{function: Member{..}}` arm,
/// mirrors Step 6's cross-object-chain base typing). `ToBase64String()`
/// (declared on `"MCD Rec"`) returns `Text`, exact-matching `P(S: Text)` and
/// PROVABLY eliminating the Record-typed sibling `P(R: Record "MCD Rec")` —
/// the real CDO shape (`Page 6175389`'s two `PrintPDFFile` sites) grounding
/// this task's expected `ambiguousResolved` flip.
#[test]
fn ws_overload_membercall_discriminator_picks_text_overload_via_member_call_result_arg() {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-overload-membercall-discriminator");
    let report = resolve_full_program(&fixture)
        .expect("resolve_full_program must succeed on ws-overload-membercall-discriminator");
    let edge = &outer_call_edge(&report, "run").edge;
    assert_eq!(edge.shape, DispatchShape::Exact, "got {:?}", edge.shape);
    assert_eq!(edge.routes.len(), 1, "got {:?}", edge.routes);
    let route = &edge.routes[0];
    assert_eq!(route.evidence, Evidence::Source, "got {route:?}");
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected a Routine target; got {route:?}");
    };
    assert_eq!(rid.name_lc, "p");
    assert_eq!(rid.params_count, 1);
    let Witness::SourceSpan { ref file, span } = route.witness else {
        panic!("expected SourceSpan witness; got {route:?}");
    };
    let src = std::fs::read_to_string(fixture.join(file)).expect("read witness file");
    let decl_text = src[span.0 as usize..span.1 as usize].to_ascii_lowercase();
    assert!(
        decl_text.contains("s: text"),
        "picked routine must be the Text overload (ToBase64String() returns Text); \
         got {decl_text:?}"
    );
}

fn ws_overload_callresult_guards_report() -> ProgramReport {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-overload-callresult-guards");
    resolve_full_program(&fixture)
        .expect("resolve_full_program must succeed on ws-overload-callresult-guards")
}

/// Test 35b (T3, negative — the inner-uniqueness decline): `RunInner
/// OverloadAmbiguous`'s `T.P(Ambiguous(5))` — `Ambiguous` has TWO arity-1
/// overloads on the SAME (calling) object. `type_call_result_arg_bare`
/// re-queries `resolve_bare` with NO argument evidence (module doc: "no
/// recursion into `pick_candidate`"), so a same-arity overload set is
/// ALWAYS seen as ambiguous from this path — the arg position stays
/// untyped, degrading the WHOLE outer call to honest `AmbiguousResolved`.
#[test]
fn ws_overload_callresult_guards_inner_overload_ambiguity_stays_ambiguous_resolved() {
    assert_stays_ambiguous_resolved(
        &ws_overload_callresult_guards_report(),
        "runinneroverloadambiguous",
    );
}

/// Test 35c (T3, POSITIVE — the SHADOWED-NAME fixture, mandatory per the
/// plan v2.1 addenda): `RunShadowedFormat`'s `T.Q(Format())` — a SOURCE
/// procedure named `Format` (returning `Integer`, NOT the catalog's `Text`)
/// shadows the global builtin at `resolve_bare`'s Step 1 (own object) —
/// the passive builtin-return catalog is structurally unreachable here.
/// Proves the catalog is gated on `resolve_bare`'s POSITIVE `Builtin`
/// verdict, never a bare name-string match: the pick lands on
/// `Q(N: Integer)` (this shadowing `Format`'s REAL return type), not
/// `Q(S: Text)` (what the catalog would have said).
#[test]
fn ws_overload_callresult_guards_shadowed_format_picks_integer_overload() {
    let report = ws_overload_callresult_guards_report();
    let edge = &outer_call_edge(&report, "runshadowedformat").edge;
    assert_eq!(edge.shape, DispatchShape::Exact, "got {:?}", edge.shape);
    assert_eq!(edge.routes.len(), 1, "got {:?}", edge.routes);
    let route = &edge.routes[0];
    assert_eq!(route.evidence, Evidence::Source, "got {route:?}");
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected a Routine target; got {route:?}");
    };
    assert_eq!(rid.name_lc, "q");
    assert_eq!(rid.params_count, 1);
    let Witness::SourceSpan { ref file, span } = route.witness else {
        panic!("expected SourceSpan witness; got {route:?}");
    };
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-overload-callresult-guards");
    let src = std::fs::read_to_string(fixture.join(file)).expect("read witness file");
    let decl_text = src[span.0 as usize..span.1 as usize].to_ascii_lowercase();
    assert!(
        decl_text.contains("n: integer"),
        "the shadowing Format's REAL return type (Integer) must drive the pick, \
         never the catalog's (Text); got {decl_text:?}"
    );
}

/// Test 35d (T3, POSITIVE — the OTHER mandatory shadowed-name pair):
/// `RunShadowedCopyStr`'s `T.R(CopyStr())` — same shape as 35c, `CopyStr`.
#[test]
fn ws_overload_callresult_guards_shadowed_copystr_picks_integer_overload() {
    let report = ws_overload_callresult_guards_report();
    let edge = &outer_call_edge(&report, "runshadowedcopystr").edge;
    assert_eq!(edge.shape, DispatchShape::Exact, "got {:?}", edge.shape);
    assert_eq!(edge.routes.len(), 1, "got {:?}", edge.routes);
    let route = &edge.routes[0];
    assert_eq!(route.evidence, Evidence::Source, "got {route:?}");
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected a Routine target; got {route:?}");
    };
    assert_eq!(rid.name_lc, "r");
    assert_eq!(rid.params_count, 1);
    let Witness::SourceSpan { ref file, span } = route.witness else {
        panic!("expected SourceSpan witness; got {route:?}");
    };
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-overload-callresult-guards");
    let src = std::fs::read_to_string(fixture.join(file)).expect("read witness file");
    let decl_text = src[span.0 as usize..span.1 as usize].to_ascii_lowercase();
    assert!(
        decl_text.contains("n: integer"),
        "the shadowing CopyStr's REAL return type (Integer) must drive the pick, \
         never the catalog's (Text); got {decl_text:?}"
    );
}

fn ws_overload_pageext_callresult_report() -> ProgramReport {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-overload-pageext-callresult");
    resolve_full_program(&fixture)
        .expect("resolve_full_program must succeed on ws-overload-pageext-callresult")
}

/// Test 35e (T3, POSITIVE — the addenda-mandatory pageext-merge fixture):
/// `RunSingleExtension`'s `T.P(PageVar.GetCount())` — `PageVar: Page "PCR
/// Base Page"`, extended by exactly ONE visible `internal` PageExtension
/// declaring `GetCount(): Integer`. Task 1's merge yields a SINGLE route
/// through `resolve_member`'s Object arm, so this call-result arg types as
/// `Integer` — exact-matching `P(N: Integer)` and eliminating `P(S: Text)`.
#[test]
fn ws_overload_pageext_callresult_single_extension_picks_integer_overload() {
    let report = ws_overload_pageext_callresult_report();
    let edge = &outer_call_edge(&report, "runsingleextension").edge;
    assert_eq!(edge.shape, DispatchShape::Exact, "got {:?}", edge.shape);
    assert_eq!(edge.routes.len(), 1, "got {:?}", edge.routes);
    let route = &edge.routes[0];
    assert_eq!(route.evidence, Evidence::Source, "got {route:?}");
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected a Routine target; got {route:?}");
    };
    assert_eq!(rid.name_lc, "p");
    assert_eq!(rid.params_count, 1);
    let Witness::SourceSpan { ref file, span } = route.witness else {
        panic!("expected SourceSpan witness; got {route:?}");
    };
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-overload-pageext-callresult");
    let src = std::fs::read_to_string(fixture.join(file)).expect("read witness file");
    let decl_text = src[span.0 as usize..span.1 as usize].to_ascii_lowercase();
    assert!(
        decl_text.contains("n: integer"),
        "picked routine must be the Integer overload (GetCount() returns Integer \
         via the single visible PageExtension); got {decl_text:?}"
    );
}

/// Test 35f (T3, NEGATIVE — the addenda-mandatory two-extension decline):
/// `RunTwoExtensions`'s `T.P(PageVar2.GetCount())` — `PageVar2: Page "PCR
/// Base Page 2"`, extended by TWO visible `internal` PageExtensions BOTH
/// declaring `GetCount(): Integer`. The merge's aggregate-then-adjudicate
/// contract feeds both candidates to the SAME ambiguity machinery (never
/// first-wins), so `resolve_member` yields >1 routes — this call-result arg
/// declines to untyped, degrading the WHOLE outer call to honest
/// `AmbiguousResolved`.
#[test]
fn ws_overload_pageext_callresult_two_extensions_stays_ambiguous_resolved() {
    assert_stays_ambiguous_resolved(&ws_overload_pageext_callresult_report(), "runtwoextensions");
}

// ---------------------------------------------------------------------------
// Tests 23p-23q: argtype-dispatch-and-page-catalog plan (v2.1), Task 2 REVIEW
// FIX (Finding 1) — `with`-scope gate for bare-identifier arg typing.
// `arg_dispatch::type_one_arg` never used to consult `WithState`, so a bare-
// identifier argument inside a `with X do` block was typed from CALLER scope
// even though AL rebinds it to the WITH-receiver's member — a constructible
// WRONG PICK (dormant on CDO, which has zero `with` blocks).
// `tests/r0-corpus/ws-overload-with-scope/`: `SomeField` is BOTH a table
// field (Decimal, on "WS With Scope Table") and a global var (Text, on the
// caller codeunit); `Target.Foo` overloads on `(Decimal)`/`(Text)`.
// ---------------------------------------------------------------------------

fn ws_overload_with_scope_report() -> ProgramReport {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-overload-with-scope");
    resolve_full_program(&fixture)
        .expect("resolve_full_program must succeed on ws-overload-with-scope")
}

/// Test 23p: `CallInsideWith` sits inside `with Rec do` — the with-scope
/// gate must degrade the bare-identifier arg `SomeField` to untyped, which
/// degrades the WHOLE call back to honest `AmbiguousResolved`. Before the
/// Finding-1 fix, this call would fail-closed-PICK `Foo(X: Text)` (the
/// GLOBAL `SomeField`'s type) — a wrong pick, since the compiler actually
/// binds `SomeField` to the WITH-receiver's Decimal field.
#[test]
fn ws_overload_with_scope_call_inside_with_stays_ambiguous_resolved() {
    use al_sem::program::resolve::edge::{ObligationOutcome, classify_obligation};

    let report = ws_overload_with_scope_report();
    let edges = edges_for_caller(&report, "callinsidewith");
    assert_eq!(
        edges.len(),
        1,
        "CallInsideWith has exactly one call obligation"
    );
    let edge = &edges[0].edge;
    assert_eq!(
        edge.shape,
        DispatchShape::AmbiguousOverload,
        "the with-scope gate must degrade the bare-identifier arg to \
         untyped, which degrades the whole call to AmbiguousOverload; got {:?}",
        edge.shape
    );
    assert_eq!(edge.routes.len(), 2, "got {:?}", edge.routes);
    assert_eq!(
        classify_obligation(edge),
        ObligationOutcome::AmbiguousResolved,
        "a bare-identifier arg inside a proven `with` block must NEVER be \
         typed from caller scope — no pick, honest AmbiguousResolved"
    );
}

/// Test 23q (control): `CallOutsideWith` is the SAME call, unshadowed by any
/// `with` — the with-scope gate must not over-fire: `SomeField` (the global,
/// Text) is typed normally and confidently picks `Foo(X: Text)` (Text/
/// Decimal are cross-family, a proven elimination of the sibling overload).
#[test]
fn ws_overload_with_scope_call_outside_with_control_picks_text_overload() {
    let report = ws_overload_with_scope_report();
    let edges = edges_for_caller(&report, "calloutsidewith");
    assert_eq!(
        edges.len(),
        1,
        "CallOutsideWith has exactly one call obligation"
    );
    let edge = &edges[0].edge;
    assert_eq!(
        edge.shape,
        DispatchShape::Exact,
        "outside any `with`, the global SomeField must type normally and \
         confidently pick; got {:?}",
        edge.shape
    );
    assert_eq!(edge.routes.len(), 1, "got {:?}", edge.routes);
    let route = &edge.routes[0];
    assert_eq!(route.evidence, Evidence::Source, "got {route:?}");
    assert!(route.fires_by_default(), "got {route:?}");
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected a Routine target; got {route:?}");
    };
    assert_eq!(rid.params_count, 1);
    let Witness::SourceSpan { ref file, span } = route.witness else {
        panic!("expected SourceSpan witness; got {route:?}");
    };
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-overload-with-scope");
    let src = std::fs::read_to_string(fixture.join(file)).expect("read witness file");
    let decl_text = src[span.0 as usize..span.1 as usize].to_ascii_lowercase();
    assert!(
        decl_text.contains("text"),
        "picked routine must be the Text overload (the global SomeField's \
         type); got {decl_text:?}"
    );
    assert!(
        !decl_text.contains("decimal"),
        "picked routine must NOT be the Decimal overload; got {decl_text:?}"
    );
}

// ---------------------------------------------------------------------------
// Tests 23d-23e: beyond-1B.3b Task 2 REVIEW FIX — compound object-duplication
// × genuine-overload dedup. `dedup_routines_preserving_genuine_overloads`
// used to be binary per run of equal-`RoutineNodeId` routines: collapse the
// WHOLE run to 1 when `run_len <= obj_dup`, else keep EVERY entry. When an
// object is embedded BOTH as workspace source AND as an embedded dep
// (`obj_dup=2`) AND that object declares a genuine same-name/same-arity
// overload pair (2 distinct source procedures colliding onto ONE
// `RoutineNodeId` — at the time this fix landed, source `sig_fp` was always
// `0`; since sigfp-and-ambiguous-reclassification plan Task 2 it is a real
// per-overload fingerprint, but the compound-DUPLICATION half of this
// scenario is unaffected: BOTH raw copies of the SAME `Resolve(Value: Text)`
// declaration still compute the SAME `sig_fp`, so they still collide onto one
// id and must still collapse to one canonical survivor), the run holds 4 raw
// entries — `run_len(4) > obj_dup(2)` kept all 4 instead of the canonical 2.
// This inflated `graph.routines` and could push a legitimate single-target
// event subscription into `ambiguous_subscriptions` (candidate count 2
// instead of 1). The fix groups a run by the routine's PARAMETER-TYPE
// SIGNATURE before collapsing, so genuine re-parse duplicates collapse
// per-signature while genuine overloads (distinct signatures) are preserved
// — 2 canonical entries in every case, never 4.
// ---------------------------------------------------------------------------

/// Hand-builds an `AppSetSnapshot` with the SAME app identity appearing
/// TWICE — once as the workspace unit, once as a synthetic embedded-dep
/// unit — mirroring the real "sibling apps in a multi-app workspace whose
/// compiled .app lands in .alpackages" scenario `build_program_graph`'s Step
/// 4 comment documents (both units interning to the SAME `AppRef`). Both
/// units embed the identical `CompoundTarget.al` source, which declares a
/// genuine same-name/same-arity `Resolve` overload pair — one plain
/// `Resolve(Value: Integer)`, one `[IntegrationEvent]`-tagged
/// `Resolve(Value: Text)`. Only the workspace unit also carries the
/// subscriber file, so the compound duplication is isolated to the
/// `Compound Overload Target` object.
fn compound_overload_dup_snapshot() -> al_sem::snapshot::AppSetSnapshot {
    use al_sem::snapshot::compilation::CompilationContext;
    use al_sem::snapshot::embedded::SourceFile;
    use al_sem::snapshot::provider::SourceRoot;
    use al_sem::snapshot::{AppSetSnapshot, AppUnit, World};

    let target_src = r#"
codeunit 50970 "Compound Overload Target"
{
    // Non-publisher overload — arity 1, param type Integer.
    procedure Resolve(Value: Integer)
    begin
    end;

    // Publisher overload — SAME name + SAME arity as the sibling above,
    // differing only by param TYPE (Text). Together they collide onto ONE
    // `RoutineNodeId` (source `sig_fp` is always 0).
    [IntegrationEvent(false, false)]
    procedure Resolve(Value: Text)
    begin
    end;
}
"#
    .to_string();

    let subscriber_src = r#"
codeunit 50971 "Compound Overload Subscriber"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Compound Overload Target", 'Resolve', '', false, false)]
    procedure OnResolve(Value: Text)
    begin
    end;
}
"#
    .to_string();

    let app_id = AppId {
        guid: String::new(),
        name: "Compound App".into(),
        publisher: "Test".into(),
        version: "1.0.0.0".into(),
    };

    let ws_source = SourceRoot {
        files: vec![
            SourceFile {
                virtual_path: "CompoundTarget.al".into(),
                text: target_src.clone().into(),
            },
            SourceFile {
                virtual_path: "CompoundSubscriber.al".into(),
                text: subscriber_src.into(),
            },
        ],
        tier: TrustTier::Workspace,
        content_hash: "ws-hash".into(),
    };
    // Synthetic "embedded dep" copy of the SAME source file — the exact
    // compound scenario `build_program_graph`'s Step 4 comment documents
    // ("Same app can appear as both a workspace source and an embedded dep").
    let dep_source = SourceRoot {
        files: vec![SourceFile {
            virtual_path: "CompoundTarget.al".into(),
            text: target_src.into(),
        }],
        tier: TrustTier::EmbeddedSource,
        content_hash: "dep-hash".into(),
    };

    let ws_unit = AppUnit {
        id: app_id.clone(),
        provenance: Provenance {
            app: app_id.clone(),
            tier: TrustTier::Workspace,
            content_hash: "ws-hash".into(),
        },
        source: Some(ws_source),
        compilation: CompilationContext::default(),
        declared_deps: vec![],
        internals_visible_to: vec![],
        abi: None,
        app_path: None,
    };
    let dep_unit = AppUnit {
        id: app_id.clone(),
        provenance: Provenance {
            app: app_id.clone(),
            tier: TrustTier::EmbeddedSource,
            content_hash: "dep-hash".into(),
        },
        source: Some(dep_source),
        compilation: CompilationContext::default(),
        declared_deps: vec![],
        internals_visible_to: vec![],
        abi: None,
        app_path: None,
    };

    AppSetSnapshot {
        workspace_app: app_id,
        apps: vec![ws_unit, dep_unit],
        world: World::Closed,
    }
}

/// Test 23d: the compound duplication must collapse `graph.routines` to the
/// CANONICAL count (2 — one per genuine overload), never inflate to 4 (2
/// overloads × obj_dup 2). Proves the content-aware (param-signature) dedup
/// fix at the `build_program_graph` layer.
#[test]
fn compound_obj_dup_and_overload_dedups_to_canonical_count() {
    use al_sem::program::abi_ingest::AbiCache;
    use al_sem::program::build::build_program_graph;

    let snap = compound_overload_dup_snapshot();
    let cache = AbiCache::new();
    let graph = build_program_graph(&snap, &cache);

    let resolve_entries: Vec<_> = graph
        .routines
        .iter()
        .filter(|r| r.id.name_lc == "resolve")
        .collect();
    assert_eq!(
        resolve_entries.len(),
        2,
        "compound case (obj_dup=2 x 2 genuine overloads = 4 raw entries) must \
         collapse to the CANONICAL count of 2 -- one per genuine overload, \
         never inflate to 4; got {} entries: {:?}",
        resolve_entries.len(),
        resolve_entries.iter().map(|r| &r.name).collect::<Vec<_>>()
    );

    // Exactly one of the two canonical entries carries the publisher
    // attribute (the `[IntegrationEvent]`-tagged overload); the other does
    // not -- proves BOTH signature groups survived distinctly, not two
    // copies of the same one.
    let publisher_count = resolve_entries
        .iter()
        .filter(|r| r.publisher_kind.is_some())
        .count();
    assert_eq!(
        publisher_count,
        1,
        "exactly one canonical `Resolve` entry must carry the publisher \
         attribute; got {publisher_count} of {}",
        resolve_entries.len()
    );

    // The object itself must still be deduped to exactly one entry (Step 4's
    // existing unconditional `objects.dedup_by` -- unaffected by this fix).
    let target_objects: Vec<_> = graph
        .objects
        .iter()
        .filter(|o| o.name == "Compound Overload Target")
        .collect();
    assert_eq!(target_objects.len(), 1, "object dedup must be unaffected");
}

/// Test 23e: the compound duplication must NOT push the legitimate
/// single-target `OnResolve` subscription into `ambiguous_subscriptions`.
/// Before the fix, the inflated 4-entry run left 2 publisher-tagged raw
/// candidates (both from the SAME genuine overload, duplicated by `obj_dup`)
/// with equal arity, so `ResolveIndex::build`'s `>1` arm found no unique
/// strict-arity match and dropped the subscription as ambiguous.
#[test]
fn compound_obj_dup_and_overload_subscription_resolves_not_ambiguous() {
    use al_sem::program::abi_ingest::AbiCache;
    use al_sem::program::build::build_program_graph;
    use al_sem::program::resolve::index::ResolveIndex;

    let snap = compound_overload_dup_snapshot();
    let cache = AbiCache::new();
    let graph = build_program_graph(&snap, &cache);
    let idx = ResolveIndex::build(&graph);

    assert!(
        idx.ambiguous_subscriptions().is_empty(),
        "a legitimate single-target subscription must NOT be pushed into \
         ambiguous_subscriptions by the compound obj_dup x overload \
         inflation; got {:?}",
        idx.ambiguous_subscriptions()
            .iter()
            .map(|a| (a.event_name_lc.clone(), a.candidate_count))
            .collect::<Vec<_>>()
    );

    // sigfp-and-ambiguous-reclassification plan Task 2: SOURCE `sig_fp` is no
    // longer unconditionally `0` — the publisher overload's real id must be
    // looked up from the graph (its `sig_fp` is a real fingerprint of its
    // `Text` parameter type), never hand-built with a hardcoded `sig_fp: 0`
    // (which would silently diverge from the actual id and always miss).
    let publisher_id = graph
        .routines
        .iter()
        .find(|r| r.id.name_lc == "resolve" && r.publisher_kind.is_some())
        .expect("the publisher overload must survive dedup")
        .id
        .clone();
    assert_eq!(
        idx.subscribers_of(&publisher_id).len(),
        1,
        "the legitimate subscriber must resolve to exactly one entry"
    );
}

// ---------------------------------------------------------------------------
// Tests 23f-23i: sigfp-and-ambiguous-reclassification plan, Task 1 (source-
// overload alias marker, `RoutineNode::source_overload_aliased`, and the
// dual-publisher event-flow collision guard —
// `resolver::emit_event_flow_edges` / `resolver::dual_publisher_alias_
// skip_count`), REFRAMED by Task 2 (real source `sig_fp`).
//
// At Task 1, source `sig_fp` was always `0`, so two genuine same-name/
// same-arity SOURCE overloads (differing only by parameter TYPE) collided
// onto ONE `RoutineNodeId`; `dedup_routines_preserving_genuine_overloads`
// already kept BOTH survivors (Task 2-of-the-EARLIER-plan review fix — see
// the tests above), but neither was flagged as aliased, so a role-lookup
// consumer like `emit_event_flow_edges`'s publisher fan-out had no way to
// know its `DeclSurface` span answer might belong to the WRONG sibling.
//
// Since Task 2 (this plan), SOURCE `sig_fp` is a real fingerprint
// (`sig_fp::source_param_sig_fp`): a genuine same-name/same-arity overload
// pair now gets DISTINCT ids and never even reaches the same dedup run, so
// it survives UNMARKED (Test 23f, updated). `source_overload_aliased`'s
// post-Task-2 role is a same-id/different-`param_sig_key` COLLISION GUARD —
// it fires ONLY when two entries' `sig_fp`s alias despite their raw content
// genuinely differing (a normalization collision this engine cannot further
// distinguish; see `build.rs`'s `source_normalization_collision_marks_both_
// survivors_collision_guard` for that scenario in isolation). Test 23g
// (a TRUE re-parse duplicate) and the dual-publisher case (Test 23h) are
// updated accordingly below.
// ---------------------------------------------------------------------------

/// A single workspace source object declaring two overloads that collide
/// onto ONE `RoutineNodeId`: `Resolve(Value: Integer)` and `Resolve(Value:
/// Text)`. Neither is a publisher — isolates the alias MARKER from the
/// event-flow guard (Test 23f).
fn two_overload_alias_snapshot() -> al_sem::snapshot::AppSetSnapshot {
    use al_sem::snapshot::compilation::CompilationContext;
    use al_sem::snapshot::embedded::SourceFile;
    use al_sem::snapshot::provider::SourceRoot;
    use al_sem::snapshot::{AppSetSnapshot, AppUnit, World};

    let src = r#"
codeunit 50980 "Alias Target"
{
    procedure Resolve(Value: Integer)
    begin
    end;

    procedure Resolve(Value: Text)
    begin
    end;
}
"#
    .to_string();

    let app_id = AppId {
        guid: String::new(),
        name: "Alias App".into(),
        publisher: "Test".into(),
        version: "1.0.0.0".into(),
    };

    let ws_source = SourceRoot {
        files: vec![SourceFile {
            virtual_path: "AliasTarget.al".into(),
            text: src.into(),
        }],
        tier: TrustTier::Workspace,
        content_hash: "ws-hash".into(),
    };

    let ws_unit = AppUnit {
        id: app_id.clone(),
        provenance: Provenance {
            app: app_id.clone(),
            tier: TrustTier::Workspace,
            content_hash: "ws-hash".into(),
        },
        source: Some(ws_source),
        compilation: CompilationContext::default(),
        declared_deps: vec![],
        internals_visible_to: vec![],
        abi: None,
        app_path: None,
    };

    AppSetSnapshot {
        workspace_app: app_id,
        apps: vec![ws_unit],
        world: World::Closed,
    }
}

/// Test 23f (sigfp-and-ambiguous-reclassification plan, Task 2 — reframed
/// from the Task-1-era `source_overload_alias_marks_both_survivors`): two
/// param-type-differing SOURCE overloads get DISTINCT non-zero `sig_fp`s (and
/// therefore DISTINCT `RoutineNodeId`s), so they no longer even share a dedup
/// run — both survive UNMARKED (`source_overload_aliased == false`).
#[test]
fn distinct_sig_fp_overloads_survive_unmarked() {
    use al_sem::program::abi_ingest::AbiCache;
    use al_sem::program::build::build_program_graph;

    let snap = two_overload_alias_snapshot();
    let cache = AbiCache::new();
    let graph = build_program_graph(&snap, &cache);

    let resolve_entries: Vec<_> = graph
        .routines
        .iter()
        .filter(|r| r.id.name_lc == "resolve")
        .collect();
    assert_eq!(
        resolve_entries.len(),
        2,
        "both genuine overloads must survive dedup; got {:?}",
        resolve_entries
            .iter()
            .map(|r| &r.param_sig_key)
            .collect::<Vec<_>>()
    );

    // DISTINCT sig_fp -> DISTINCT full RoutineNodeId (Task 2's whole point).
    assert_ne!(
        resolve_entries[0].id.sig_fp,
        resolve_entries[1].id.sig_fp,
        "param-type-differing overloads must get distinct sig_fp; got {:?}",
        resolve_entries
            .iter()
            .map(|r| r.id.sig_fp)
            .collect::<Vec<_>>()
    );
    assert!(
        resolve_entries.iter().all(|r| r.id.sig_fp != 0),
        "a non-empty param list must never fingerprint to the ABI-parity \
         zero sentinel; got {:?}",
        resolve_entries
            .iter()
            .map(|r| r.id.sig_fp)
            .collect::<Vec<_>>()
    );
    assert_ne!(
        resolve_entries[0].id, resolve_entries[1].id,
        "distinct sig_fp must yield distinct RoutineNodeId end-to-end"
    );

    // Post-Task-2 marker semantics: normal (distinct-id) overloads are
    // UNMARKED — the collision guard only fires for a residual same-id
    // survivor (see `build.rs`'s
    // `source_normalization_collision_marks_both_survivors_collision_guard`).
    assert!(
        resolve_entries.iter().all(|r| !r.source_overload_aliased),
        "distinct-id genuine overloads must NOT be marked source_overload_aliased; got {:?}",
        resolve_entries
            .iter()
            .map(|r| (r.param_sig_key.clone(), r.source_overload_aliased))
            .collect::<Vec<_>>()
    );
}

/// A single workspace source object embedded TWICE (workspace + embedded
/// dep, identical content, mirroring `compound_overload_dup_snapshot`'s own
/// obj_dup pattern) declaring ONE procedure — no overload pair, a TRUE
/// re-parse duplicate (Test 23g, control).
fn true_duplicate_snapshot() -> al_sem::snapshot::AppSetSnapshot {
    use al_sem::snapshot::compilation::CompilationContext;
    use al_sem::snapshot::embedded::SourceFile;
    use al_sem::snapshot::provider::SourceRoot;
    use al_sem::snapshot::{AppSetSnapshot, AppUnit, World};

    let src = r#"
codeunit 50981 "Dup Target"
{
    procedure Resolve(Value: Integer)
    begin
    end;
}
"#
    .to_string();

    let app_id = AppId {
        guid: String::new(),
        name: "Dup App".into(),
        publisher: "Test".into(),
        version: "1.0.0.0".into(),
    };

    let ws_source = SourceRoot {
        files: vec![SourceFile {
            virtual_path: "DupTarget.al".into(),
            text: src.clone().into(),
        }],
        tier: TrustTier::Workspace,
        content_hash: "ws-hash".into(),
    };
    let dep_source = SourceRoot {
        files: vec![SourceFile {
            virtual_path: "DupTarget.al".into(),
            text: src.into(),
        }],
        tier: TrustTier::EmbeddedSource,
        content_hash: "dep-hash".into(),
    };

    let ws_unit = AppUnit {
        id: app_id.clone(),
        provenance: Provenance {
            app: app_id.clone(),
            tier: TrustTier::Workspace,
            content_hash: "ws-hash".into(),
        },
        source: Some(ws_source),
        compilation: CompilationContext::default(),
        declared_deps: vec![],
        internals_visible_to: vec![],
        abi: None,
        app_path: None,
    };
    let dep_unit = AppUnit {
        id: app_id.clone(),
        provenance: Provenance {
            app: app_id.clone(),
            tier: TrustTier::EmbeddedSource,
            content_hash: "dep-hash".into(),
        },
        source: Some(dep_source),
        compilation: CompilationContext::default(),
        declared_deps: vec![],
        internals_visible_to: vec![],
        abi: None,
        app_path: None,
    };

    AppSetSnapshot {
        workspace_app: app_id,
        apps: vec![ws_unit, dep_unit],
        world: World::Closed,
    }
}

/// Test 23g (CONTROL): a true re-parse duplicate (same `param_sig_key`)
/// still collapses to ONE unmarked survivor.
#[test]
fn true_duplicate_collapses_unmarked() {
    use al_sem::program::abi_ingest::AbiCache;
    use al_sem::program::build::build_program_graph;

    let snap = true_duplicate_snapshot();
    let cache = AbiCache::new();
    let graph = build_program_graph(&snap, &cache);

    let resolve_entries: Vec<_> = graph
        .routines
        .iter()
        .filter(|r| r.id.name_lc == "resolve")
        .collect();
    assert_eq!(
        resolve_entries.len(),
        1,
        "a true re-parse duplicate must collapse to ONE canonical survivor; got {}",
        resolve_entries.len()
    );
    assert!(
        !resolve_entries[0].source_overload_aliased,
        "a true re-parse duplicate's lone survivor must NOT be marked \
         source_overload_aliased"
    );
}

/// A single workspace source object declaring two overloads that BOTH carry
/// `[IntegrationEvent]` — a TRUE dual-publisher alias collision (Test 23h).
fn dual_publisher_alias_snapshot() -> al_sem::snapshot::AppSetSnapshot {
    use al_sem::snapshot::compilation::CompilationContext;
    use al_sem::snapshot::embedded::SourceFile;
    use al_sem::snapshot::provider::SourceRoot;
    use al_sem::snapshot::{AppSetSnapshot, AppUnit, World};

    let src = r#"
codeunit 50982 "Dual Publisher Target"
{
    [IntegrationEvent(false, false)]
    procedure Resolve(Value: Integer)
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure Resolve(Value: Text)
    begin
    end;
}
"#
    .to_string();

    let app_id = AppId {
        guid: String::new(),
        name: "Dual Publisher App".into(),
        publisher: "Test".into(),
        version: "1.0.0.0".into(),
    };

    let ws_source = SourceRoot {
        files: vec![SourceFile {
            virtual_path: "DualPublisherTarget.al".into(),
            text: src.into(),
        }],
        tier: TrustTier::Workspace,
        content_hash: "ws-hash".into(),
    };

    let ws_unit = AppUnit {
        id: app_id.clone(),
        provenance: Provenance {
            app: app_id.clone(),
            tier: TrustTier::Workspace,
            content_hash: "ws-hash".into(),
        },
        source: Some(ws_source),
        compilation: CompilationContext::default(),
        declared_deps: vec![],
        internals_visible_to: vec![],
        abi: None,
        app_path: None,
    };

    AppSetSnapshot {
        workspace_app: app_id,
        apps: vec![ws_unit],
        world: World::Closed,
    }
}

/// Test 23h (sigfp-and-ambiguous-reclassification plan, Task 2 — reframed
/// from the Task-1-era `dual_publisher_alias_skips_event_flow_edges`): since
/// Task 2, two param-type-differing publisher overloads get DISTINCT
/// `sig_fp`s/ids, so the Task-1 skip-only guard no longer fires for them
/// (`dual_publisher_alias_skip_count == 0`) — EACH publisher emits its OWN
/// EventFlow edge, and its `site.span` is the FIDELITY FIX this whole plan
/// exists for: it must match ITS OWN declaration's `name_origin`, never a
/// sibling's (the last-write-wins `DeclSurface` corruption Task 1's guard used to
/// paper over by skipping both).
#[test]
fn distinct_sig_fp_publishers_both_emit_correct_spans() {
    use al_sem::program::abi_ingest::AbiCache;
    use al_sem::program::build::build_program_graph;
    use al_sem::program::resolve::decl_surface::DeclSurface;
    use al_sem::program::resolve::index::ResolveIndex;
    use al_sem::program::resolve::resolver::{
        dual_publisher_alias_skip_count, emit_event_flow_edges,
    };
    use al_sem::snapshot::parse_snapshot;

    let snap = dual_publisher_alias_snapshot();
    let cache = AbiCache::new();
    let graph = build_program_graph(&snap, &cache);
    let parsed = parse_snapshot(&snap);
    let index = ResolveIndex::build(&graph);
    let surface = DeclSurface::build(&graph, &parsed);

    // Precondition: both overloads survived, both UNMARKED (distinct ids),
    // both publishers.
    let resolve_entries: Vec<_> = graph
        .routines
        .iter()
        .filter(|r| r.id.name_lc == "resolve")
        .collect();
    assert_eq!(
        resolve_entries.len(),
        2,
        "both overloads must survive dedup"
    );
    assert!(
        resolve_entries
            .iter()
            .all(|r| !r.source_overload_aliased && r.publisher_kind.is_some()),
        "fixture precondition: both survivors must be UNMARKED publishers \
         (distinct sig_fp); got {:?}",
        resolve_entries
            .iter()
            .map(|r| (r.source_overload_aliased, r.publisher_kind.is_some()))
            .collect::<Vec<_>>()
    );
    assert_ne!(
        resolve_entries[0].id.sig_fp, resolve_entries[1].id.sig_fp,
        "the two publisher overloads must have distinct sig_fp"
    );

    // The Task 1 guard must NOT fire — no alias survives to trigger it.
    assert_eq!(
        dual_publisher_alias_skip_count(&graph.routines),
        0,
        "distinct-id publisher overloads must never be counted as a \
         dual-publisher-alias skip"
    );

    let edges = emit_event_flow_edges(&graph, &index, &surface);
    assert_eq!(
        edges.len(),
        2,
        "each distinct-id publisher must emit its OWN EventFlow edge; got {} edges",
        edges.len()
    );

    // Per-overload span fidelity: each publisher's edge must carry ITS OWN
    // decl's name-origin span (via DeclSurface, now keyed by a distinct id per
    // sibling — no more last-write-wins collision).
    for entry in &resolve_entries {
        let matching: Vec<_> = edges.iter().filter(|e| e.from == entry.id).collect();
        assert_eq!(
            matching.len(),
            1,
            "each publisher id must have exactly one EventFlow edge; entry sig_fp={}",
            entry.id.sig_fp
        );
        let (decl, path) = surface
            .get_with_path(&entry.id)
            .expect("publisher must be in DeclSurface under its own distinct id");
        let expected_span = CanonicalSpan {
            unit: path.to_string(),
            start: SourcePos {
                line: decl.name_origin.start.row,
                col: decl.name_origin.start.column,
            },
            end: SourcePos {
                line: decl.name_origin.end.row,
                col: decl.name_origin.end.column,
            },
        };
        assert_eq!(
            matching[0].site.span, expected_span,
            "publisher span must match ITS OWN decl's name-origin, never a \
             sibling's — the exact span-corruption bug Task 2 fixes"
        );
    }

    // The two spans must themselves be DISTINCT (Integer-overload and
    // Text-overload are declared on different lines) — a residual collision
    // (both edges carrying the SAME span) would silently hide the bug even
    // though `matching.len() == 1` held for each id.
    assert_ne!(
        edges[0].site.span, edges[1].site.span,
        "the two publishers' spans must differ (declared on different lines)"
    );
}

/// Test 23i (sigfp-and-ambiguous-reclassification plan, Task 2 — the 5-site
/// audit's parity fixture): the SAME two-overload declaration
/// (`tests/fixtures/sigfp_overload_identity`), read independently through
/// ALL FOUR live `sig_fp::source_routine_node_id` call sites
/// (`node_extract::extract_nodes` via `graph.routines`;
/// `resolve::decl_surface::DeclSurface::build`; `resolve::full::
/// resolve_full_program`'s `resolve_full_program_from_parts` site;
/// `resolve::stub::resolve_program`), must agree on the SAME id for each
/// overload — proving the shared constructor closes the "5 independent
/// reconstruction sites silently diverge" risk the plan's audit called out.
/// Each overload's own outgoing call must also attribute to ITS OWN id,
/// never its sibling's (per-overload caller attribution).
#[test]
fn sigfp_identity_agrees_across_all_four_live_sites() {
    use al_sem::program::abi_ingest::AbiCache;
    use al_sem::program::build::build_program_graph;
    use al_sem::program::resolve::decl_surface::DeclSurface;
    use al_sem::program::resolve::stub;
    use al_sem::snapshot::{SnapshotBuilder, parse_snapshot};

    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/sigfp_overload_identity");

    // Site 1: node_extract::extract_nodes (via build_program_graph).
    let snap = SnapshotBuilder {
        workspace_root: fixture.clone(),
        local_providers: vec![],
    }
    .build()
    .expect("snapshot must build");
    let cache = AbiCache::new();
    let graph = build_program_graph(&snap, &cache);

    let resolve_entries: Vec<_> = graph
        .routines
        .iter()
        .filter(|r| r.id.name_lc == "resolve")
        .collect();
    assert_eq!(
        resolve_entries.len(),
        2,
        "both overloads must survive dedup"
    );
    assert_ne!(
        resolve_entries[0].id.sig_fp, resolve_entries[1].id.sig_fp,
        "the two overloads must have distinct sig_fp"
    );
    assert!(
        resolve_entries.iter().all(|r| !r.source_overload_aliased),
        "a genuine distinct-type overload pair must be unmarked"
    );

    let int_id = resolve_entries
        .iter()
        .find(|r| r.param_sig_key == "integer")
        .expect("Integer overload must exist")
        .id
        .clone();
    let text_id = resolve_entries
        .iter()
        .find(|r| r.param_sig_key == "text")
        .expect("Text overload must exist")
        .id
        .clone();

    // Site 2: resolve::decl_surface::DeclSurface::build — each id must retrieve the
    // CORRECT decl (matching its own param type), never a last-write-wins
    // collision with its sibling.
    let parsed = parse_snapshot(&snap);
    let surface = DeclSurface::build(&graph, &parsed);
    let int_decl = surface
        .get(&int_id)
        .expect("Integer overload must be in DeclSurface under its own id");
    assert_eq!(int_decl.params.len(), 1);
    assert_eq!(int_decl.params[0].ty.as_deref(), Some("Integer"));
    let text_decl = surface
        .get(&text_id)
        .expect("Text overload must be in DeclSurface under its own id");
    assert_eq!(text_decl.params.len(), 1);
    assert_eq!(text_decl.params[0].ty.as_deref(), Some("Text"));

    // Site 3: resolve::full::resolve_full_program (its internal
    // resolve_full_program_from_parts site) builds its OWN graph from the
    // SAME on-disk fixture — if its `source_routine_node_id` call diverged
    // from Site 1's even slightly, filtering by the id computed above would
    // come up EMPTY here. Also proves per-overload caller attribution: each
    // overload's OWN outgoing call resolves to its OWN distinctly-named
    // helper, never merged with its sibling's.
    let report = resolve_full_program(&fixture).expect("resolve_full_program must succeed");

    let int_edges: Vec<_> = report
        .edges
        .iter()
        .filter(|ce| ce.edge.from == int_id)
        .collect();
    assert_eq!(
        int_edges.len(),
        1,
        "Integer overload must have exactly one outgoing call obligation \
         (HelperInt()); site 1/site 3 ids disagree if this is 0"
    );
    let RouteTarget::Routine(ref int_target) = int_edges[0].edge.routes[0].target else {
        panic!(
            "expected a resolved Routine target for HelperInt(); got {:?}",
            int_edges[0].edge.routes[0].target
        );
    };
    assert_eq!(
        int_target.name_lc, "helperint",
        "the Integer overload's own call must attribute to HelperInt, not its sibling"
    );

    let text_edges: Vec<_> = report
        .edges
        .iter()
        .filter(|ce| ce.edge.from == text_id)
        .collect();
    assert_eq!(
        text_edges.len(),
        1,
        "Text overload must have exactly one outgoing call obligation (HelperText())"
    );
    let RouteTarget::Routine(ref text_target) = text_edges[0].edge.routes[0].target else {
        panic!(
            "expected a resolved Routine target for HelperText(); got {:?}",
            text_edges[0].edge.routes[0].target
        );
    };
    assert_eq!(
        text_target.name_lc, "helpertext",
        "the Text overload's own call must attribute to HelperText, not its sibling"
    );

    // Site 4: resolve::stub::resolve_program, reusing the SAME graph/parsed
    // from Site 1/2 — must key edges under the SAME ids Site 1 computed (id
    // PARITY, the property this fixture proves). `extract_raw_sites`
    // correlates a call site to its caller by NAME alone (documented
    // "Multi-object-file limitation" — it has no per-overload arity/id
    // granularity), so BOTH `Resolve` entries pick up BOTH call sites
    // (HelperInt() and HelperText()) here; that's a pre-existing, orthogonal
    // stub-resolver limitation, NOT a sig_fp identity divergence. The
    // property under test is that EACH id appears at all (id parity) —
    // `0` would mean Site 4's `source_routine_node_id` call diverged from
    // Site 1's.
    let stub_edges = stub::resolve_program(&graph, &parsed);
    assert_eq!(
        stub_edges.iter().filter(|e| e.from == int_id).count(),
        2,
        "stub resolver must key edges under the Integer overload's own id \
         (2, both call sites — see the name-only correlation note above)"
    );
    assert_eq!(
        stub_edges.iter().filter(|e| e.from == text_id).count(),
        2,
        "stub resolver must key edges under the Text overload's own id \
         (2, both call sites — see the name-only correlation note above)"
    );
}

// ---------------------------------------------------------------------------
// Tests 22+: beyond-1B.3b Task 5 — Page/PageExtension implicit `Rec` via
// `ObjectNode.source_table`, end-to-end over `ws-page-rec`.
//
// Root fix: `infer_implicit_rec`'s Page/PageExtension arm used to hardcode
// `Record{table: None}` (the source table was not yet on `ObjectNode`). It now
// resolves `ObjectNode.source_table` through the fail-closed
// `ResolveIndex::resolve_object_ref` (Task 4): only a single unambiguous
// in-closure match yields a table; anything else (no property, ambiguous
// cross-app name, out-of-closure) stays `None` — a guessed table would be a
// false `Source` edge, the cardinal sin. Report/ReportExtension are
// deliberately EXCLUDED (per-dataitem scoping, not object-level) and keep
// returning `Record{table: None}` unconditionally.
// ---------------------------------------------------------------------------

/// Loads `tests/r0-corpus/ws-page-rec` and returns the full
/// `resolve_full_program` report — shared by Tests 22a-22e below.
fn ws_page_rec_report() -> ProgramReport {
    let fixture =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus/ws-page-rec");
    resolve_full_program(&fixture).expect("resolve_full_program must succeed on ws-page-rec")
}

/// All classified `CallSite` edges whose caller routine is `(owning object's
/// declared numeric id, routine name_lc)`. Object-scoped (unlike
/// `edges_for_caller` above, which filters by routine name alone) because
/// this fixture has several distinct pages that all declare an `OnOpenPage`
/// trigger — filtering by name alone would conflate them.
fn edges_for_object_routine<'a>(
    report: &'a ProgramReport,
    object_id_number: i64,
    routine_name_lc: &str,
) -> Vec<&'a ClassifiedEdge> {
    report
        .edges
        .iter()
        .filter(|ce| match &ce.obligation_id {
            ObligationId::CallSite { caller, .. } => {
                caller.object.id_equals_number(object_id_number)
                    && caller.name_lc == routine_name_lc
            }
            ObligationId::Publisher(_) => false,
        })
        .collect()
}

/// Test 22a (fixture a, POSITIVE): `CustomerCard` (Page 50961, `SourceTable =
/// Customer`) has 3 call obligations in `OnOpenPage`:
/// - `Rec.GetDisplayName()` — a NON-builtin table procedure — must resolve to
///   `Customer.GetDisplayName` with `Evidence::Source` and the exact target
///   id. This is the Task 5 fix: before it, the Page's implicit Rec always
///   carried `Record{table: None}`, so this call was an honest `Unknown`.
/// - `Rec.FieldCaption(1)` — a genuine Record-catalog builtin — must STAY
///   `Evidence::Catalog` (table-independent per the `ReceiverType::Record` doc;
///   resolving the table must not disturb genuine builtins).
/// - `Rec.SetRange(...)` — a `record_op_names` call — dispatches through the
///   SEPARATE implicit-trigger fan-out (`CalleeShape::RecordOp`), not
///   `resolve_member`'s catalog; `"setrange"` is not one of the
///   insert/modify/delete/validate/rename triggers that fan-out maps, so it
///   legitimately produces ZERO routes (`Multicast` + `Partial` completeness)
///   both BEFORE and AFTER the fix — resolving the table must not
///   mis-reclassify it as `Source` or `Unknown`.
#[test]
fn ws_page_rec_source_table_resolves_non_builtin_and_preserves_builtins() {
    let report = ws_page_rec_report();
    let edges = edges_for_object_routine(&report, 50961, "onopenpage");
    assert_eq!(
        edges.len(),
        3,
        "CustomerCard.OnOpenPage must have 3 call obligations"
    );

    let call_edges: Vec<&&ClassifiedEdge> = edges
        .iter()
        .filter(|ce| ce.edge.kind == EdgeKind::Call)
        .collect();
    assert_eq!(
        call_edges.len(),
        2,
        "2 Member calls (GetDisplayName, FieldCaption)"
    );

    let source_edge = call_edges
        .iter()
        .find(|ce| ce.edge.routes.first().map(|r| &r.evidence) == Some(&Evidence::Source))
        .expect("one call edge must be Evidence::Source (GetDisplayName)");
    assert_eq!(source_edge.edge.routes.len(), 1, "single-dispatch call");
    let route = &source_edge.edge.routes[0];
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected RouteTarget::Routine, got {:?}", route.target);
    };
    assert_eq!(rid.name_lc, "getdisplayname");
    assert_eq!(rid.object.kind, ObjectKind::Table);
    assert!(
        rid.object.id_equals_number(50960),
        "must resolve to the Customer table (id 50960); got {:?}",
        rid.object
    );
    assert!(
        matches!(route.witness, Witness::SourceSpan { .. }),
        "witness must be SourceSpan; got {:?}",
        route.witness
    );

    let catalog_edge = call_edges
        .iter()
        .find(|ce| ce.edge.routes.first().map(|r| &r.evidence) == Some(&Evidence::Catalog))
        .expect("one call edge must be Evidence::Catalog (FieldCaption)");
    let croute = &catalog_edge.edge.routes[0];
    let RouteTarget::Builtin(BuiltinId(ref id)) = croute.target else {
        panic!("expected RouteTarget::Builtin, got {:?}", croute.target);
    };
    assert_eq!(id, "Record::fieldcaption");

    let record_op_edges: Vec<&&ClassifiedEdge> = edges
        .iter()
        .filter(|ce| ce.edge.kind == EdgeKind::ImplicitTrigger)
        .collect();
    assert_eq!(record_op_edges.len(), 1, "1 RecordOp call (SetRange)");
    let ro = record_op_edges[0];
    assert_eq!(ro.edge.shape, DispatchShape::Multicast);
    assert!(
        ro.edge.routes.is_empty(),
        "SetRange fans out to zero object/field triggers (not in the \
         insert/modify/delete/validate/rename map) — must stay honest-empty, \
         NOT reclassified Source or Unknown; got {:?}",
        ro.edge.routes
    );
}

/// Test 22b (fixture b, NEGATIVE): a Page with no `SourceTable` property at
/// all keeps the implicit Rec at `Record{table: None}` — the non-builtin
/// `Rec.Foo()` stays honest `Unknown`.
#[test]
fn ws_page_rec_no_source_table_stays_unknown() {
    let report = ws_page_rec_report();
    let edges = edges_for_object_routine(&report, 50962, "onopenpage");
    assert_eq!(edges.len(), 1);
    let route = &edges[0].edge.routes[0];
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 22c (fixture c, NEGATIVE — the soundness backstop): `"Amb Table"` is
/// declared as a Table in BOTH dependency apps (PageRecLibA and PageRecLibB),
/// neither of which is this workspace's own app — `resolve_object_ref` must
/// DECLINE (`Ambiguous`), never guess one of the two. `Rec.Bar()` stays
/// honest `Unknown`.
#[test]
fn ws_page_rec_cross_app_ambiguous_source_table_declines_to_unknown() {
    let report = ws_page_rec_report();
    let edges = edges_for_object_routine(&report, 50963, "onopenpage");
    assert_eq!(edges.len(), 1);
    let route = &edges[0].edge.routes[0];
    assert_eq!(
        route.target,
        RouteTarget::Unresolved,
        "ambiguous cross-app SourceTable must NOT resolve to either dependency's table"
    );
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 22d: a LOCAL `var Rec: Record "Other Table"` in `OnOpenPage` shadows
/// the implicit Rec (variable lookup, step 2 of `infer_receiver_type`, runs
/// BEFORE the implicit-Rec/SourceTable step). Even though `ShadowVarPage`'s
/// own `SourceTable = Customer`, `Rec.OtherProc()` must resolve against the
/// DECLARED type "Other Table" (id 50964), never against Customer.
#[test]
fn ws_page_rec_local_var_shadows_implicit_source_table() {
    let report = ws_page_rec_report();
    let edges = edges_for_object_routine(&report, 50965, "onopenpage");
    assert_eq!(edges.len(), 1);
    let route = &edges[0].edge.routes[0];
    assert_eq!(route.evidence, Evidence::Source);
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected RouteTarget::Routine, got {:?}", route.target);
    };
    assert_eq!(rid.name_lc, "otherproc");
    assert!(
        rid.object.id_equals_number(50964),
        "must resolve to \"Other Table\" (id 50964), NOT Customer (50960); got {:?}",
        rid.object
    );
}

/// Test 22e (fixture e, UPDATED — dataitem-receivers plan, Task 1):
/// Report/ReportExtension implicit-Rec is now ROUTINE-CONTEXTUAL —
/// `ReportWithDataitem`'s `OnAfterGetRecord` trigger is nested inside
/// `dataitem(Cust; Customer)`, so the lowerer threads
/// `RoutineDecl.dataitem_source_table = Some("Customer")` and
/// `infer_implicit_rec`'s Report/ReportExtension arm resolves it exactly like
/// Page's `SourceTable` precedent. `Rec.GetDisplayName()` now correctly
/// resolves `Evidence::Source` to `Customer.GetDisplayName` — this is the
/// intended fix, not a regression (see `tests/r0-corpus/ws-report-dataitem/`
/// for the dedicated fixture set, and `receiver.rs`'s
/// `infer_rec_in_report_dataitem_trigger_resolves_dataitem_table` unit test
/// for the isolated mechanism).
#[test]
fn ws_page_rec_report_dataitem_resolves_via_dataitem_source_table() {
    let report = ws_page_rec_report();
    let edges = edges_for_object_routine(&report, 50966, "onaftergetrecord");
    assert_eq!(edges.len(), 1);
    let route = &edges[0].edge.routes[0];
    assert_eq!(route.evidence, Evidence::Source);
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected RouteTarget::Routine, got {:?}", route.target);
    };
    assert_eq!(rid.name_lc, "getdisplayname");
    assert_eq!(rid.object.kind, ObjectKind::Table);
    assert!(
        rid.object.id_equals_number(50960),
        "must resolve to the Customer table (id 50960); got {:?}",
        rid.object
    );
}

// ---------------------------------------------------------------------------
// Tests 24+: beyond-1B.3b Task 5.5 — implicit Base App/System App dependency
// wired into the `src/program` closure via app.json `application`/`platform`.
//
// Root fix: the `src/program` closure builder (`src/snapshot/snapshot.rs`)
// used to read ONLY the explicit app.json `dependencies[]` array. Real BC apps
// declare Base App via the top-level `application` field, NOT `dependencies[]`
// — so Base App was systematically absent from every app's closure and every
// cross-Microsoft-layer call resolved `OutOfClosure` (an honest `Unknown`).
// `crate::dependencies::append_implicit_ms_tier_deps` now appends implicit
// `AppDependency` rows for Base App/System App whenever `application`/
// `platform` is non-empty, mirroring the already-correct
// `engine::deps::cross_app_l3::read_workspace_declared_dependencies` template.
//
// Both fixtures below ship an IDENTICAL synthetic Base App `.app`
// (`437dbf0e-84ff-417a-965d-ed2bb9650972`, Table 9999 "Base App Widget" with
// non-builtin procedure `DoBaseThing`) in `.alpackages/` and an identical
// workspace call site (`Codeunit 50100 "WS Base Caller".Run` ->
// `BaseRec.DoBaseThing()`). The ONLY difference is whether app.json declares
// `application` — proving the injection is gated on that field, not a side
// effect of the Base App `.app` merely being present on disk.
// ---------------------------------------------------------------------------

/// Loads `tests/r0-corpus/ws-baseapp-closure` (app.json HAS `application`).
fn ws_baseapp_closure_report() -> ProgramReport {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-baseapp-closure");
    resolve_full_program(&fixture).expect("resolve_full_program must succeed on ws-baseapp-closure")
}

/// Loads `tests/r0-corpus/ws-baseapp-closure-control` (app.json has NO
/// `application` field).
fn ws_baseapp_closure_control_report() -> ProgramReport {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-baseapp-closure-control");
    resolve_full_program(&fixture)
        .expect("resolve_full_program must succeed on ws-baseapp-closure-control")
}

/// Test 24a (POSITIVE): with `application` set, Base App is implicitly wired
/// into the workspace's closure, so `BaseRec.DoBaseThing()` resolves to the
/// synthetic Base App table's procedure. The dep `.app` ships NO embedded
/// source (SymbolOnly tier, ABI-only) — per `make_routine_route`, a resolved
/// SymbolOnly boundary is `Evidence::Opaque` + `RouteTarget::AbiSymbol` (a
/// "Resolved" boundary route, matching L3's External treatment of dep
/// symbols), NOT `Unresolved`/`Unknown`. Before the Task 5.5 fix this call was
/// an honest `Unknown` (`OutOfClosure` — Base App wasn't even in the closure).
#[test]
fn ws_baseapp_closure_resolves_via_implicit_application_dependency() {
    let report = ws_baseapp_closure_report();
    let edges = edges_for_object_routine(&report, 50100, "run");
    assert_eq!(edges.len(), 1, "Run has exactly 1 call obligation");
    let route = &edges[0].edge.routes[0];
    assert_eq!(
        route.evidence,
        Evidence::Opaque,
        "Base App must now be in-closure via the implicit `application` \
         dependency and resolve as an ABI boundary (not Unknown); got {:?}",
        route
    );
    let RouteTarget::AbiSymbol { ref key } = route.target else {
        panic!("expected RouteTarget::AbiSymbol, got {:?}", route.target);
    };
    assert_eq!(key.routine_name_lc, "dobasething");
    assert_eq!(key.object_type, "table");
    assert_eq!(
        key.object_number, 9999,
        "must resolve to the synthetic Base App Widget table (id 9999); got {:?}",
        key
    );
    assert!(
        matches!(route.witness, Witness::AbiSymbol { .. }),
        "Base App is a SymbolOnly-tier dep app (no embedded source) — witness \
         must be AbiSymbol; got {:?}",
        route.witness
    );
}

/// Test 24b (NEGATIVE/CONTROL): the identical call, with the identical Base
/// App `.app` present in `.alpackages`, but app.json has NO `application`
/// field — no implicit dependency is injected, Base App stays out of the
/// closure, and the call stays honest `Unknown` (`OutOfClosure`).
#[test]
fn ws_baseapp_closure_control_no_application_field_stays_unknown() {
    let report = ws_baseapp_closure_control_report();
    let edges = edges_for_object_routine(&report, 50100, "run");
    assert_eq!(edges.len(), 1, "Run has exactly 1 call obligation");
    let route = &edges[0].edge.routes[0];
    assert_eq!(
        route.target,
        RouteTarget::Unresolved,
        "without `application`, Base App must stay OUT of the closure — no injection"
    );
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

// ---------------------------------------------------------------------------
// Tests 26+: beyond-1B.3b Task 6 — Codeunit implicit `Rec` via
// `ObjectNode.table_no`, end-to-end over `ws-codeunit-rec`.
//
// Root fix: `infer_implicit_rec`'s Codeunit arm used to unconditionally
// return `Unknown` (Codeunit had no arm at all — it fell into the catch-all).
// It now resolves `ObjectNode.table_no` through the fail-closed
// `ResolveIndex::resolve_object_ref` (Task 4), the direct analog of Task 5's
// Page/`SourceTable` fix: a single unambiguous in-closure match yields
// `Record{table: Some(id)}`; a declared-but-unresolved `TableNo` (cross-app
// ambiguity, out-of-closure) stays `Record{table: None}` — mirroring Page's
// non-`Unique` treatment, since a Record entity DOES exist there (builtins
// still resolve table-independently). No `TableNo` at all — including
// `Subtype = Test`/`TestRunner` codeunits, which never declare one — has no
// implicit-Rec entity to type at all and stays the honest `Unknown`, never
// `Record{table: None}`.
// ---------------------------------------------------------------------------

/// Loads `tests/r0-corpus/ws-codeunit-rec` and returns the full
/// `resolve_full_program` report — shared by Tests 26a-26e below.
fn ws_codeunit_rec_report() -> ProgramReport {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-codeunit-rec");
    resolve_full_program(&fixture).expect("resolve_full_program must succeed on ws-codeunit-rec")
}

/// Test 26a (fixture a, POSITIVE): `Item Recalc` (Codeunit 50971, `TableNo =
/// Item`) has 3 call obligations in `OnRun`:
/// - `Rec.Recalculate()` — a NON-builtin table procedure — must resolve to
///   `Item.Recalculate` with `Evidence::Source` and the exact target id. This
///   is the Task 6 fix: before it, the Codeunit's implicit Rec was always
///   `Unknown`, so this call was an honest `Unknown` too.
/// - `Rec.FieldCaption(1)` — a genuine Record-catalog builtin — must STAY
///   `Evidence::Catalog` (table-independent per the `ReceiverType::Record`
///   doc; resolving the table must not disturb genuine builtins).
/// - `Rec.SetRange(...)` — a `record_op_names` call — dispatches through the
///   SEPARATE implicit-trigger fan-out (`CalleeShape::RecordOp`), not
///   `resolve_member`'s catalog; `"setrange"` is not one of the
///   insert/modify/delete/validate/rename triggers that fan-out maps, so it
///   legitimately produces ZERO routes both BEFORE and AFTER the fix —
///   resolving the table must not mis-reclassify it as `Source` or `Unknown`.
#[test]
fn ws_codeunit_rec_table_no_resolves_non_builtin_and_preserves_builtins() {
    let report = ws_codeunit_rec_report();
    let edges = edges_for_object_routine(&report, 50971, "onrun");
    assert_eq!(
        edges.len(),
        3,
        "Item Recalc.OnRun must have 3 call obligations"
    );

    let call_edges: Vec<&&ClassifiedEdge> = edges
        .iter()
        .filter(|ce| ce.edge.kind == EdgeKind::Call)
        .collect();
    assert_eq!(
        call_edges.len(),
        2,
        "2 Member calls (Recalculate, FieldCaption)"
    );

    let source_edge = call_edges
        .iter()
        .find(|ce| ce.edge.routes.first().map(|r| &r.evidence) == Some(&Evidence::Source))
        .expect("one call edge must be Evidence::Source (Recalculate)");
    assert_eq!(source_edge.edge.routes.len(), 1, "single-dispatch call");
    let route = &source_edge.edge.routes[0];
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected RouteTarget::Routine, got {:?}", route.target);
    };
    assert_eq!(rid.name_lc, "recalculate");
    assert_eq!(rid.object.kind, ObjectKind::Table);
    assert!(
        rid.object.id_equals_number(50970),
        "must resolve to the Item table (id 50970); got {:?}",
        rid.object
    );
    assert!(
        matches!(route.witness, Witness::SourceSpan { .. }),
        "witness must be SourceSpan; got {:?}",
        route.witness
    );

    let catalog_edge = call_edges
        .iter()
        .find(|ce| ce.edge.routes.first().map(|r| &r.evidence) == Some(&Evidence::Catalog))
        .expect("one call edge must be Evidence::Catalog (FieldCaption)");
    let croute = &catalog_edge.edge.routes[0];
    let RouteTarget::Builtin(BuiltinId(ref id)) = croute.target else {
        panic!("expected RouteTarget::Builtin, got {:?}", croute.target);
    };
    assert_eq!(id, "Record::fieldcaption");

    let record_op_edges: Vec<&&ClassifiedEdge> = edges
        .iter()
        .filter(|ce| ce.edge.kind == EdgeKind::ImplicitTrigger)
        .collect();
    assert_eq!(record_op_edges.len(), 1, "1 RecordOp call (SetRange)");
    let ro = record_op_edges[0];
    assert_eq!(ro.edge.shape, DispatchShape::Multicast);
    assert!(
        ro.edge.routes.is_empty(),
        "SetRange fans out to zero object/field triggers (not in the \
         insert/modify/delete/validate/rename map) — must stay honest-empty, \
         NOT reclassified Source or Unknown; got {:?}",
        ro.edge.routes
    );
}

/// Test 26b (fixture b, NEGATIVE): a Codeunit with no `TableNo` property at
/// all has no implicit-Rec entity — the non-builtin `Rec.Foo()` stays honest
/// `Unknown`.
#[test]
fn ws_codeunit_rec_no_table_no_stays_unknown() {
    let report = ws_codeunit_rec_report();
    let edges = edges_for_object_routine(&report, 50972, "onrun");
    assert_eq!(edges.len(), 1);
    let route = &edges[0].edge.routes[0];
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 26c (fixture c, NEGATIVE): `Subtype = TestRunner` never declares
/// `TableNo` (no statically-typed implicit Rec for Test/TestRunner codeunits
/// — unhandled even in the legacy L3 engine). Falls into the same "no
/// `TableNo`" arm as 26b — `Rec.Bar()` stays honest `Unknown`, nothing
/// fabricated for the Subtype.
#[test]
fn ws_codeunit_rec_test_runner_subtype_stays_unknown() {
    let report = ws_codeunit_rec_report();
    let edges = edges_for_object_routine(&report, 50973, "onrun");
    assert_eq!(edges.len(), 1);
    let route = &edges[0].edge.routes[0];
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 26d (fixture d, NEGATIVE — the soundness backstop): `"Amb Table"` is
/// declared as a Table in BOTH dependency apps (CodeunitRecLibA and
/// CodeunitRecLibB), neither of which is this workspace's own app —
/// `resolve_object_ref` must DECLINE (`Ambiguous`), never guess one of the
/// two. `TableNo` IS declared, so the implicit Rec stays `Record{table:
/// None}` internally, but `Rec.Baz()` (non-builtin) still resolves to the
/// honest `Unknown` route since there is no table to look the method up
/// against.
#[test]
fn ws_codeunit_rec_cross_app_ambiguous_table_no_declines_to_unknown() {
    let report = ws_codeunit_rec_report();
    let edges = edges_for_object_routine(&report, 50974, "onrun");
    assert_eq!(edges.len(), 1);
    let route = &edges[0].edge.routes[0];
    assert_eq!(
        route.target,
        RouteTarget::Unresolved,
        "ambiguous cross-app TableNo must NOT resolve to either dependency's table"
    );
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 26e: a LOCAL `var Rec: Record "Other Table"` in `OnRun` shadows the
/// implicit Rec (variable lookup, step 2 of `infer_receiver_type`, runs
/// BEFORE step 3's implicit-Rec/TableNo resolution). Even though `Shadow Var
/// Codeunit`'s own `TableNo = Item`, `Rec.OtherProc()` must resolve against
/// the DECLARED type "Other Table" (id 50975), never against Item.
#[test]
fn ws_codeunit_rec_local_var_shadows_implicit_table_no() {
    let report = ws_codeunit_rec_report();
    let edges = edges_for_object_routine(&report, 50976, "onrun");
    assert_eq!(edges.len(), 1);
    let route = &edges[0].edge.routes[0];
    assert_eq!(route.evidence, Evidence::Source);
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected RouteTarget::Routine, got {:?}", route.target);
    };
    assert_eq!(rid.name_lc, "otherproc");
    assert!(
        rid.object.id_equals_number(50975),
        "must resolve to \"Other Table\" (id 50975), NOT Item (50970); got {:?}",
        rid.object
    );
}

// ---------------------------------------------------------------------------
// Tests 28+: beyond-1B.3b Task 7 — `CurrPage.<part>.Page` subpage-instance
// receivers (control-aware, fail-closed), end-to-end over
// `ws-compound-receiver`.
//
// Root fix: `infer_receiver_type` matched the WHOLE lowercased receiver text
// against its arms — a compound receiver like `"currpage.lines.page"` never
// matched anything and fell through to `Unknown`. Step 0 now recognizes
// EXACTLY the `<part>.Page` shape (one control segment + trailing `.Page`
// accessor): a `Part` control's target resolves through the fail-closed
// `ResolveIndex::resolve_object_ref` to the subpage Page object, carrying its
// id MECHANICALLY on `ReceiverType::Object` so `resolve_member` short-
// circuits rather than re-resolving by name. `CurrPage.<part>` alone (no
// `.Page`) is the CONTROL — a structurally different receiver — and is
// deliberately NOT modeled; nor are `SystemPart`/`UserControl` controls or
// any chain deeper than one `.Page` accessor. All of those stay honest
// `Unknown` rather than fabricate a route.
// ---------------------------------------------------------------------------

/// Loads `tests/r0-corpus/ws-compound-receiver` and returns the full
/// `resolve_full_program` report — shared by Tests 28a-28e below.
fn ws_compound_receiver_report() -> ProgramReport {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-compound-receiver");
    resolve_full_program(&fixture)
        .expect("resolve_full_program must succeed on ws-compound-receiver")
}

/// Test 28 (fixtures a-e, combined): `"Customer Card"` (Page 50991)'s
/// `OnOpenPage` has 9 call obligations — 1 POSITIVE + 8 NEGATIVE, all in one
/// routine (mirrors `ws_page_rec`/`ws_codeunit_rec`'s per-routine grouping).
///
/// - (a) POSITIVE: `CurrPage.Lines.Page.RefreshLines()` resolves to
///   `"Customer Card Part".RefreshLines` (id 50990), `Evidence::Source`,
///   exact target id.
/// - (b)-(e): every other call — the bare control (`Update`/`Editable`, no
///   `.Page`), the deep chain (`Lines.Page.Foo.Bar`), the unknown part
///   (`Nope`), and the `SystemPart`/`UserControl` controls (`Notes`/
///   `MyAddIn`, WITH and WITHOUT `.Page`) — must ALL stay honest `Unknown`.
///   Asserting the exact COUNT (8) alongside the uniform `Unknown`
///   classification catches any one of them silently starting to resolve
///   (which would drop the count) as well as any one of them being
///   misclassified as something other than `Unknown`.
#[test]
fn ws_compound_receiver_currpage_part_page_resolves_subpage_all_others_stay_unknown() {
    let report = ws_compound_receiver_report();
    let edges = edges_for_object_routine(&report, 50991, "onopenpage");
    assert_eq!(edges.len(), 9, "OnOpenPage must have 9 call obligations");

    let source_edges: Vec<&&ClassifiedEdge> = edges
        .iter()
        .filter(|ce| ce.edge.routes.first().map(|r| &r.evidence) == Some(&Evidence::Source))
        .collect();
    assert_eq!(
        source_edges.len(),
        1,
        "exactly ONE call must resolve — the CurrPage.Lines.Page subpage instance call"
    );
    let route = &source_edges[0].edge.routes[0];
    assert_eq!(route.evidence, Evidence::Source);
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected RouteTarget::Routine, got {:?}", route.target);
    };
    assert_eq!(rid.name_lc, "refreshlines");
    assert_eq!(rid.object.kind, ObjectKind::Page);
    assert!(
        rid.object.id_equals_number(50990),
        "must resolve to \"Customer Card Part\" (id 50990); got {:?}",
        rid.object
    );
    assert!(
        matches!(route.witness, Witness::SourceSpan { .. }),
        "witness must be SourceSpan; got {:?}",
        route.witness
    );

    let unknown_edges: Vec<&&ClassifiedEdge> = edges
        .iter()
        .filter(|ce| {
            matches!(
                ce.edge.routes.first().map(|r| &r.evidence),
                Some(Evidence::Unknown(_))
            )
        })
        .collect();
    assert_eq!(
        unknown_edges.len(),
        8,
        "the other 8 calls (bare control, deep chain, unknown part, \
         SystemPart/UserControl with and without .Page) must ALL stay honest \
         Unknown — none may be fabricated as a route to \"Customer Card \
         Part\" or anything else"
    );
    for ce in &unknown_edges {
        assert_eq!(
            ce.edge.routes.first().map(|r| &r.target),
            Some(&RouteTarget::Unresolved),
            "an Unknown-evidence route must target Unresolved; got {:?}",
            ce.edge.routes
        );
    }
}

// ---------------------------------------------------------------------------
// Task 3 (no CDO — always runs): fresh-native `UnknownReason` diagnostic +
// stratified breakdown, end-to-end over a real `resolve_full_program` run
// (not just the synthetic edges `edge::tests::unknown_reason_breakdown_
// sums_to_unknown_count` constructs directly).
// ---------------------------------------------------------------------------

/// Runs `unknown_reason_breakdown` over `resolve_full_program`'s real output
/// (not synthetic edges) for a corpus of existing `ws-*`/fixture workspaces
/// chosen to span multiple structurally-distinct decline sites, and pins:
/// (i) per-fixture `sum(breakdown.values()) == histogram.unknown` (the
/// EXHAUSTIVE stratification invariant — the same one `aldump`'s
/// `unknownByReason` relies on); (ii) the COMBINED corpus spans >=4 distinct
/// [`UnknownReason`]s (observed via a one-off `--ignored` dump — see git
/// history — then pinned here); (iii) the full histogram is untouched by
/// Task 3 (a `real_unknown_rate`/`unknown` count sanity check per fixture).
#[test]
fn unknown_reason_breakdown_over_real_fixtures_sums_and_spans_reasons() {
    use al_sem::program::resolve::edge::{UnknownReason, unknown_reason_breakdown};
    use std::collections::BTreeMap;

    let fixtures = [
        "tests/fixtures/full_program_fixture",
        "tests/r0-corpus/ws-compound-receiver",
        "tests/r0-corpus/ws-codeunit-rec",
        "tests/r0-corpus/ws-page-rec",
        "tests/r0-corpus/ws-builtin-shadow",
        "tests/r0-corpus/ws-overload-collision",
    ];

    let mut combined: BTreeMap<UnknownReason, usize> = BTreeMap::new();
    for fx in fixtures {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(fx);
        assert!(path.exists(), "fixture must exist: {fx}");
        let report =
            resolve_full_program(&path).unwrap_or_else(|| panic!("{fx}: resolve_full_program"));

        let edges: Vec<&_> = report.edges.iter().map(|ce| &ce.edge).collect();
        let breakdown = unknown_reason_breakdown(edges.iter().copied());
        let sum: usize = breakdown.values().sum();
        assert_eq!(
            sum, report.histogram.unknown,
            "{fx}: sum(unknownByReason) must equal the Unknown obligation count; \
             breakdown={breakdown:?}"
        );
        for (reason, count) in breakdown {
            *combined.entry(reason).or_insert(0) += count;
        }
    }

    assert!(
        combined.len() >= 4,
        "combined corpus must span >=4 distinct UnknownReasons, got {}: {combined:?}",
        combined.len()
    );
    // Pin the specific reasons this corpus is known to exercise (observed via
    // the one-off dump; a change here means the corpus's decline sites
    // shifted — investigate before updating).
    //
    // `OverloadAmbiguous` REMOVED (Task 4, sigfp-and-ambiguous-reclassification
    // plan — the metric-definition change): `ws-overload-collision`'s ONLY
    // genuine ambiguity site (`Target.Resolve(5)` against two same-arity
    // `Resolve` overloads) was this corpus's sole `OverloadAmbiguous` source —
    // it now classifies `ObligationOutcome::AmbiguousResolved` (candidate-
    // carrying, not `Unknown`), so `unknown_reason_breakdown` (which only
    // walks `Unknown`-classified edges) no longer surfaces it anywhere in this
    // corpus. See `ws_overload_collision_ambiguous_call_becomes_ambiguous_
    // resolved_with_two_candidates` for the direct fixture pinning the new
    // shape.
    for expected in [
        UnknownReason::CodeunitTableNoExcluded,
        UnknownReason::CompoundReceiver,
        UnknownReason::UntrackedReceiver,
        UnknownReason::ReceiverOutOfClosure,
    ] {
        assert!(
            combined.contains_key(&expected),
            "expected reason {expected} to appear in the combined breakdown: {combined:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Test 29 (no CDO — always runs): beyond-1B.3b Task 8 grep-guard.
//
// 1B.3b Task 3 established (module-doc convention only, no CI enforcement)
// that `src/program/resolve` is L3-INDEPENDENT except for ONE sanctioned
// reuse: `builtins.rs::global_builtins` re-exports the platform
// builtin-membership catalog from `engine::l3::global_builtins`. Two reviewers
// flagged that this invariant was convention-only — nothing stopped a future
// task from adding a new `engine::l3`/`engine::l2` import elsewhere in the
// directory and silently reopening the L3 dependency the whole 1B.3b arc
// worked to retire. This test closes that gap: it scans every `.rs` file
// under `src/program/resolve` (flat directory, no subdirectories — verified
// at the time of writing) and fails on any `engine::l3`/`engine::l2` mention
// in CODE (not doc/line comments — several files' module docs legitimately
// EXPLAIN the invariant by naming `engine::l3` in prose, e.g. `differential.rs`
// / `semantic_golden.rs` / `member_catalog.rs`; those must NOT trip the
// guard) outside `builtins.rs`.
// ---------------------------------------------------------------------------

/// Fails if any `src/program/resolve/*.rs` file OTHER than `builtins.rs`
/// contains a live `engine::l3`/`engine::l2` reference outside a `//`/`///`/
/// `//!` comment. `builtins.rs` is the ONE sanctioned exception
/// (`global_builtins` re-export, 1B.3b Task 3) and is skipped entirely — its
/// own module doc explains and bounds that reuse.
///
/// Comment-stripping is a simple "truncate at the first `//` on the line"
/// pass — sufficient here because every file under this directory uses
/// `//`-style (line/doc/module-doc) comments exclusively (no `/* */` block
/// comments), verified at the time of writing. A future block comment would
/// need this test upgraded; until then, a false NEGATIVE (missing a real
/// import hidden after a `//` on the same line) is the only failure mode,
/// never a false positive that would mask a real new dependency.
#[test]
fn resolve_module_has_no_stray_engine_l3_l2_imports() {
    let resolve_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/program/resolve");
    let mut offenders: Vec<String> = Vec::new();
    let mut scanned_files = 0usize;

    let entries = std::fs::read_dir(&resolve_dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", resolve_dir.display()));
    for entry in entries {
        let entry = entry.expect("readable dir entry");
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let file_name = path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or_default()
            .to_string();
        // The ONE sanctioned exception — see this test's doc comment.
        if file_name == "builtins.rs" {
            continue;
        }
        scanned_files += 1;

        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        for (i, raw_line) in content.lines().enumerate() {
            let code = match raw_line.find("//") {
                Some(idx) => &raw_line[..idx],
                None => raw_line,
            };
            if code.contains("engine::l3") || code.contains("engine::l2") {
                offenders.push(format!("{file_name}:{}: {}", i + 1, raw_line.trim()));
            }
        }
    }

    assert!(
        scanned_files > 5,
        "grep-guard scanned suspiciously few files ({scanned_files}) under \
         {} — directory listing may be broken (test would pass vacuously)",
        resolve_dir.display(),
    );
    assert!(
        offenders.is_empty(),
        "engine::l3/engine::l2 reference(s) found in src/program/resolve \
         OUTSIDE the sanctioned builtins.rs::global_builtins exception \
         (1B.3b Task 3 / beyond-1B.3b Task 8 grep-guard) — this directory is \
         meant to stay fully L3-INDEPENDENT except that one deliberate reuse. \
         Either move the new code to use a different, non-L3 source, or (if \
         the reuse is deliberate and bounded like builtins.rs's) extend this \
         guard's exception list with the same justification:\n{:#?}",
        offenders,
    );
}

// ---------------------------------------------------------------------------
// Test 30 (no CDO — always runs): Task 1 (I1) grep-guard — the caller set of
// the pick-first base functions `ProgramGraph::resolve_object` /
// `ResolveIndex::object_by_number` is a KNOWN, AUDITED allowlist.
//
// Task 1's root fix made both base functions fail-closed themselves (own-app
// shadow preserved, but >1 VISIBLE-in-closure dependency match now DECLINES
// (`None`) instead of silently picking the lowest `ObjectNodeId` — I1's
// cardinal sin: a confident WRONG `Source` route). The Step-1 caller audit
// found every existing call site is a legitimate SEMANTIC AL-object-reference
// resolution (extension-base lookup, `ObjectRun` target resolution, typed
// `Object` receiver dispatch, event-subscriber publisher resolution, and the
// numeric/name fallback inside `resolve_object_name_lc`) that inherits the
// fail-closed behavior automatically — none of them needed migrating to
// `ResolveIndex::resolve_object_ref`, and NO genuinely non-semantic
// (indexing/diagnostic) caller was found, so no `resolve_object_first_by_
// stable_id` escape hatch was created.
//
// Task 2 (mirrors I1) MIGRATED `receiver.rs`'s two entries (the
// `resolve_object_name_lc` numeric/name fallback pair) to
// `ResolveIndex::resolve_object_ref` — the exact "(b)" migration this guard's
// own message anticipates: `ParsedType::Object` now carries a losslessly
// shaped `ObjectRef` (mirrors `ParsedType::Record`'s `table_ref`), so
// `resolve_object_ref_lc` calls `resolve_object_ref` directly instead of
// `graph.resolve_object`/`index.object_by_number`. Their removal from
// `expected` below is that migration being reflected, not a regression.
//
// Plan v2.1 Task 3 ADDS one entry: `receiver.rs`'s `interface_own_routine_
// node` resolves a cross-object chain's `Interface`-typed prefix by NAME to
// look up the interface's own declared member signature — a genuine
// semantic caller, structurally identical to `resolve_member`'s existing
// `Object` arm entry below (typed-receiver-by-name dispatch, own-app-first
// then closure-scoped, ambiguous cross-app name declines to `None` for
// free). No `Ambiguous`/`OutOfClosure` distinction is needed here either.
//
// Plan v2.1 Task 2 (T1-review fold-in) ADDS one entry: `index.rs`'s new
// `count_unknown_include_sender_plus1_subscribers` preflight diagnostic
// resolves the same publisher-object identity the subscriber-wiring loop
// above it already does, via the SAME base function — a genuine semantic
// caller reusing the audited resolution semantics for a read-only count,
// never a distinct resolution path. No `Ambiguous`/`OutOfClosure`
// distinction is needed (identical rationale to the sibling wiring entry).
//
// This guard locks that audited set in place: a NEW call site appearing in
// `src/program/resolve/*.rs` PRODUCTION code (before each file's `#[cfg(test)]`
// module marker — test fixtures directly exercising the API are expected and
// unbounded, not part of this guard) that isn't already in the allowlist below
// must be deliberately reviewed — is it a genuine semantic caller (inherits
// the fix for free, add it here with justification), or does it need the
// `Ambiguous`/`OutOfClosure` distinction (then it must call
// `ResolveIndex::resolve_object_ref` instead, per `resolve_source_table_ref`
// / `resolve_pageext_base_page` / `resolve_tableext_base_table`'s template)?
// ---------------------------------------------------------------------------

/// Fails if the set of PRODUCTION (pre-`#[cfg(test)]`) call sites of
/// `.resolve_object(` / `.object_by_number(` in `src/program/resolve/*.rs`
/// drifts from the Task 1 audited allowlist — new or removed call sites both
/// trip this (both are worth a deliberate look: a removal might mean the
/// caller was migrated to `resolve_object_ref` and this allowlist is now
/// stale; an addition needs the classification above).
///
/// Matching is by TRIMMED LINE TEXT (not line number), the same
/// comment-stripping convention as
/// `resolve_module_has_no_stray_engine_l3_l2_imports`, so unrelated edits
/// elsewhere in a file never spuriously trip this guard — only an edit to
/// (or the addition/removal of) one of these specific call expressions does.
#[test]
fn resolve_module_pick_first_base_function_callers_are_a_known_allowlist() {
    let resolve_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/program/resolve");

    // (file_name, expected trimmed production call-site line).
    let expected: Vec<(&str, &str)> = vec![
        (
            "resolver.rs",
            "&& let Some(base_obj) = graph.resolve_object(from_object.id.app, base_kind, extends_target)",
        ),
        (
            "resolver.rs",
            "graph.resolve_object(from, object_kind, target_ref)",
        ),
        (
            "resolver.rs",
            ".object_by_number(graph, from, object_kind, n)",
        ),
        (
            "resolver.rs",
            "None => graph.resolve_object(from_object.id.app, *kind, name_lc),",
        ),
        (
            "index.rs",
            "let Some(pub_obj) = graph.resolve_object(sub_app, kind, &args.publisher_name)",
        ),
        (
            "index.rs",
            "let Some(pub_obj) = graph.resolve_object(sub_app, kind, &args.publisher_name) else {",
        ),
        (
            "receiver.rs",
            "let iface = graph.resolve_object(from_object.id.app, ObjectKind::Interface, name_lc)?;",
        ),
    ];

    let mut found: Vec<(String, String)> = Vec::new();
    let mut scanned_files = 0usize;

    let entries = std::fs::read_dir(&resolve_dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", resolve_dir.display()));
    for entry in entries {
        let entry = entry.expect("readable dir entry");
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let file_name = path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or_default()
            .to_string();
        scanned_files += 1;

        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        // Only PRODUCTION code — everything from the file's `#[cfg(test)]`
        // module marker onward is test fixture code, exempt from this guard.
        let production_code = match content.find("\n#[cfg(test)]") {
            Some(idx) => &content[..idx],
            None => &content[..],
        };
        for raw_line in production_code.lines() {
            let code = match raw_line.find("//") {
                Some(idx) => &raw_line[..idx],
                None => raw_line,
            };
            let trimmed = code.trim();
            // Exclude the `object_by_number` FUNCTION DEFINITION itself (not
            // a call site) — the only line matching `object_by_number(` that
            // is a `fn` declaration rather than an invocation.
            if trimmed.starts_with("pub fn object_by_number(")
                || trimmed.starts_with("fn object_by_number(")
            {
                continue;
            }
            if trimmed.contains("resolve_object(") || trimmed.contains("object_by_number(") {
                found.push((file_name.clone(), trimmed.to_string()));
            }
        }
    }

    assert!(
        scanned_files > 5,
        "grep-guard scanned suspiciously few files ({scanned_files}) under \
         {} — directory listing may be broken (test would pass vacuously)",
        resolve_dir.display(),
    );

    let mut found_sorted = found.clone();
    found_sorted.sort();
    let mut expected_sorted: Vec<(String, String)> = expected
        .iter()
        .map(|(f, l)| (f.to_string(), l.to_string()))
        .collect();
    expected_sorted.sort();

    assert_eq!(
        found_sorted, expected_sorted,
        "the set of PRODUCTION call sites of resolve_object()/object_by_number() \
         in src/program/resolve/*.rs drifted from the Task 1 (I1) audited \
         allowlist. Every semantic caller inherits the root fix's fail-closed \
         behavior for free — but a NEW call site must still be deliberately \
         classified: (a) a genuine semantic caller → add it to `expected` in \
         this test with a one-line justification, or (b) a caller that needs \
         the Ambiguous/OutOfClosure distinction → migrate it to \
         `ResolveIndex::resolve_object_ref` instead (see \
         `resolve_source_table_ref`/`resolve_pageext_base_page`/ \
         `resolve_tableext_base_table` for the template). A REMOVED expected \
         entry likely means a caller was migrated to `resolve_object_ref` and \
         this allowlist is now stale — delete the corresponding `expected` \
         entry.\nfound:\n{found_sorted:#?}\nexpected:\n{expected_sorted:#?}",
    );
}

// ---------------------------------------------------------------------------
// Tests 27+: beyond-1B.3b Task 3 — `resolve_bare` Step 3 (bare implicit-Rec),
// with-guarded + builtin-collision-fail-closed, visibility-scoped, end-to-end
// over `ws-bare-implicit-rec`.
//
// Root fix: `resolve_bare`'s Step 3 was an empty `// TODO` — a bare
// (unqualified) call inside a Page/Table/TableExtension/PageExtension trigger
// that falls through Step 1 (own object) and Step 2 (extension base) now
// implicitly dispatches to `Rec` via `resolve_in_table_scope` (Task 2's
// visibility-scoped table∪extensions search), gated by a tri-state `with`-
// guard (`WithState`, `extract.rs`) and a builtin/intrinsic PROBE-THEN-DECIDE
// collision check (fail-closed to `Unknown` on ANY unproven precedence). Every
// letter below matches the task brief's fixture list (a)-(k); (d)-(k) are all
// NEGATIVE/precedence proofs — the correctness contract that Step 3 does NOT
// over-fire is as load-bearing as the positive cases.
// ---------------------------------------------------------------------------

/// Loads `tests/r0-corpus/ws-bare-implicit-rec` and returns the full
/// `resolve_full_program` report — shared by Tests 27a-27k below.
fn ws_bare_implicit_rec_report() -> ProgramReport {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-bare-implicit-rec");
    resolve_full_program(&fixture)
        .expect("resolve_full_program must succeed on ws-bare-implicit-rec")
}

/// Test 27a (fixture a, POSITIVE): `IR Page A` (`SourceTable = "IR Table A"`,
/// NO own `GetName`) — bare `GetName();` in `OnOpenPage` must resolve through
/// Step 3 to `"IR Table A".GetName`, `Evidence::Source`. Before Task 3 this
/// was an honest `Unknown` (Step 3 was an empty TODO).
#[test]
fn ws_bare_implicit_rec_page_source_table_proc_resolves_via_step3() {
    let report = ws_bare_implicit_rec_report();
    let edges = edges_for_object_routine(&report, 50971, "onopenpage");
    assert_eq!(edges.len(), 1, "IR Page A.OnOpenPage has 1 call obligation");
    let ce = edges[0];
    assert_eq!(ce.edge.kind, EdgeKind::Call);
    let route = &ce.edge.routes[0];
    assert_eq!(
        route.evidence,
        Evidence::Source,
        "bare GetName() must resolve through Step 3 implicit-Rec; got {route:?}"
    );
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected RouteTarget::Routine, got {:?}", route.target);
    };
    assert_eq!(rid.name_lc, "getdisplaytext");
    assert_eq!(rid.object.kind, ObjectKind::Table);
    assert!(
        rid.object.id_equals_number(50970),
        "must resolve to \"IR Table A\" (id 50970); got {:?}",
        rid.object
    );
    assert!(matches!(route.witness, Witness::SourceSpan { .. }));
}

/// Test 27b (fixture b, OWN-OBJECT SHADOW): `IR Page B` has the SAME
/// `SourceTable = "IR Table A"` (which ALSO declares `GetName`) but ALSO
/// declares its OWN `GetName`. Step 1 (own object) must win — the bare call
/// must resolve to THIS PAGE's `GetName`, never reaching Step 3, even though
/// Step 3 would have found a matching candidate too.
#[test]
fn ws_bare_implicit_rec_own_object_shadows_step3() {
    let report = ws_bare_implicit_rec_report();
    let edges = edges_for_object_routine(&report, 50972, "onopenpage");
    assert_eq!(edges.len(), 1, "IR Page B.OnOpenPage has 1 call obligation");
    let route = &edges[0].edge.routes[0];
    assert_eq!(route.evidence, Evidence::Source);
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected RouteTarget::Routine, got {:?}", route.target);
    };
    assert_eq!(rid.name_lc, "getdisplaytext");
    assert_eq!(
        rid.object.kind,
        ObjectKind::Page,
        "must resolve to the PAGE's own GetName, not the table's; got {:?}",
        rid.object
    );
    assert!(
        rid.object.id_equals_number(50972),
        "must resolve to IR Page B itself (id 50972); got {:?}",
        rid.object
    );
}

/// Test 27c (fixture c, POSITIVE — visible TableExtension): `IR Page C`
/// (`SourceTable = "IR Table A"`) calls bare `ExtProc();`, declared only on
/// the visible TableExtension `IR Table A Ext C`. Must resolve through Step 3
/// to the extension's `ExtProc`.
#[test]
fn ws_bare_implicit_rec_visible_table_extension_resolves_via_step3() {
    let report = ws_bare_implicit_rec_report();
    let edges = edges_for_object_routine(&report, 50974, "onopenpage");
    assert_eq!(edges.len(), 1, "IR Page C.OnOpenPage has 1 call obligation");
    let route = &edges[0].edge.routes[0];
    assert_eq!(route.evidence, Evidence::Source);
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected RouteTarget::Routine, got {:?}", route.target);
    };
    assert_eq!(rid.name_lc, "extproc");
    assert_eq!(rid.object.kind, ObjectKind::TableExtension);
    assert!(
        rid.object.id_equals_number(50973),
        "must resolve to \"IR Table A Ext C\" (id 50973); got {:?}",
        rid.object
    );
}

/// Test 27d (fixture d, NEGATIVE — sibling-extension ambiguity): `IR Page D`
/// calls bare `Dup();`, declared identically on TWO visible TableExtensions
/// of "IR Table A" — must stay honest `Unknown` (never pick one arbitrarily).
#[test]
fn ws_bare_implicit_rec_sibling_extension_ambiguity_is_unknown() {
    let report = ws_bare_implicit_rec_report();
    let edges = edges_for_object_routine(&report, 50977, "onopenpage");
    assert_eq!(edges.len(), 1, "IR Page D.OnOpenPage has 1 call obligation");
    let route = &edges[0].edge.routes[0];
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(
        matches!(route.evidence, Evidence::Unknown(_)),
        "ambiguous sibling-extension Dup() must stay honest Unknown, never pick-first; got {route:?}"
    );
}

/// Test 27e (fixture e, NEGATIVE — builtin collision): `IR Page E` calls bare
/// `StrLen(Txt)` (arity 1), which collides in name+arity between the implicit
/// table's own `StrLen` procedure and the global `strlen` intrinsic. Must
/// stay honest `Unknown` — NEVER `Catalog` (the PROBE-THEN-DECIDE guard).
#[test]
fn ws_bare_implicit_rec_builtin_collision_is_unknown_not_catalog() {
    let report = ws_bare_implicit_rec_report();
    let edges = edges_for_object_routine(&report, 50979, "onopenpage");
    assert_eq!(edges.len(), 1, "IR Page E.OnOpenPage has 1 call obligation");
    let route = &edges[0].edge.routes[0];
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(
        matches!(route.evidence, Evidence::Unknown(_)),
        "StrLen(Txt) table-proc/builtin collision must fail closed to Unknown, \
         never assume the table wins; got {route:?}"
    );
}

/// Test 27f (fixture f, REBASELINED — pageext-merge-and-final-residual plan,
/// Task 2): `IR Page F` calls bare `Update();` (arity 0), which collides in
/// name+arity between the implicit table's own `Update` procedure and
/// `PageInstance`'s catalog entry `Update`. PRE-Task-2 this failed closed to
/// `Unknown` (no compiler-verified precedence captured). Task 2 grounded
/// `Update` (MS Learn: <https://learn.microsoft.com/en-us/dynamics365/
/// business-central/dev-itpro/developer/methods-auto/page/page-update-method>)
/// as having NO bare-call form anywhere in AL — always receiver-qualified
/// (`CurrPage.Update()`) — so the "PageInstance intrinsic" reading was never
/// real: the table's own `Update` procedure now correctly wins. This is a
/// DELIBERATE rebaseline (the false-`Unknown` this test used to pin was
/// itself the bug), not a regression — see
/// `resolver::INSTANCE_ONLY_NEVER_BARE`'s doc for the full grounding matrix.
#[test]
fn ws_bare_implicit_rec_page_intrinsic_now_resolves_via_grounded_suppression() {
    let report = ws_bare_implicit_rec_report();
    let edges = edges_for_object_routine(&report, 50981, "onopenpage");
    assert_eq!(edges.len(), 1, "IR Page F.OnOpenPage has 1 call obligation");
    let route = &edges[0].edge.routes[0];
    assert_eq!(route.evidence, Evidence::Source);
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected RouteTarget::Routine, got {:?}", route.target);
    };
    assert_eq!(rid.name_lc, "update");
    assert_eq!(rid.object.kind, ObjectKind::Table);
    assert!(
        rid.object.id_equals_number(50980),
        "must resolve to \"IR Page Intrinsic Table\" (id 50980); got {:?}",
        rid.object
    );
}

/// Test 27g (fixture g, NEGATIVE — `with`-block): `IR Page G`'s bare
/// `GetNameW();` call sits inside `with OtherRec do begin ... end`, where
/// `OtherRec` is a DIFFERENT record than the page's own `SourceTable`
/// (`"IR With Target Table"`, which DOES declare a matching `GetNameW`). The
/// with-guard (`WithState::InsideWith`) must skip Step 3 entirely — stays
/// honest `Unknown`, never `"IR With Target Table".GetNameW`.
#[test]
fn ws_bare_implicit_rec_inside_with_block_skips_step3() {
    let report = ws_bare_implicit_rec_report();
    let edges = edges_for_object_routine(&report, 50984, "onopenpage");
    assert_eq!(edges.len(), 1, "IR Page G.OnOpenPage has 1 call obligation");
    let route = &edges[0].edge.routes[0];
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(
        matches!(route.evidence, Evidence::Unknown(_)),
        "a bare call inside `with OtherRec do` must NEVER resolve through the \
         page's own SourceTable implicit-Rec — the with-guard must skip Step 3; \
         got {route:?}"
    );
}

/// Test 27h (fixture h, NEGATIVE — no implicit table): `IR No Table CU` (a
/// plain Codeunit, no `TableNo`) calls bare `Foo();` — not its own procedure,
/// not a builtin. Step 3's strict-kind guard structurally excludes Codeunit —
/// stays honest `Unknown`.
#[test]
fn ws_bare_implicit_rec_codeunit_no_table_stays_unknown() {
    let report = ws_bare_implicit_rec_report();
    let edges = edges_for_object_routine(&report, 50985, "onrun");
    assert_eq!(edges.len(), 1, "IR No Table CU.OnRun has 1 call obligation");
    let route = &edges[0].edge.routes[0];
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 27i (fixture i, SHADOW-GUARD — NOT a Step-3 proof): `IR Self Table`'s
/// `Run` calls bare `Recalc();`, declared on the SAME table. Resolves via
/// Step 1 (own object) — documents that Step 1 short-circuits before Step 3
/// is ever reached, even for a `Table` kind (one of Step 3's four eligible
/// kinds).
#[test]
fn ws_bare_implicit_rec_table_own_trigger_resolves_via_step1_not_step3() {
    let report = ws_bare_implicit_rec_report();
    let edges = edges_for_object_routine(&report, 50986, "run");
    assert_eq!(edges.len(), 1, "IR Self Table.Run has 1 call obligation");
    let route = &edges[0].edge.routes[0];
    assert_eq!(route.evidence, Evidence::Source);
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected RouteTarget::Routine, got {:?}", route.target);
    };
    assert_eq!(rid.name_lc, "recalc");
    assert_eq!(rid.object.kind, ObjectKind::Table);
    assert!(
        rid.object.id_equals_number(50986),
        "must resolve to IR Self Table itself (id 50986); got {:?}",
        rid.object
    );
}

/// Test 27j (fixture j, PRECEDENCE — PageExtension base vs SourceTable):
/// `IR PageExt J` (a PageExtension of `IR PageExt Base Page`, whose base page
/// declares its OWN `Foo` AND whose `SourceTable` ALSO declares a `Foo`)
/// calls bare `Foo()` from its own `CallFoo` procedure. Must resolve to the
/// BASE PAGE's `Foo` via Step 2 (extension-base) — Step 2 runs BEFORE Step 3
/// (implicit-Rec) in `resolve_bare`'s precedence order. This pins
/// PRE-EXISTING ordering (Task 3 does not change Steps 1-2); Step 3 merely
/// stays unreached here.
#[test]
fn ws_bare_implicit_rec_pageext_base_precedes_step3_source_table() {
    let report = ws_bare_implicit_rec_report();
    let edges = edges_for_object_routine(&report, 50989, "callfoo");
    assert_eq!(edges.len(), 1, "IR PageExt J.CallFoo has 1 call obligation");
    let route = &edges[0].edge.routes[0];
    assert_eq!(route.evidence, Evidence::Source);
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected RouteTarget::Routine, got {:?}", route.target);
    };
    assert_eq!(rid.name_lc, "foo");
    assert_eq!(
        rid.object.kind,
        ObjectKind::Page,
        "must resolve to the BASE PAGE's Foo (Step 2), not the SourceTable's \
         (Step 3); got {:?}",
        rid.object
    );
    assert!(
        rid.object.id_equals_number(50988),
        "must resolve to \"IR PageExt Base Page\" (id 50988), NOT \
         \"IR PageExt Src Table\" (id 50987); got {:?}",
        rid.object
    );
}

/// Test 27k (fixture k, NEGATIVE — strict-kind): Report and Codeunit+TableNo
/// both call a bare `Foo();` matching a real, resolvable table procedure —
/// Step 3's strict `ObjectKind` guard (`{Table, Page, TableExtension,
/// PageExtension}` ONLY) structurally excludes both kinds, so neither
/// resolves. The Codeunit+TableNo case is the stronger proof: its implicit
/// Rec IS statically typed (Task 6, for EXPLICIT `Rec.Foo()` calls) yet the
/// BARE fallback still never fires.
#[test]
fn ws_bare_implicit_rec_strict_kind_report_and_codeunit_tableno_stay_unknown() {
    let report = ws_bare_implicit_rec_report();

    let report_edges = edges_for_object_routine(&report, 50991, "onaftergetrecord");
    assert_eq!(
        report_edges.len(),
        1,
        "IR Strict Kind Report.OnAfterGetRecord has 1 call obligation"
    );
    let report_route = &report_edges[0].edge.routes[0];
    assert_eq!(report_route.target, RouteTarget::Unresolved);
    assert!(
        matches!(report_route.evidence, Evidence::Unknown(_)),
        "Report is structurally excluded from Step 3; got {report_route:?}"
    );

    let cu_edges = edges_for_object_routine(&report, 50992, "onrun");
    assert_eq!(
        cu_edges.len(),
        1,
        "IR Strict Kind CU2.OnRun has 1 call obligation"
    );
    let cu_route = &cu_edges[0].edge.routes[0];
    assert_eq!(cu_route.target, RouteTarget::Unresolved);
    assert!(
        matches!(cu_route.evidence, Evidence::Unknown(_)),
        "Codeunit is structurally excluded from Step 3 even WITH a matching \
         TableNo; got {cu_route:?}"
    );
}

// ---------------------------------------------------------------------------
// Review-fix fixtures (Task 3 NEEDS-FIXES finding): the original (a)-(k) set
// covers Step 3's implicit-Rec table computation for all four eligible
// `ObjectKind`s (Table/Page/TableExtension/PageExtension), but only the
// `Page` arm ((a)/(c)) had a fixture where Step 3 actually FIRES and returns
// `Evidence::Source` from a POSITIVE call site of that exact kind: `Table`
// ((i)) short-circuits at Step 1; `TableExtension` appears only as a
// resolution TARGET ((c)/(d)), never as the CALLER; `PageExtension` ((j)) is
// the NEGATIVE case where Step 2 wins and Step 3 is never entered. The two
// tests below close that gap — a bare call inside a `TableExtension`
// resolving through the sibling-extension union, and a bare call inside a
// `PageExtension` resolving through the base page's inherited `SourceTable`.
// ---------------------------------------------------------------------------

/// Review-fix fixture, POSITIVE — `TableExtension` CALLER reaching Step 3 via
/// the sibling-extension union: `IR TableExt A`'s `CallShared` makes a bare
/// call to `SharedProc()`, declared ONLY on the SIBLING TableExtension
/// `IR TableExt B` (both extend "IR TableExt Base T"). Step 1 (own object)
/// and Step 2 (extension base, base-table-only) both decline — only Step 3's
/// `resolve_in_table_scope` (base table ∪ ALL its visible TableExtensions)
/// finds it, via the sibling.
#[test]
fn ws_bare_implicit_rec_tableextension_caller_resolves_sibling_via_step3() {
    let report = ws_bare_implicit_rec_report();
    let edges = edges_for_object_routine(&report, 50995, "callshared");
    assert_eq!(
        edges.len(),
        1,
        "IR TableExt A.CallShared has 1 call obligation"
    );
    let ce = edges[0];
    assert_eq!(ce.edge.kind, EdgeKind::Call);
    let route = &ce.edge.routes[0];
    assert_eq!(
        route.evidence,
        Evidence::Source,
        "bare SharedProc() must resolve through Step 3's sibling-extension \
         union; got {route:?}"
    );
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected RouteTarget::Routine, got {:?}", route.target);
    };
    assert_eq!(rid.name_lc, "sharedproc");
    assert_eq!(
        rid.object.kind,
        ObjectKind::TableExtension,
        "must resolve to the SIBLING extension's SharedProc, not the base \
         table's; got {:?}",
        rid.object
    );
    assert!(
        rid.object.id_equals_number(50994),
        "must resolve to \"IR TableExt B\" (id 50994), NOT the caller \
         \"IR TableExt A\" (50995) or the base table (50993); got {:?}",
        rid.object
    );
    assert!(matches!(route.witness, Witness::SourceSpan { .. }));
}

/// Review-fix fixture, POSITIVE — `PageExtension` CALLER reaching Step 3 via
/// the base page's inherited `SourceTable`: `IR PageExt2 Ext`'s
/// `CallOnlyOnTable` makes a bare call to `OnlyOnTable()`, declared ONLY on
/// `IR PageExt2 Src Table` (the `SourceTable` of the base page "IR PageExt2
/// Base Page", which does NOT declare `OnlyOnTable` itself). Step 1 (own
/// object) and Step 2 (extension base, base-PAGE-only) both decline — only
/// Step 3's `resolve_pageext_base_source_table` → `resolve_in_table_scope`
/// finds it, on the SourceTable.
#[test]
fn ws_bare_implicit_rec_pageextension_caller_resolves_sourcetable_via_step3() {
    let report = ws_bare_implicit_rec_report();
    let edges = edges_for_object_routine(&report, 50998, "callonlyontable");
    assert_eq!(
        edges.len(),
        1,
        "IR PageExt2 Ext.CallOnlyOnTable has 1 call obligation"
    );
    let ce = edges[0];
    assert_eq!(ce.edge.kind, EdgeKind::Call);
    let route = &ce.edge.routes[0];
    assert_eq!(
        route.evidence,
        Evidence::Source,
        "bare OnlyOnTable() must resolve through Step 3's PageExtension \
         SourceTable lookup; got {route:?}"
    );
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected RouteTarget::Routine, got {:?}", route.target);
    };
    assert_eq!(rid.name_lc, "onlyontable");
    assert_eq!(
        rid.object.kind,
        ObjectKind::Table,
        "must resolve to the SourceTable's OnlyOnTable, not the base page's \
         or the extension's; got {:?}",
        rid.object
    );
    assert!(
        rid.object.id_equals_number(50996),
        "must resolve to \"IR PageExt2 Src Table\" (id 50996), NOT the base \
         page \"IR PageExt2 Base Page\" (50997) or the caller \"IR PageExt2 \
         Ext\" (50998); got {:?}",
        rid.object
    );
    assert!(matches!(route.witness, Witness::SourceSpan { .. }));
}

// ---------------------------------------------------------------------------
// Tests 29+: beyond-1B.3b Task 3 — `Func().Method()` compound-receiver
// resolution (prefix typed via `resolve_bare`, fail-closed), end-to-end over
// `ws-compound-call-result`.
//
// Root feature: `infer_receiver_type`'s new Step 5 (`src/program/resolve/
// receiver.rs`, `infer_call_result_receiver`) types a `Func().Method()`
// receiver by the return type of the bare same-object function `Func()`:
// local-shadowing guard first (a param/local/global named identically to
// `Func` SHADOWS it in AL — `resolve_bare` cannot see variables), then
// `resolve_bare` as a TYPE QUERY (reusing its own-object/extension-base/
// implicit-Rec/builtin precedence, same-arity-overload-ambiguity decline, and
// builtin/Rec-shadow PROBE-THEN-DECIDE collision guard), then a non-scalar
// `return_type` guard, then `classify_type_text` →
// `parsed_type_to_receiver` (the SAME fail-closed conversion Step 2's
// declared-variable path uses). Every letter below matches the task brief's
// fixture list; (b)-(h2) are all NEGATIVE/decline proofs.
// ---------------------------------------------------------------------------

/// Loads `tests/r0-corpus/ws-compound-call-result` and returns the full
/// `resolve_full_program` report — shared by Tests 29a-29l below.
fn ws_compound_call_result_report() -> ProgramReport {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-compound-call-result");
    resolve_full_program(&fixture)
        .expect("resolve_full_program must succeed on ws-compound-call-result")
}

/// The route for the OUTER `.Method()` member call of a `Func().Method()`
/// fixture routine.
///
/// Every fixture routine here has exactly TWO call obligations, not one: the
/// extractor walks `Func().Method()` recursively and emits a call site for
/// EVERY `Call` node it contains — `Func()` is a genuine call in its own
/// right (classified `CalleeShape::Bare`, resolved independently via ordinary
/// bare-call precedence, wholly UNRELATED to Task 3's new receiver-typing
/// step) alongside the OUTER `.Method()` call (`CalleeShape::Member`, the one
/// Task 3 actually types). The outer call's span always covers the WHOLE
/// `Func().Method()` expression, so it is always the WIDEST (by
/// `end.col - start.col`, both single-line spans in this fixture) of the
/// routine's obligations — a robust, order-independent selector.
fn outer_member_route(
    report: &ProgramReport,
    object_id_number: i64,
    routine_name_lc: &str,
) -> Route {
    let edges = edges_for_object_routine(report, object_id_number, routine_name_lc);
    assert_eq!(
        edges.len(),
        2,
        "{routine_name_lc} (object {object_id_number}) must have exactly 2 call \
         obligations (the inner Func() bare call + the outer .Method() member \
         call); got {:?}",
        edges.iter().map(|ce| &ce.edge).collect::<Vec<_>>()
    );
    let outer = edges
        .iter()
        .max_by_key(|ce| ce.edge.site.span.end.col as i64 - ce.edge.site.span.start.col as i64)
        .expect("edges is non-empty (asserted len == 2 above)");
    assert_eq!(
        outer.edge.kind,
        EdgeKind::Call,
        "the outer (widest-span) obligation must be the Member call"
    );
    outer.edge.routes[0].clone()
}

/// Test 29a (fixture a, POSITIVE): `GetCustomer()` (own, unique arity-0,
/// `Record "CR Customer"` return) types the receiver `Record{table:
/// Some(CRCustomer)}`; `Name` is a non-builtin Customer procedure — must
/// resolve `Source`, exact target id.
#[test]
fn ws_compound_call_result_record_return_resolves_to_source() {
    let report = ws_compound_call_result_report();
    let route = outer_member_route(&report, 51003, "testrecordreturn");
    assert_eq!(route.evidence, Evidence::Source);
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected RouteTarget::Routine, got {:?}", route.target);
    };
    assert_eq!(rid.name_lc, "name");
    assert_eq!(rid.object.kind, ObjectKind::Table);
    assert!(
        rid.object.id_equals_number(51000),
        "must resolve to \"CR Customer\" (id 51000); got {:?}",
        rid.object
    );
    assert!(matches!(route.witness, Witness::SourceSpan { .. }));
}

/// Test 29b (Codeunit-return shape, POSITIVE): `GetHelper()` (own, unique
/// arity-0, `Codeunit "CR Helper"` return) types the receiver `Object{Codeunit,
/// "CR Helper"}`; `DoWork` must resolve `Source`, exact target id. Return-type-
/// SHAPE coverage (Task-2 finding 3): `Codeunit X` alongside 29a's `Record X`.
#[test]
fn ws_compound_call_result_codeunit_return_resolves_to_source() {
    let report = ws_compound_call_result_report();
    let route = outer_member_route(&report, 51003, "testcodeunitreturn");
    assert_eq!(route.evidence, Evidence::Source);
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected RouteTarget::Routine, got {:?}", route.target);
    };
    assert_eq!(rid.name_lc, "dowork");
    assert_eq!(rid.object.kind, ObjectKind::Codeunit);
    assert!(
        rid.object.id_equals_number(51001),
        "must resolve to \"CR Helper\" (id 51001); got {:?}",
        rid.object
    );
    assert!(matches!(route.witness, Witness::SourceSpan { .. }));
}

/// Test 29c (fixture g, Interface-return POSITIVE/behavioral): `GetIFoo()`
/// (own, unique arity-0, `Interface ICRFoo` return) types the receiver
/// `Interface{"icrfoo"}` — Phase B fans out POLYMORPHICALLY to `ICRFoo`'s sole
/// implementer (`CR Foo Impl`), never a concrete guess. Return-type-SHAPE
/// coverage (Task-2 finding 3): `Interface IFoo` alongside 29a/29b.
#[test]
fn ws_compound_call_result_interface_return_fans_out_polymorphic() {
    let report = ws_compound_call_result_report();
    let edges = edges_for_object_routine(&report, 51003, "testinterfacereturn");
    assert_eq!(
        edges.len(),
        2,
        "TestInterfaceReturn has 2 call obligations (the inner GetIFoo() bare \
         call + the outer .Bar() member call); got {:?}",
        edges.iter().map(|ce| &ce.edge).collect::<Vec<_>>()
    );
    let ce = edges
        .iter()
        .max_by_key(|ce| ce.edge.site.span.end.col as i64 - ce.edge.site.span.start.col as i64)
        .expect("edges is non-empty (asserted len == 2 above)");
    assert_eq!(
        ce.edge.shape,
        DispatchShape::Polymorphic,
        "an Interface-return receiver must fan out Polymorphic, never a \
         concrete single guess; got {:?}",
        ce.edge.shape
    );
    assert_eq!(
        ce.edge.routes.len(),
        1,
        "ICRFoo has exactly one implementer; got {:?}",
        ce.edge.routes
    );
    let route = &ce.edge.routes[0];
    assert_eq!(route.evidence, Evidence::Source);
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected RouteTarget::Routine, got {:?}", route.target);
    };
    assert_eq!(rid.name_lc, "bar");
    assert_eq!(rid.object.kind, ObjectKind::Codeunit);
    assert!(
        rid.object.id_equals_number(51002),
        "must resolve to \"CR Foo Impl\" (id 51002); got {:?}",
        rid.object
    );
}

/// Test 29d (fixture b, NEGATIVE — wrong-overload guard): `GetX` is
/// overloaded (arity 0 → `Codeunit "CR Helper"`, arity 1 → `Record "CR
/// Customer"`); `GetX(1, 2)` (arity 2) matches NEITHER declared overload —
/// `resolve_bare`'s Step 1 (own object, zero arity-matched candidates) must
/// decline; `infer_call_result_receiver` requires a `RouteTarget::Routine`
/// and gets `Unresolved` instead — stays honest `Unknown`, never falls back
/// to either overload's return type.
#[test]
fn ws_compound_call_result_overload_arity_mismatch_stays_unknown() {
    let report = ws_compound_call_result_report();
    let route = outer_member_route(&report, 51003, "testoverloadaritymismatch");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(
        matches!(route.evidence, Evidence::Unknown(_)),
        "GetX(1, 2), matching neither the arity-0 nor arity-1 overload, must \
         stay honest Unknown; got {route:?}"
    );
}

/// Test 29e (fixture c, NEGATIVE — scalar return): `GetCount(): Integer` —
/// nothing to dispatch a member call on.
#[test]
fn ws_compound_call_result_scalar_return_stays_unknown() {
    let report = ws_compound_call_result_report();
    let route = outer_member_route(&report, 51003, "testscalarreturn");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(
        matches!(route.evidence, Evidence::Unknown(_)),
        "a scalar Integer return must never be treated as a dispatchable \
         receiver; got {route:?}"
    );
}

/// Test 29f (fixture d, NEGATIVE — absent prefix): `Nonexistent()` is not
/// declared anywhere reachable from `CallResultCaller`.
#[test]
fn ws_compound_call_result_absent_prefix_stays_unknown() {
    let report = ws_compound_call_result_report();
    let route = outer_member_route(&report, 51003, "testabsentprefix");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 29g (fixture d, NEGATIVE — arity mismatch, single overload):
/// `GetSingle` is declared ONLY at arity 1; called here with arity 0.
#[test]
fn ws_compound_call_result_arity_mismatch_single_overload_stays_unknown() {
    let report = ws_compound_call_result_report();
    let route = outer_member_route(&report, 51003, "testaritymismatchsingle");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 29h (fixture e, NEGATIVE — Rec/builtin-shadow): bare `Update()` used
/// as a compound-receiver prefix collides between the implicit-Rec table's
/// own (non-scalar-returning) `Update` procedure and the bare-callable
/// `PageInstance` intrinsic `Update` — `resolve_bare`'s PROBE-THEN-DECIDE
/// guard fails closed to `Unresolved{BuiltinPrecedenceCollision}` (never
/// assumes the table wins), so `Update().Bar()` stays honest `Unknown`.
#[test]
fn ws_compound_call_result_rec_builtin_shadow_stays_unknown() {
    let report = ws_compound_call_result_report();
    let route = outer_member_route(&report, 51004, "onopenpage");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(
        matches!(route.evidence, Evidence::Unknown(_)),
        "Update().Bar() must fail closed on the unproven table-vs-PageInstance-\
         intrinsic precedence collision, never assume the table wins; got {route:?}"
    );
}

/// Test 29i (local-var-shadow NEGATIVE, round-2 gemini critical): a local
/// `Integer` named identically to `CallResultCaller`'s OWN `GetCustomer`
/// procedure (the fixture-a positive target) SHADOWS it — the guard must fire
/// BEFORE ever calling `resolve_bare`, even though `GetCustomer()` would
/// otherwise resolve cleanly (proving the guard is load-bearing).
#[test]
fn ws_compound_call_result_local_var_shadow_stays_unknown() {
    let report = ws_compound_call_result_report();
    let route = outer_member_route(&report, 51003, "testlocalvarshadow");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(
        matches!(route.evidence, Evidence::Unknown(_)),
        "a local variable named identically to an own procedure must shadow \
         it — the Func() receiver must stay Unknown, never type via the \
         shadowed procedure's return type; got {route:?}"
    );
}

/// Test 29j (fixture h, NEGATIVE — cross-object chain, still `Unknown` but
/// no longer for the OLD reason): `Obj.DoWork().Bar()` — the receiver of
/// `.Bar()` is `Obj.DoWork()`, whose `function` is a MEMBER expression
/// (`Obj.DoWork`), not a bare identifier. Originally this shape was
/// structurally deferred entirely (pre-plan-v2.1); plan v2.1 Task 3 now
/// ENGAGES this exact shape via `infer_compound_member_receiver`'s new
/// cross-object-chain arm — but `DoWork()` here declares NO return type
/// (see `CRHelper.Codeunit.al`), so the arm's non-scalar-return guard
/// declines, same observable `Unknown` outcome via a different, now-real
/// mechanism. See `tests/r0-corpus/ws-cross-object-chain` for the positive/
/// negative matrix that shape actually exercises end to end.
#[test]
fn ws_compound_call_result_cross_object_chain_stays_unknown() {
    let report = ws_compound_call_result_report();
    let route = outer_member_route(&report, 51003, "testcrossobjectchain");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 29k (fixture h, DEFERRED-shape guard NEGATIVE — string-literal-dot
/// arg): `Foo('a.b').Bar()` — proves the AST-based (not `receiver_text`-based)
/// inspection is never confused by a dot embedded in a string-literal
/// argument; `Foo` is not declared anywhere, so this stays `Unknown` regardless.
#[test]
fn ws_compound_call_result_string_literal_dot_arg_stays_unknown() {
    let report = ws_compound_call_result_report();
    let route = outer_member_route(&report, 51003, "teststringliteralarg");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 29l (fixture f, NEGATIVE — cross-app-ambiguous return type):
/// `GetH()`'s return type "CRHelperShared" is a Codeunit declared in BOTH the
/// "CRLibA" and "CRLibB" dependencies — `parsed_type_to_receiver` inherits
/// the fail-closed `ResolveIndex::resolve_object_ref` decline (never guesses
/// either dependency's Codeunit), so `GetH().Bar()` stays honest `Unknown`.
#[test]
fn ws_compound_call_result_cross_app_ambiguous_return_stays_unknown() {
    let report = ws_compound_call_result_report();
    let route = outer_member_route(&report, 51003, "testcrossappambiguous");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(
        matches!(route.evidence, Evidence::Unknown(_)),
        "a cross-app-ambiguous return type (two deps declaring the same \
         Codeunit name) must never be guessed; got {route:?}"
    );
}

// ---------------------------------------------------------------------------
// Tests 30+: beyond-1B.3b Task 4 — `<Framework>.<Prop|Method()>` compound
// receivers (versioned table) + `this.<rest>`, end-to-end over
// `ws-compound-framework`.
//
// Root feature: `infer_receiver_type`'s new Step 6 (`src/program/resolve/
// receiver.rs`, `infer_receiver_type_for_expr` / `infer_compound_member_
// receiver` / `infer_this_member`) types a `<Framework>.<Prop|Method()>`
// receiver by recursing the AST-native base and consulting the versioned
// `framework_return_kind` table (`src/program/resolve/framework_returns.rs`),
// and separately strips a `this.<rest>` prefix by resolving `<rest>` against
// the object-GLOBALS-only self scope. Every letter below matches the task
// brief's fixture list; (d)-(i) are all NEGATIVE/decline proofs. (j) was
// ALSO a negative (deferred) at the time this suite was written — the
// record-field-chains plan's Task 3 landed the deferred mechanism, so (j) is
// now a POSITIVE; see that test's own doc.
// ---------------------------------------------------------------------------

/// Loads `tests/r0-corpus/ws-compound-framework` and returns the full
/// `resolve_full_program` report — shared by Tests 30a-30j below.
fn ws_compound_framework_report() -> ProgramReport {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-compound-framework");
    resolve_full_program(&fixture)
        .expect("resolve_full_program must succeed on ws-compound-framework")
}

/// The route for the OUTERMOST call obligation of a fixture routine —
/// generalizes `outer_member_route` (Task 3) to an ARBITRARY chain depth (1,
/// 2, or 3 nested `Call` nodes, depending on the fixture: a bare `<base>.
/// <member>` property access with no inner call has exactly 1 obligation; a
/// two-hop `<base>.<mid>().<leaf>()` chain has 2; a three-hop
/// `<base>.<a>().<b>().<leaf>()` chain has 3) by always picking the WIDEST
/// span (by `end.col - start.col`, single-line spans in this fixture) — the
/// outermost expression's span always covers every inner one.
fn widest_call_route(
    report: &ProgramReport,
    object_id_number: i64,
    routine_name_lc: &str,
) -> Route {
    let edges = edges_for_object_routine(report, object_id_number, routine_name_lc);
    assert!(
        !edges.is_empty(),
        "{routine_name_lc} (object {object_id_number}) must have at least 1 call obligation"
    );
    let outer = edges
        .iter()
        .max_by_key(|ce| ce.edge.site.span.end.col as i64 - ce.edge.site.span.start.col as i64)
        .expect("edges is non-empty (asserted above)");
    assert_eq!(
        outer.edge.kind,
        EdgeKind::Call,
        "the outer (widest-span) obligation must be the Member call"
    );
    outer.edge.routes[0].clone()
}

/// Test 30a (fixture a, POSITIVE): `Response.Content().ReadAs(Body)` —
/// `Response: HttpResponseMessage` → `Content()` (table-verified) →
/// `HttpContent` — `ReadAs` is a real HttpContent catalog member, so the
/// outer call resolves `Evidence::Catalog`.
#[test]
fn ws_compound_framework_http_response_content_resolves_catalog() {
    let report = ws_compound_framework_report();
    let route = widest_call_route(&report, 51101, "testhttpresponsecontent");
    assert_eq!(route.evidence, Evidence::Catalog);
    let RouteTarget::Builtin(ref bid) = route.target else {
        panic!("expected RouteTarget::Builtin, got {:?}", route.target);
    };
    assert_eq!(bid.0, "HttpContent::readas");
}

/// Test 30b (fixture b, POSITIVE): `JToken.AsObject().Get('key', Found)` —
/// `JToken: JsonToken` → `AsObject()` (table-verified) → `JsonObject` — `Get`
/// is a real JsonObject catalog member, so the outer call resolves
/// `Evidence::Catalog`.
#[test]
fn ws_compound_framework_jsontoken_asobject_resolves_catalog() {
    let report = ws_compound_framework_report();
    let route = widest_call_route(&report, 51101, "testjsontokenasobject");
    assert_eq!(route.evidence, Evidence::Catalog);
    let RouteTarget::Builtin(ref bid) = route.target else {
        panic!("expected RouteTarget::Builtin, got {:?}", route.target);
    };
    assert_eq!(bid.0, "JsonObject::get");
}

/// Test 30c (fixture c, POSITIVE): `this.DialogWindow.Open()` — `this`-strip
/// resolves `DialogWindow` against the object-GLOBALS-only self scope →
/// `Framework(Dialog)` — `Open` is a real Dialog catalog member, so the call
/// resolves `Evidence::Catalog`. Exactly 1 call obligation (no inner call —
/// `this.DialogWindow` has no parens).
#[test]
fn ws_compound_framework_this_strip_dialogwindow_resolves_catalog() {
    let report = ws_compound_framework_report();
    let edges = edges_for_object_routine(&report, 51101, "testthisstripdialogwindow");
    assert_eq!(
        edges.len(),
        1,
        "this.DialogWindow.Open() has exactly 1 call obligation (no inner call)"
    );
    let route = &edges[0].edge.routes[0];
    assert_eq!(route.evidence, Evidence::Catalog);
    let RouteTarget::Builtin(ref bid) = route.target else {
        panic!("expected RouteTarget::Builtin, got {:?}", route.target);
    };
    assert_eq!(bid.0, "Dialog::open");
}

/// Test 30d (fixture d, NEGATIVE — base not a known framework type):
/// `Foo.Content().ReadAs(Body)` — `Foo` is not declared anywhere reachable
/// from this object; the recursive base-typing declines, so the whole chain
/// declines.
#[test]
fn ws_compound_framework_base_not_framework_stays_unknown() {
    let report = ws_compound_framework_report();
    let route = widest_call_route(&report, 51101, "testbasenotframework");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 30e (fixture e, NEGATIVE — table-miss): `Response.Bar().ReadAs(Body)`
/// — `Response` types `Framework(HttpResponseMessage)` but `"Bar"` is not a
/// table entry for that kind — fail-closed.
#[test]
fn ws_compound_framework_table_miss_stays_unknown() {
    let report = ws_compound_framework_report();
    let route = widest_call_route(&report, 51101, "testtablemiss");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 30f REBASELINE (receiver-closure plan v2.1 Task 2 — corrects a WRONG
/// negative test): `Response.Content.ReadAs(Body)` (property form, no
/// parens) resolves EXACTLY like the parens'd form (fixture a) — AL's
/// parens are OPTIONAL on a zero-arg procedure call (the standing
/// al-parens-optional-procedure-calls correction), so `Content` written
/// without parens is the SAME call as `Content()`. Exactly 1 call
/// obligation (`Response.Content` has no parens, so no inner call node —
/// the fallback lookup happens entirely inside `infer_receiver_type`'s
/// receiver-typing of this ONE call, it does not add a second obligation).
/// This test previously asserted `Unknown` under the false premise "AL
/// procedures ALWAYS require parens"; corrected per this project's
/// correctness-over-compatibility working principle.
#[test]
fn ws_compound_framework_parens_less_property_form_resolves_catalog() {
    let report = ws_compound_framework_report();
    let edges =
        edges_for_object_routine(&report, 51101, "testparenslesspropertyformresolvestomethod");
    assert_eq!(
        edges.len(),
        1,
        "Response.Content.ReadAs(Body) has exactly 1 call obligation (Content has no parens)"
    );
    let route = &edges[0].edge.routes[0];
    assert_eq!(route.evidence, Evidence::Catalog);
    let RouteTarget::Builtin(ref bid) = route.target else {
        panic!("expected RouteTarget::Builtin, got {:?}", route.target);
    };
    assert_eq!(bid.0, "HttpContent::readas");
}

/// Test 30g (fixture g, NEGATIVE — wrong arity): `Response.Content(X).ReadAs(Body)`
/// (1 arg) never matches the table's arity-0 entry.
#[test]
fn ws_compound_framework_wrong_arity_stays_unknown() {
    let report = ws_compound_framework_report();
    let route = widest_call_route(&report, 51101, "testwrongarity");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 30h (fixture h, NEGATIVE — recursion mis-type): `Response.Bar().
/// Content().ReadAs(Body)` — `Response.Bar()` is itself a table-miss
/// (declines), so the OUTER `.Content()` hop's base is `Unknown`, not
/// `Framework` — the whole chain declines. 3 nested call obligations
/// (`Bar()`, `Content()`, `ReadAs(...)`).
#[test]
fn ws_compound_framework_recursion_mistype_stays_unknown() {
    let report = ws_compound_framework_report();
    let edges = edges_for_object_routine(&report, 51101, "testrecursionmistype");
    assert_eq!(
        edges.len(),
        3,
        "Response.Bar().Content().ReadAs(Body) has 3 nested call obligations"
    );
    let route = widest_call_route(&report, 51101, "testrecursionmistype");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 30i (fixture i, NEGATIVE — same-named member on a non-framework
/// type): `Cust.Content().ReadAs(Body)` where `Cust: Record "CF Customer"`
/// types `Record{..}`, not `Framework` — the table lookup never engages, even
/// though `"content"` happens to be a valid HttpResponseMessage table member.
#[test]
fn ws_compound_framework_non_framework_base_never_hits_table() {
    let report = ws_compound_framework_report();
    let route = widest_call_route(&report, 51101, "testsamenamedmemberonnonframeworkbase");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 30j (fixture j, POSITIVE post-Task-3 — record-field member-of-member):
/// `Rec.BlobField.CreateOutStream()` now resolves `Evidence::Catalog`.
/// `Rec` types `Record{..}`, so `framework_return_kind` (THIS table) never
/// engages — but the record-field-chains plan's Task 3 landed a SEPARATE
/// mechanism (`ResolveIndex::field_in_table` + the new non-method `Member`
/// arm in `infer_compound_member_receiver`) that types `BlobField` (a real
/// `Blob` field on "CF Customer") as `Framework(Blob)`; `CreateOutStream` is
/// a real Blob catalog member. Exactly 1 call obligation (`Rec.BlobField`
/// has no parens). Pre-Task-3 this stayed `Unknown` (deferred) — see
/// `tests/r0-corpus/ws-record-field-chain/` for the dedicated fixture set
/// this task added.
#[test]
fn ws_compound_framework_record_field_resolves_framework_blob() {
    let report = ws_compound_framework_report();
    let edges = edges_for_object_routine(&report, 51101, "testrecordfieldresolvesframeworkblob");
    assert_eq!(
        edges.len(),
        1,
        "Rec.BlobField.CreateOutStream() has exactly 1 call obligation (BlobField has no parens)"
    );
    let route = &edges[0].edge.routes[0];
    assert_eq!(route.evidence, Evidence::Catalog);
    let RouteTarget::Builtin(ref bid) = route.target else {
        panic!("expected RouteTarget::Builtin, got {:?}", route.target);
    };
    assert_eq!(bid.0, "Blob::createoutstream");
}

// ---------------------------------------------------------------------------
// Tests 30k+: Task 4 (chain-tables plan) — Xml framework chains
// (`framework_returns.rs`) + the NEW RecordRef/FieldRef/KeyRef typed-return
// table (`recordref_returns.rs`), end-to-end over `ws-chain-tables`.
//
// Root feature: the SAME `infer_receiver_type_for_expr` / `infer_compound_
// member_receiver` funnel (`src/program/resolve/receiver.rs`) that Task 4
// (beyond-1B.3b) built for `<Framework>.<Prop|Method()>` receivers now also
// carries (a) Xml entries in `framework_return_kind` and (b) a NEW, distinct
// `recordref_family_return_kind` table for the `RecordRef`/`FieldRef`/
// `KeyRef` unit-variant family. See `PROOF.md` for the real-CDO-source
// grounding of every positive fixture and the HTTPCONTENT investigation
// finding (fixture n8).
// ---------------------------------------------------------------------------

/// Loads `tests/r0-corpus/ws-chain-tables` and returns the full
/// `resolve_full_program` report — shared by the tests below.
fn ws_chain_tables_report() -> ProgramReport {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-chain-tables");
    resolve_full_program(&fixture).expect("resolve_full_program must succeed on ws-chain-tables")
}

/// Fixture (a1, POSITIVE): `XmlElement.Create('root').AsXmlNode()` — arity-1
/// `Create` chain-types to `Xml`; `AsXmlNode` is a real XML catalog LEAF
/// member, so the outer call resolves `Evidence::Catalog`.
#[test]
fn ws_chain_tables_xml_create_arity1_as_xml_node_resolves_catalog() {
    let report = ws_chain_tables_report();
    let route = widest_call_route(&report, 51201, "testxmlelementcreatearity1asxmlnode");
    assert_eq!(route.evidence, Evidence::Catalog);
    let RouteTarget::Builtin(ref bid) = route.target else {
        panic!("expected RouteTarget::Builtin, got {:?}", route.target);
    };
    assert_eq!(bid.0, "Xml::asxmlnode");
}

/// Fixture (a2, POSITIVE): `XmlElement.Create('root', '', 'InnerText').
/// AsXmlNode()` — arity-3 `Create` (the REAL CDO arity) chain-types to `Xml`
/// exactly like arity-1.
#[test]
fn ws_chain_tables_xml_create_arity3_as_xml_node_resolves_catalog() {
    let report = ws_chain_tables_report();
    let route = widest_call_route(&report, 51201, "testxmlelementcreatearity3asxmlnode");
    assert_eq!(route.evidence, Evidence::Catalog);
    let RouteTarget::Builtin(ref bid) = route.target else {
        panic!("expected RouteTarget::Builtin, got {:?}", route.target);
    };
    assert_eq!(bid.0, "Xml::asxmlnode");
}

/// Fixture (a3, POSITIVE): `Node.AsXmlElement().GetChildNodes()` —
/// `AsXmlElement()` chain-types to `Xml`; `GetChildNodes` is a real XML
/// catalog LEAF member.
#[test]
fn ws_chain_tables_xml_as_xml_element_get_child_nodes_resolves_catalog() {
    let report = ws_chain_tables_report();
    let route = widest_call_route(&report, 51201, "testxmlnodeasxmlelementgetchildnodes");
    assert_eq!(route.evidence, Evidence::Catalog);
    let RouteTarget::Builtin(ref bid) = route.target else {
        panic!("expected RouteTarget::Builtin, got {:?}", route.target);
    };
    assert_eq!(bid.0, "Xml::getchildnodes");
}

/// Fixture (a4, POSITIVE): `Child.AsXmlText().Value()` — `AsXmlText()`
/// chain-types to `Xml`; `Value` is a real XML catalog LEAF member.
#[test]
fn ws_chain_tables_xml_as_xml_text_value_resolves_catalog() {
    let report = ws_chain_tables_report();
    let route = widest_call_route(&report, 51201, "testxmlnodeasxmltextvalue");
    assert_eq!(route.evidence, Evidence::Catalog);
    let RouteTarget::Builtin(ref bid) = route.target else {
        panic!("expected RouteTarget::Builtin, got {:?}", route.target);
    };
    assert_eq!(bid.0, "Xml::value");
}

/// Fixture (b, POSITIVE): `RecRef.KeyIndex(1).FieldIndex(1).Value()` —
/// `KeyIndex(1)` chain-types `RecordRef`->`KeyRef`, `FieldIndex(1)`
/// chain-types `KeyRef`->`FieldRef`, `Value` is a real FieldRef catalog LEAF
/// member.
#[test]
fn ws_chain_tables_recordref_keyindex_fieldindex_value_resolves_catalog() {
    let report = ws_chain_tables_report();
    let route = widest_call_route(&report, 51201, "testrecordrefkeyindexfieldindexvalue");
    assert_eq!(route.evidence, Evidence::Catalog);
    let RouteTarget::Builtin(ref bid) = route.target else {
        panic!("expected RouteTarget::Builtin, got {:?}", route.target);
    };
    assert_eq!(bid.0, "FieldRef::value");
}

/// Fixture (c, POSITIVE): `RecRef.Field(1).Caption()` — `Field(1)`
/// chain-types `RecordRef`->`FieldRef`; `Caption` is a real FieldRef catalog
/// LEAF member. Covers the table's `Field` row independently of fixture (b)
/// (which exercises `FieldIndex`/`KeyIndex`).
#[test]
fn ws_chain_tables_recordref_field_caption_resolves_catalog() {
    let report = ws_chain_tables_report();
    let route = widest_call_route(&report, 51201, "testrecordreffieldcaption");
    assert_eq!(route.evidence, Evidence::Catalog);
    let RouteTarget::Builtin(ref bid) = route.target else {
        panic!("expected RouteTarget::Builtin, got {:?}", route.target);
    };
    assert_eq!(bid.0, "FieldRef::caption");
}

/// Fixture (n1, NEGATIVE — un-tabled Xml member): `Node.Attributes().
/// Count()` — `Attributes` is a real XML catalog LEAF member but
/// deliberately not chain-tabled; the outer `Count()` call's receiver stays
/// `Unknown`.
#[test]
fn ws_chain_tables_xml_untabled_member_chain_stays_unknown() {
    let report = ws_chain_tables_report();
    let route = widest_call_route(&report, 51201, "testxmluntabledmemberchain");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Fixture (n2) REBASELINE (receiver-closure plan v2.1 Task 2 — corrects a
/// WRONG negative test): `Node.AsXmlElement.GetChildNodes()` (`AsXmlElement`
/// with no parens) resolves EXACTLY like the parens'd form (fixture a3) — AL's
/// parens are OPTIONAL on a zero-arg procedure call. Exactly 1 call
/// obligation (`AsXmlElement` has no parens, so no inner call node). This
/// test previously asserted `Unknown` under the false premise "AL procedures
/// ALWAYS require parens"; corrected per this project's
/// correctness-over-compatibility working principle.
#[test]
fn ws_chain_tables_xml_parens_less_property_form_resolves_catalog() {
    let report = ws_chain_tables_report();
    let edges = edges_for_object_routine(
        &report,
        51201,
        "testxmlparenslesspropertyformresolvestomethod",
    );
    assert_eq!(
        edges.len(),
        1,
        "Node.AsXmlElement.GetChildNodes() has exactly 1 call obligation (AsXmlElement has no parens)"
    );
    let route = &edges[0].edge.routes[0];
    assert_eq!(route.evidence, Evidence::Catalog);
    let RouteTarget::Builtin(ref bid) = route.target else {
        panic!("expected RouteTarget::Builtin, got {:?}", route.target);
    };
    assert_eq!(bid.0, "Xml::getchildnodes");
}

/// Fixture (n3, NEGATIVE — wrong arity): `XmlElement.Create().AsXmlNode()`
/// (0 args) never matches — no documented overload takes zero arguments.
#[test]
fn ws_chain_tables_xml_wrong_arity_create_stays_unknown() {
    let report = ws_chain_tables_report();
    let route = widest_call_route(&report, 51201, "testxmlwrongaritycreate");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Fixture (n4, NEGATIVE — wrong arity, RecordRef family): `RecRef.
/// KeyIndex(1, 2).FieldCount()` (2 args) never matches the table's arity-1
/// entry.
#[test]
fn ws_chain_tables_recordref_family_wrong_arity_stays_unknown() {
    let report = ws_chain_tables_report();
    let route = widest_call_route(&report, 51201, "testrecordreffamilywrongarity");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Fixture (n5, NEGATIVE — same-named member on a non-RecordRef-family
/// receiver): `Rec.FieldIndex(1).Value()` where `Rec: Record "CT Item"`
/// types `Record{..}`, not `RecordRef`/`FieldRef`/`KeyRef` — the
/// recordref-family table lookup never engages, even though `"fieldindex"`
/// happens to be a valid RecordRef/KeyRef table member name.
#[test]
fn ws_chain_tables_record_fieldindex_not_recordref_family_stays_unknown() {
    let report = ws_chain_tables_report();
    let route = widest_call_route(&report, 51201, "testrecordfieldindexnotrecordreffamily");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Fixture (n6, NEGATIVE — `FieldRef.Value` chain-decline, round-1 I4):
/// `SourceRecRef.Field(1).Value().SomeMethod()` — `Value` is variant-like
/// LEAF data, never a chainable receiver; the outer `.SomeMethod()` call's
/// receiver stays `Unknown`. 3 nested call obligations (`Field(1)`,
/// `Value()`, `SomeMethod()`).
#[test]
fn ws_chain_tables_fieldref_value_chain_decline_stays_unknown() {
    let report = ws_chain_tables_report();
    let edges = edges_for_object_routine(&report, 51201, "testfieldrefvaluechaindecline");
    assert_eq!(
        edges.len(),
        3,
        "SourceRecRef.Field(1).Value().SomeMethod() has 3 nested call obligations"
    );
    let route = widest_call_route(&report, 51201, "testfieldrefvaluechaindecline");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Fixture (n7, NEGATIVE — unvalidated/omitted entry stays declined):
/// `FRef.Record().Number()` — `FieldRef.Record()` is a real,
/// MS-Learn-documented method (returns `RecordRef`) but deliberately out of
/// this task's reviewed scope — must stay `Unknown`.
#[test]
fn ws_chain_tables_fieldref_record_unvalidated_stays_unknown() {
    let report = ws_chain_tables_report();
    let route = widest_call_route(&report, 51201, "testfieldrefrecordunvalidateddecline");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Fixture (n8, NEGATIVE — HTTPCONTENT investigation finding, see
/// `PROOF.md`): `Content.AsText()` on a genuinely `HttpContent`-typed
/// receiver stays `Unknown` — `AsText` is NOT a real `HttpContent` member
/// (verified against methods-auto/httpcontent AND `member_builtins.json`);
/// the catalog is already complete and correct, so this regression-pins that
/// it is NOT extended with a fabricated entry. Exactly 1 call obligation
/// (`Content` is a plain declared variable, not a chain).
#[test]
fn ws_chain_tables_httpcontent_astext_stays_unknown() {
    let report = ws_chain_tables_report();
    let edges = edges_for_object_routine(&report, 51201, "testhttpcontentastextstaysunknown");
    assert_eq!(
        edges.len(),
        1,
        "Content.AsText() has exactly 1 call obligation (Content is a plain variable)"
    );
    let route = &edges[0].edge.routes[0];
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

// ---------------------------------------------------------------------------
// Tests 31+: Task 1 — protected-ABI soundness. `tests/r0-corpus/
// ws-protected-abi` (a real SymbolOnly probe `.app`, "ProtAbiDep": Page 60000
// "Dep Page" [protected P/public Pub/internal I/local L], Codeunit 60001
// "Dep Arity" [protected GetWorker() + public GetWorker(ID)/Get(ID)],
// Codeunit 60002 "Dep IfaceImpl" implements IProtWorker [protected DoIt]) end
// to end through the REAL `SymbolReference.json` → `AbiRoutine` →
// `abi_ingest` → `resolve_in_object` pipeline — proving the fix at the
// ingestion+selection boundary, not just the fabricated-graph unit tests
// already covering `resolve_in_object`'s internals.
// ---------------------------------------------------------------------------

/// Loads `tests/r0-corpus/ws-protected-abi` and returns the full
/// `resolve_full_program` report — shared by Tests 31a-31i below.
fn ws_protected_abi_report() -> ProgramReport {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-protected-abi");
    resolve_full_program(&fixture).expect("resolve_full_program must succeed on ws-protected-abi")
}

/// Test 31a (fixture a, NEGATIVE — the false-route this task closes): `Dep
/// Page.P()` is `protected` in the real ABI `SymbolReference.json`
/// (`"IsProtected":true`); `ProtCaller` is NOT an extension of "Dep Page", so
/// this must decline honest `Unknown(ProtectedNotVisible)`. Before Task 1,
/// `resolve_in_object`'s SymbolOnly branch took `candidates.first()` with NO
/// visibility check, so this call FALSE-resolved to an `Opaque`/`AbiSymbol`
/// route — this is the exact false-`Source`-adjacent vector Task 1 closes.
#[test]
fn ws_protected_abi_object_receiver_protected_excluded() {
    let report = ws_protected_abi_report();
    let route = &edges_for_object_routine(&report, 51000, "testprotectedexcluded")[0]
        .edge
        .routes[0];
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert_eq!(
        route.evidence,
        Evidence::Unknown(UnknownReason::ProtectedNotVisible),
        "a non-extending caller must never see a protected ABI member; got {route:?}"
    );
    assert_eq!(route.witness, Witness::None);
}

/// Test 31b (fixture b, CONTROL): `Dep Page.Pub()` carries no ABI access
/// modifier (`Access::Public`) — must still resolve as an `Opaque` ABI
/// boundary route, exactly as before Task 1. Proves the fix does not
/// over-decline a genuinely-visible ABI member.
#[test]
fn ws_protected_abi_object_receiver_public_control_resolves() {
    let report = ws_protected_abi_report();
    let route = &edges_for_object_routine(&report, 51000, "testpubliccontrol")[0]
        .edge
        .routes[0];
    assert_eq!(route.evidence, Evidence::Opaque);
    let RouteTarget::AbiSymbol { ref key } = route.target else {
        panic!("expected RouteTarget::AbiSymbol, got {:?}", route.target);
    };
    assert_eq!(key.routine_name_lc, "pub");
    assert!(matches!(route.witness, Witness::AbiSymbol { .. }));
}

/// Test 31c (fixture c, POSITIVE — carry-Protected, not drop): a GENUINE
/// workspace `PageExtension` of "Dep Page" (`DepPageExtOk`) calling `P()`
/// bare (extension-base fallback) MUST resolve — AL lets an extension call
/// its base's `protected` members. Proves `Access::Protected` is CARRIED
/// (not dropped like `local`/`internal`) and that `object_extends`'s
/// self-or-extends rule is tier-agnostic against a real SymbolOnly base.
#[test]
fn ws_protected_abi_genuine_extension_sees_protected() {
    let report = ws_protected_abi_report();
    let route = &edges_for_object_routine(&report, 51001, "callprotected")[0]
        .edge
        .routes[0];
    assert_eq!(route.evidence, Evidence::Opaque);
    let RouteTarget::AbiSymbol { ref key } = route.target else {
        panic!("expected RouteTarget::AbiSymbol, got {:?}", route.target);
    };
    assert_eq!(key.routine_name_lc, "p");
    assert!(matches!(route.witness, Witness::AbiSymbol { .. }));
}

/// Test 31d (fixture d, CONTROL — Tier-1 remediation H-1 REBASELINE):
/// `internal`/`local` ABI routines used to be DROPPED entirely at ingestion,
/// so a caller outside the declaring object/app saw a bare
/// `Unknown(MemberNotFound)` — indistinguishable from a name that never
/// existed at all. H-1 (Task T1.2) fixed the root cause: AL's `local` on an
/// event PUBLISHER restricts raising, not subscribing, and dropping
/// `internal` made the InternalsVisibleTo friend map inert for SymbolOnly
/// deps (see `abi_ingest::ingest_abi`'s doc). Both routines now SURVIVE
/// ingestion carrying their real `Access` variant, and the SAME visibility
/// check `ws_protected_abi_object_receiver_protected_excluded` already
/// proves for `Access::Protected` now correctly declines them too — with
/// the SPECIFIC reason (`InternalNotVisible`/`LocalNotVisible`), not the
/// generic "name never existed" `MemberNotFound`. This is a genuine,
/// intended behavior improvement (a more precise decline reason), not a
/// regression — see CHANGELOG's H-1 entry.
#[test]
fn ws_protected_abi_internal_and_local_still_declines_but_with_precise_reason() {
    let report = ws_protected_abi_report();

    let internal_route = &edges_for_object_routine(&report, 51000, "testinternalabsentcontrol")[0]
        .edge
        .routes[0];
    assert_eq!(internal_route.target, RouteTarget::Unresolved);
    assert_eq!(
        internal_route.evidence,
        Evidence::Unknown(UnknownReason::InternalNotVisible),
        "IsInternal routines now survive ingestion (H-1) and correctly \
         decline via the SPECIFIC InternalNotVisible reason, not a bare \
         MemberNotFound; got {internal_route:?}"
    );

    let local_route = &edges_for_object_routine(&report, 51000, "testlocalabsentcontrol")[0]
        .edge
        .routes[0];
    assert_eq!(local_route.target, RouteTarget::Unresolved);
    assert_eq!(
        local_route.evidence,
        Evidence::Unknown(UnknownReason::LocalNotVisible),
        "IsLocal routines now survive ingestion (H-1) and correctly decline \
         via the SPECIFIC LocalNotVisible reason, not a bare MemberNotFound; \
         got {local_route:?}"
    );
}

/// Test 31e (fixture e): `IProtWorker` has TWO implementers — the dep's
/// SymbolOnly `Dep IfaceImpl` (`protected DoIt`) and the workspace's
/// `IfaceImplWs` (`public DoIt`). The polymorphic fan-out must apply
/// PER-CANDIDATE visibility independently: the dep route declines
/// (`ProtectedNotVisible`), the workspace route resolves `Source`. Neither a
/// visible sibling nor an excluded sibling may influence the other's route.
#[test]
fn ws_protected_abi_interface_fanout_respects_visibility() {
    let report = ws_protected_abi_report();
    let edges = edges_for_object_routine(&report, 51004, "testinterfacefanout");
    assert_eq!(edges.len(), 1, "Worker.DoIt() is a single call obligation");
    let edge = &edges[0].edge;
    assert_eq!(edge.shape, DispatchShape::Polymorphic);
    assert_eq!(
        edge.routes.len(),
        2,
        "IProtWorker has exactly 2 implementers; got {:?}",
        edge.routes
    );

    let abi_route = edge
        .routes
        .iter()
        .find(|r| r.evidence != Evidence::Source)
        .expect("the dep's SymbolOnly implementer must still emit a route (never dropped)");
    assert_eq!(
        abi_route.evidence,
        Evidence::Unknown(UnknownReason::ProtectedNotVisible),
        "the SymbolOnly implementer's protected DoIt must decline from a \
         non-extending caller; got {abi_route:?}"
    );
    assert_eq!(abi_route.target, RouteTarget::Unresolved);

    let source_route = edge
        .routes
        .iter()
        .find(|r| r.evidence == Evidence::Source)
        .expect("the workspace implementer's public DoIt must resolve");
    let RouteTarget::Routine(ref rid) = source_route.target else {
        panic!(
            "expected RouteTarget::Routine, got {:?}",
            source_route.target
        );
    };
    assert!(
        rid.object.id_equals_number(51003),
        "must resolve to \"IfaceImplWs\" (id 51003); got {:?}",
        rid.object
    );
}

/// Test 31f (fixture f, NEGATIVE — the mixed-arity/mixed-access vector this
/// task closes): `GetWorker` is overloaded in the dep ABI — arity-0
/// `protected` and arity-1 `public`. An arity-0 call must decline honest
/// `Unknown`, NEVER silently select the visible arity-1 sibling by
/// `candidates.first()` order (an order/visibility-dependent pick is exactly
/// the false-`Source`-adjacent vector this task closes).
#[test]
fn ws_protected_abi_mixed_arity_protected_arm_excluded() {
    let report = ws_protected_abi_report();
    let route = &edges_for_object_routine(&report, 51000, "testmixedarityprotectedarm")[0]
        .edge
        .routes[0];
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(
        matches!(route.evidence, Evidence::Unknown(_)),
        "the arity-0 protected GetWorker() must never resolve via the \
         visible arity-1 sibling; got {route:?}"
    );
}

/// Test 31f control (fixture f POSITIVE half): `GetWorker(1)` — the arity-1
/// `public` overload of the SAME name — must resolve normally, proving the
/// arity-0 decline above is a genuine arity+access selection, not a blanket
/// name-level exclusion.
#[test]
fn ws_protected_abi_mixed_arity_public_arm_resolves() {
    let report = ws_protected_abi_report();
    let route = &edges_for_object_routine(&report, 51000, "testmixedaritypublicarm")[0]
        .edge
        .routes[0];
    assert_eq!(route.evidence, Evidence::Opaque);
    let RouteTarget::AbiSymbol { ref key } = route.target else {
        panic!("expected RouteTarget::AbiSymbol, got {:?}", route.target);
    };
    assert_eq!(key.routine_name_lc, "getworker");
    assert_eq!(
        key.params_count, 1,
        "must select the arity-1 overload, not the arity-0 protected one"
    );
}

/// Test 31g/31i (fixtures g + i, NEGATIVE — the name-only-scan-vs-emission
/// vector this task closes): `Get(ID: Integer)` is the ONLY declared overload
/// of `Get` in the dep ABI (public, arity 1). `DepArity.Get()` (arity 0) must
/// NOT emit a `Catalog`/resolved edge — the existence boolean
/// (`object_has_visible_member_candidate`, name-only for SymbolOnly) may be
/// `true`, but that is diagnostics-only, never edge evidence; exactly-one-
/// same-name is insufficient at the wrong arity.
#[test]
fn ws_protected_abi_wrong_arity_single_overload_no_emit() {
    let report = ws_protected_abi_report();
    let route = &edges_for_object_routine(&report, 51000, "testwrongaritypubliconly")[0]
        .edge
        .routes[0];
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(
        matches!(route.evidence, Evidence::Unknown(_)),
        "a SINGLE visible public Get(ID) called at arity 0 must NOT emit \
         (name-only existence must never justify an edge); got {route:?}"
    );
}

/// Test 31h (fixture h, NEGATIVE — stranger-extension identity): the
/// workspace `TableExtension "DepPageExtStranger" extends "Dep Page"` resolves
/// its base among TABLE-kind objects only (kind-scoped lookup) — landing on
/// the WORKSPACE `StrangerTable` (Table 60000 "Dep Page", zero procedures),
/// NEVER the ABI's Page 60000 "Dep Page" (same id AND name, different
/// `ObjectNodeId.kind`). `P` is genuinely absent on the workspace stranger
/// table, so this must stay `Unknown` — never resolving to the ABI base's
/// protected `P()` via an id/name identity collision.
#[test]
fn ws_protected_abi_stranger_extension_never_sees_base() {
    let report = ws_protected_abi_report();
    let route = &edges_for_object_routine(&report, 51002, "callprotectedstranger")[0]
        .edge
        .routes[0];
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(
        matches!(route.evidence, Evidence::Unknown(_)),
        "a same-id/name but WRONG-KIND stranger extension must never see \
         the ABI base's protected P(); got {route:?}"
    );
}

// ---------------------------------------------------------------------------
// Tests 32+: plan v2.1 Task 3 — cross-object call-result chain resolution
// (`Var.Method().X()`) via a PURE `resolve_member` type-query, fail-closed.
// `tests/r0-corpus/ws-cross-object-chain` (a real SOURCE object graph +
// TWO real SymbolOnly probe `.app`s — "CrossChainDep" carrying a
// `GetContent(): Codeunit "Dep Http Content"` nested-`Subtype` ABI return,
// and "CrossChainDep2" declaring a same-named "Dep Shared" codeunit for the
// cross-app-ambiguous-return negative) end to end through the REAL
// `infer_compound_member_receiver`'s new arm
// (`src/program/resolve/receiver.rs`).
//
// Root feature: when the compound receiver's function is
// `ExprKind::Member{base, member}` (strictly the procedure-CALL form) and
// `base` types to `Object`/`Record`/`SelfObject`/`Interface`, the base call's
// return type is typed via a PURE `resolve_member(base_ty, member_lc, arity,
// ..)` type-query: EXACTLY ONE route required; `RouteTarget::Routine`/
// `AbiSymbol` read the resolved routine's declared `return_type` (Task 2's
// Name+Id cross-validation applied for every ABI-sourced return); anything
// else declines. Every letter below matches the task brief's fixture list.
// ---------------------------------------------------------------------------

/// Loads `tests/r0-corpus/ws-cross-object-chain` and returns the full
/// `resolve_full_program` report — shared by all tests below.
fn ws_cross_object_chain_report() -> ProgramReport {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-cross-object-chain");
    resolve_full_program(&fixture)
        .expect("resolve_full_program must succeed on ws-cross-object-chain")
}

/// The route for the OUTERMOST `Call`-kind obligation of a chain fixture
/// routine — the WIDEST-span `Call` edge (source spans nest for a
/// single-line chain expression, so the outermost call always covers the
/// most columns). Unlike `outer_member_route` (the sibling
/// `ws-compound-call-result` helper), this does NOT assert a fixed edge
/// count: a chain may have 2 obligations (one inner call + the outer member
/// call) or 3 (a 3-level chain) — `ws-cross-object-chain`'s own fixtures
/// exercise both shapes.
fn outer_chain_route(
    report: &ProgramReport,
    object_id_number: i64,
    routine_name_lc: &str,
) -> Route {
    let edges = edges_for_object_routine(report, object_id_number, routine_name_lc);
    assert!(
        !edges.is_empty(),
        "{routine_name_lc} (object {object_id_number}) must have at least one call obligation"
    );
    let outer = edges
        .iter()
        .filter(|ce| ce.edge.kind == EdgeKind::Call)
        .max_by_key(|ce| ce.edge.site.span.end.col as i64 - ce.edge.site.span.start.col as i64)
        .expect("at least one Call-kind edge");
    outer.edge.routes[0].clone()
}

/// Test 32a (fixture a, POSITIVE): SOURCE prefix. `Helper.GetCustomer(No)`
/// (unique arity-1, `Record "CC Customer"` return) types the chain receiver
/// `Record{table: Some(CCCustomer)}`; `Name` is a non-builtin Customer
/// procedure — must resolve `Source`, exact target id.
#[test]
fn ws_cross_object_chain_source_prefix_resolves_to_source() {
    let report = ws_cross_object_chain_report();
    let route = outer_chain_route(&report, 51206, "testsourceprefix");
    assert_eq!(route.evidence, Evidence::Source);
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected RouteTarget::Routine, got {:?}", route.target);
    };
    assert_eq!(rid.name_lc, "name");
    assert_eq!(rid.object.kind, ObjectKind::Table);
    assert!(
        rid.object.id_equals_number(51200),
        "must resolve to \"CC Customer\" (id 51200); got {:?}",
        rid.object
    );
    assert!(matches!(route.witness, Witness::SourceSpan { .. }));
}

/// Test 32b (fixture b, POSITIVE): ABI prefix carrying a nested `Subtype`.
/// `Response.GetContent()`'s declared return (reconstructed from the ABI
/// `ReturnTypeDefinition.Subtype`, Task 2) types the chain receiver
/// `Object{Codeunit, "dep http content"}`; `ReadAs` is a PUBLIC ABI member on
/// that object — must resolve `Opaque`/`AbiSymbol`.
#[test]
fn ws_cross_object_chain_abi_prefix_with_subtype_resolves() {
    let report = ws_cross_object_chain_report();
    let route = outer_chain_route(&report, 51206, "testabiprefix");
    assert_eq!(route.evidence, Evidence::Opaque);
    let RouteTarget::AbiSymbol { ref key } = route.target else {
        panic!("expected RouteTarget::AbiSymbol, got {:?}", route.target);
    };
    assert_eq!(key.routine_name_lc, "readas");
    assert_eq!(
        key.object_number, 60101,
        "must dispatch on \"Dep Http Content\" (id 60101)"
    );
    assert!(matches!(route.witness, Witness::AbiSymbol { .. }));
}

/// Test 32c (fixture c, NEGATIVE — leaf visibility): `Response.GetContent()`
/// types the chain exactly like (b), but the leaf `Secret` is an ABI
/// `internal` member — never visible to this non-friend caller app (dropped
/// entirely at ABI ingestion, see `abi_ingest::ingest_abi`). Proves the new
/// chain-typing arm does not bypass Phase B's ordinary visibility discipline
/// at the leaf — it only types the RECEIVER, never the member itself.
#[test]
fn ws_cross_object_chain_abi_leaf_internal_not_visible() {
    let report = ws_cross_object_chain_report();
    let route = outer_chain_route(&report, 51206, "testabileafinternalnotvisible");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 32d (fixture d, POSITIVE — single-implementer interface prefix
/// SUCCESS control): `ICCFoo` has EXACTLY ONE implementer (`CC Foo Impl`) in
/// the closure — `resolve_member`'s Interface fan-out yields exactly 1
/// route, the route-count guard accepts, and the chain types
/// `Object{Codeunit, "cc helper"}` (AL guarantees the implementer's
/// signature matches the interface's); `DoWork` must resolve `Source`.
#[test]
fn ws_cross_object_chain_single_impl_interface_resolves() {
    let report = ws_cross_object_chain_report();
    let route = outer_chain_route(&report, 51206, "testinterfacesingleimpl");
    assert_eq!(route.evidence, Evidence::Source);
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected RouteTarget::Routine, got {:?}", route.target);
    };
    assert_eq!(rid.name_lc, "dowork");
    assert_eq!(rid.object.kind, ObjectKind::Codeunit);
    assert!(
        rid.object.id_equals_number(51201),
        "must resolve to \"CC Helper\" (id 51201); got {:?}",
        rid.object
    );
    assert!(matches!(route.witness, Witness::SourceSpan { .. }));
}

/// Test 32e (fixture N1, NEGATIVE — polymorphic prefix, conservative
/// decline): `ICCBar` has TWO implementers — `resolve_member`'s Interface
/// fan-out yields 2 routes; the route-count guard must decline rather than
/// guess either implementer's return type.
#[test]
fn ws_cross_object_chain_polymorphic_interface_declines() {
    let report = ws_cross_object_chain_report();
    let route = outer_chain_route(&report, 51206, "testinterfacepolymorphicdeclines");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 32f (fixture N2a, NEGATIVE — builtin-only prefix): `Rec.Next()`
/// resolves via the platform Record catalog (`RouteTarget::Builtin`), which
/// carries no modeled return type to chain onto.
#[test]
fn ws_cross_object_chain_builtin_prefix_declines() {
    let report = ws_cross_object_chain_report();
    let route = outer_chain_route(&report, 51206, "testbuiltinprefixdeclines");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 32g (fixture N2b, NEGATIVE — wrong-arity SOURCE prefix):
/// `GetCustomer` is declared ONLY at arity 1; called here with arity 0 —
/// `resolve_member`'s Object arm returns a single `Unresolved
/// (OverloadAmbiguous)` route, which the new arm declines rather than trust.
#[test]
fn ws_cross_object_chain_wrong_arity_source_prefix_declines() {
    let report = ws_cross_object_chain_report();
    let route = outer_chain_route(&report, 51206, "testwrongaritysourceprefixdeclines");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 32h (fixture N3, NEGATIVE — ABI same-name overloads, DIFFERENT
/// returns): `Dep Overload` declares two `Get` overloads at the SAME arity
/// (1), differing only in the parameter's OUTER kind (`Codeunit`/`Page`) —
/// ABI parameter types are degraded (no `Subtype` carried on parameters),
/// but the two overloads still remain two DISTINCT arity-1 candidates here
/// (their outer kind differs) — `resolve_member`'s own arity+visibility
/// selection sees 2 candidates and returns `Unresolved(OverloadAmbiguous)`;
/// the new arm's route-target check declines rather than guess either
/// overload's return type.
#[test]
fn ws_cross_object_chain_abi_overload_ambiguous_declines() {
    let report = ws_cross_object_chain_report();
    let route = outer_chain_route(&report, 51206, "testabioverloadambiguousdeclines");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 32i (fixture N4a, NEGATIVE — scalar return): `GetCount(): Integer`
/// has nothing to dispatch a member call on.
#[test]
fn ws_cross_object_chain_scalar_return_declines() {
    let report = ws_cross_object_chain_report();
    let route = outer_chain_route(&report, 51206, "testscalarreturndeclines");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 32j (fixture N4b, NEGATIVE — no declared return type at all):
/// `DoNothing()` declares no return type.
#[test]
fn ws_cross_object_chain_no_return_type_declines() {
    let report = ws_cross_object_chain_report();
    let route = outer_chain_route(&report, 51206, "testnoreturntypedeclines");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 32k (fixture N5, NEGATIVE — cross-app-ambiguous return):
/// `GetShared()`'s declared return `Codeunit "Dep Shared"` names an object
/// declared IDENTICALLY in BOTH `CrossChainDep` and `CrossChainDep2` —
/// genuinely ambiguous in this workspace's dependency closure;
/// `parsed_type_to_receiver` (and, at the leaf, `resolve_member`'s own
/// `graph.resolve_object` re-lookup) both decline rather than guess either
/// dependency's codeunit.
#[test]
fn ws_cross_object_chain_cross_app_ambiguous_return_declines() {
    let report = ws_cross_object_chain_report();
    let route = outer_chain_route(&report, 51206, "testcrossappambiguousreturndeclines");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 32l (fixture N6, NEGATIVE — Name+Id cross-validation mismatch, Task
/// 2): `GetMismatch()`'s declared `Subtype` names "Dep Http Content" but
/// carries the WRONG `Id` (99999, not that object's real id 60101) — the
/// resolved object's `declared_id` disagrees with the Subtype's `Id`, so the
/// whole receiver typing declines rather than trust a name-only match.
#[test]
fn ws_cross_object_chain_name_id_mismatch_declines() {
    let report = ws_cross_object_chain_report();
    let route = outer_chain_route(&report, 51206, "testnameidmismatchdeclines");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 32m (fixture N7/N9, NEGATIVE — cross-object-chain arm correctly
/// never engages): `Rec."No."` (property/field-access form, NO parens) is
/// never the CROSS-OBJECT-CHAIN arm — that arm is STRICTLY the
/// procedure-CALL form (round-1 I7). Post record-field-chains-plan Task 3,
/// `Rec."No."` DOES now type via the NEW record-field arm — "No." is a real
/// `Code[20]` field on "CC Customer", so it resolves `Framework(Text)` (Code
/// classifies as Text, `classify_type_text`) — but `.Name()` still stays
/// honestly `Unknown`, because `"name"` is not a real `member_catalog::TEXT`
/// member (`CatalogMiss`, not "arm doesn't exist"). Same observable route,
/// different — now more precise — reason.
#[test]
fn ws_cross_object_chain_field_property_chain_declines() {
    let report = ws_cross_object_chain_report();
    let route = outer_chain_route(&report, 51206, "testfieldpropertychaindeclines");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 32n (fixture N8, NEGATIVE — 3-level chain, middle hop fails to
/// type): hop 1 (`Helper.GetCustomer(No)`) types fine (`Record{CCCustomer}`);
/// hop 2 (`<hop1>.NoSuchMethod()`) has no such member on "CC Customer"
/// (source or catalog) — declines to `Unknown`; the OUTER `.Name()` call's
/// receiver is therefore `Unknown` too — no partial guessing propagates
/// through a failed middle hop.
#[test]
fn ws_cross_object_chain_three_level_middle_hop_fails_declines() {
    let report = ws_cross_object_chain_report();
    let route = outer_chain_route(&report, 51206, "testthreelevelmiddlehopfailsdeclines");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 32o (fixture N10, NEGATIVE — wrong-arity ABI prefix): `Dep Arity
/// Chain` declares `Get(ID: Integer): Codeunit "Dep Http Content"` — ONE
/// candidate, but ONLY at arity 1; called here with arity 0 — a single
/// visible same-name ABI candidate at the WRONG arity must not emit.
#[test]
fn ws_cross_object_chain_wrong_arity_abi_declines() {
    let report = ws_cross_object_chain_report();
    let route = outer_chain_route(&report, 51206, "testwrongarityabideclines");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 32p (fixture N11 — Task 2 SUPERSEDES Task 3's review-fix framing):
/// `Dep Collapse` declares two `Get` overloads at the SAME arity (1) AND the
/// SAME outer parameter kind (`Codeunit`), differing ONLY in the parameter's
/// Subtype (`Dep A` Id 60130 vs `Dep C` Id 60140 — a real probe-`.app`
/// SymbolReference.json entry, both quote-free Name+Id present). PRE-Task-2,
/// `AbiParameter::type_text` fingerprinted only the outer keyword, never a
/// Subtype, so both overloads hashed to the IDENTICAL `RoutineNodeId` and
/// collapsed to ONE arbitrary survivor at ABI ingestion
/// (`RoutineNode::abi_overload_collapsed`) — the chain declined via the
/// ABI-PREFIX UNIQUENESS GUARD (`resolve_abi_prefix_routine`/
/// `routine_node_for_type_query`) rather than type off an arbitrary
/// sibling's return.
///
/// POST-Task-2: `abi_ingest::param_type_fp` now reconstructs each param's
/// FULL source-shaped text (`Codeunit "Dep A"` / `Codeunit "Dep C"`), so the
/// two overloads' `sig_fp`s DIFFER — they never collapse at all and survive
/// as TWO DISTINCT `RoutineNodeId`s (`abi_overload_collapsed` stays `false`
/// on both). The chain still declines, but via a DIFFERENT — and more
/// honest — mechanism: `resolve_member`'s own arity+visibility selection now
/// sees 2 live, visible, same-arity candidates and returns
/// `Unresolved(OverloadAmbiguous)` directly (see the companion test
/// `ws_cross_object_chain_abi_overload_uncollapsed_plain_dispatch_declines_ambiguous`
/// below, which pins the INNER `Get(Helper)` call's route specifically).
/// The outer chain's assertion below is therefore UNCHANGED (still
/// `Unresolved`/`Unknown(_)`) — only the reason inside `Unknown` moved from
/// whatever the collapsed survivor's chain-guard produced to
/// `OverloadAmbiguous` propagating from the inner call's own failure to type.
///
/// Pre-Task-2/pre-Task-3 (the original bug both tests pin): the survivor was
/// the FIRST raw JSON entry (`Get(X: Codeunit "Dep A")`, returning `Codeunit
/// "Dep Http Content"`), so this chain would have wrongly resolved
/// `Object{Codeunit, "dep http content"}` and emitted an `Opaque` route to
/// `ReadAs` — silently ignoring the second, differently-typed overload.
#[test]
fn ws_cross_object_chain_abi_overload_collapsed_declines() {
    let report = ws_cross_object_chain_report();
    let route = outer_chain_route(&report, 51206, "testabioverloadcollapseddeclines");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 32p companion (Task 2, round-1 critical requirement (d)): pins the
/// INNER `DepCollapse.Get(Helper)` call specifically — NOT the outer
/// `.ReadAs()` chain call the sibling test above checks. Post-Task-2, the
/// two `Get` overloads (`Dep A` Id 60130 / `Dep C` Id 60140) carry DISTINCT
/// `sig_fp`s and never collapse — `resolve_in_object` finds 2 arity-1,
/// visible candidates via PLAIN dispatch (not a chain type-query). This is
/// the exact call site that, PRE-Task-2, silently resolved `Opaque` to an
/// ARBITRARY survivor — the round-1 critical "unguarded plain-dispatch
/// false-Opaque" class that task closed (only the OUTER `.ReadAs()` chain
/// declined before, via the separate chain-guard; the inner call itself was
/// never checked).
///
/// REBASELINED (Task 4, sigfp-and-ambiguous-reclassification plan — the
/// metric-definition change, correctness over backwards compatibility): both
/// candidates are CONCRETE (SymbolOnly `Evidence::Opaque` +
/// `RouteTarget::AbiSymbol` — the plan's "exact `Opaque`+`AbiSymbol` for
/// SymbolOnly" prevalidation-pass shape, never a false `Unresolved`), so this
/// is no longer a single `Unknown(OverloadAmbiguous)` decline — it is
/// candidate-carrying `ObligationOutcome::AmbiguousResolved`, TWO `AbiSymbol`
/// routes (one per `Get` overload), each carrying `Condition::
/// AmbiguousDispatch`. The chain still CANNOT type-query through it (the
/// sibling `ws_cross_object_chain_abi_overload_collapsed_declines` above
/// stays `Unresolved`/`Unknown` — `routine_node_for_type_query`'s `[route]`
/// slice pattern declines on a 2-route result, unaffected by this
/// reclassification) — only the INNER plain-dispatch call's own obligation
/// changed shape.
#[test]
fn ws_cross_object_chain_abi_overload_uncollapsed_plain_dispatch_becomes_ambiguous_resolved() {
    use al_sem::program::resolve::edge::{Condition, ObligationOutcome, classify_obligation};

    let report = ws_cross_object_chain_report();
    let edges = edges_for_object_routine(&report, 51206, "testabioverloadcollapseddeclines");
    assert!(!edges.is_empty(), "must have at least one call obligation");
    // The INNER call is the NARROWEST-span `Call`-kind edge (the outer
    // `.ReadAs()` chain call's span strictly contains it) — the mirror
    // image of `outer_chain_route`'s widest-span selection.
    let inner = edges
        .iter()
        .filter(|ce| ce.edge.kind == EdgeKind::Call)
        .min_by_key(|ce| ce.edge.site.span.end.col as i64 - ce.edge.site.span.start.col as i64)
        .expect("at least one Call-kind edge");

    assert_eq!(
        inner.edge.shape,
        DispatchShape::AmbiguousOverload,
        "got {:?}",
        inner.edge.shape
    );
    let routes = &inner.edge.routes;
    assert_eq!(
        routes.len(),
        2,
        "one route per Get overload; got {routes:?}"
    );

    let mut seen_keys = std::collections::HashSet::new();
    for route in routes {
        assert_eq!(
            route.evidence,
            Evidence::Opaque,
            "SymbolOnly candidates are concrete Opaque, never Source; got {route:?}"
        );
        assert!(
            route.conditions.contains(&Condition::AmbiguousDispatch),
            "every candidate route must carry AmbiguousDispatch; got {route:?}"
        );
        assert!(
            !route.fires_by_default(),
            "an AmbiguousDispatch route must not fire by default; got {route:?}"
        );
        let RouteTarget::AbiSymbol { ref key } = route.target else {
            panic!("expected an AbiSymbol target; got {route:?}");
        };
        assert!(
            seen_keys.insert(key.clone()),
            "candidate routes must target DISTINCT AbiRoutineKeys; got {routes:?}"
        );
    }

    assert_eq!(
        classify_obligation(&inner.edge),
        ObligationOutcome::AmbiguousResolved,
        "a closed, all-concrete 2-candidate ABI overload set is \
         AmbiguousResolved, never Unknown — the metric-definition change"
    );
}

/// Test 32q (fixture N12 — Task 2 REVIEW FIX): `Dep Run Collapse` declares
/// its `OnRun` entry trigger via a LITERALLY DUPLICATED raw ABI entry
/// (0-arg — `sig_fp` folds to the fixed `0` for an empty `Parameters[]`, see
/// `abi_ingest::param_type_fp`), so `dedup_routines_preserving_genuine_
/// overloads` collapses both raw entries into ONE survivor marked
/// `abi_overload_collapsed`. `Codeunit.Run(...)` dispatches through
/// `resolve_object_run` — an entry-trigger lookup by ROLE (fixed name) that
/// bypasses `resolve_in_object`'s name+arity selection ENTIRELY, so, before
/// the Task 2 review fix, this path never consulted the collapse marker at
/// all: it would have resolved the arbitrary raw-JSON survivor CONFIDENTLY
/// as an `Opaque`/`AbiSymbol` route despite the underlying duplicate/
/// collision being unresolved. Post-fix: `resolve_object_run` now applies
/// its own `routine_is_collapse_marked` guard and must decline
/// `Unresolved`/`Unknown(OverloadAmbiguous)` instead — no route/edge to the
/// collapsed survivor.
#[test]
fn ws_cross_object_chain_object_run_collapsed_trigger_declines() {
    let report = ws_cross_object_chain_report();
    let edges = edges_for_object_routine(&report, 51206, "testobjectruncollapsedtriggerdeclines");
    assert!(!edges.is_empty(), "must have at least one Run obligation");
    let run_edge = edges
        .iter()
        .find(|ce| ce.edge.kind == EdgeKind::Run)
        .expect("expected exactly one Run-kind edge");
    assert_eq!(
        run_edge.edge.routes.len(),
        1,
        "expected exactly one route on the ObjectRun edge; got {:?}",
        run_edge.edge.routes
    );
    let route = &run_edge.edge.routes[0];
    assert_eq!(
        route.target,
        RouteTarget::Unresolved,
        "Codeunit.Run to a codeunit whose sole OnRun candidate is \
         collapse-MARKED must never resolve confidently — resolve_object_run \
         bypasses resolve_in_object entirely (Task 2 review fix); got {route:?}"
    );
    assert_eq!(
        route.evidence,
        Evidence::Unknown(UnknownReason::OverloadAmbiguous),
        "expected Unknown(OverloadAmbiguous); got {route:?}"
    );
}

// ---------------------------------------------------------------------------
// Tests 33+: record-field-chains plan Task 3 — table-field type index +
// `Rec."Field".X()` / `Rec.Field.X()` record-field chains + EnumType chain
// base, end-to-end over `ws-record-field-chain`.
//
// Root feature: `ResolveIndex::field_in_table` (`src/program/resolve/
// index.rs`, visibility-scoped base+extension field lookup, unique-match-
// or-decline) feeds the new non-method `Member{object, member}` arm in
// `infer_compound_member_receiver` (`src/program/resolve/receiver.rs`),
// which types `Rec."Field"` / `Rec.Field` via `classify_type_text` on the
// field's declared type text — the SAME strict classification every other
// declared type goes through, never `FieldDecl::is_blob_like`. A SEPARATE
// new arm, `enum_chain_return_kind` (`src/program/resolve/
// framework_returns.rs`), types `Ordinals()`/`Names()` on an Enum-field-
// typed base as `Framework(List)`, enabling the multi-level chain. See
// `tests/r0-corpus/ws-record-field-chain/PROOF.md` for the real-CDO-source
// grounding of every positive fixture.
// ---------------------------------------------------------------------------

/// Loads `tests/r0-corpus/ws-record-field-chain` and returns the full
/// `resolve_full_program` report — shared by the tests below.
fn ws_record_field_chain_report() -> ProgramReport {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-record-field-chain");
    resolve_full_program(&fixture)
        .expect("resolve_full_program must succeed on ws-record-field-chain")
}

/// The route for the OUTERMOST call obligation of a fixture routine —
/// mirrors `widest_call_route`/`outer_chain_route` (picks the widest-span
/// `Call`-kind edge; this fixture set's chains are all single-line).
fn rfc_outer_route(report: &ProgramReport, object_id_number: i64, routine_name_lc: &str) -> Route {
    let edges = edges_for_object_routine(report, object_id_number, routine_name_lc);
    assert!(
        !edges.is_empty(),
        "{routine_name_lc} (object {object_id_number}) must have at least 1 call obligation"
    );
    let outer = edges
        .iter()
        .filter(|ce| ce.edge.kind == EdgeKind::Call)
        .max_by_key(|ce| ce.edge.site.span.end.col as i64 - ce.edge.site.span.start.col as i64)
        .expect("at least one Call-kind edge");
    outer.edge.routes[0].clone()
}

/// Test 33a (fixture a, POSITIVE): `Rec."Error Message".CreateInStream(S)` —
/// Blob field -> `Framework(Blob)` -> `CreateInStream` is a real Blob
/// catalog member, so the outer call resolves `Evidence::Catalog`.
#[test]
fn ws_record_field_chain_blob_field_resolves_catalog() {
    let report = ws_record_field_chain_report();
    let route = rfc_outer_route(&report, 51504, "testblobfieldchain");
    assert_eq!(route.evidence, Evidence::Catalog);
    let RouteTarget::Builtin(ref bid) = route.target else {
        panic!("expected RouteTarget::Builtin, got {:?}", route.target);
    };
    assert_eq!(bid.0, "Blob::createinstream");
}

/// Test 33b (fixture b, POSITIVE — multi-level chain):
/// `Rec."eSeal Service".Ordinals().Count()` — Enum field -> `EnumType` ->
/// `.Ordinals()` [new `enum_chain_return_kind` arm] -> `Framework(List)` ->
/// `.Count()` is a real List catalog member. 2 nested call obligations
/// (`Ordinals()`, `Count()`).
#[test]
fn ws_record_field_chain_enum_field_multilevel_resolves_catalog() {
    let report = ws_record_field_chain_report();
    let edges = edges_for_object_routine(&report, 51504, "testenumfieldmultilevelchain");
    assert_eq!(
        edges.len(),
        2,
        "Rec.\"eSeal Service\".Ordinals().Count() has 2 nested call obligations"
    );
    let route = rfc_outer_route(&report, 51504, "testenumfieldmultilevelchain");
    assert_eq!(route.evidence, Evidence::Catalog);
    let RouteTarget::Builtin(ref bid) = route.target else {
        panic!("expected RouteTarget::Builtin, got {:?}", route.target);
    };
    assert_eq!(bid.0, "List::count");
}

/// Test 33c (fixture c, POSITIVE — TableExtension folding):
/// `Rec."Ext Blob".CreateInStream(S)` — a field declared on "RFC Base Ext"
/// (a `TableExtension` of "RFC Base") resolves through the SAME arm as a
/// base field, via `ResolveIndex::field_in_table`'s extension folding.
#[test]
fn ws_record_field_chain_extension_field_folds_resolves_catalog() {
    let report = ws_record_field_chain_report();
    let route = rfc_outer_route(&report, 51504, "testextensionfieldchain");
    assert_eq!(route.evidence, Evidence::Catalog);
    let RouteTarget::Builtin(ref bid) = route.target else {
        panic!("expected RouteTarget::Builtin, got {:?}", route.target);
    };
    assert_eq!(bid.0, "Blob::createinstream");
}

/// Test 33e (fixture e, NEGATIVE): unknown field name — `field_in_table`
/// genuinely finds nothing, so the receiver stays `Unknown`.
#[test]
fn ws_record_field_chain_unknown_field_name_stays_unknown() {
    let report = ws_record_field_chain_report();
    let route = rfc_outer_route(&report, 51504, "testunknownfieldname");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 33f (fixture f, NEGATIVE): a scalar-typed field (`Integer`) — the
/// member call on it declines (`Primitive` receiver -> `CatalogMiss`).
#[test]
fn ws_record_field_chain_scalar_field_stays_unknown() {
    let report = ws_record_field_chain_report();
    let route = rfc_outer_route(&report, 51504, "testscalarfieldmembercall");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 33g (fixture g, NEGATIVE — fail-closed duplicate): "Dup Field" is
/// declared by BOTH the base table and a visible `TableExtension` —
/// `field_in_table` must decline (ambiguous), never arbitrarily pick either
/// candidate.
#[test]
fn ws_record_field_chain_duplicate_field_across_extension_stays_unknown() {
    let report = ws_record_field_chain_report();
    let route = rfc_outer_route(&report, 51504, "testduplicatefieldacrossbaseextension");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 33h (fixture h, NEGATIVE): a Page (non-Record) receiver with a
/// quoted member — the record-field arm's `Record{table: Some(..)}` guard
/// never engages for an `Object{kind: Page}` receiver, even though the
/// quoted text names a real field on the page's own SourceTable.
#[test]
fn ws_record_field_chain_page_receiver_quoted_member_stays_unknown() {
    let report = ws_record_field_chain_report();
    let route = rfc_outer_route(&report, 51504, "testpagereceiverquotedmember");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 33i (fixture i, NEGATIVE/regression-proof): a local variable named
/// identically to a real field, referenced BARE (no `Rec.` prefix) — the
/// pre-existing variable lookup (Step 2, unaffected by Task 3) wins outright;
/// the record-field arm is never even reached (it only fires when the
/// `Member`'s `object` sub-expression already resolved to a Record). `.Trim`
/// is a real Text catalog member, so this resolves via `Framework(Text)`,
/// proving the non-field binding wins and the field is never mis-typed.
#[test]
fn ws_record_field_chain_var_shadows_field_name_non_field_wins() {
    let report = ws_record_field_chain_report();
    let route = rfc_outer_route(&report, 51504, "testvarnameshadowsfieldnamenonfieldwins");
    assert_eq!(route.evidence, Evidence::Catalog);
    let RouteTarget::Builtin(ref bid) = route.target else {
        panic!("expected RouteTarget::Builtin, got {:?}", route.target);
    };
    assert_eq!(
        bid.0, "Text::trim",
        "the LOCAL VARIABLE binding must win (Framework(Text) -> Text::trim), \
         never a field mis-typing"
    );
}

// ---------------------------------------------------------------------------
// Tests 34+: record-field-chains plan Task 4 — bare implicit-Rec QUOTED-field
// receivers (`"Field".X()` with NO `Rec.` prefix, inside a Table/
// TableExtension's own procedure), end-to-end over
// `ws-bare-implicit-rec-field`.
//
// Root feature: `infer_receiver_type`'s new Step 3a (`src/program/resolve/
// receiver.rs`) — a QUOTED bare identifier in Table/TableExtension scope,
// after Step 2's (quote-parity-fixed) var/param/global lookup misses, looks
// the name up in the SAME visibility-scoped `ResolveIndex::field_in_table`
// surface Task 3's explicit `Rec."Field"` arm consults, gated on
// `WithState::NoWithProven` (mirrors `resolve_bare`'s own Step 3) and on the
// round-2 routine-shadow guard (`ResolveIndex::table_scope_has_routine`) —
// AL's parens are optional on a zero-argument call, so a bare quoted name is
// structurally ambiguous between a field reference and a parens-less
// procedure-call chain. See `tests/r0-corpus/ws-bare-implicit-rec-field/
// PROOF.md` for the real-CDO-source grounding.
// ---------------------------------------------------------------------------

/// Loads `tests/r0-corpus/ws-bare-implicit-rec-field` and returns the full
/// `resolve_full_program` report — shared by the tests below.
fn ws_bare_implicit_rec_field_report() -> ProgramReport {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-bare-implicit-rec-field");
    resolve_full_program(&fixture)
        .expect("resolve_full_program must succeed on ws-bare-implicit-rec-field")
}

/// Test 34a (fixture a, POSITIVE): `"File Blob".CreateInStream(S)` inside
/// the TABLE's own procedure (no `Rec.` prefix at all) — the implicit-Rec
/// field types `Framework(Blob)`, resolving the Blob catalog leaf.
#[test]
fn ws_bare_implicit_rec_field_blob_resolves_catalog() {
    let report = ws_bare_implicit_rec_field_report();
    let route = rfc_outer_route(&report, 51520, "testbareblobfield");
    assert_eq!(route.evidence, Evidence::Catalog);
    let RouteTarget::Builtin(ref bid) = route.target else {
        panic!("expected RouteTarget::Builtin, got {:?}", route.target);
    };
    assert_eq!(bid.0, "Blob::createinstream");
}

/// Extra positive: a bare Text field — `.Trim()` is a real Text catalog
/// member (the measured "~1 Text[250] `.Trim()`" population from the plan's
/// grounding).
#[test]
fn ws_bare_implicit_rec_field_text_resolves_catalog() {
    let report = ws_bare_implicit_rec_field_report();
    let route = rfc_outer_route(&report, 51520, "testbaretextfield");
    assert_eq!(route.evidence, Evidence::Catalog);
    let RouteTarget::Builtin(ref bid) = route.target else {
        panic!("expected RouteTarget::Builtin, got {:?}", route.target);
    };
    assert_eq!(bid.0, "Text::trim");
}

/// Test 34b (fixture b, POSITIVE — TableExtension scope, own field):
/// `"Ext Blob".CreateInStream(S)` inside the TableExtension's own procedure.
#[test]
fn ws_bare_implicit_rec_field_tableext_own_field_resolves_catalog() {
    let report = ws_bare_implicit_rec_field_report();
    let route = rfc_outer_route(&report, 51521, "testbareownextfield");
    assert_eq!(route.evidence, Evidence::Catalog);
    let RouteTarget::Builtin(ref bid) = route.target else {
        panic!("expected RouteTarget::Builtin, got {:?}", route.target);
    };
    assert_eq!(bid.0, "Blob::createinstream");
}

/// Test 34b (fixture b, POSITIVE — TableExtension scope, base field folded):
/// `"File Blob".CreateInStream(S)` inside the TableExtension's own procedure
/// — the BASE table's field, visible via `field_in_table`'s extension
/// folding.
#[test]
fn ws_bare_implicit_rec_field_tableext_base_field_resolves_catalog() {
    let report = ws_bare_implicit_rec_field_report();
    let route = rfc_outer_route(&report, 51521, "testbarebasefieldfromextension");
    assert_eq!(route.evidence, Evidence::Catalog);
    let RouteTarget::Builtin(ref bid) = route.target else {
        panic!("expected RouteTarget::Builtin, got {:?}", route.target);
    };
    assert_eq!(bid.0, "Blob::createinstream");
}

/// Test 34f (fixture f, NEGATIVE): an unknown quoted name — `field_in_table`
/// genuinely finds nothing, so the receiver stays `Unknown`.
#[test]
fn ws_bare_implicit_rec_field_unknown_name_stays_unknown() {
    let report = ws_bare_implicit_rec_field_report();
    let route = rfc_outer_route(&report, 51520, "testbareunknownfield");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 34c/34d (fixture c+d, QUOTE-PARITY + PRECEDENCE): a quoted name
/// matching BOTH a local var AND a real table field — the var MUST win
/// (AL scoping), resolving `Framework(Text)` (`.Trim()`), never the field's
/// `Framework(Blob)`. Pre-quote-parity-fix, this would have fallen through
/// Step 2 to Step 3a and (post-Task-4) mistyped the field — the exact
/// false-`Source` class the fix exists to prevent.
#[test]
fn ws_bare_implicit_rec_field_var_shadows_field_quote_parity() {
    let report = ws_bare_implicit_rec_field_report();
    let route = rfc_outer_route(&report, 51520, "testbarevarshadowsfieldquoteparity");
    assert_eq!(route.evidence, Evidence::Catalog);
    let RouteTarget::Builtin(ref bid) = route.target else {
        panic!("expected RouteTarget::Builtin, got {:?}", route.target);
    };
    assert_eq!(
        bid.0, "Text::trim",
        "the LOCAL VARIABLE binding must win (Framework(Text) -> Text::trim), \
         never the same-named field (Framework(Blob))"
    );
}

/// Round-2 soundness correction (coordinator-required regression fixture):
/// a same-named ROUTINE ("Shadowed Field") declared on the same table must
/// block field-typing — AL's parens are optional on a zero-argument call,
/// so a bare quoted name is ambiguous between the field and a parens-less
/// call to the procedure; must decline to `Unknown`, never mistyped as the
/// field.
#[test]
fn ws_bare_implicit_rec_field_routine_shadow_stays_unknown() {
    let report = ws_bare_implicit_rec_field_report();
    let route = rfc_outer_route(&report, 51520, "testbareroutineshadowsfield");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Test 34e (fixture e, NEGATIVE): a quoted-field-shaped bare receiver in a
/// NON-Table/TableExtension object (a Codeunit) — Step 3a's `ObjectKind`
/// guard declines, even though "File Blob" is a real field name on "RBF
/// Base" elsewhere in this same app (proving the OBJECT-KIND gate, not
/// merely "no such field").
#[test]
fn ws_bare_implicit_rec_field_non_table_scope_stays_unknown() {
    let report = ws_bare_implicit_rec_field_report();
    let route = rfc_outer_route(&report, 51522, "testbarefieldreceivernontablescope");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

// ---------------------------------------------------------------------------
// Dataitem-receivers plan, Task 1: report-dataitem receivers end to end,
// over `ws-report-dataitem/`. See that fixture's `PROOF.md` for the full
// real-CDO-source grounding of every positive fixture.
//
// Root features: `infer_receiver_type`'s new Step 2b (dataitem-NAME
// receiver, `src/program/resolve/receiver.rs`); the routine-contextual
// Report/ReportExtension arm of `infer_implicit_rec`; the centralized
// quote-aware `is_atomic_receiver_token` guard (replaces the naive
// dot-substring check that mislabeled a dot-bearing quoted dataitem name
// `CompoundReceiver`); the additive `modify()` lowerer fix
// (`crates/al-syntax/src/lower/mod.rs`) + its resolve-time dataset-context
// fallback.
// ---------------------------------------------------------------------------

/// Loads `tests/r0-corpus/ws-report-dataitem` and returns the full
/// `resolve_full_program` report — shared by the tests below.
fn ws_report_dataitem_report() -> ProgramReport {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-report-dataitem");
    resolve_full_program(&fixture).expect("resolve_full_program must succeed on ws-report-dataitem")
}

/// Fixtures (a)+(c), POSITIVE: a trigger nested inside `dataitem(Cust; "RD
/// Customer")` types the explicit `Rec.GetDisplayName()` receiver by the
/// dataitem's source table (`RoutineDecl.dataitem_source_table` threaded
/// into `infer_implicit_rec`'s Report arm) — resolves `Evidence::Source` to
/// `"RD Customer".GetDisplayName`, a NON-builtin table procedure.
#[test]
fn ws_report_dataitem_trigger_resolves_via_dataitem_source_table() {
    let report = ws_report_dataitem_report();
    let route = rfc_outer_route(&report, 51700, "onaftergetrecord");
    assert_eq!(route.evidence, Evidence::Source);
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected RouteTarget::Routine, got {:?}", route.target);
    };
    assert_eq!(rid.name_lc, "getdisplayname");
    assert_eq!(rid.object.kind, ObjectKind::Table);
    assert!(
        rid.object.id_equals_number(51710),
        "must resolve to \"RD Customer\" (id 51710); got {:?}",
        rid.object
    );
}

/// Fixture (a), POSITIVE (Step 2b): a bare dataitem-NAME receiver
/// (`Cust.GetDisplayName()`), called from a routine with NO enclosing
/// dataitem context at all — proves the lookup is routine-independent (a
/// dataitem name is in scope as a record var across ALL the report's
/// routines).
#[test]
fn ws_report_dataitem_bare_name_receiver_resolves() {
    let report = ws_report_dataitem_report();
    let route = rfc_outer_route(&report, 51700, "testbarecustname");
    assert_eq!(route.evidence, Evidence::Source);
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected RouteTarget::Routine, got {:?}", route.target);
    };
    assert_eq!(rid.name_lc, "getdisplayname");
    assert!(rid.object.id_equals_number(51710));
}

/// Fixture (b), POSITIVE (the quote-guard fix): a QUOTED dataitem name with
/// an EMBEDDED PERIOD (`"Sales Cr.Memo Header Filter"`, the real CDO shape)
/// resolves via Step 2b — the naive dot-substring guard this task replaces
/// would have mislabeled this `CompoundReceiver` before it ever reached the
/// dataitem-name lookup.
#[test]
fn ws_report_dataitem_dot_bearing_quoted_name_resolves() {
    let report = ws_report_dataitem_report();
    let route = rfc_outer_route(&report, 51700, "testbaredotbearingname");
    assert_eq!(route.evidence, Evidence::Source);
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected RouteTarget::Routine, got {:?}", route.target);
    };
    assert_eq!(rid.name_lc, "getfilters");
    assert!(
        rid.object.id_equals_number(51711),
        "must resolve to \"RD Sales Header\" (id 51711); got {:?}",
        rid.object
    );
}

/// REQUESTPAGE ISOLATION (binding, round-1 addendum), NEGATIVE: a
/// requestpage trigger's implicit Rec must NEVER bind a report dataitem's
/// table — even though the SAME report has a dataitem-bearing dataset,
/// `Rec.GetDisplayName()` inside `requestpage { trigger OnOpenPage() .. }`
/// stays honest `Unknown`.
#[test]
fn ws_report_dataitem_requestpage_trigger_never_binds_dataitem_table() {
    let report = ws_report_dataitem_report();
    let route = rfc_outer_route(&report, 51700, "onopenpage");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// NEGATIVE (var shadows dataitem, AL scoping): a LOCAL var (`Cust: Record
/// "RD Sales Header"`) of a DIFFERENT table than the "Cust" dataitem's own
/// (`"RD Customer"`) must win — Step 2 (var lookup) runs strictly before
/// Step 2b (dataitem lookup). A mistaken Step-2b hit would resolve against
/// `"RD Customer"` instead, observably distinguishable from the correct
/// Step-2 var hit against `"RD Sales Header"`.
#[test]
fn ws_report_dataitem_local_var_shadows_dataitem_name() {
    let report = ws_report_dataitem_report();
    let route = rfc_outer_route(&report, 51700, "testvarshadowsdataitem");
    assert_eq!(route.evidence, Evidence::Source);
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected RouteTarget::Routine, got {:?}", route.target);
    };
    assert_eq!(rid.name_lc, "getfilters");
    assert!(
        rid.object.id_equals_number(51711),
        "the local var must win, resolving to \"RD Sales Header\" (51711) \
         not the dataitem's \"RD Customer\" (51710); got {:?}",
        rid.object
    );
}

/// NEGATIVE (collision guard, fail-closed): a dataitem name ("RD Collide")
/// that is ALSO a report procedure name must decline — AL's parens-optional
/// zero-arg call makes the receiver structurally ambiguous between "the
/// dataitem record" and "a parens-less call to the procedure".
#[test]
fn ws_report_dataitem_collision_with_procedure_declines() {
    let report = ws_report_dataitem_report();
    let route = rfc_outer_route(&report, 51700, "testcollisiondeclines");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// NEGATIVE (a genuinely compound receiver stays compound): an unquoted
/// `A.B` shaped receiver must never be mis-routed into the atomic
/// dataitem-name lookup.
#[test]
fn ws_report_dataitem_genuinely_compound_receiver_stays_unknown() {
    let report = ws_report_dataitem_report();
    let route = rfc_outer_route(&report, 51700, "testgenuinelycompoundreceiverstaysunknown");
    assert_eq!(route.target, RouteTarget::Unresolved);
    assert!(matches!(route.evidence, Evidence::Unknown(_)));
}

/// Fixture (d), POSITIVE (the `modify()` lowerer gap + resolve-time
/// fallback): a ReportExtension's `dataset { modify(Cust) { trigger
/// OnAfterGetRecord .. } }` — pre-fix, `enclosing_member` AND
/// `dataitem_source_table` were both `None` for this trigger (the lowerer's
/// generic Name-based member-wrapper gate never recognized
/// `modify_modification`, whose target lives in the `target` field). Post-
/// fix, the additive `Target` read populates `enclosing_member` +
/// `in_dataset_modify_context`, and the resolver's confirmed-dataset-context
/// fallback resolves the implicit Rec via the merged own+base dataitem map
/// (here: the BASE report's own "Cust" -> "RD Customer").
#[test]
fn ws_report_dataitem_extension_modify_trigger_resolves_via_fallback() {
    let report = ws_report_dataitem_report();
    let route = rfc_outer_route(&report, 51701, "onaftergetrecord");
    assert_eq!(route.evidence, Evidence::Source);
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected RouteTarget::Routine, got {:?}", route.target);
    };
    assert_eq!(rid.name_lc, "getdisplayname");
    assert!(
        rid.object.id_equals_number(51710),
        "must resolve to \"RD Customer\" (id 51710) via the base report's \
         \"Cust\" dataitem; got {:?}",
        rid.object
    );
}

/// Fixture (e), POSITIVE (ReportExtension base-dataitem fallback): the
/// extension has NO dataitems of its own — a bare dataitem-NAME receiver
/// naming the BASE report's "Cust" dataitem still resolves, via the
/// extends-target base-dataitem fallback (mirrors the PageExtension
/// `SourceTable` inheritance pattern).
#[test]
fn ws_report_dataitem_extension_resolves_base_dataitem_name() {
    let report = ws_report_dataitem_report();
    let route = rfc_outer_route(&report, 51701, "exttestbasedataitemname");
    assert_eq!(route.evidence, Evidence::Source);
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected RouteTarget::Routine, got {:?}", route.target);
    };
    assert_eq!(rid.name_lc, "getdisplayname");
    assert!(rid.object.id_equals_number(51710));
}

// ---------------------------------------------------------------------------
// T0.3: builtin-dispatch justification audit
// ---------------------------------------------------------------------------
//
// The north-star metric counts only `unknown` edges as holes; a missed
// dispatch edge that lands in `builtin` is structurally invisible to it (see
// `member_catalog::ENTRY_DISPATCH_BUILTIN_IDS`'s doc for the two classifier
// gaps this audit makes visible: `Page.RunModal(Page::"X")`-shaped keyword
// calls, and declared Page/Report-typed variable `.RunModal()` calls). This
// audit is DIAGNOSTIC + ratchet only — it never changes resolution outcome.

/// Loads `tests/r0-corpus/ws-builtin-dispatch-audit` and returns the full
/// `resolve_full_program` report.
fn ws_builtin_dispatch_audit_report() -> ProgramReport {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-builtin-dispatch-audit");
    resolve_full_program(&fixture)
        .expect("resolve_full_program must succeed on ws-builtin-dispatch-audit")
}

/// T1.3 (deep-review-remediation plan) fixture-level proof (no CDO needed):
/// the audit flags ZERO sites in `AuditCaller.Codeunit.al` now that BOTH
/// classifier gaps are closed — the brief's cited `Page.RunModal(Page::"X")`
/// keyword-receiver shape, its `Report.RunModal(Report::"X")` analog, AND the
/// declared-Page-variable shape (`MyPage.RunModal()`) all now classify as
/// `CalleeShape::ObjectRun` / dispatch through `resolve_member_with_args`'s
/// entry-trigger special case respectively — neither ever reaches
/// `resolve_member_with_args`'s generic Member arm, so `builtin_dispatch_
/// finding` (populated ONLY by that arm) never fires. Pre-fix this asserted
/// flagged==3/indeterminate==1 (T0.3); this is the red-then-green flip —
/// `ws_builtin_dispatch_audit_sites_produce_entry_trigger_run_edges` below
/// proves the counts dropped BECAUSE real `Run` edges appeared, not because
/// the audit went quiet for the wrong reason.
///
/// The dynamic-target call (`Page.RunModal(DynamicPageId)`) drops out of
/// `indeterminate` too — it is now `CalleeShape::ObjectRun` from
/// classification onward (the classifier only inspects the callee, not the
/// argument shape; `target_ref` extraction is what decides static vs.
/// dynamic, same as `Codeunit.Run(SomeVar)` already worked), so it never
/// touches the `Member`-arm audit path either. It keeps its honest dynamic
/// handling via `resolve_object_run`'s `DynamicOpen` branch — see the
/// companion test for the positive `HonestDynamic` assertion.
#[test]
fn ws_builtin_dispatch_audit_flags_zero_after_entry_trigger_fix() {
    let report = ws_builtin_dispatch_audit_report();
    let audit = &report.builtin_dispatch_audit;

    assert_eq!(
        audit.flagged.len(),
        0,
        "T1.3 must close every flagged builtin-dispatch site; got {:#?}",
        audit.flagged
    );
    assert_eq!(
        audit.indeterminate.len(),
        0,
        "the dynamic-target RunModal call must classify as ObjectRun (never \
         reaching the Member-arm audit) after T1.3 — got {:#?}",
        audit.indeterminate
    );
}

/// T1.3 positive proof: the audit going quiet above is BECAUSE real
/// entry-trigger `Run` edges now exist, not because the sites vanished or
/// silently declined. All 3 previously-flagged sites resolve to
/// `EdgeKind::Run` / `Evidence::Source` routes targeting the exact trigger
/// `resolve_object_run`'s `entry_trigger_name` mapping predicts (Page →
/// `OnOpenPage`, Report → `OnPreReport`) — proven against REAL declared
/// triggers in `AuditTargetPage.Page.al`/`AuditTargetReport.Report.al` (added
/// by T1.3), not merely an Opaque boundary. The dynamic-target call resolves
/// to `ObligationOutcome::HonestDynamic` via the same `EdgeKind::Run`
/// machinery, `Codeunit.Run(var)`'s pre-existing dynamic handling mirrored
/// exactly for Page/Report (never `Unknown`).
#[test]
fn ws_builtin_dispatch_audit_sites_produce_entry_trigger_run_edges() {
    let report = ws_builtin_dispatch_audit_report();

    // Both Page-targeting sites (keyword-receiver + declared-variable) hit
    // the SAME target trigger: Page 50951's OnOpenPage. The keyword-receiver
    // form classifies as `CalleeShape::ObjectRun` → `EdgeKind::Run`; the
    // declared-variable form stays `CalleeShape::Member` → `EdgeKind::Call`
    // (only its ROUTE changes — same as `Codeunit.Run(TypedVar)`'s
    // pre-existing entry-trigger special case, which is also `EdgeKind::Call`)
    // — both are genuine dispatches into the SAME Source routine, just
    // extracted through different classifier paths.
    for (routine_lc, expected_kind) in [
        ("flaggedpagekeywordrunmodal", EdgeKind::Run),
        ("flaggeddeclaredpagevarrunmodal", EdgeKind::Call),
    ] {
        let edges = edges_for_object_routine(&report, 50953, routine_lc);
        let run_edge = edges
            .iter()
            .find(|ce| ce.edge.kind == expected_kind)
            .unwrap_or_else(|| {
                panic!("{routine_lc}: expected exactly one {expected_kind:?}-kind edge")
            });
        assert_eq!(
            run_edge.edge.routes.len(),
            1,
            "{routine_lc}: expected exactly one route; got {:?}",
            run_edge.edge.routes
        );
        let route = &run_edge.edge.routes[0];
        assert_eq!(
            route.evidence,
            Evidence::Source,
            "{routine_lc}: must dispatch to Page 50951's declared OnOpenPage \
             trigger, not fall back to Opaque/Unknown/Catalog; got {route:?}"
        );
        let RouteTarget::Routine(ref rid) = route.target else {
            panic!(
                "{routine_lc}: expected RouteTarget::Routine (OnOpenPage), got {:?}",
                route.target
            );
        };
        assert_eq!(rid.name_lc, "onopenpage");
        assert_eq!(rid.object.kind, ObjectKind::Page);
        assert!(
            rid.object.id_equals_number(50951),
            "{routine_lc}: must target Page 50951 (Audit Target Page); got {:?}",
            rid.object
        );
    }

    // Report-keyword site hits Report 50952's OnPreReport.
    {
        let edges = edges_for_object_routine(&report, 50953, "flaggedreportkeywordrunmodal");
        let run_edge = edges
            .iter()
            .find(|ce| ce.edge.kind == EdgeKind::Run)
            .expect("flaggedreportkeywordrunmodal: expected exactly one Run-kind edge");
        assert_eq!(run_edge.edge.routes.len(), 1);
        let route = &run_edge.edge.routes[0];
        assert_eq!(
            route.evidence,
            Evidence::Source,
            "must dispatch to Report 50952's declared OnPreReport trigger; got {route:?}"
        );
        let RouteTarget::Routine(ref rid) = route.target else {
            panic!(
                "expected RouteTarget::Routine (OnPreReport), got {:?}",
                route.target
            );
        };
        assert_eq!(rid.name_lc, "onprereport");
        assert_eq!(rid.object.kind, ObjectKind::Report);
        assert!(
            rid.object.id_equals_number(50952),
            "must target Report 50952 (Audit Target Report); got {:?}",
            rid.object
        );
    }

    // Dynamic-target site: honest dynamic dispatch, never Unknown.
    {
        use al_sem::program::resolve::edge::{ObligationOutcome, classify_obligation};

        let edges =
            edges_for_object_routine(&report, 50953, "indeterminatepagekeyworddynamictarget");
        let run_edge = edges
            .iter()
            .find(|ce| ce.edge.kind == EdgeKind::Run)
            .expect("dynamic-target call: expected exactly one Run-kind edge");
        assert_eq!(
            run_edge.edge.shape,
            DispatchShape::DynamicOpen,
            "a runtime-variable RunModal target must keep the SAME dynamic \
             handling Codeunit.Run(var) already had; got {:?}",
            run_edge.edge.shape
        );
        assert_eq!(
            classify_obligation(&run_edge.edge),
            ObligationOutcome::HonestDynamic,
            "must never classify as Unknown — the north-star metric gate"
        );
    }
}

/// T1.3 CDO ratchet (gated — no `CDO_WS`, silently skips): the flagged
/// population on the real CDO workspace is PINNED to
/// `CDO_ENTRY_DISPATCH_FLAGGED_PIN`. T0.3 pinned this at 94 (both
/// classifier gaps still open); T1.3 (deep-review-remediation plan) closed
/// BOTH populations the audit covers — the keyword-receiver shape
/// (`extract::classify_call`'s Check 2 now also accepts `runmodal` for
/// Page/Report) AND the declared-variable shape (`resolve_member_with_args`'s
/// `ReceiverType::Object` arm now dispatches Page/Report `Run`/`RunModal` to
/// the entry trigger via the shared `dispatch_entry_trigger`, not just
/// Codeunit.Run) — MEASURED at 0, exactly as this doc predicted for the
/// both-populations-fixed case. A DIFFERENT number in EITHER direction from a
/// re-measurement that was not an intentional classifier change is a
/// regression: either a resolver change silently started/stopped landing
/// sites in the flagged catalog, or this audit's own logic drifted.
#[test]
fn cdo_builtin_dispatch_audit_flagged_count_is_pinned() {
    let Some(shared) = cdo_shared() else {
        return;
    };

    const CDO_ENTRY_DISPATCH_FLAGGED_PIN: usize = 0;

    let report = &shared.report;
    let audit = &report.builtin_dispatch_audit;

    // Determinism: sorted output, stable across repeated re-derivation from
    // the SAME report (not re-parsing CDO twice — this test asserts the
    // audit's own sort is idempotent, not merely "ran once").
    let mut resorted = audit.flagged.clone();
    resorted.sort();
    assert_eq!(
        audit.flagged, resorted,
        "flagged sites must already be sorted"
    );

    assert_eq!(
        audit.flagged.len(),
        CDO_ENTRY_DISPATCH_FLAGGED_PIN,
        "CDO flagged builtin-dispatch population moved off the pinned baseline \
         ({} vs pinned {}) — investigate before re-pinning: either a resolver \
         change intentionally landed/removed sites in the flagged catalog \
         (expected direction: DOWN, once T1.3 lands), or this is a real \
         regression.\nflagged sites:\n{:#?}",
        audit.flagged.len(),
        CDO_ENTRY_DISPATCH_FLAGGED_PIN,
        audit.flagged,
    );

    eprintln!(
        "T0.3 CDO builtin-dispatch audit: flagged={} indeterminate={}\nflagged sites:\n{:#?}",
        audit.flagged.len(),
        audit.indeterminate.len(),
        audit.flagged,
    );
}

// ---------------------------------------------------------------------------
// Unicode-fold arc: cross-case non-ASCII identifier resolution — the moat
// claim made executable.
//
// `tests/r0-corpus/ws-unicode-fold/` pairs a Danish object-name cross-case
// spelling (`Løbenr Mgt.` declared / `LØBENR MGT.` referenced — differing
// only in the case of the non-ASCII `Ø`/`ø`) with a German routine-name
// cross-case spelling (`Prüfung` declared / `PRÜFUNG` called — differing
// only in the case of the non-ASCII `Ü`/`ü`). Under the OLD
// `to_ascii_lowercase`-only fold neither pair is the same key: an ASCII-only
// fold leaves `Ø`/`ø` and `Ü`/`ü` byte-for-byte untouched, so e.g. `Ø`
// (U+00D8) and `ø` (U+00F8) fold to two DIFFERENT strings and the call would
// land `Unknown`, not `Source`. Under `fold_identifier`'s simple 1:1 Unicode
// fold (`crates/al-syntax/src/casing.rs`) both spellings fold to the SAME
// key, so both calls resolve. See `.superpowers/sdd/unicode-fold-investigation.md`.
// ---------------------------------------------------------------------------

fn ws_unicode_fold_report() -> ProgramReport {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus/ws-unicode-fold");
    resolve_full_program(&fixture).expect("resolve_full_program must succeed on ws-unicode-fold")
}

/// Axis 1 — OBJECT NAME fold: `CallCrossCaseObjectName` declares its
/// receiver `Codeunit "LØBENR MGT."` (all-caps, non-ASCII `Ø`) against a
/// codeunit declared `"Løbenr Mgt."` (title case, `ø`) and calls `Beregn()`
/// (same-case, isolating this axis from axis 2 below). This must resolve via
/// `Evidence::Source` to `Løbenr Mgt.::Beregn` — it would NOT under
/// ASCII-only folding (see module doc above).
#[test]
fn ws_unicode_fold_cross_case_object_name_resolves_to_source() {
    let report = ws_unicode_fold_report();
    let edges = edges_for_caller(&report, "callcrosscaseobjectname");
    assert_eq!(
        edges.len(),
        1,
        "CallCrossCaseObjectName must have exactly one call obligation"
    );
    let routes = &edges[0].edge.routes;
    assert_eq!(routes.len(), 1, "single-dispatch call site");
    let route = &routes[0];

    assert_eq!(
        route.evidence,
        Evidence::Source,
        "Codeunit \"LØBENR MGT.\" must resolve to the same object as declared \
         \"Løbenr Mgt.\" under a Unicode-aware fold (Ø/ø are the SAME letter, \
         different case only) — got {route:?}"
    );
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected RouteTarget::Routine, got {:?}", route.target);
    };
    assert_eq!(rid.name_lc, "beregn");
    assert_eq!(rid.object.kind, ObjectKind::Codeunit);
}

/// Axis 2 — MEMBER (routine) NAME fold: `CallCrossCaseMemberName` declares
/// its receiver `Codeunit "Løbenr Mgt."` (same-case, isolating this axis
/// from axis 1 above) and calls `PRÜFUNG()` (all-caps, non-ASCII `Ü`)
/// against a procedure declared `Prüfung` (`ü`). This must resolve via
/// `Evidence::Source` to `Løbenr Mgt.::Prüfung` (`name_lc` folds to
/// `"prüfung"`, NOT the two-char `str::to_lowercase()`-style form) — it
/// would NOT under ASCII-only folding (see module doc above).
#[test]
fn ws_unicode_fold_cross_case_member_name_resolves_to_source() {
    let report = ws_unicode_fold_report();
    let edges = edges_for_caller(&report, "callcrosscasemembername");
    assert_eq!(
        edges.len(),
        1,
        "CallCrossCaseMemberName must have exactly one call obligation"
    );
    let routes = &edges[0].edge.routes;
    assert_eq!(routes.len(), 1, "single-dispatch call site");
    let route = &routes[0];

    assert_eq!(
        route.evidence,
        Evidence::Source,
        "PRÜFUNG() must resolve to the same routine as declared Prüfung under \
         a Unicode-aware fold (Ü/ü are the SAME letter, different case only) \
         — got {route:?}"
    );
    let RouteTarget::Routine(ref rid) = route.target else {
        panic!("expected RouteTarget::Routine, got {:?}", route.target);
    };
    assert_eq!(rid.name_lc, "prüfung");
    assert_eq!(rid.object.kind, ObjectKind::Codeunit);
}
