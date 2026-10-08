//! R1d EXIT GATE (= R1 COMPLETE) — native L2-DIRECT capability-fact invariant oracle.
//!
//! These are ground-truth-free, structural invariant oracles run NATIVELY against
//! the Rust L2 capability extraction (`src/engine/l2/capability/**`, the port of
//! al-sem's 13 `extractCapabilities` family extractors run intraprocedurally on a
//! PRE-RESOLVE routine). They assert the L2-DIRECT capability CONTRACT DIRECTLY on
//! each emitted `CapabilityFact` — NOT a golden diff against expected JSON (that is
//! `l2cap_vectors.rs`, byte-parity with al-sem), and NOT the downstream L4 effect
//! summaries / digest / prove that the TS soundness oracles add (see "Covered vs
//! deferred" below).
//!
//! ## Why an L2-DIRECT oracle (not a golden / not the L4 effect oracle)
//!
//! The `l2cap_vectors.rs` test pins the EXACT extractor output against committed
//! al-sem-generated vectors (the corpus differential adds 152/152 byte parity).
//! That guards "Rust == al-sem". This file guards a complementary, ground-truth-free
//! property: "the L2-direct capability SURFACE is well-formed and sound by
//! construction" — every fact is direct/self, never carries an L3-resolved id
//! (top-level OR nested), publish is never produced at L2, and NO fact ever comes
//! from unreachable code (the soundness core: effects never originate in code that
//! cannot run). These invariants hold independent of any specific expected string,
//! so they catch a whole CLASS of regressions a golden would miss (e.g. a family
//! that starts leaking a `resourceId`, or one that stops honoring the unreachable
//! filter) — and, because the differential is byte-parity with al-sem, a STRUCTURAL
//! failure here would mean BOTH engines are wrong, which the comments flag loudly.
//!
//! Each invariant is a focused `#[test]` over a small inline AL fixture, driven
//! through the real projector via [`project_named_routine`] + [`extract_capabilities`]
//! (the same entry points the emitter + `l2cap_vectors.rs` use). The fixtures are
//! independent of the finite `ws-*` corpus, so they catch capability bugs the
//! corpus misses.
//!
//! ## Covered (the L2 direct-capability contract, R1d's guard)
//!   - every direct fact has `provenance == "direct"` AND `via == "self"`
//!     (the L2 surface is intraprocedural — inherited/cone facts are L4);
//!   - NO fact carries a forbidden `resourceId`/`tableId` anywhere — top-level OR
//!     nested inside any `ValueSource` (resourceArgSource / the extra arg-sources /
//!     a `constant-var` initializer chain) — verified by a RECURSIVE key scan over
//!     the serialized JSON (the strip is structural: those keys are not even
//!     declared on the serde types, so the scan proves the projection cannot mint
//!     them);
//!   - NO `op:"publish"` fact exists at L2 — a publisher routine (`[IntegrationEvent]`)
//!     emits ZERO direct facts (publish is L4-injected from the resolved eventGraph);
//!     a subscriber (`[EventSubscriber]`) emits exactly one `subscribe` fact;
//!   - an op/callsite with `controlContext == "unreachable"` produces NO capability
//!     fact + emits an index-stage diagnostic (the soundness core — a `Cust.Modify()`
//!     after a bare `Error()` yields no table fact);
//!   - a table fact's witness op is a record op on a RECORD-typed receiver (the
//!     receiver-genus intuition); a member call on a Codeunit-typed receiver does
//!     NOT yield a table fact;
//!   - SOFTENED confidence: `if confidence == "static" then resourceId is present`.
//!     Since R1d STRIPS `resourceId`, the contrapositive at L2 is "no L2 fact for a
//!     resource-id-bearing kind is static" — asserted concretely: a table fact is
//!     `"unresolved"` at L2 (its id is unresolved pre-resolve).
//!
//! ## Deferred (NOT the L2 direct surface; R2/L4 gates)
//!   - INHERITED / cone capability facts + their provenance (`provenance != "direct"`,
//!     `via != "self"`) — composed over the call graph in L4 summaries.
//!   - The L4-INJECTED `op:"publish"` facts (minted in `summary-runner.ts` from the
//!     RESOLVED `model.eventGraph`) — an R2/L4 surface, EXCLUDED here by construction.
//!   - The L3-resolved `resourceId`/`tableId` + the resolved `confidence` upgrade
//!     (literal/enum → `"static"` once the id binds) — R2 (L3 resolve). At L2 we
//!     assert the id is STRUCTURALLY ABSENT and resource-id-bearing facts stay
//!     unresolved.
//!   - The L4-augmented coverage status/reasons (uncertainty-derived reasons +
//!     complete→partial downgrade from `summary.uncertainties`) — L4. The extractor's
//!     OWN status/reasons (+ the L2 opaque override) are what R1d compares.
//!
//! If any case below revealed a Rust/invariant divergence, the fix would live in
//! `src/engine/l2/capability/**`. As of this gate every case passes with no
//! `src/engine/l2/capability/**` change required.

use al_sem::program::body::capability::{CapabilityFact, extract_capabilities};
use al_sem::program::body::features::PRoutine;
use al_sem::program::body::l2_workspace::project_named_routine;

const APP_GUID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
const SOURCE_UNIT_ID: &str = "ws:src/vec.al";

/// Project a single named routine from inline AL through the real Rust L2 pipeline
/// (features → control-context → operation-order → `extract_capabilities`), exactly
/// as the emitter does. Panics if the routine isn't found — a missing routine is an
/// oracle failure.
fn project(source: &str, routine: &str) -> PRoutine {
    project_named_routine(source, routine, APP_GUID, SOURCE_UNIT_ID)
        .unwrap_or_else(|| panic!("routine `{routine}` not found by the Rust L2 projector"))
}

/// The direct capability facts of a routine (the populated `capabilityFactsDirect`),
/// re-derived from the extractor so the assertions target the live extraction path
/// (identical to `routine.capability_facts_direct`).
fn facts(source: &str, routine: &str) -> Vec<CapabilityFact> {
    let r = project(source, routine);
    extract_capabilities(&r).facts
}

/// Serialize a fact to JSON for the recursive forbidden-key scan.
fn to_json(fact: &CapabilityFact) -> serde_json::Value {
    serde_json::to_value(fact).expect("fact serializes")
}

/// Recursively scan a JSON value for ANY key in `forbidden`. Returns the first path
/// (dotted) at which a forbidden key appears, or `None` if clean. Used to prove no
/// fact — top-level OR nested in any `ValueSource` — leaks an L3-resolved id.
fn find_forbidden_key(value: &serde_json::Value, forbidden: &[&str], path: &str) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                if forbidden.contains(&k.as_str()) {
                    return Some(if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}.{k}")
                    });
                }
                let child_path = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                if let Some(hit) = find_forbidden_key(v, forbidden, &child_path) {
                    return Some(hit);
                }
            }
            None
        }
        serde_json::Value::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                if let Some(hit) = find_forbidden_key(v, forbidden, &format!("{path}[{i}]")) {
                    return Some(hit);
                }
            }
            None
        }
        _ => None,
    }
}

/// Resolve a table fact's witness record op's receiver declared type within the
/// SAME routine: the witness op id → the matching `record_operation` →
/// `record_variable_name` → the variable index → declared type. Returns `None` if
/// the witness can't be resolved (itself a fixture/oracle failure for a table fact).
fn witness_receiver_type(routine: &PRoutine, fact: &CapabilityFact) -> Option<String> {
    let wop = fact.witness_operation_id.as_deref()?;
    let recop = routine
        .features
        .record_operations
        .iter()
        .find(|op| op.id == wop)?;
    let var_lc = recop.record_variable_name.to_lowercase();
    routine
        .features
        .variables
        .iter()
        .find(|v| v.name.to_lowercase() == var_lc)
        .map(|v| v.declared_type.clone())
}

// ===========================================================================
// provenance == "direct" AND via == "self" on EVERY direct fact
// ===========================================================================

/// The L2 surface is intraprocedural: every fact the extractors emit is a DIRECT
/// self fact. Inherited/cone facts (`provenance != "direct"`) are an L4 surface and
/// must never appear here. Driven over a routine exercising MANY families at once
/// (table read/modify, commit, dispatch, http, error) so the invariant is checked
/// across resourceKinds in one shot.
#[test]
fn every_fact_is_direct_and_self() {
    let src = "codeunit 50100 A\n{\n    procedure P(var Cust: Record Customer; Client: HttpClient; Resp: HttpResponseMessage)\n    begin\n        Cust.FindSet();\n        Cust.Modify();\n        Commit();\n        Client.Get('https://x', Resp);\n        Page.RunModal(42);\n        Error('boom');\n    end;\n}";
    let fs = facts(src, "P");
    assert!(
        !fs.is_empty(),
        "fixture sanity: expected several direct facts, got none"
    );
    for f in &fs {
        assert_eq!(
            f.provenance, "direct",
            "L2 fact must be provenance=direct, got {:?} (op={})",
            f.provenance, f.op
        );
        assert_eq!(
            f.via, "self",
            "L2 fact must be via=self, got {:?} (op={})",
            f.via, f.op
        );
    }
}

// ===========================================================================
// NO forbidden resourceId/tableId — top-level OR nested (recursive scan)
// ===========================================================================

/// NO fact may carry `resourceId` or `tableId` ANYWHERE — top-level, nested in a
/// `resourceArgSource`/extra arg-source `ValueSource`, or deep in a `constant-var`
/// initializer chain. These are L3-resolved (R2). The scan is recursive over the
/// serialized JSON; a single hit is a hard failure. Exercised over facts that DO
/// carry nested `ValueSource`s (an http body arg + a storage key/value arg) so the
/// recursive arm is actually traversed, plus the table family (whose witness op
/// would carry a `tableId` if the strip regressed).
#[test]
fn no_fact_carries_forbidden_resource_or_table_id() {
    const FORBIDDEN: &[&str] = &["resourceId", "tableId"];

    // A routine spanning table + http(body) + isolated-storage(key/value) +
    // a constant-var initializer feeding the http body — the cases whose
    // ValueSources nest the deepest.
    let src = "codeunit 50100 A\n{\n    procedure P(var Cust: Record Customer; Client: HttpClient; Content: HttpContent; Resp: HttpResponseMessage)\n    var\n        Payload: Text;\n        StoreVal: Text;\n    begin\n        Payload := 'hello';\n        Cust.FindSet();\n        Cust.Modify();\n        Content.WriteFrom(Payload);\n        Client.Post('https://x', Content, Resp);\n        IsolatedStorage.Set('mykey', StoreVal, DataScope::Company);\n    end;\n}";
    let fs = facts(src, "P");
    assert!(
        fs.len() >= 2,
        "fixture sanity: expected several facts (table + io), got {}",
        fs.len()
    );
    for f in &fs {
        let json = to_json(f);
        if let Some(hit) = find_forbidden_key(&json, FORBIDDEN, "") {
            panic!(
                "L2 capability fact (op={}, kind={}) LEAKS a forbidden L3 id at `{}` — \
                 this is structurally impossible if the serde projection is correct, so a \
                 hit means BOTH engines are wrong (the differential is byte-parity). \
                 Fix src/engine/l2/capability/**. Fact JSON: {}",
                f.op, f.resource_kind, hit, json
            );
        }
    }
}

// ===========================================================================
// NO op:"publish" at L2 (publisher = zero facts; subscriber = one subscribe)
// ===========================================================================

/// A publisher routine (`[IntegrationEvent]`) emits ZERO direct capability facts —
/// `op:"publish"` is L4-INJECTED from the RESOLVED eventGraph (`summary-runner.ts`),
/// never produced by the L2 extractor. `extractEvents` at L2 emits SUBSCRIBE only.
#[test]
fn publisher_emits_no_facts_and_no_publish_op() {
    let src = "codeunit 50100 A\n{\n    [IntegrationEvent(false, false)]\n    procedure OnFooHappened(var Cust: Record Customer)\n    begin\n    end;\n}";
    let fs = facts(src, "OnFooHappened");
    assert!(
        fs.is_empty(),
        "an [IntegrationEvent] publisher must emit ZERO direct facts at L2 (publish is \
         L4-injected), got {:?}",
        fs.iter().map(|f| f.op.as_str()).collect::<Vec<_>>()
    );
}

/// No fact ANYWHERE in the L2 surface carries `op:"publish"`. Asserted over a
/// routine that mixes a subscriber attribute with body effects — the subscriber
/// yields exactly one `subscribe` fact, and `publish` never appears.
#[test]
fn no_publish_op_subscriber_yields_subscribe() {
    let src = "codeunit 50100 A\n{\n    [EventSubscriber(ObjectType::Codeunit, Codeunit::\"Some Cdu\", 'OnFooHappened', '', false, false)]\n    procedure HandleFoo(var Cust: Record Customer)\n    begin\n        Cust.Modify();\n    end;\n}";
    let fs = facts(src, "HandleFoo");
    for f in &fs {
        assert_ne!(
            f.op, "publish",
            "NO L2 fact may be op=publish (publish is L4-injected), found one"
        );
    }
    let subscribe_count = fs.iter().filter(|f| f.op == "subscribe").count();
    assert_eq!(
        subscribe_count,
        1,
        "a subscriber routine must emit exactly one `subscribe` fact, got {} (facts: {:?})",
        subscribe_count,
        fs.iter().map(|f| f.op.as_str()).collect::<Vec<_>>()
    );
}

// ===========================================================================
// unreachable code produces NO fact (+ emits an index diagnostic) — soundness core
// ===========================================================================

/// The soundness core: effects NEVER come from unreachable code. A `Cust.Modify()`
/// after a bare unconditional `Error()` is `controlContext == "unreachable"` (R1b)
/// → the unreachable filter drops it → NO table fact, and an index-stage diagnostic
/// is emitted for the excluded site. (The bare `Error()` itself is reachable at
/// top-level, so its `ui-error`/`error-throw` facts DO appear — we assert only that
/// the post-Error record op contributes nothing.)
#[test]
fn unreachable_record_op_yields_no_table_fact_and_a_diagnostic() {
    let src = "codeunit 50100 A\n{\n    procedure P(var Cust: Record Customer)\n    begin\n        Error('always');\n        Cust.Modify();\n    end;\n}";
    let r = project(src, "P");
    let result = extract_capabilities(&r);

    // No table fact (the only record op is the unreachable Modify).
    let table_facts = result
        .facts
        .iter()
        .filter(|f| f.resource_kind == "table")
        .count();
    assert_eq!(
        table_facts, 0,
        "the unreachable Cust.Modify() must produce NO table fact (effects never come from \
         unreachable code), got {} table fact(s)",
        table_facts
    );

    // The excluded site emits exactly one index-stage info diagnostic.
    assert!(
        result.diagnostics.iter().any(|d| d.severity == "info"
            && d.stage == "index"
            && d.message.contains("unreachable code")),
        "the unreachable filter must emit an index-stage `unreachable code` diagnostic, got {:?}",
        result.diagnostics
    );
}

/// Stronger reachability discrimination: the SAME record op (`Cust.Modify()`) yields
/// a table fact when reachable, and yields NONE when made unreachable by a preceding
/// bare `Error()`. Pins the filter to controlContext, not to the op's mere presence.
#[test]
fn same_record_op_facts_iff_reachable() {
    let reachable = "codeunit 50100 A\n{\n    procedure P(var Cust: Record Customer)\n    begin\n        Cust.Modify();\n    end;\n}";
    let unreachable = "codeunit 50100 A\n{\n    procedure P(var Cust: Record Customer)\n    begin\n        Error('boom');\n        Cust.Modify();\n    end;\n}";

    let reachable_table = facts(reachable, "P")
        .iter()
        .filter(|f| f.resource_kind == "table")
        .count();
    let unreachable_table = facts(unreachable, "P")
        .iter()
        .filter(|f| f.resource_kind == "table")
        .count();

    assert_eq!(
        reachable_table, 1,
        "a reachable Cust.Modify() must yield exactly one table fact"
    );
    assert_eq!(
        unreachable_table, 0,
        "the same Cust.Modify() made unreachable must yield NO table fact"
    );
}

// ===========================================================================
// a table fact's witness is a record op on a Record-typed receiver
// ===========================================================================

/// Every table fact's witness operation is a record op whose receiver variable is
/// declared as a `Record` (the receiver-genus intuition). This is the positive half
/// of the genus discrimination.
#[test]
fn table_fact_witness_is_record_op_on_record_receiver() {
    let src = "codeunit 50100 A\n{\n    procedure P(var Cust: Record Customer)\n    begin\n        Cust.FindSet();\n        Cust.Modify();\n    end;\n}";
    let r = project(src, "P");
    let fs = extract_capabilities(&r).facts;
    let table_facts: Vec<&CapabilityFact> =
        fs.iter().filter(|f| f.resource_kind == "table").collect();
    assert!(
        !table_facts.is_empty(),
        "fixture sanity: expected table facts for FindSet/Modify"
    );
    for f in &table_facts {
        assert!(
            f.witness_operation_id.is_some(),
            "a table fact must carry a witness OPERATION id (a record op), got {:?}",
            f
        );
        let recv = witness_receiver_type(&r, f).unwrap_or_else(|| {
            panic!(
                "table fact witness op `{:?}` must resolve to a record op + a declared receiver \
                 type",
                f.witness_operation_id
            )
        });
        assert!(
            recv.to_lowercase().starts_with("record"),
            "a table fact's witness receiver must be Record-typed, got declared type {:?}",
            recv
        );
    }
}

/// The negative half: a member call on a CODEUNIT-typed receiver (`Mgr.Modify()`
/// where `Mgr: Codeunit ...`) is NOT a record op — it yields NO table fact. This is
/// the receiver-genus discrimination: `Modify` is only a table effect on a Record.
#[test]
fn member_call_on_codeunit_receiver_yields_no_table_fact() {
    let src = "codeunit 50100 A\n{\n    procedure P(Mgr: Codeunit \"Some Mgr\")\n    begin\n        Mgr.Modify();\n    end;\n}";
    let fs = facts(src, "P");
    let table_facts = fs.iter().filter(|f| f.resource_kind == "table").count();
    assert_eq!(
        table_facts,
        0,
        "a `Modify()` on a Codeunit-typed receiver is NOT a record op — it must yield NO table \
         fact, got {} (facts: {:?})",
        table_facts,
        fs.iter().map(|f| f.op.as_str()).collect::<Vec<_>>()
    );
}

// ===========================================================================
// SOFTENED confidence: confidence=="static" ⇒ resourceId present
// ===========================================================================

/// SOFTENED confidence invariant (`if confidence == "static" then resourceId is
/// present`). R1d STRIPS `resourceId`, so the L2 contrapositive is: any fact for a
/// RESOURCE-ID-BEARING kind must NOT be `"static"` at L2 (its id is unresolved
/// pre-resolve). Asserted concretely on the table family — a table fact must be
/// `"unresolved"` at L2 (it would only be `"static"` once its tableId resolves in
/// R2). Presence-only kinds (ui/error/commit) legitimately stay `"static"` because
/// they bear no resourceId at all — they are NOT resource-id-bearing, so the
/// softened rule does not constrain them.
#[test]
fn table_facts_are_unresolved_not_static_at_l2() {
    let src = "codeunit 50100 A\n{\n    procedure P(var Cust: Record Customer)\n    begin\n        Cust.FindSet();\n        Cust.Insert();\n        Cust.Delete();\n    end;\n}";
    let fs = facts(src, "P");
    let table_facts: Vec<&CapabilityFact> =
        fs.iter().filter(|f| f.resource_kind == "table").collect();
    assert_eq!(
        table_facts.len(),
        3,
        "fixture sanity: expected read+insert+delete table facts, got {}",
        table_facts.len()
    );
    for f in &table_facts {
        assert_eq!(
            f.confidence, "unresolved",
            "a table fact (resource-id-bearing) must be `unresolved` at L2 — it can only become \
             `static` once its tableId RESOLVES (R2). A `static` table fact here would carry a \
             resourceId, violating the softened confidence rule. Got {:?}",
            f.confidence
        );
        assert_ne!(
            f.confidence, "static",
            "no resource-id-bearing L2 fact may be `static` (resourceId is stripped at L2)"
        );
    }
}
