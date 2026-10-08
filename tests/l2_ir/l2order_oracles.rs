//! R1c EXIT GATE — native L2-DIRECT ordering-never-overclaim invariant oracle.
//!
//! These are ground-truth-free, STRUCTURAL ordering invariants run NATIVELY
//! against the Rust L2 operation-order walker (`src/engine/l2/operation_order.rs`
//! over the validated R1a CFN skeleton + R1b's shared `control_flow.rs`). They
//! assert the `OperationOrder` (`orderId`, `frameId`, `onSuccessPath`,
//! `dominatesSuccessReturn`) + `ScopeFrame[]` invariants DIRECTLY on each
//! `OperationSite`/`CallSite` — NOT a golden diff against expected ints (that is
//! `l2order_vectors.rs`), and NOT the downstream L4 effect-reachability the TS
//! `ordering-never-overclaim` / `ordering-metamorphic` oracles add (see "Covered
//! vs deferred" below).
//!
//! ## Why an L2-DIRECT STRUCTURAL oracle (not the TS L4-effect port)
//!
//! The TS `ordering-never-overclaim*.test.ts` + `ordering-metamorphic*.test.ts`
//! oracles reason over the DOWNSTREAM `src/digest/ordering.ts` happens-before
//! graph (`buildHBEdges`, `dom`, `mayCoExecute`, `orderedBefore`): a
//! `must_all_paths` edge only when `dom` holds, no edge between mutually-exclusive
//! sibling branch frames, no intra-iteration loop edge, etc. Those edges are an L4
//! surface — they consume the `OrderedOp.frameChain`/`orderId`/`onSuccessPath`/
//! `dominatesSuccessReturn` that L2 EMITS but L2 itself never builds the graph.
//! The Rust L2 output stops at the index boundary, so porting `buildHBEdges` here
//! would assert nothing about L2.
//!
//! Instead we assert, at the L2 boundary, the STRUCTURAL ordering facts the HB
//! graph ULTIMATELY RESTS ON. The TS never-overclaim contract is "no
//! `must_all_paths` edge unless `dom`"; `dom` is itself derived from
//! `dominatesSuccessReturn` + the frame chain + `onSuccessPath`. So if L2 emits
//! those four fields soundly — a non-root-frame op NEVER claims
//! `dominatesSuccessReturn`, an error-arm op is NEVER `onSuccessPath`, the frame
//! chain is well-formed and its branch-termination flags match the actual arm
//! termination — the downstream never-overclaim property the TS oracle guards
//! follows by construction. A walker bug (wrong dominance timing, exit-vs-error
//! confusion, broken frame numbering / parent chain, mis-set branch flags) breaks
//! one of the invariants below.
//!
//! Each invariant is a focused `#[test]` over a small inline AL fixture, driven
//! through the real walker via [`analyze_named_routine_order`] (the same entry
//! point the emitter + `l2order_vectors.rs` use, INCLUDING the error-call
//! source-range post-pass). The fixtures are independent of the finite `ws-*`
//! corpus, so they catch ordering bugs the corpus misses.
//!
//! ## Covered (the L2 operation-order structural core, R1c's guard)
//!   - `dominatesSuccessReturn == false` for EVERY order in a non-root frame
//!     (`kind != "block"` OR `parentFrameId != -1`), and for every
//!     conditionLeaf / `error` / `exit` leaf, and for any root op AFTER a
//!     conditional normal-return (`if Cond then exit`, one-arm-exit `if`, `case`
//!     with an exit arm);
//!   - a root-block direct op BEFORE any conditional normal-return MAY dominate
//!     (true) — the only `true` shape the walker emits;
//!   - `onSuccessPath == false` for any order inside a frame with
//!     `branchAlwaysTerminates && branchTerminatesBy == "error"`, and for any
//!     unreachable-after-bare-exit/error site; `onSuccessPath == true` for
//!     exit-arm sites (exit is a normal return) and (usually) a bare top-level
//!     `Error`;
//!   - frame-table well-formedness: every referenced `frameId` resolves to a
//!     `ScopeFrame`; `parentFrameId` chains terminate at root (-1); the root
//!     frame is `kind == "block"`, `parentFrameId == -1`; branch-frame fields
//!     match termination (exit/error → `alwaysTerminates == true` +
//!     `mayFallThrough == false` + `terminatesBy` set; fallthrough →
//!     `alwaysTerminates == false` + `mayFallThrough == true` + no
//!     `terminatesBy`); root/loop/try frames OMIT all three branch fields;
//!   - relative pre-order: condition leaf BEFORE its owning op/call; if-condition
//!     BEFORE then/else; then BEFORE else; case selector BEFORE branches; loop
//!     condition BEFORE body (incl. the `repeat` quirk — condition leaves precede
//!     the body).
//!
//! ## NOT asserted (per the plan — these are VALID at L2, not bugs)
//!   - Global contiguous/unique `orderId` across emitted entries: `exit`/`error`
//!     leaves consume an orderId without projecting, and a leaf with BOTH an
//!     op-id and a callsite-id clones the SAME orderId into both maps, so emitted
//!     orderIds have GAPS + ALIASES. We assert RELATIVE pre-order (`<`), never
//!     density/uniqueness.
//!   - The loop-contained-exit dominance case: the walker treats loops as NOT
//!     normal-return-possible (verbatim from al-sem), so an `exit` inside a loop
//!     does NOT set `normalReturnPossibleBeforeHere` — meaning
//!     `dominatesSuccessReturn` is NOT a full postdominance proof. We do not
//!     assert anything about that case (it is the documented R1c→R1d caveat).
//!
//! ## Deferred (NOT L2 operation-order; later gates / not L2-observable)
//!   - The TS oracles' downstream happens-before EDGES (`buildHBEdges`: a
//!     `must_all_paths` edge only when `dom`; no edge between exclusive sibling
//!     branches; no intra-iteration loop edge; the `may_some_path` vs
//!     `must_all_paths` quantifier split). Those depend on `src/digest/ordering.ts`
//!     consuming the L2 frame chain + order fields, and are an R2 (digest) surface.
//!     At L2 we assert the upstream structural facts they all rest on (this file).
//!
//! If any case below revealed a Rust/invariant divergence, the differential is
//! byte-parity with al-sem — so a STRUCTURAL-invariant failure would mean BOTH
//! engines are wrong (the fix would then live in `src/engine/l2/operation_order.rs`
//! AND al-sem). As of this gate every case passes with no `src/engine/l2/**`
//! change required.

use al_sem::program::body::operation_order::{
    OperationOrder, RoutineOperationOrder, ScopeFrame, analyze_named_routine_order,
};
use std::collections::HashMap;

const APP_GUID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
const MODEL_INSTANCE_ID: &str = "r0";
const SOURCE_UNIT_ID: &str = "ws:src/vec.al";

/// Run the full Rust L2 operation-order analysis over an inline single-file
/// workspace and return the result for `routine` (panics if the routine isn't
/// found — a missing routine is itself an oracle failure).
fn analyze(source: &str, routine: &str) -> RoutineOperationOrder {
    analyze_named_routine_order(source, routine, APP_GUID, MODEL_INSTANCE_ID, SOURCE_UNIT_ID)
        .unwrap_or_else(|| panic!("routine `{routine}` not found by the Rust L2 walker"))
}

/// Frame-id → frame lookup for an analysis.
fn frame_map(a: &RoutineOperationOrder) -> HashMap<i64, &ScopeFrame> {
    a.scope_frames.iter().map(|f| (f.frame_id, f)).collect()
}

/// The `OperationOrder` of the FIRST operation site at the given 1-based source
/// line (optionally filtered by `kind`). Lines read 1-based in the fixture
/// string; anchors are 0-based, so we convert. Returns `None` when the site has
/// no order (absent).
fn op_order_at_line(
    a: &RoutineOperationOrder,
    line_1based: u32,
    kind_filter: Option<&str>,
) -> Option<OperationOrder> {
    let line = line_1based - 1;
    let op = a
        .operation_sites
        .iter()
        .find(|op| {
            op.source_anchor.start_line == line && kind_filter.map(|k| op.kind == k).unwrap_or(true)
        })
        .unwrap_or_else(|| {
            panic!(
                "no operation site (kind={:?}) at line {line}; sites present: {:?}",
                kind_filter,
                a.operation_sites
                    .iter()
                    .map(|o| (o.source_anchor.start_line, o.kind.as_str()))
                    .collect::<Vec<_>>()
            )
        });
    a.by_operation.get(&op.id).copied()
}

/// The `OperationOrder` of the FIRST call site at the given 1-based source line.
fn callsite_order_at_line(a: &RoutineOperationOrder, line_1based: u32) -> Option<OperationOrder> {
    let line = line_1based - 1;
    let cs = a
        .call_sites
        .iter()
        .find(|cs| cs.source_anchor.start_line == line)
        .unwrap_or_else(|| {
            panic!(
                "no call site at line {line}; sites present: {:?}",
                a.call_sites
                    .iter()
                    .map(|c| (c.source_anchor.start_line, c.callee_text.as_str()))
                    .collect::<Vec<_>>()
            )
        });
    a.by_callsite.get(&cs.id).copied()
}

/// Whether a frame is the root block (`kind == "block"`, `parentFrameId == -1`).
fn is_root_frame(f: &ScopeFrame) -> bool {
    f.kind == "block" && f.parent_frame_id == -1
}

/// Walk a frame's parent chain to its terminus. Panics on a dangling parent or a
/// cycle (both are oracle failures). Returns the chain frameIds root-last.
fn parent_chain(start: i64, frames: &HashMap<i64, &ScopeFrame>) -> Vec<i64> {
    let mut chain = Vec::new();
    let mut cur = start;
    let mut guard = 0;
    loop {
        guard += 1;
        assert!(
            guard < 1000,
            "frame parent chain did not terminate (cycle?) from {start}"
        );
        let f = frames.get(&cur).unwrap_or_else(|| {
            panic!("frameId {cur} in parent chain does not resolve to a ScopeFrame")
        });
        chain.push(cur);
        if f.parent_frame_id == -1 {
            break;
        }
        cur = f.parent_frame_id;
    }
    chain
}

// ===========================================================================
// Frame-table well-formedness (asserted on EVERY fixture via this helper)
// ===========================================================================

/// Assert the frame table is well-formed and every referenced frameId resolves;
/// every parent chain terminates at a root (-1); branch-frame fields match
/// termination; root/loop/try frames omit all three branch fields. Called from
/// every test to make the invariant pervasive.
fn assert_frame_table_wellformed(a: &RoutineOperationOrder) {
    let frames = frame_map(a);

    // At most one root frame; if any frame exists, exactly one is the root.
    let roots: Vec<_> = a.scope_frames.iter().filter(|f| is_root_frame(f)).collect();
    if !a.scope_frames.is_empty() {
        assert_eq!(
            roots.len(),
            1,
            "exactly one root frame expected when frames exist, got {:?}",
            a.scope_frames
        );
        assert_eq!(roots[0].frame_id, 0, "root frame must be frameId 0");
    }

    for f in &a.scope_frames {
        // Every parent chain terminates at -1 (root) without dangling/cycling.
        let chain = parent_chain(f.frame_id, &frames);
        let last = *chain.last().unwrap();
        assert!(
            is_root_frame(frames[&last]),
            "frame {} chain must terminate at the root block, ended at {:?}",
            f.frame_id,
            frames[&last]
        );

        match f.kind.as_str() {
            "if-then" | "if-else" | "case-branch" => {
                // Branch frames ALWAYS carry both flags (even when false).
                let always = f.branch_always_terminates.unwrap_or_else(|| {
                    panic!(
                        "branch frame {} must carry branchAlwaysTerminates",
                        f.frame_id
                    )
                });
                let falls = f.branch_may_fall_through.unwrap_or_else(|| {
                    panic!(
                        "branch frame {} must carry branchMayFallThrough",
                        f.frame_id
                    )
                });
                // always-terminates and may-fall-through are exact complements.
                assert_eq!(
                    always, !falls,
                    "branch frame {}: alwaysTerminates and mayFallThrough must be complements (always={always}, falls={falls})",
                    f.frame_id
                );
                if always {
                    let by = f.branch_terminates_by.as_deref().unwrap_or_else(|| {
                        panic!(
                            "always-terminating branch frame {} must carry branchTerminatesBy",
                            f.frame_id
                        )
                    });
                    assert!(
                        by == "exit" || by == "error",
                        "branch frame {} terminatesBy must be exit|error, got {by:?}",
                        f.frame_id
                    );
                } else {
                    assert!(
                        f.branch_terminates_by.is_none(),
                        "fall-through branch frame {} must NOT carry branchTerminatesBy, got {:?}",
                        f.frame_id,
                        f.branch_terminates_by
                    );
                }
            }
            "block" | "loop" | "try" => {
                // Root/loop/try OMIT all three branch fields.
                assert!(
                    f.branch_always_terminates.is_none()
                        && f.branch_terminates_by.is_none()
                        && f.branch_may_fall_through.is_none(),
                    "non-branch frame {} (kind={}) must omit all branch fields, got {:?}",
                    f.frame_id,
                    f.kind,
                    f
                );
            }
            other => panic!("unexpected frame kind {other:?} on frame {}", f.frame_id),
        }
    }

    // Every order referenced by a site resolves to a frame in the table.
    for ord in a.by_operation.values().chain(a.by_callsite.values()) {
        assert!(
            frames.contains_key(&ord.frame_id),
            "order references frameId {} not in the frame table {:?}",
            ord.frame_id,
            a.scope_frames
        );
    }
}

/// Assert that EVERY order whose frame is a non-root frame has
/// `dominatesSuccessReturn == false`. The walker only ever sets the flag for a
/// reachable root-block direct op.
fn assert_no_nonroot_dominance(a: &RoutineOperationOrder) {
    let frames = frame_map(a);
    for ord in a.by_operation.values().chain(a.by_callsite.values()) {
        let f = frames[&ord.frame_id];
        if !is_root_frame(f) && ord.dominates_success_return {
            panic!(
                "order in non-root frame {:?} must NOT dominate the success return: {:?}",
                f, ord
            );
        }
    }
}

// ===========================================================================
// dominatesSuccessReturn — non-root frames never dominate
// ===========================================================================

/// An op directly in a branch / loop / case body NEVER dominates the success
/// return — only a reachable root-block direct op can. Asserted both via the
/// pervasive helper and on the specific branch/loop sites.
#[test]
fn nonroot_frame_op_never_dominates() {
    let src = "codeunit 50100 A\n{\n    procedure P(var Cust: Record Customer; flag: Boolean)\n    begin\n        if flag then\n            Cust.Insert();\n        while Cust.Next() <> 0 do\n            Cust.Modify();\n        case flag of\n            true:\n                Cust.Delete();\n        end;\n    end;\n}";
    let a = analyze(src, "P");
    assert_frame_table_wellformed(&a);
    assert_no_nonroot_dominance(&a);

    // if-then body (line 6).
    let then_op = op_order_at_line(&a, 6, None).expect("then-body op has order");
    assert!(
        !then_op.dominates_success_return,
        "if-then body op must NOT dominate, got {:?}",
        then_op
    );
    // loop body (line 8).
    let loop_op = op_order_at_line(&a, 8, None).expect("loop-body op has order");
    assert!(
        !loop_op.dominates_success_return,
        "loop-body op must NOT dominate, got {:?}",
        loop_op
    );
    // case branch body (line 11).
    let case_op = op_order_at_line(&a, 11, None).expect("case-branch body op has order");
    assert!(
        !case_op.dominates_success_return,
        "case-branch body op must NOT dominate, got {:?}",
        case_op
    );
}

/// Condition leaves and `error` leaves NEVER dominate the success return (they
/// are in expression position / never normal-return). (A bare `exit` produces no
/// projected site, so dominance for the exit leaf itself is asserted structurally
/// via the pervasive `assert_no_nonroot_dominance` — exit's order, when present,
/// is set with `dominates_success_return == false` by the walker.)
#[test]
fn condition_and_error_leaves_never_dominate() {
    let src = "codeunit 50100 A\n{\n    procedure P(var Cust: Record Customer; bad: Boolean)\n    begin\n        if Cust.IsEmpty() then\n            Error('boom');\n    end;\n}";
    let a = analyze(src, "P");
    assert_frame_table_wellformed(&a);
    assert_no_nonroot_dominance(&a);

    // `Cust.IsEmpty()` (line 5) is an if-condition leaf (record op) → never dominates.
    let cond = op_order_at_line(&a, 5, None).expect("condition leaf has order");
    assert!(
        !cond.dominates_success_return,
        "if-condition leaf must NOT dominate, got {:?}",
        cond
    );
    // `Error('boom')` (line 6) — the error leaf (callsite) never dominates.
    let err = callsite_order_at_line(&a, 6).expect("error call has order");
    assert!(
        !err.dominates_success_return,
        "error leaf must NOT dominate, got {:?}",
        err
    );
    // The error-call OP paired by the post-pass also never dominates.
    let err_op = op_order_at_line(&a, 6, Some("error-call")).expect("error-call op has order");
    assert!(
        !err_op.dominates_success_return,
        "error-call op must NOT dominate, got {:?}",
        err_op
    );
}

/// A root-block op AFTER a conditional normal-return (`if Cond then exit`) does
/// NOT dominate; an op BEFORE it MAY dominate (true). This is the
/// `normalReturnPossibleBeforeHere` timing invariant.
#[test]
fn root_op_after_conditional_exit_does_not_dominate_before_does() {
    let src = "codeunit 50100 A\n{\n    procedure P(var Cust: Record Customer; flag: Boolean)\n    begin\n        Cust.Insert();\n        if flag then\n            exit;\n        Cust.Modify();\n    end;\n}";
    let a = analyze(src, "P");
    assert_frame_table_wellformed(&a);
    assert_no_nonroot_dominance(&a);

    // BEFORE the conditional exit (line 5) → MAY dominate.
    let before = op_order_at_line(&a, 5, None).expect("pre-guard op has order");
    assert!(
        before.dominates_success_return,
        "root op BEFORE a conditional exit MAY dominate (expected true), got {:?}",
        before
    );
    // AFTER the conditional exit (line 8) → must NOT dominate.
    let after = op_order_at_line(&a, 8, None).expect("post-guard op has order");
    assert!(
        !after.dominates_success_return,
        "root op AFTER a conditional exit must NOT dominate, got {:?}",
        after
    );
}

/// A one-arm-exit `if` (`if Cond then exit;` with implicit else) is a conditional
/// normal-return: a root op after it does NOT dominate.
#[test]
fn root_op_after_case_exit_arm_does_not_dominate() {
    let src = "codeunit 50100 A\n{\n    procedure P(var Cust: Record Customer; i: Integer)\n    begin\n        Cust.Insert();\n        case i of\n            1:\n                exit;\n            2:\n                Cust.Modify();\n        end;\n        Cust.Delete();\n    end;\n}";
    let a = analyze(src, "P");
    assert_frame_table_wellformed(&a);
    assert_no_nonroot_dominance(&a);

    // BEFORE the case (line 5) → MAY dominate.
    let before = op_order_at_line(&a, 5, None).expect("pre-case op has order");
    assert!(
        before.dominates_success_return,
        "root op BEFORE a case-with-exit-arm MAY dominate, got {:?}",
        before
    );
    // AFTER the case-with-exit-arm (line 12) → must NOT dominate (a case arm can
    // normal-return).
    let after = op_order_at_line(&a, 12, None).expect("post-case op has order");
    assert!(
        !after.dominates_success_return,
        "root op AFTER a case-with-exit-arm must NOT dominate, got {:?}",
        after
    );
}

// ===========================================================================
// onSuccessPath — exit-arm = true, error-arm = false, unreachable = false
// ===========================================================================

/// Any order inside a frame with `branchAlwaysTerminates && terminatesBy=="error"`
/// is NOT on the success path. An exit-arm site IS on the success path (exit is a
/// normal return). This is the `term != error` (NOT `!terminates`) rule.
#[test]
fn error_arm_not_on_success_exit_arm_is() {
    // The exit arm carries a record op BEFORE the exit so we can observe its
    // onSuccessPath (a bare `exit` itself projects no site).
    let src = "codeunit 50100 A\n{\n    procedure P(var Cust: Record Customer; bad: Boolean; done: Boolean)\n    begin\n        if bad then\n            Error('boom');\n        if done then begin\n            Cust.Insert();\n            exit;\n        end;\n    end;\n}";
    let a = analyze(src, "P");
    assert_frame_table_wellformed(&a);

    let frames = frame_map(&a);

    // Pervasive: every order inside an error-terminating frame is NOT onSuccessPath.
    for ord in a.by_operation.values().chain(a.by_callsite.values()) {
        let f = frames[&ord.frame_id];
        if f.branch_always_terminates == Some(true)
            && f.branch_terminates_by.as_deref() == Some("error")
        {
            assert!(
                !ord.on_success_path,
                "order inside an error-terminating frame {:?} must NOT be onSuccessPath: {:?}",
                f, ord
            );
        }
    }

    // The Error() callsite (line 6) is inside the error-only if-then arm → false.
    let err = callsite_order_at_line(&a, 6).expect("error call has order");
    assert!(
        !err.on_success_path,
        "error-arm site must NOT be on the success path, got {:?}",
        err
    );
    // `Cust.Insert()` (line 8) is inside the EXIT arm (terminatesBy == "exit").
    // exit is a NORMAL return → the exit-arm site IS on the success path
    // (the `term != error` rule, NOT `!terminates`).
    let in_exit_arm = op_order_at_line(&a, 8, None).expect("exit-arm op has order");
    assert!(
        in_exit_arm.on_success_path,
        "exit-arm site IS on the success path (exit is a normal return), got {:?}",
        in_exit_arm
    );
    // Sanity: that op's frame really is an exit-terminating branch.
    let exit_frame = frames[&in_exit_arm.frame_id];
    assert_eq!(
        exit_frame.branch_terminates_by.as_deref(),
        Some("exit"),
        "exit-arm op frame must be exit-terminating, got {:?}",
        exit_frame
    );
}

/// Sites after a bare unconditional `exit` / `Error()` are unreachable →
/// `onSuccessPath == false`.
#[test]
fn unreachable_after_bare_terminator_not_on_success() {
    let src = "codeunit 50100 A\n{\n    procedure P(var Cust: Record Customer)\n    begin\n        exit;\n        Cust.Modify();\n        Cust.Insert();\n    end;\n}";
    let a = analyze(src, "P");
    assert_frame_table_wellformed(&a);

    // After the bare exit (lines 6, 7) → unreachable → not on the success path.
    let after1 = op_order_at_line(&a, 6, None).expect("unreachable op has order");
    assert!(
        !after1.on_success_path,
        "op after a bare exit must NOT be on the success path, got {:?}",
        after1
    );
    let after2 = op_order_at_line(&a, 7, None).expect("unreachable op has order");
    assert!(
        !after2.on_success_path,
        "second op after a bare exit must NOT be on the success path, got {:?}",
        after2
    );
}

/// A bare top-level `Error()` follows the AMBIENT onSuccessPath (usually true) —
/// the walker must NOT force all error leaves to false.
#[test]
fn bare_top_level_error_follows_ambient_success() {
    let src =
        "codeunit 50100 A\n{\n    procedure P()\n    begin\n        Error('always');\n    end;\n}";
    let a = analyze(src, "P");
    assert_frame_table_wellformed(&a);

    // The bare top-level Error() callsite (line 5) is at the ambient (root,
    // reachable) success path → true.
    let err = callsite_order_at_line(&a, 5).expect("bare error call has order");
    assert!(
        err.on_success_path,
        "bare top-level Error() must follow the ambient (true) success path, got {:?}",
        err
    );
    // The paired error-call op shares the callsite's order verbatim (post-pass).
    let err_op = op_order_at_line(&a, 5, Some("error-call")).expect("error-call op has order");
    assert_eq!(
        err_op, err,
        "error-call op must COPY the paired callsite's order verbatim, got op={:?} cs={:?}",
        err_op, err
    );
}

// ===========================================================================
// Frame-table well-formedness: branch flags match termination
// ===========================================================================

/// A fall-through branch frame (`if flag then Cust.Modify();`) has
/// `alwaysTerminates == false`, `mayFallThrough == true`, no `terminatesBy`. An
/// exit/error arm has `alwaysTerminates == true`, `mayFallThrough == false`,
/// `terminatesBy` set.
#[test]
fn branch_frame_flags_match_termination() {
    let src = "codeunit 50100 A\n{\n    procedure P(var Cust: Record Customer; flag: Boolean; bad: Boolean)\n    begin\n        if flag then\n            Cust.Modify();\n        if bad then\n            exit;\n        if bad then\n            Error('x');\n    end;\n}";
    let a = analyze(src, "P");
    assert_frame_table_wellformed(&a);

    let then_frames: Vec<&ScopeFrame> = a
        .scope_frames
        .iter()
        .filter(|f| f.kind == "if-then")
        .collect();
    assert_eq!(then_frames.len(), 3, "expected 3 if-then frames");

    // Fall-through arm.
    let ft = then_frames[0];
    assert_eq!(ft.branch_always_terminates, Some(false));
    assert_eq!(ft.branch_may_fall_through, Some(true));
    assert_eq!(ft.branch_terminates_by, None);

    // Exit arm.
    let ex = then_frames[1];
    assert_eq!(ex.branch_always_terminates, Some(true));
    assert_eq!(ex.branch_may_fall_through, Some(false));
    assert_eq!(ex.branch_terminates_by.as_deref(), Some("exit"));

    // Error arm.
    let er = then_frames[2];
    assert_eq!(er.branch_always_terminates, Some(true));
    assert_eq!(er.branch_may_fall_through, Some(false));
    assert_eq!(er.branch_terminates_by.as_deref(), Some("error"));
}

/// Root / loop / try frames OMIT all three branch fields; the root is the unique
/// `kind=="block"`, `parentFrameId==-1` frame, and a loop frame's parent is the
/// root (or an enclosing frame), never -1.
#[test]
fn root_loop_try_frames_omit_branch_fields() {
    let src = "codeunit 50100 A\n{\n    procedure P(var Cust: Record Customer)\n    begin\n        while Cust.Next() <> 0 do\n            Cust.Modify();\n    end;\n}";
    let a = analyze(src, "P");
    assert_frame_table_wellformed(&a); // checks the OMIT invariant pervasively

    let loop_frames: Vec<&ScopeFrame> =
        a.scope_frames.iter().filter(|f| f.kind == "loop").collect();
    assert_eq!(loop_frames.len(), 1, "expected one loop frame");
    let lf = loop_frames[0];
    assert!(
        lf.branch_always_terminates.is_none()
            && lf.branch_terminates_by.is_none()
            && lf.branch_may_fall_through.is_none(),
        "loop frame must omit branch fields, got {:?}",
        lf
    );
    // The loop frame's parent is the root (the loop is at top level here).
    assert_eq!(
        lf.parent_frame_id, 0,
        "loop frame parent must be the root frame"
    );
}

// ===========================================================================
// Relative pre-order (orderId ordering, NOT density/uniqueness)
// ===========================================================================

/// A condition leaf is assigned an orderId BEFORE the op/call it guards, and the
/// if-condition leaf comes BEFORE the then-body, which comes BEFORE the else-body.
#[test]
fn preorder_condition_before_then_before_else() {
    let src = "codeunit 50100 A\n{\n    procedure P(var Cust: Record Customer)\n    begin\n        if Cust.IsEmpty() then\n            Cust.Init()\n        else\n            Cust.Modify();\n    end;\n}";
    let a = analyze(src, "P");
    assert_frame_table_wellformed(&a);

    let cond = op_order_at_line(&a, 5, None).expect("if-condition leaf has order");
    let then_op = op_order_at_line(&a, 6, None).expect("then-body op has order");
    let else_op = op_order_at_line(&a, 8, None).expect("else-body op has order");

    assert!(
        cond.order_id < then_op.order_id,
        "if-condition leaf (order {}) must precede the then-body (order {})",
        cond.order_id,
        then_op.order_id
    );
    assert!(
        then_op.order_id < else_op.order_id,
        "then-body (order {}) must precede the else-body (order {})",
        then_op.order_id,
        else_op.order_id
    );
}

/// The case selector / arm conditions precede each branch body, and branch
/// bodies follow their selector in source order.
#[test]
fn preorder_case_selector_before_branches() {
    let src = "codeunit 50100 A\n{\n    procedure P(var Cust: Record Customer; i: Integer)\n    begin\n        case Cust.Count() of\n            1:\n                Cust.Insert();\n            2:\n                Cust.Modify();\n        end;\n    end;\n}";
    let a = analyze(src, "P");
    assert_frame_table_wellformed(&a);

    // `Cust.Count()` (line 5) is the case selector condition leaf (record op).
    let selector = op_order_at_line(&a, 5, None).expect("case selector leaf has order");
    let arm1 = op_order_at_line(&a, 7, None).expect("case arm 1 body has order");
    let arm2 = op_order_at_line(&a, 9, None).expect("case arm 2 body has order");

    assert!(
        selector.order_id < arm1.order_id,
        "case selector (order {}) must precede arm 1 (order {})",
        selector.order_id,
        arm1.order_id
    );
    assert!(
        arm1.order_id < arm2.order_id,
        "case arm 1 (order {}) must precede arm 2 (order {})",
        arm1.order_id,
        arm2.order_id
    );
}

/// A loop condition leaf precedes the loop body (the `while` header is evaluated
/// before the body).
#[test]
fn preorder_loop_condition_before_body() {
    let src = "codeunit 50100 A\n{\n    procedure P(var Cust: Record Customer)\n    begin\n        while Cust.Next() <> 0 do\n            Cust.Modify();\n    end;\n}";
    let a = analyze(src, "P");
    assert_frame_table_wellformed(&a);

    // `Cust.Next()` (line 5) is the while-condition leaf (record op).
    let cond = op_order_at_line(&a, 5, None).expect("loop condition leaf has order");
    let body = op_order_at_line(&a, 6, None).expect("loop body op has order");
    assert!(
        cond.order_id < body.order_id,
        "loop condition (order {}) must precede the loop body (order {})",
        cond.order_id,
        body.order_id
    );
}

/// The `repeat` quirk: the loop's condition leaves precede the body even though
/// `until` is syntactically at the END. The walker walks the condition leaves
/// first (in the parent frame), then the loop frame + body.
#[test]
fn preorder_repeat_condition_before_body() {
    let src = "codeunit 50100 A\n{\n    procedure P(var Cust: Record Customer)\n    begin\n        repeat\n            Cust.Modify();\n        until Cust.Next() = 0;\n    end;\n}";
    let a = analyze(src, "P");
    assert_frame_table_wellformed(&a);

    // `Cust.Next()` (line 7, the `until` condition) is the repeat-loop condition
    // leaf (record op) — walked BEFORE the body per the repeat quirk → lower
    // orderId than the body op on line 6.
    let cond = op_order_at_line(&a, 7, None).expect("repeat until-condition leaf has order");
    let body = op_order_at_line(&a, 6, None).expect("repeat body op has order");
    assert!(
        cond.order_id < body.order_id,
        "repeat condition (order {}) must precede the body (order {}) [repeat quirk]",
        cond.order_id,
        body.order_id
    );
    // The body lives in a loop frame; the condition leaf is in the parent (root).
    let frames = frame_map(&a);
    assert_eq!(
        frames[&cond.frame_id].kind, "block",
        "repeat condition leaf must be in the parent (root block) frame"
    );
    assert_eq!(
        frames[&body.frame_id].kind, "loop",
        "repeat body op must be in the loop frame"
    );
}

// ===========================================================================
// TryFunction → no orders, no frames
// ===========================================================================

/// A `[TryFunction]` routine yields NO order on any site and an EMPTY frame
/// table.
#[test]
fn tryfunction_no_orders_no_frames() {
    let src = "codeunit 50100 A\n{\n    [TryFunction]\n    procedure TryIt(var Cust: Record Customer)\n    begin\n        Cust.FindSet();\n        if Cust.IsEmpty() then\n            Cust.Modify();\n    end;\n}";
    let a = analyze(src, "TryIt");
    assert!(
        a.scope_frames.is_empty(),
        "TryFunction routine must have an empty frame table, got {:?}",
        a.scope_frames
    );
    assert!(
        !a.operation_sites.is_empty(),
        "fixture sanity: TryFunction routine should still have operation sites"
    );
    for op in &a.operation_sites {
        assert!(
            !a.by_operation.contains_key(&op.id),
            "TryFunction op `{}` @line {} must have ABSENT order",
            op.kind,
            op.source_anchor.start_line
        );
    }
    for cs in &a.call_sites {
        assert!(
            !a.by_callsite.contains_key(&cs.id),
            "TryFunction call site @line {} must have ABSENT order",
            cs.source_anchor.start_line
        );
    }
}
