//! Gap G-18 (docs/engine-gaps.md): d1 reports an op as "in a loop" when, on the
//! REAL call path to that op, it is NOT inside any loop — the loop is attributed
//! from a SIBLING call path.
//!
//! Root cause: two routines COLLIDE on the internal routine id, so their derived
//! call-site ids (`{rid}/cs{n}`) collide too and `edges_by_from[{rid}]` mixes BOTH
//! bodies' edges under one key. d1's root-edge lookup (`find(callsite_id == cs.id)`)
//! can then pick the SIBLING body's edge for THIS body's in-loop call site — walking
//! a chain the loop is not on (the CDO batch-7 `eDocumentsConfigExists` shape).
//!
//! The fix: the picked edge's TARGET must match the call site's own callee name
//! (the resolver resolves by name, so a genuinely-own edge ALWAYS matches — the
//! guard only ever filters cross-body edges under a colliding id). That guard is
//! `l5::detectors::d1::edge_target_matches_callsite_callee`, applied at three call
//! sites — of which exactly ONE affects emitted findings:
//!
//!   - `d1_graph.rs:220` — the PRODUCTION root-edge lookup. **This is what (d)/(e)
//!     below discriminate**: deleting the guard there (alone, or together with the
//!     other two) makes both fail with the G-18 false positive back in the output.
//!   - `d1.rs:1103` — inside `detect_d1_premerge`, the `#[cfg(test)]` shadow oracle.
//!     Its copy of the guard exists to keep the oracle matching production; deleting
//!     it changes no shipped finding and no test in the suite notices (measured).
//!   - `d1.rs:1345` — inside `enumerate_direct_ops`, production but feeding only the
//!     `skipped_opaque_callee` / `skipped_dynamic_dispatch` STAT counters, never a
//!     finding; likewise unobservable from any findings assertion.
//!
//! ## ⟨task-3 fix wave, review I-1⟩ Why the collision is now STATED, not derived
//!
//! When this file was written, two page actions each carrying `trigger OnAction()`
//! were a REAL collision: `compute_routine_id` keyed app/object/kind/name/signature
//! with no member discriminator. Task 3 (`feat(l3)!: conditional enclosing-member
//! discriminator on the internal routine id`) added one, and that exact shape now
//! yields DISTINCT ids — see `l3_workspace::tests::
//! same_name_member_triggers_get_distinct_routine_ids`. The two-page-action tests
//! below therefore stopped exercising the guard entirely: they kept passing with the
//! guard deleted from all three call sites, and they were its only coverage repo-wide.
//!
//! The guard is still load-bearing, and that is measured, not hypothetical: 15
//! collision groups / 19 routines survive on the 8020 corpus — XMLport same-name
//! `fieldelement`s at different nesting paths, and preproc `#if`/`#else`
//! alternatives, neither of which a flat member name can separate. So this file now
//! carries BOTH:
//!
//!   - the derived-shape BEHAVIOUR tests (a)/(b)/(c), which pin that ordinary sibling
//!     page actions produce no false positive and that genuine transitive findings
//!     keep firing — each now asserting its own (post-Task-3, non-colliding)
//!     precondition rather than a stale one; and
//!   - the STATED-collision tests (d)/(e), which force the residual shape by hand and
//!     are the real coverage of the guard. They hold under ANY id schema, forever,
//!     because they never ask `compute_routine_id` for a collision.
//!
//! This mirrors `l5::detector_context::tests::
//! hand_stated_id_collision_keeps_a_real_summary_and_derived_row`, which took the
//! same remedy for the sibling defect one module over.

use al_sem::engine::l3::l3_workspace::L3Resolved;
use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-0000000g18ab";

fn resolve(files: &[(String, String)]) -> L3Resolved {
    assemble_and_resolve_inline_program_default(files, APP_GUID)
}

/// Run d1 in isolation over an already-assembled workspace and return its findings.
/// `run_detectors` rebuilds the symbol table and combined graph from
/// `resolved.workspace.routines` on every call, reading whatever ids are on those
/// routines AT CALL TIME; [`force_id_collision`] rebases the precomputed calls too.
fn run_d1_on(resolved: &L3Resolved) -> Vec<Finding> {
    let d1: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == "d1-db-op-in-loop")
        .collect();
    assert_eq!(d1.len(), 1, "d1 detector must be registered exactly once");
    run_detectors(resolved, &d1).findings
}

/// Run d1 in isolation over an inline workspace and return its emitted findings.
fn run_d1(files: &[(String, String)]) -> Vec<Finding> {
    run_d1_on(&resolve(files))
}

fn al(name: &str, body: &str) -> (String, String) {
    (format!("src/{name}.al"), body.to_string())
}

fn root_causes(findings: &[Finding]) -> Vec<&str> {
    findings.iter().map(|f| f.root_cause.as_str()).collect()
}

/// The internal routine ids of every routine named `name`, sorted.
fn ids_named(resolved: &L3Resolved, name: &str) -> Vec<String> {
    let mut v: Vec<String> = resolved
        .workspace
        .routines
        .iter()
        .filter(|r| r.name.eq_ignore_ascii_case(name))
        .map(|r| r.id.clone())
        .collect();
    v.sort();
    v
}

/// STATE the residual routine-id collision by hand: force every routine named
/// `routine_name` to carry the literal same internal id, and rewrite its call sites'
/// derived ids onto that shared prefix so the two bodies' `{rid}/cs{n}` sequences
/// collide exactly the way a real collision's do (each body numbers its own call
/// sites from 0 — `l2::ir_walk` mints `format!("{routine_id}/cs{n}")` with a
/// per-routine counter — so a prefix rewrite reproduces the real overlap byte-for-byte).
///
/// Returns how many routines were forced, so callers can assert the fixture
/// precondition.
///
/// **What is deliberately NOT rewritten, and why.** A real collision also collides
/// `{rid}/op{n}`, `{rid}/loop{n}` and the other derived ids. Those are irrelevant to
/// this guard and rewriting them would weaken the tests, not strengthen them: d1
/// resolves a call site's loop against `routine.loops` — the OWNING routine's own vec
/// — so colliding loop ids changes no lookup, while rewriting `loops[].id` without
/// also rewriting every `loop_stack` entry that references it would break the
/// `loop_by_id` lookup and make these tests vacuously green. The collision this guard
/// exists for is exactly (shared routine id) × (shared call-site id), which is what
/// makes `edges_by_from[{rid}].find(callsite_id == cs.id)` able to return the sibling
/// body's edge. That is what this states.
///
/// This never asks `compute_routine_id` whether these two routines collide — it makes
/// them collide — so it is independent of the id schema, today's and any future one.
///
/// The model's calls and events are the program engine's, resolved once at build
/// time from the ORIGINAL ids (engine-switch S9.5; the L3 builder re-resolved them
/// from the routines on every read). So the precomputed edges are rebased the same
/// way: an edge end equal to an old id becomes the shared id, and an edge's call-site
/// and operation ids get the same prefix rewrite as the routine's own call sites.
fn force_id_collision(resolved: &mut L3Resolved, routine_name: &str, shared_id: &str) -> usize {
    let mut old_ids: Vec<String> = Vec::new();
    for r in resolved.workspace.routines.iter_mut() {
        if !r.name.eq_ignore_ascii_case(routine_name) {
            continue;
        }
        let old_id = std::mem::replace(&mut r.id, shared_id.to_string());
        for cs in r.call_sites.iter_mut() {
            if let Some(new) = rebase(&old_id, shared_id, &cs.id) {
                cs.id = new;
            }
            if let Some(new) = rebase(&old_id, shared_id, &cs.operation_id) {
                cs.operation_id = new;
            }
        }
        old_ids.push(old_id);
    }
    let rekey = |id: &mut String| {
        if old_ids.contains(id) {
            *id = shared_id.to_string();
        }
    };
    let rebase_any = |id: &mut String| {
        if let Some(new) = old_ids.iter().find_map(|o| rebase(o, shared_id, id)) {
            *id = new;
        }
    };
    if let Some(calls) = resolved.precomputed_calls.as_mut() {
        for e in &mut std::sync::Arc::make_mut(calls).edges {
            rekey(&mut e.from);
            e.to.as_mut().map(rekey);
            e.candidates.iter_mut().flatten().for_each(rekey);
            rebase_any(&mut e.callsite_id);
            rebase_any(&mut e.operation_id);
        }
    }
    if let Some(events) = resolved.precomputed_events.as_mut() {
        let graph = &mut std::sync::Arc::make_mut(events).graph;
        for e in &mut graph.edges {
            rekey(&mut e.subscriber_routine_id);
        }
        for s in &mut graph.events {
            s.publisher_routine_id.as_mut().map(rekey);
        }
    }
    old_ids.len()
}

/// `s` with its `old` routine-id prefix replaced by `shared` (a derived id such as
/// `{rid}/cs{n}`), or `None` when `s` does not start with `old`.
fn rebase(old: &str, shared: &str, s: &str) -> Option<String> {
    s.strip_prefix(old)
        .map(|suffix| format!("{shared}{suffix}"))
}

const TABLES: &str = r#"
table 50801 "G18 Setup"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

table 50802 "G18 Cust"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

table 50803 "G18 Log"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}
"#;

// --- (a) BEHAVIOUR: sibling page actions produce no cross-body attribution ---

/// Two actions on one page, both with `trigger OnAction()`. `RunBatch.OnAction` loops
/// calling an UNRESOLVED external routine (no edge of its own); `Finish.OnAction` is
/// STRAIGHT-LINE and calls the local `HandleSetup → CreateSetup` chain that does
/// `IsEmpty`/`Insert` on G18 Setup.
///
/// The loop is NOT on any path to `CreateSetup`'s ops, so d1 must emit NOTHING.
///
/// ⟨task-3 fix wave⟩ This was the original G-18 reproduction: pre-Task-3 the two
/// `OnAction` bodies shared one internal routine id, the looping body's in-loop call
/// site picked the SIBLING's `HandleSetup` edge under the shared callsite id, and d1
/// flagged both ops. Task 3's enclosing-member discriminator gives them DISTINCT ids,
/// so this is now a BEHAVIOUR test of the shape a real page produces — it asserts that
/// non-colliding precondition explicitly rather than claiming a collision it no longer
/// has. The stated-collision coverage of the guard is in (d)/(e) below.
#[test]
fn sibling_onaction_loop_is_not_attributed_to_the_straightline_path() {
    let page = r#"
page 50801 "G18 Wizard"
{
    PageType = Card;
    SourceTable = "G18 Cust";

    actions
    {
        area(Processing)
        {
            action(RunBatch)
            {
                trigger OnAction()
                var
                    Cust: Record "G18 Cust";
                begin
                    repeat
                        ProcessExternalLine();
                    until Cust.Next() = 0;
                end;
            }
            action(Finish)
            {
                trigger OnAction()
                begin
                    HandleSetup();
                end;
            }
        }
    }

    local procedure HandleSetup()
    begin
        CreateSetup();
    end;

    local procedure CreateSetup()
    var
        Setup: Record "G18 Setup";
    begin
        if Setup.IsEmpty() then
            Setup.Insert();
    end;
}
"#;
    let src = format!("{TABLES}{page}");
    let resolved = resolve(&[al("G18Wizard", &src)]);

    let ids = ids_named(&resolved, "OnAction");
    assert_eq!(ids.len(), 2, "fixture precondition: two OnAction bodies");
    assert_ne!(
        ids[0], ids[1],
        "post-Task-3 precondition: two page actions' OnAction triggers get DISTINCT \
         internal ids (the enclosing-member discriminator). If this ever fails, this \
         test has silently become a collision test again — which is what (d)/(e) are \
         for, stated rather than derived."
    );

    let findings = run_d1_on(&resolved);
    assert!(
        findings.is_empty(),
        "no loop is on the actual path to CreateSetup's ops — the loop in the \
         SIBLING action's OnAction must not be attributed to the straight-line \
         Finish → HandleSetup → CreateSetup chain (G-18). findings: {:#?}",
        root_causes(&findings)
    );
}

// --- (b) CONTROL: a REAL in-loop chain from a sibling action still fires ----

/// Same two-OnAction page shape, but the LOOPING action's in-loop call is RESOLVED to
/// `LoopHelper` (which writes G18 Log). That chain genuinely runs per iteration → d1
/// must still fire on `LoopHelper`'s Insert at `high`, and must still NOT flag the
/// sibling straight-line `StraightHelper` op.
///
/// This also discriminates a guard that OVER-rejects: `edge_target_matches_callsite_callee`
/// runs on every root-edge lookup, colliding id or not, so a guard hard-wired to
/// `false` (or one that mis-normalizes the callee/target names) silences this test.
#[test]
fn real_inloop_chain_from_a_sibling_action_still_fires() {
    let page = r#"
page 50802 "G18 Worklist"
{
    PageType = Card;
    SourceTable = "G18 Cust";

    actions
    {
        area(Processing)
        {
            action(RunBatch)
            {
                trigger OnAction()
                var
                    Cust: Record "G18 Cust";
                begin
                    repeat
                        LoopHelper();
                    until Cust.Next() = 0;
                end;
            }
            action(Finish)
            {
                trigger OnAction()
                begin
                    StraightHelper();
                end;
            }
        }
    }

    local procedure LoopHelper()
    var
        Log: Record "G18 Log";
    begin
        Log.Insert();
    end;

    local procedure StraightHelper()
    var
        Setup: Record "G18 Setup";
    begin
        if Setup.IsEmpty() then
            Setup.Insert();
    end;
}
"#;
    let src = format!("{TABLES}{page}");
    let findings = run_d1(&[al("G18Worklist", &src)]);
    assert_eq!(
        findings.len(),
        1,
        "exactly the genuine in-loop chain (OnAction loop → LoopHelper.Insert) \
         must fire — nothing on the sibling straight-line path. findings: {:#?}",
        root_causes(&findings)
    );
    let f = &findings[0];
    assert!(
        f.root_cause
            .contains("A loop in OnAction reaches Insert on G18 Log in LoopHelper"),
        "the genuine transitive finding must keep firing with the loop \
         attributed to the looping OnAction. rootCause: {}",
        f.root_cause
    );
    assert_eq!(
        f.severity, "high",
        "write at loop depth 1 stays high. rootCause: {}",
        f.root_cause
    );
    assert!(
        !findings
            .iter()
            .any(|f| f.root_cause.contains("StraightHelper") || f.root_cause.contains("G18 Setup")),
        "the sibling straight-line path must stay clean. findings: {:#?}",
        root_causes(&findings)
    );
}

// --- (c) CONTROL: vanilla transitive in-loop finding unaffected -------------

/// The plain G-1/G-4 shape — a codeunit loop calling a leaf that inserts. Must
/// keep firing at `high` (the guard must never suppress a genuine transitive
/// finding outside the colliding-trigger shape).
#[test]
fn vanilla_transitive_inloop_finding_still_fires() {
    let src = format!(
        r#"{TABLES}
codeunit 50801 "G18 Vanilla"
{{
    procedure LoopCaller(var Cust: Record "G18 Cust")
    begin
        repeat
            CreateLogEntry();
        until Cust.Next() = 0;
    end;

    procedure CreateLogEntry()
    var
        Log: Record "G18 Log";
    begin
        Log.Insert();
    end;
}}
"#
    );
    let findings = run_d1(&[al("G18Vanilla", &src)]);
    assert_eq!(
        findings.len(),
        1,
        "the vanilla transitive Insert must fire. findings: {:#?}",
        root_causes(&findings)
    );
    assert!(
        findings[0]
            .root_cause
            .contains("A loop in LoopCaller reaches Insert on G18 Log"),
        "rootCause: {}",
        findings[0].root_cause
    );
    assert_eq!(findings[0].severity, "high");
}

// --- (d) STATED COLLISION: the guard's reject direction ---------------------

/// ⟨task-3 fix wave, review I-1⟩ **The real coverage of
/// `edge_target_matches_callsite_callee`.** Same asymmetry as (a) — the looping body's
/// callee is UNRESOLVED, so it owns NO edge and the ONLY edge carrying its call-site
/// id belongs to the sibling body — but the collision is STATED by
/// [`force_id_collision`] instead of derived from `compute_routine_id`.
///
/// Because the looping body has no edge of its own, the pre-guard lookup is
/// deterministic: `find(callsite_id == "{shared}/cs0")` can only return the sibling's
/// `HandleSetup` edge, and d1 walks it into `CreateSetup`'s `IsEmpty`/`Insert` on
/// G18 Setup, attributing a loop that is not on that path. With the guard, the call
/// site's own callee (`ProcessExternalLine`) does not match the edge target's name
/// (`HandleSetup`) and the edge is rejected. Deleting the guard from the production
/// lookup (`d1_graph.rs:220`) makes this test fail — see the module doc for why the
/// other two call sites are not findings-observable.
///
/// The routines are stated as two `#if`/`#else` preproc alternatives of ONE action
/// only to name the residual shape honestly — the engine's union-read preproc handling
/// admits both bodies, and it is one of the two real 8020 residual shapes. The
/// collision itself does not depend on the source shape at all; it is forced.
#[test]
fn hand_stated_collision_does_not_splice_the_sibling_bodys_edge() {
    let page = r#"
page 50803 "G18 Preproc"
{
    PageType = Card;
    SourceTable = "G18 Cust";

    actions
    {
        area(Processing)
        {
            action(Batch)
            {
                trigger OnAction()
                var
                    Cust: Record "G18 Cust";
                begin
                    repeat
                        ProcessExternalLine();
                    until Cust.Next() = 0;
                end;
            }
            action(BatchLegacy)
            {
                trigger OnAction()
                begin
                    HandleSetup();
                end;
            }
        }
    }

    local procedure HandleSetup()
    begin
        CreateSetup();
    end;

    local procedure CreateSetup()
    var
        Setup: Record "G18 Setup";
    begin
        if Setup.IsEmpty() then
            Setup.Insert();
    end;
}
"#;
    let src = format!("{TABLES}{page}");
    let mut resolved = resolve(&[al("G18Preproc", &src)]);

    const SHARED_ID: &str = "hand-stated-g18-collision-id";
    let forced = force_id_collision(&mut resolved, "OnAction", SHARED_ID);
    assert_eq!(
        forced, 2,
        "fixture precondition: both OnAction trigger bodies must be in the model"
    );
    // The collision is real all the way down to the derived call-site ids: both bodies
    // now own a call site literally named `{SHARED_ID}/cs0`, which is what lets
    // `edges_by_from[SHARED_ID].find(callsite_id == cs.id)` cross bodies at all.
    let colliding_cs: Vec<&str> = resolved
        .workspace
        .routines
        .iter()
        .filter(|r| r.id == SHARED_ID)
        .filter_map(|r| r.call_sites.first())
        .map(|cs| cs.id.as_str())
        .collect();
    assert_eq!(
        colliding_cs.len(),
        2,
        "both colliding bodies must own at least one call site"
    );
    assert_eq!(
        colliding_cs[0], colliding_cs[1],
        "precondition: the two bodies' FIRST call sites must share a derived id — \
         that shared id is the whole G-18 mechanism"
    );

    let findings = run_d1_on(&resolved);
    assert!(
        findings.is_empty(),
        "under a STATED routine-id collision, the looping body's in-loop call site \
         (callee `ProcessExternalLine`, unresolved) must not pick up the sibling \
         body's `HandleSetup` edge and walk it into CreateSetup's ops — that is the \
         G-18 false positive `edge_target_matches_callsite_callee` exists to reject. \
         findings: {:#?}",
        root_causes(&findings)
    );
}

// --- (e) STATED COLLISION: the guard's accept direction ---------------------

/// ⟨task-3 fix wave, review I-1⟩ The firing-preservation half of (d): under the SAME
/// stated collision, a genuinely-own in-loop edge must still be found and walked. The
/// guard can never reject one — the call resolver is name-keyed, so an own edge's
/// target always carries the call site's callee name — and this pins that the
/// collision itself does not cost the finding.
///
/// Without this, a "guard" that rejected every edge under a shared `from` key would
/// pass (d) while silently deleting every genuine finding on the 15 residual 8020
/// collision groups.
#[test]
fn hand_stated_collision_still_fires_a_genuine_inloop_chain() {
    let page = r#"
page 50804 "G18 Preproc Worklist"
{
    PageType = Card;
    SourceTable = "G18 Cust";

    actions
    {
        area(Processing)
        {
            action(Batch)
            {
                trigger OnAction()
                var
                    Cust: Record "G18 Cust";
                begin
                    repeat
                        LoopHelper();
                    until Cust.Next() = 0;
                end;
            }
            action(BatchLegacy)
            {
                trigger OnAction()
                begin
                    StraightHelper();
                end;
            }
        }
    }

    local procedure LoopHelper()
    var
        Log: Record "G18 Log";
    begin
        Log.Insert();
    end;

    local procedure StraightHelper()
    var
        Setup: Record "G18 Setup";
    begin
        if Setup.IsEmpty() then
            Setup.Insert();
    end;
}
"#;
    let src = format!("{TABLES}{page}");
    let mut resolved = resolve(&[al("G18PreprocWorklist", &src)]);

    const SHARED_ID: &str = "hand-stated-g18-collision-id-2";
    let forced = force_id_collision(&mut resolved, "OnAction", SHARED_ID);
    assert_eq!(
        forced, 2,
        "fixture precondition: both OnAction trigger bodies must be in the model"
    );

    let findings = run_d1_on(&resolved);
    assert_eq!(
        findings.len(),
        1,
        "the genuine in-loop chain (loop → LoopHelper.Insert) must survive the \
         collision — the guard filters CROSS-body edges, never own ones. findings: {:#?}",
        root_causes(&findings)
    );
    assert!(
        findings[0]
            .root_cause
            .contains("A loop in OnAction reaches Insert on G18 Log in LoopHelper"),
        "rootCause: {}",
        findings[0].root_cause
    );
    assert!(
        !findings
            .iter()
            .any(|f| f.root_cause.contains("StraightHelper") || f.root_cause.contains("G18 Setup")),
        "the sibling straight-line body's op must stay clean under the collision. \
         findings: {:#?}",
        root_causes(&findings)
    );
}
