//! R1a EXIT GATE — native receiver soundness oracles over the Rust L2 walker.
//!
//! These are the **ground-truth-free, metamorphic** receiver-classification
//! oracles ported from al-sem's TS soundness suite, run NATIVELY against the Rust
//! L2 body walker (spec §6 — soundness is NOT transitive; the dump is a lossy
//! subset + finite corpus, so the metamorphic variants must drive the Rust engine
//! directly). They catch receiver-classification bugs the finite `ws-*` corpus
//! misses.
//!
//! ## What the TS oracles assert, and how it maps to L2
//!
//! The TS oracles (`test/soundness/metamorphic-receiver.test.ts`,
//! `test/soundness/receiver-genus-matrix.test.ts`) run the FULL pipeline
//! (analyze → snapshot → digest → prove) and assert at the EFFECT level
//! (`DB_INSERT`/`COMMIT` direct vs transitive, `unresolved[]`). That effect-level
//! behavior is DOWNSTREAM of, and entirely determined by, the L2
//! **record-op vs call-site** classification decision in
//! `src/engine/l2/{body_walk,classify,record_op}.rs`:
//!
//!   - A member/implicit call `Receiver.Op()` where `Op` is a record built-in
//!     becomes a `RecordOperation` (→ a direct DB effect downstream) IFF the
//!     receiver classifies as Record-typed (per `classify_receiver`).
//!   - Any other receiver genus (Codeunit / Page / other-type / unknown /
//!     compound) becomes a `CallSite` (→ a call edge the digest traverses; a
//!     direct DB effect is NEVER fabricated).
//!
//! So at the L2 boundary the faithful, non-weakened invariant is:
//!   * metamorphic-receiver: syntactic variants of the SAME underlying record
//!     operation (`with R do Op()` ≡ `R.Op()`; trigger bare `Op` ≡ `Rec.Op()`)
//!     yield the SAME classification (record-op) AND the same `RecordOpType`,
//!     with NO spurious record-op-shaped call-site, for both normal and temporary
//!     records.
//!   * receiver-genus-matrix: `Receiver.Op()` classifies as a record-op ONLY when
//!     the receiver is Record-typed; Codeunit/Page-object/other-type/compound
//!     receivers classify as call-sites and NEVER as a direct record-op.
//!
//! ## Covered vs deferred (honest scoping)
//!
//! COVERED here (the receiver-classification core, R1a's guard per spec §6):
//!   - metamorphic equivalence: `with R do Op` ≡ `R.Op`, trigger bare `Op` ≡
//!     `Rec.Op`, temp `with R do Op` ≡ temp `R.Op` — Insert/Modify/Delete.
//!   - genus matrix: Record (explicit local, `with`, implicit trigger Rec,
//!     object-global, page SourceTable, tableextension trigger, pageextension
//!     Rec), Codeunit facade, other-type (`List`), compound `Factory().Op()`.
//!
//! DEFERRED (NOT receiver-classification; later gates / not L2-observable):
//!   - The TS oracles' `provenance: direct vs transitive`, `may-commit`/
//!     `commits-on-success-path` PROVE answers, and `resourceId`/`resourceDisplay`
//!     table-identity assertions depend on L3 resolve (call-graph + record-type
//!     unification) + L4 summaries + digest/prove. Those are R1b+/R2 surfaces and
//!     are intentionally out of R1a. At L2 we assert the upstream invariant they
//!     all rest on (the record-op vs call-site split), which is the thing a
//!     receiver-classification bug would break. The `tableId`/`resourceId` fields
//!     are STRUCTURALLY ABSENT from the L2 projection (forbidden), so identity is
//!     not L2-checkable by construction.
//!
//! If any case below revealed a Rust/invariant divergence, the fix would live in
//! `src/engine/l2/` (classify / record_op / body_walk). As of this gate every case
//! passes with no `src/engine/l2/**` change required.

use al_sem::program::body::features::PFeatures;
use al_sem::program::body::features_for_named_routine;

const APP_GUID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
const MODEL_INSTANCE_ID: &str = "r0";
const SOURCE_UNIT_ID: &str = "ws:src/vec.al";

/// The canonical probe table reused across fixtures (mirrors `PROBE_TABLE`).
const PROBE_TABLE: &str = r#"table 50100 "Probe Rec" { fields { field(1; "No."; Integer){} field(2; "Name"; Text[50]){} } keys { key(PK; "No."){ Clustered = true; } } }"#;

/// Run the Rust L2 walk over an inline single-file workspace and return the
/// projected features for `routine` (panics if the routine isn't found — a
/// missing routine is itself an oracle failure).
fn features(source: &str, routine: &str) -> PFeatures {
    features_for_named_routine(source, routine, APP_GUID, MODEL_INSTANCE_ID, SOURCE_UNIT_ID)
        .unwrap_or_else(|| panic!("routine `{routine}` not found by the Rust L2 walker"))
}

/// The L2-observable receiver classification of a single record built-in call.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Classification {
    /// Emitted as a RecordOperation with this RecordOpType.
    RecordOp(String),
    /// Emitted as a CallSite (never a direct record-op).
    CallSite,
}

/// Reduce a routine's features to the classification of its SINGLE record-op-named
/// call. The oracle fixtures each contain exactly one such operation, so we assert
/// it landed in exactly one bucket. `record_op_method_lc` is the lowercased method
/// name under test (`insert`/`modify`/`delete`) used to find the matching callsite.
fn classify_single(f: &PFeatures, record_op_method_lc: &str) -> Classification {
    // A record-op classification: exactly one RecordOperation, and no call-site
    // that is the same record-op-named method (no spurious duplicate).
    let record_ops = &f.record_operations;
    let method_lc = record_op_method_lc.to_lowercase();

    // Call-sites whose invoked method is the record-op under test.
    let op_callsites: Vec<&_> = f
        .call_sites
        .iter()
        .filter(|cs| callsite_method_lc(cs).as_deref() == Some(method_lc.as_str()))
        .collect();

    match (record_ops.len(), op_callsites.len()) {
        (1, 0) => Classification::RecordOp(record_ops[0].op.clone()),
        (0, n) if n >= 1 => Classification::CallSite,
        (ro, cs) => panic!(
            "ambiguous classification for `{record_op_method_lc}`: \
             {ro} record-op(s) AND {cs} matching call-site(s) \
             — a receiver-classification bug (spurious duplicate)"
        ),
    }
}

/// Lowercased invoked method name of a call-site (member method, or bare name).
fn callsite_method_lc(cs: &al_sem::program::body::features::PCallSite) -> Option<String> {
    use al_sem::program::body::features::PCallee;
    match &cs.callee {
        PCallee::Member { method, .. } => Some(method.to_lowercase()),
        PCallee::Bare { name } => Some(name.to_lowercase()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// metamorphic-receiver: receiver-equivalent syntactic variants → same class
// ---------------------------------------------------------------------------

/// `with R do Op()` ≡ `R.Op()` — same record-op classification + RecordOpType,
/// for Insert/Modify/Delete. (Ported from metamorphic-receiver.test.ts
/// "with R do Op() === R.Op()".)
#[test]
fn metamorphic_with_do_equals_member() {
    for (op_method, op_call) in [
        ("Insert", "Insert()"),
        ("Modify", "Modify()"),
        ("Delete", "Delete()"),
    ] {
        let method_lc = op_method.to_lowercase();
        let sugar_src = format!(
            "{PROBE_TABLE}\ncodeunit 50120 C {{ procedure P() var R: Record \"Probe Rec\"; begin with R do {op_call} end; }}"
        );
        let plain_src = format!(
            "{PROBE_TABLE}\ncodeunit 50121 C {{ procedure P() var R: Record \"Probe Rec\"; begin R.{op_call} end; }}"
        );
        let sugar = classify_single(&features(&sugar_src, "P"), &method_lc);
        let plain = classify_single(&features(&plain_src, "P"), &method_lc);
        assert_eq!(
            sugar,
            Classification::RecordOp(op_method.to_string()),
            "`with R do {op_call}` must classify as a {op_method} record-op"
        );
        assert_eq!(
            sugar, plain,
            "metamorphic divergence: `with R do {op_call}` != `R.{op_call}`"
        );
    }
}

/// trigger bare `Op` ≡ `Rec.Op()` — implicit trigger-Rec receiver equals explicit
/// `Rec`. (Ported from metamorphic-receiver.test.ts "trigger bare Op === Rec.Op",
/// generalized to Insert/Modify/Delete.)
#[test]
fn metamorphic_trigger_bare_equals_rec_member() {
    for (op_method, trigger) in [
        ("Insert", "OnInsert"),
        ("Modify", "OnModify"),
        ("Delete", "OnDelete"),
    ] {
        let method_lc = op_method.to_lowercase();
        let bare_src = format!(
            "table 50100 \"Probe Rec\" {{ fields {{ field(1; \"No.\"; Integer){{}} }} keys {{ key(PK; \"No.\"){{Clustered=true;}} }} trigger {trigger}() begin {op_method}; end; }}"
        );
        let explicit_src = format!(
            "table 50100 \"Probe Rec\" {{ fields {{ field(1; \"No.\"; Integer){{}} }} keys {{ key(PK; \"No.\"){{Clustered=true;}} }} trigger {trigger}() begin Rec.{op_method}(); end; }}"
        );
        let bare = classify_single(&features(&bare_src, trigger), &method_lc);
        let explicit = classify_single(&features(&explicit_src, trigger), &method_lc);
        assert_eq!(
            bare,
            Classification::RecordOp(op_method.to_string()),
            "trigger bare `{op_method}` must classify as a {op_method} record-op"
        );
        assert_eq!(
            bare, explicit,
            "metamorphic divergence: trigger bare `{op_method}` != `Rec.{op_method}()`"
        );
    }
}

/// temp `with R do Op()` ≡ temp `R.Op()` — temporary records classify identically
/// to normal records at L2 (the receiver is still Record-typed). (Ported from
/// receiver-genus-matrix.test.ts "with TempRec do Insert() parity".)
#[test]
fn metamorphic_temp_with_do_equals_member() {
    let sugar_src = format!(
        "{PROBE_TABLE}\ncodeunit 50114 C {{ procedure P() var R: Record \"Probe Rec\" temporary; begin with R do Insert(); end; }}"
    );
    let plain_src = format!(
        "{PROBE_TABLE}\ncodeunit 50115 C {{ procedure P() var R: Record \"Probe Rec\" temporary; begin R.Insert(); end; }}"
    );
    let sugar = classify_single(&features(&sugar_src, "P"), "insert");
    let plain = classify_single(&features(&plain_src, "P"), "insert");
    assert_eq!(sugar, Classification::RecordOp("Insert".to_string()));
    assert_eq!(
        sugar, plain,
        "metamorphic divergence: temp `with R do Insert()` != temp `R.Insert()`"
    );
}

// ---------------------------------------------------------------------------
// receiver-genus-matrix: record-op IFF receiver is Record-typed
// ---------------------------------------------------------------------------

/// Explicit Record receiver (the control) → record-op present.
/// (receiver-genus-matrix "explicit record receiver".)
#[test]
fn genus_explicit_record_receiver_is_record_op() {
    let src = format!(
        "{PROBE_TABLE}\ncodeunit 50110 C {{ procedure P() var R: Record \"Probe Rec\"; begin R.Insert(); end; }}"
    );
    assert_eq!(
        classify_single(&features(&src, "P"), "insert"),
        Classification::RecordOp("Insert".to_string())
    );
}

/// Codeunit facade receiver (`W: Codeunit`) → call-site, NOT a direct record-op.
/// This is THE soundness case: a codeunit procedure named after a record built-in
/// must not fabricate a DB effect. (receiver-genus-matrix "codeunit facade".)
#[test]
fn genus_codeunit_facade_is_callsite_not_record_op() {
    let src = format!(
        "{PROBE_TABLE}\ncodeunit 50101 \"Probe Worker\" {{ procedure Insert() begin end; }}\ncodeunit 50111 C {{ procedure P() var W: Codeunit \"Probe Worker\"; begin W.Insert(); end; }}"
    );
    let f = features(&src, "P");
    assert_eq!(
        classify_single(&f, "insert"),
        Classification::CallSite,
        "codeunit facade `W.Insert()` must be a call-site, never a record-op"
    );
    assert!(
        f.record_operations.is_empty(),
        "codeunit facade must produce ZERO record operations"
    );
}

/// Compound receiver `Factory().Insert()` → call-site, never a direct record-op
/// (the receiver is not a simple identifier → unknown genus).
/// (receiver-genus-matrix "compound receiver Factory().Insert()".)
#[test]
fn genus_compound_receiver_is_callsite_not_record_op() {
    let src = format!(
        "{PROBE_TABLE}\ncodeunit 50112 C {{ procedure Factory(): Record \"Probe Rec\" var R: Record \"Probe Rec\"; begin exit(R); end; procedure P() begin Factory().Insert(); end; }}"
    );
    let f = features(&src, "P");
    assert!(
        f.record_operations.is_empty(),
        "compound `Factory().Insert()` must produce ZERO record operations (unknown receiver)"
    );
}

/// Other-typed receiver (`List of [Integer]`) → call-site, never a record-op.
/// (Extends the genus matrix to the "other" genus that classify_receiver yields
/// for non-record, non-callable-object declared types.)
#[test]
fn genus_other_typed_receiver_is_callsite_not_record_op() {
    let src = format!(
        "{PROBE_TABLE}\ncodeunit 50130 C {{ procedure P() var L: List of [Integer]; begin L.Insert(); end; }}"
    );
    let f = features(&src, "P");
    assert!(
        f.record_operations.is_empty(),
        "other-typed `L.Insert()` (List) must produce ZERO record operations"
    );
}

/// Table-trigger implicit Rec write → record-op (implicit Rec is Record-typed).
/// (receiver-genus-matrix "table trigger implicit Rec op".)
#[test]
fn genus_table_trigger_implicit_rec_is_record_op() {
    let src = r#"table 50100 "Probe Rec" { fields { field(1; "No."; Integer){} field(2; "Name"; Text[50]){} } keys { key(PK; "No."){ Clustered = true; } } trigger OnModify() begin "Name" := 'x'; Modify; end; }"#;
    assert_eq!(
        classify_single(&features(src, "OnModify"), "modify"),
        Classification::RecordOp("Modify".to_string())
    );
}

/// Object-global record write `gRec.Insert()` → record-op (global is Record-typed).
/// (receiver-genus-matrix "object-global record write".)
#[test]
fn genus_object_global_record_write_is_record_op() {
    let src = format!(
        "{PROBE_TABLE}\ncodeunit 50120 C {{ var gRec: Record \"Probe Rec\"; procedure P() begin gRec.Insert(); end; }}"
    );
    assert_eq!(
        classify_single(&features(&src, "P"), "insert"),
        Classification::RecordOp("Insert".to_string())
    );
}

/// Page SourceTable implicit `Rec.Modify()` → record-op (page Rec is Record-typed).
/// (receiver-genus-matrix "page SourceTable implicit Rec write".)
#[test]
fn genus_page_source_table_rec_write_is_record_op() {
    let src = format!(
        "{PROBE_TABLE}\npage 50140 \"P\" {{ PageType = List; SourceTable = \"Probe Rec\"; procedure DoIt() begin Rec.Modify(); end; }}"
    );
    assert_eq!(
        classify_single(&features(&src, "DoIt"), "modify"),
        Classification::RecordOp("Modify".to_string())
    );
}

/// Tableextension trigger implicit Rec write → record-op.
/// (receiver-genus-matrix "tableextension trigger implicit Rec write".)
#[test]
fn genus_tableextension_trigger_rec_write_is_record_op() {
    let src = format!(
        "{PROBE_TABLE}\ntableextension 50160 \"Ext60\" extends \"Probe Rec\" {{ fields {{ field(50; \"Extra\"; Integer){{}} }} trigger OnAfterModify() begin Modify; end; }}"
    );
    assert_eq!(
        classify_single(&features(&src, "OnAfterModify"), "modify"),
        Classification::RecordOp("Modify".to_string())
    );
}

/// Pageextension `Rec.Modify()` → record-op (extends a page whose Rec is
/// Record-typed). (receiver-genus-matrix "pageextension Rec write".)
#[test]
fn genus_pageextension_rec_write_is_record_op() {
    let src = format!(
        "{PROBE_TABLE}\npage 50161 \"Base P61\" {{ PageType = List; SourceTable = \"Probe Rec\"; }}\npageextension 50162 \"PExt62\" extends \"Base P61\" {{ procedure DoIt() begin Rec.Modify(); end; }}"
    );
    assert_eq!(
        classify_single(&features(&src, "DoIt"), "modify"),
        Classification::RecordOp("Modify".to_string())
    );
}
