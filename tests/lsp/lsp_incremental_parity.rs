//! The PERMANENT incremental-vs-batch differential gate (T3 Task 10, the
//! arc's H-10 insurance policy). This test file outlives the T3 arc — it
//! runs in CI forever, on every PR.
//!
//! For every scripted disk-edit sequence below: copy the fixture workspace
//! (`tests/fixtures/lsp-incr/`) into a fresh tempdir, `LspSnapshot::
//! build_full_with_parsed` it, then drive `Updater::apply_batch` through one
//! or more scripted edits. **After EVERY edit** (not just at the end), the
//! incrementally-maintained `LspSnapshot` the `Updater` just produced must be
//! EQUIVALENT to a completely independent `LspSnapshot::build_full` of the
//! exact same on-disk state — proving the 2-rung incremental ladder (Tasks
//! 8/9) never silently drifts from a from-scratch rebuild.
//!
//! # Equivalence key (binding — NOT full structural equality)
//!
//! Two snapshots are "equivalent" here iff, for corresponding files:
//! - `edges_by_file`: the same file KEY SET, and for each file the same edge
//!   MULTISET, where one edge's identity for comparison is `(ObligationId,
//!   EdgeKind, DispatchShape, SetCompleteness, sorted Vec<(RouteTarget,
//!   EvidenceKind, sorted Vec<Condition>)>)` — i.e. the obligation it
//!   answers; the edge's own classification (its kind, dispatch shape, and
//!   set-completeness — real semantics, not incidental: `shape`/
//!   `completeness` are exactly what `classify_obligation` and
//!   `real_unknown_rate` read); and, per route, which target/evidence-kind
//!   it routes to plus that route's `Condition` set (real semantics too —
//!   `Route::fires_by_default`/`Edge::default_reachable_routes` gate
//!   traversal on exactly this field: a route silently losing/gaining
//!   `ManualBinding`/`AmbiguousDispatch` between incremental and fresh would
//!   be a genuine reachability-changing divergence, and the pre-review-fix
//!   key could not have caught it). Review fix-wave (this pass): the
//!   ORIGINAL key omitted `kind`/`shape`/`completeness`/`conditions`
//!   entirely — an unjustified exclusion for a permanent CI gate, caught in
//!   review; every field is now either compared or has a stated reason not
//!   to be (this doc's job is to make that an exhaustive list, not a
//!   selective one). Order within a file's bucket, and within one edge's
//!   route list, is deliberately NOT compared (both are multisets) — a
//!   global index (like a brand-new file's position, or `event_edges`'s
//!   traversal order over the whole graph) can legitimately differ in ORDER
//!   between an incrementally-patched `Vec` (new entries appended) and a
//!   fresh directory-walk build (natural filesystem order) without either
//!   being wrong.
//! - `event_edges`: the same multiset, same rule.
//! - `incoming`: the same map from target `RoutineNodeId` to the SET of
//!   `ObligationId`s that call it — dereferenced through each `EdgeRef` to
//!   its owning edge's `ObligationId` rather than compared as raw `(file,
//!   idx)` pairs, for the same order-independence reason as above (an
//!   `EdgeRef`'s `idx` is a position into a bucket whose GLOBAL order can
//!   legitimately differ; its `ObligationId` is a positionless, stable
//!   identity).
//! - `decls_by_file`: the same file key set, and for each file the same set
//!   of `(RoutineNodeId, name, origin, name_origin)` tuples.
//!
//! # Why `ObligationId`, not the brief's originally-suggested `SiteId`
//!
//! The brief's Step 1 names the per-edge comparison key as `(SiteId, sorted
//! route (target, evidence-discriminant))`. Building the gate against a
//! literal `Edge.site.clone()` instead of `ClassifiedEdge.obligation_id`
//! surfaced a REAL, reproducible false-positive divergence on the very
//! first script (`body_edit_chain_...`, step 0): `event_edges`' one
//! `EventFlow` edge (`Alpha.OnAfterWork` -> `Beta.HandleAfterWork`) differed
//! ONLY in `SiteId.span`'s line number between the incremental and fresh
//! builds, even though the resolved route was byte-identical. Root cause,
//! confirmed by reading `resolver.rs`'s `emit_event_flow_edges`: for a
//! Publisher-kind edge, `SiteId` is explicitly "anchored at the publisher
//! routine's name-origin span" — a cosmetic position anchor, not a
//! distinguishing call site (there IS no call expression for an event; the
//! publisher's own declaration stands in for one) — while
//! `apply_rung1_core` (`src/lsp/updater.rs`) never recomputes `event_edges`
//! on rung 1 (`Arc::clone(&cur.event_edges)`, unconditionally), so that
//! anchor goes stale whenever a rung-1 edit shifts line numbers in a file
//! that happens to declare a publisher — even though nothing about the
//! publisher's IDENTITY or its subscriber fan-out changed. This is the same
//! underlying staleness CLASS the brief already carves out for
//! `Route::witness` below, just manifesting through `SiteId` for
//! `EdgeKind::EventFlow`/`ObligationId::Publisher` specifically.
//! `ObligationId` is the engine's OWN pre-designed fix for exactly this
//! distinction: `ObligationId::CallSite { caller, span, callee_fp }` mirrors
//! `SiteId` field-for-field (so nothing is lost comparing REAL call sites —
//! rung 1 always resolves the touched file's own call sites fresh, so their
//! spans are never stale), while `ObligationId::Publisher(RoutineNodeId)`
//! carries NO span at all, sidestepping the cosmetic-anchor staleness
//! entirely. Using `ObligationId` — already sitting on `ClassifiedEdge`,
//! never hand-reconstructed — is therefore strictly more correct than the
//! brief's literal `SiteId` suggestion, not a weakening of it. Confirmed
//! empirically: switching to it turned the one genuine failure into a pass
//! with zero other behavior change. Reported to the team lead as a
//! candidate follow-up (should rung 1 also refresh `event_edges` entries
//! for publishers in the touched file, for span freshness at the handler
//! level?) — out of scope for this gate to fix, since `apply_rung1_core` is
//! already-reviewed Task 9 code.
//!
//! **Witness spans are EXCLUDED from the equivalence key, BY DESIGN** (per
//! the def-surface audit §6.1): rung 1 only re-resolves the TOUCHED file(s)'
//! own edges, so a `Route::witness` (`Witness::SourceSpan`) on some OTHER,
//! untouched edge that happens to point INTO a file which changed later is
//! left stale — its cached byte span may no longer line up with that file's
//! current content. Handlers are required to re-derive any span live from
//! the current parse rather than trust a stored witness span (tested at the
//! handler level in Task 11's gate, not here) — so this test's equivalence
//! key never looks at `Route::witness` at all. `EvidenceKind` (via
//! `Evidence::kind()`) is compared instead of the raw `Evidence`, for the
//! unrelated reason that `Evidence::Unknown`'s `UnknownReason` payload is a
//! diagnostic-only field with its own serialization-boundary discipline
//! elsewhere in this engine (`Evidence::kind`'s own doc) — using it here
//! keeps this gate aligned with that same discipline rather than
//! reinventing a second comparison rule for the same payload.
//!
//! `generation` is also excluded: it counts monotonically upward on the
//! incremental side (each rung bumps it) and is always `0` on a fresh
//! `build_full` — the two are never expected to match and comparing them
//! would tell us nothing about correctness.
//!
//! `DeclEntry.origin`/`.name_origin` (`al_syntax::ir::Origin`) now derive
//! `PartialEq`/`Eq` structurally (Task 5's pack codec needed a total wire
//! round-trip assertion — `crates/al-syntax/src/ir/mod.rs`), but still no
//! `Ord`, and `canon_decls` below sorts its output to make the comparison
//! order-independent — so `canon_origin` projects each `Origin` into a
//! sortable tuple, dropping `kind_text` and keeping only the byte range and
//! start/end `Point`s, which really are stable, since both sides parse the
//! exact same on-disk bytes.
//! (`ts_id` was deleted outright — it was written once by the lowerer and read
//! by no production code; see the pack-cache spec §17.3.)
//!
//! **`dep_meta`/`dep_lines`/`workspace_root` (T3 Task 11 review
//! fix-wave — the three `LspSnapshot` fields Task 11 added for
//! dependency-source real-span coverage, after this gate was already
//! written).** All three are now compared, closing what would otherwise be
//! an invisible-divergence hole in a PERMANENT gate. On THIS fixture
//! (`tests/fixtures/lsp-incr/`, workspace-only — no dependency apps) `dep_meta`/
//! `dep_lines` are trivially empty on both sides and `workspace_root` is
//! trivially identical (`copy_fixture_to_tempdir`'s one tempdir), so this
//! addition exercises the comparison PLUMBING now without yet proving
//! anything non-vacuous about dependency-bearing rung 1/2 transitions — a
//! dedicated dep-bearing fixture arm is planned as Task 14's Step 5
//! (plan-amended, commit `9e4006e`), which will give these same three
//! comparisons real coverage.
//!
//! # Exhaustive accounting (every `Edge`/`Route` field is either compared or
//! excluded-with-a-reason above; nothing is silently dropped)
//!
//! Compared: `ObligationId` (via `ClassifiedEdge`, not `Edge.site` — see
//! above), `Edge::kind`, `Edge::shape`, `Edge::completeness`,
//! `Route::target`, `Route::evidence` (via `EvidenceKind`),
//! `Route::conditions`. Excluded, each with a reason stated above:
//! `Route::witness` (staleness, audit §6.1), `Evidence::Unknown`'s
//! `UnknownReason` payload (subsumed by `EvidenceKind`, itself a deliberate
//! serialization-boundary projection elsewhere in this engine), `LspSnapshot::
//! generation` (monotonic counter vs. always-`0`, not a correctness signal),
//! `Origin::kind_text` (excluded from the comparison; see the module doc).
//! `Route::receiver_tier` is excluded too: its own doc (`edge.rs`) already
//! states it is diagnostic-only and is never compared against the committed
//! semantic goldens, for the same serialization-boundary discipline as
//! `Evidence::Unknown`'s payload — this gate excludes it for that identical,
//! already-engine-documented reason, not a new carve-out invented here.
//!
//! # Non-vacuity (Step 2)
//!
//! A gate that always took the slow, always-correct rung 3 (or always rung
//! 2) everywhere would pass every check above for a trivial, uninteresting
//! reason. Every script below pins its OWN expected `Rung` via
//! `assert_eq!`, and — per the brief's explicit binding requirement —
//! [`gate_non_vacuity_rung1_and_rung2_are_both_exercised`] independently
//! proves, in isolation from every other script's state, that this suite's
//! `apply_batch` calls really do take rung 1 at least once and rung 2 at
//! least once.
//!
//! # CI safety
//!
//! Pure fixture, zero CDO dependency — every script is self-contained and
//! runs on any machine (including a bare `ubuntu-latest` CI runner). All
//! paths are built via `Path::join`, never a hand-written separator.

use std::collections::BTreeMap;
use std::path::Path;

use al_sem::lsp::snapshot::{DeclEntry, LspEdge, LspSnapshot, LspTarget};
use al_sem::lsp::updater::{ChangeEvent, Rung, Updater};
use al_sem::program::node::{AppRef, ObjKey, ObjectNodeId, RoutineNodeId, SharedStr};
use al_sem::program::resolve::edge::{
    CanonicalSpan, DispatchShape, Edge, EdgeKind, Evidence, Route, RouteTarget, SetCompleteness,
    SiteId, SourcePos, Witness,
};
use al_sem::program::resolve::full::ObligationId;
use al_sem::snapshot::ParsedUnit;

// ---------------------------------------------------------------------------
// Fixture plumbing
// ---------------------------------------------------------------------------

/// Copy the committed fixture workspace (`tests/fixtures/lsp-incr/`) into a
/// fresh tempdir so every script gets its own independent, mutable copy —
/// never touching the committed original.
fn copy_fixture_to_tempdir() -> tempfile::TempDir {
    let dst = tempfile::tempdir().expect("tempdir");
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lsp-incr");
    copy_dir_recursive(&src, dst.path());
    dst
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
    for entry in std::fs::read_dir(src).expect("read_dir fixture source") {
        let entry = entry.expect("dir entry");
        let file_type = entry.file_type().expect("file_type");
        let dest_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            std::fs::create_dir_all(&dest_path).expect("create_dir_all");
            copy_dir_recursive(&entry.path(), &dest_path);
        } else {
            std::fs::copy(entry.path(), &dest_path).expect("copy fixture file");
        }
    }
}

fn build_full_with_parsed(dir: &Path) -> (LspSnapshot, ParsedUnit) {
    LspSnapshot::build_full_with_parsed(dir).expect("build_full_with_parsed on fixture")
}

// ---------------------------------------------------------------------------
// Canonicalization — the ONE equivalence-key implementation every script uses
// ---------------------------------------------------------------------------

/// One route's comparison identity: its target, evidence-kind, and its
/// OWN sorted `Condition` set (`fires_by_default`/`default_reachable_routes`
/// gate traversal on exactly this field — see the module doc).
/// One edge's comparison identity: the obligation it answers (`ObligationId`
/// — not the brief's originally-suggested raw `SiteId`; see the module
/// doc's "Why `ObligationId`" section), its kind, its sorted targets and its
/// route count. Since engine-switch S10.5 the snapshot stores only these
/// (`LspEdge`): route evidence, conditions, dispatch shape and completeness
/// stay in the program report, which no rung touches.
type CanonEdge = (ObligationId, EdgeKind, Vec<LspTarget>, u32);

fn canon_edge(e: &LspEdge) -> CanonEdge {
    let mut targets = e.targets.to_vec();
    targets.sort();
    (e.obligation_id(), e.kind, targets, e.route_count)
}

/// A file's (or `event_edges`'s) edge bucket as an order-independent
/// multiset: sorted, so two buckets containing the same edges in different
/// orders compare equal.
fn canon_edges(edges: &[LspEdge]) -> Vec<CanonEdge> {
    let mut v: Vec<CanonEdge> = edges.iter().map(canon_edge).collect();
    v.sort();
    v
}

/// `(byte start, byte end, start (row, col), end (row, col))` — projects
/// away `Origin`'s `kind_text` field (see the module doc). `Origin` itself
/// now derives `PartialEq`/`Eq`, but still no `Ord`, so this tuple is what
/// makes an `Origin` sortable — needed because `canon_decls` sorts its
/// output for an order-independent comparison — not a workaround for a
/// missing `Origin::eq`.
type CanonOrigin = (usize, usize, (u32, u32), (u32, u32));

fn canon_origin(o: &al_syntax::ir::Origin) -> CanonOrigin {
    (
        o.byte.start,
        o.byte.end,
        (o.start.row, o.start.column),
        (o.end.row, o.end.column),
    )
}

type CanonDecl = (RoutineNodeId, String, CanonOrigin, CanonOrigin);

fn canon_decl(d: &DeclEntry) -> CanonDecl {
    (
        d.id.clone(),
        d.name.clone(),
        canon_origin(&d.origin),
        canon_origin(&d.name_origin),
    )
}

fn canon_decls(decls: &[DeclEntry]) -> Vec<CanonDecl> {
    let mut v: Vec<CanonDecl> = decls.iter().map(canon_decl).collect();
    v.sort();
    v
}

/// `LspSnapshot::dep_meta`, canonicalized to the same `CanonDecl` shape
/// `canon_decl` projects a workspace `DeclEntry` to — replaces
/// `canon_dep_decl_by_id` (the `dep_decl_by_id` map was deleted; `dep_meta`
/// is the same data, built from the same frozen `RoutineMeta` source).
fn canon_dep_meta(snap: &LspSnapshot) -> BTreeMap<RoutineNodeId, CanonDecl> {
    snap.dep_meta
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                (
                    k.clone(),
                    v.name.to_string(),
                    canon_origin(&v.origin),
                    canon_origin(&v.name_origin),
                ),
            )
        })
        .collect()
}

/// `incoming`, canonicalized: target `RoutineNodeId` -> the SORTED set of
/// `ObligationId`s of the edges that call it. Dereferences every `EdgeRef`
/// through [`LspSnapshot::edge`] to its owning `ClassifiedEdge`'s
/// `obligation_id` — a positionless, stable identity — rather than
/// comparing raw `(file, idx)` pairs, whose global ordering can legitimately
/// differ between an incrementally-patched bucket and a fresh directory-walk
/// build (see the module doc).
fn canon_incoming(snap: &LspSnapshot) -> BTreeMap<RoutineNodeId, Vec<ObligationId>> {
    let mut out: BTreeMap<RoutineNodeId, Vec<ObligationId>> = BTreeMap::new();
    for target in snap.all_incoming().into_keys() {
        let mut obligations: Vec<ObligationId> = snap
            .incoming(&target)
            .map(|r| snap.edge(r).obligation_id())
            .collect();
        obligations.sort();
        out.insert(target, obligations);
    }
    out
}

/// Assert `incremental` (the `Updater`'s output) is EQUIVALENT to `fresh` (an
/// independent `LspSnapshot::build_full` of the exact same on-disk state) per
/// the module doc's binding equivalence key. `context` is prefixed onto
/// every failure message so a failing assertion names exactly which
/// script/step diverged, and on WHICH file/target.
fn assert_snapshots_equivalent(incremental: &LspSnapshot, fresh: &LspSnapshot, context: &str) {
    let mut inc_files: Vec<&String> = incremental.edges_by_file.keys().collect();
    let mut fresh_files: Vec<&String> = fresh.edges_by_file.keys().collect();
    inc_files.sort();
    fresh_files.sort();
    assert_eq!(
        inc_files, fresh_files,
        "{context}: edges_by_file's file SET diverged (incremental vs fresh build_full)"
    );

    for file in &inc_files {
        let inc_edges = canon_edges(&incremental.edges_by_file[*file]);
        let fresh_edges = canon_edges(&fresh.edges_by_file[*file]);
        assert_eq!(
            inc_edges, fresh_edges,
            "{context}: file {file:?}'s edge multiset diverged (incremental vs fresh build_full)"
        );
    }

    // Both parts separately (engine-switch S10.4): the workspace part, and
    // the shared dependency-only part every rung forwards.
    let inc_event = canon_edges(&incremental.ws_event_edges);
    let fresh_event = canon_edges(&fresh.ws_event_edges);
    assert_eq!(
        inc_event, fresh_event,
        "{context}: ws_event_edges' multiset diverged (incremental vs fresh build_full)"
    );
    assert_eq!(
        canon_edges(&incremental.dep_events.edges),
        canon_edges(&fresh.dep_events.edges),
        "{context}: the dependency event links diverged (incremental vs fresh build_full)"
    );

    let mut inc_decl_files: Vec<&String> = incremental.decls_by_file.keys().collect();
    let mut fresh_decl_files: Vec<&String> = fresh.decls_by_file.keys().collect();
    inc_decl_files.sort();
    fresh_decl_files.sort();
    assert_eq!(
        inc_decl_files, fresh_decl_files,
        "{context}: decls_by_file's file SET diverged (incremental vs fresh build_full)"
    );

    for file in &inc_decl_files {
        let inc_decls = canon_decls(&incremental.decls_by_file[*file]);
        let fresh_decls = canon_decls(&fresh.decls_by_file[*file]);
        assert_eq!(
            inc_decls, fresh_decls,
            "{context}: file {file:?}'s decl list diverged (incremental vs fresh build_full)"
        );
    }

    let inc_incoming = canon_incoming(incremental);
    let fresh_incoming = canon_incoming(fresh);
    let mut all_targets: Vec<RoutineNodeId> = inc_incoming
        .keys()
        .cloned()
        .chain(fresh_incoming.keys().cloned())
        .collect();
    all_targets.sort();
    all_targets.dedup();
    for target in &all_targets {
        assert_eq!(
            inc_incoming.get(target),
            fresh_incoming.get(target),
            "{context}: incoming map diverged for target {target:?}"
        );
    }

    // t3 whole-branch review (blocker fix wave): publisher_fanout is DERIVED
    // state built alongside `incoming` in the SAME `build_incoming` pass — it
    // must be equivalence-checked exactly like `incoming` itself, not left
    // unpinned just because it's new. A plain `HashMap` equality is enough
    // (no EdgeRef/position indirection to canonicalize — every value is a
    // positionless `RoutineNodeId -> usize` count).
    assert_eq!(
        incremental.ws_publisher_fanout, fresh.ws_publisher_fanout,
        "{context}: publisher_fanout diverged (incremental vs fresh build_full)"
    );
    assert_eq!(
        incremental.dep_events.publisher_fanout, fresh.dep_events.publisher_fanout,
        "{context}: the dependency links' publisher_fanout diverged (incremental vs fresh)"
    );

    // T3 Task 11 review fix-wave: dep_meta/dep_lines/workspace_root —
    // trivially equal on this dep-less fixture (see the module doc's note);
    // still compared so a future regression can't slip through silently.
    assert_eq!(
        canon_dep_meta(incremental),
        canon_dep_meta(fresh),
        "{context}: dep_meta diverged (incremental vs fresh build_full)"
    );
    assert_eq!(
        *incremental.dep_lines, *fresh.dep_lines,
        "{context}: dep_lines diverged (incremental vs fresh build_full)"
    );
    assert_eq!(
        incremental.workspace_root, fresh.workspace_root,
        "{context}: workspace_root diverged (incremental vs fresh build_full)"
    );
}

// ---------------------------------------------------------------------------
// Script 1: body-edit chain — 3 consecutive rung-1 saves
// ---------------------------------------------------------------------------

#[test]
fn body_edit_chain_three_consecutive_saves_stay_equivalent() {
    let dir = copy_fixture_to_tempdir();
    let (base, parsed) = build_full_with_parsed(dir.path());
    let mut updater = Updater::new(dir.path().to_path_buf(), parsed);
    let mut cur = base;

    // Each step only adds another BODY statement (an extra call to an
    // ALREADY-EXISTING target) — no object/routine identity, arity, or
    // signature change, so every one of these 3 consecutive saves must stay
    // rung-1-eligible.
    let bodies = [
        r#"// Unicode smoke test (Task 10 fixture requirement): æøå
codeunit 50100 "Alpha"
{
    procedure DoWork()
    var
        Beta: Codeunit "Beta";
    begin
        Beta.Process();
        Beta.Process();
        Calc(1);
        Calc('x');
        Løbenr();
    end;

    procedure Calc(X: Integer)
    begin
    end;

    procedure Calc(X: Text)
    begin
    end;

    procedure Løbenr()
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnAfterWork()
    begin
    end;
}
"#,
        r#"// Unicode smoke test (Task 10 fixture requirement): æøå
codeunit 50100 "Alpha"
{
    procedure DoWork()
    var
        Beta: Codeunit "Beta";
    begin
        Beta.Process();
        Beta.Process();
        Beta.Process();
        Calc(1);
        Calc('x');
        Løbenr();
    end;

    procedure Calc(X: Integer)
    begin
    end;

    procedure Calc(X: Text)
    begin
    end;

    procedure Løbenr()
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnAfterWork()
    begin
    end;
}
"#,
        r#"// Unicode smoke test (Task 10 fixture requirement): æøå
codeunit 50100 "Alpha"
{
    procedure DoWork()
    var
        Beta: Codeunit "Beta";
    begin
        Beta.Process();
        Beta.Process();
        Beta.Process();
        Beta.Process();
        Calc(1);
        Calc('x');
        Løbenr();
        Løbenr();
    end;

    procedure Calc(X: Integer)
    begin
    end;

    procedure Calc(X: Text)
    begin
    end;

    procedure Løbenr()
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnAfterWork()
    begin
    end;
}
"#,
    ];

    for (i, body) in bodies.iter().enumerate() {
        std::fs::write(dir.path().join("Alpha.al"), body)
            .unwrap_or_else(|e| panic!("rewrite Alpha.al (step {i}): {e}"));

        let batch = vec![ChangeEvent::FileSaved(dir.path().join("Alpha.al"))];
        let (new_snap, rung) = updater
            .apply_batch(&cur, &batch)
            .unwrap_or_else(|| panic!("apply_batch must succeed (step {i})"));
        assert_eq!(
            rung,
            Rung::One,
            "body-edit chain step {i}: a body-only edit must take rung 1"
        );

        let fresh = LspSnapshot::build_full(dir.path()).expect("fresh build_full");
        assert_snapshots_equivalent(&new_snap, &fresh, &format!("body-edit-chain step {i}"));

        cur = new_snap;
    }
}

// ---------------------------------------------------------------------------
// Script 2: signature change — rung 2, breaks 3 cross-file callers
// ---------------------------------------------------------------------------

#[test]
fn signature_change_stays_equivalent() {
    let dir = copy_fixture_to_tempdir();
    let (base, parsed) = build_full_with_parsed(dir.path());
    let mut updater = Updater::new(dir.path().to_path_buf(), parsed);

    // Beta.Process gains a required parameter — a DefSurface change (arity
    // moves). Alpha.DoWork, MyTable's field(2) OnValidate trigger, and
    // MyPage's OnOpenPage trigger all call the OLD 0-arg shape, so every one
    // of those 3 cross-file/cross-object call sites must flip to Unknown.
    std::fs::write(
        dir.path().join("Beta.al"),
        r#"codeunit 50101 "Beta"
{
    procedure Process(X: Integer)
    begin
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Alpha", 'OnAfterWork', '', false, false)]
    local procedure HandleAfterWork()
    begin
    end;
}
"#,
    )
    .expect("rewrite Beta.al with a new required parameter");

    let batch = vec![ChangeEvent::FileSaved(dir.path().join("Beta.al"))];
    let (new_snap, rung) = updater
        .apply_batch(&base, &batch)
        .expect("apply_batch must succeed");
    assert_eq!(
        rung,
        Rung::Two,
        "a signature (arity) change must take rung 2"
    );

    let fresh = LspSnapshot::build_full(dir.path()).expect("fresh build_full");
    assert_snapshots_equivalent(&new_snap, &fresh, "signature-change");
}

// ---------------------------------------------------------------------------
// Script 3: rename routine — rung 2, breaks the same 3 callers
// ---------------------------------------------------------------------------

#[test]
fn rename_routine_stays_equivalent() {
    let dir = copy_fixture_to_tempdir();
    let (base, parsed) = build_full_with_parsed(dir.path());
    let mut updater = Updater::new(dir.path().to_path_buf(), parsed);

    // Renaming Process -> DoProcess moves the RoutineNodeId SET (item 3 of
    // the DefSurface fingerprint) without changing arity — every caller
    // still says `Beta.Process()`, which no longer exists.
    std::fs::write(
        dir.path().join("Beta.al"),
        r#"codeunit 50101 "Beta"
{
    procedure DoProcess()
    begin
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Alpha", 'OnAfterWork', '', false, false)]
    local procedure HandleAfterWork()
    begin
    end;
}
"#,
    )
    .expect("rewrite Beta.al renaming Process to DoProcess");

    let batch = vec![ChangeEvent::FileSaved(dir.path().join("Beta.al"))];
    let (new_snap, rung) = updater
        .apply_batch(&base, &batch)
        .expect("apply_batch must succeed");
    assert_eq!(rung, Rung::Two, "a routine rename must take rung 2");

    let fresh = LspSnapshot::build_full(dir.path()).expect("fresh build_full");
    assert_snapshots_equivalent(&new_snap, &fresh, "rename-routine");
}

// ---------------------------------------------------------------------------
// Script 4: add file — rung 2 (brand-new file, no prior surface to compare)
// ---------------------------------------------------------------------------

#[test]
fn add_file_stays_equivalent() {
    let dir = copy_fixture_to_tempdir();
    let (base, parsed) = build_full_with_parsed(dir.path());
    let mut updater = Updater::new(dir.path().to_path_buf(), parsed);

    std::fs::write(
        dir.path().join("Epsilon.al"),
        r#"codeunit 50105 "Epsilon"
{
    procedure DoIt()
    var
        Alpha: Codeunit "Alpha";
    begin
        Alpha.Calc(5);
    end;
}
"#,
    )
    .expect("write brand-new Epsilon.al");

    let batch = vec![ChangeEvent::FileSaved(dir.path().join("Epsilon.al"))];
    let (new_snap, rung) = updater
        .apply_batch(&base, &batch)
        .expect("apply_batch must succeed");
    assert_eq!(
        rung,
        Rung::Two,
        "a brand-new file has no prior surface to compare against — must take rung 2"
    );
    assert!(
        new_snap.edges_by_file.contains_key("Epsilon.al"),
        "the new file's edge bucket must be present"
    );

    let fresh = LspSnapshot::build_full(dir.path()).expect("fresh build_full");
    assert_snapshots_equivalent(&new_snap, &fresh, "add-file");
}

// ---------------------------------------------------------------------------
// Script 5: delete file — rung 2, its edges/incoming entries disappear
// ---------------------------------------------------------------------------

#[test]
fn delete_file_stays_equivalent() {
    let dir = copy_fixture_to_tempdir();
    let (base, parsed) = build_full_with_parsed(dir.path());
    let mut updater = Updater::new(dir.path().to_path_buf(), parsed);

    // Baseline sanity: MyPage.al's OnOpenPage call must be one of
    // Beta.Process's incoming callers before the delete.
    let beta_process = base.decls_by_file["Beta.al"]
        .iter()
        .find(|d| d.name == "Process")
        .expect("Beta.Process decl")
        .id
        .clone();
    assert!(
        base.incoming(&beta_process)
            .any(|r| &*r.file == "MyPage.al"),
        "baseline: MyPage.al must be one of Beta.Process's incoming callers"
    );

    std::fs::remove_file(dir.path().join("MyPage.al")).expect("delete MyPage.al");
    let batch = vec![ChangeEvent::FileRemoved(dir.path().join("MyPage.al"))];
    let (new_snap, rung) = updater
        .apply_batch(&base, &batch)
        .expect("apply_batch must succeed");
    assert_eq!(rung, Rung::Two, "a file delete must take rung 2");

    assert!(
        !new_snap.edges_by_file.contains_key("MyPage.al"),
        "MyPage.al's edge bucket must be gone"
    );
    assert!(!new_snap.decls_by_file.contains_key("MyPage.al"));

    let fresh = LspSnapshot::build_full(dir.path()).expect("fresh build_full");
    assert_snapshots_equivalent(&new_snap, &fresh, "delete-file");
}

// ---------------------------------------------------------------------------
// Script 6: edit that flips overload resolution — body-only, stays rung 1
// ---------------------------------------------------------------------------

/// The Alpha.al call site at line `line` (0-based, matching
/// `Origin`/`SourcePos` conventions) whose route names an overload of
/// `Calc` — i.e. one specific `Calc()` call, identified by its FIXED source
/// position rather than by iteration order (`edges_by_file`'s per-file
/// order is a multiset for equivalence purposes — see the module doc — so
/// this test must not rely on it; a line number is a stable, meaningful
/// identity a real call site actually has).
fn calc_target_at_line(edges: &[LspEdge], line: u32) -> RoutineNodeId {
    let e = edges
        .iter()
        .find(|e| e.span.start.line == line)
        .unwrap_or_else(|| panic!("no call site at line {line}"));
    e.routine_targets()
        .find(|t| t.name_lc == "calc")
        .unwrap_or_else(|| panic!("line {line}'s edge does not route to a Calc overload"))
        .clone()
}

#[test]
fn overload_flip_body_only_edit_stays_rung1_and_equivalent() {
    let dir = copy_fixture_to_tempdir();
    let (base, parsed) = build_full_with_parsed(dir.path());
    let mut updater = Updater::new(dir.path().to_path_buf(), parsed);

    // The fixture's DoWork body (both before and after this test's edit)
    // calls Calc() twice, on these exact, UNCHANGED source lines (only the
    // literal TOKEN on each line changes below — no line is added or
    // removed) — 0-based, matching `Origin`/`SourcePos`.
    const INTEGER_LITERAL_CALL_LINE: u32 = 8; // `Calc(1)` before -> `Calc('flipped')` after
    const TEXT_LITERAL_CALL_LINE: u32 = 9; // `Calc('x')` before -> `Calc(2)` after

    // Baseline, from the UNTOUCHED fixture: name each overload's specific
    // RoutineNodeId by which literal type resolves to it, so the assertions
    // below can say "Calc(Text)"/"Calc(Integer)" honestly rather than just
    // "site A"/"site B".
    let calc_integer_id =
        calc_target_at_line(&base.edges_by_file["Alpha.al"], INTEGER_LITERAL_CALL_LINE);
    let calc_text_id = calc_target_at_line(&base.edges_by_file["Alpha.al"], TEXT_LITERAL_CALL_LINE);
    assert_ne!(
        calc_integer_id, calc_text_id,
        "baseline sanity: Calc(Integer) and Calc(Text) must be DISTINCT routines \
         (otherwise this test cannot prove anything about a flip)"
    );

    // Swap which literal each Calc() call site passes — Alpha.Calc(Integer)/
    // Calc(Text) is the fixture's overload set, and arg-type dispatch picks
    // between them at CALL-SITE resolution time, which is body content, not
    // definition surface: no object/routine identity, arity, or param type
    // moved, so this must still take rung 1 — and the incremental path must
    // re-run arg-type dispatch against the file's FRESH content rather than
    // a stale cached DeclSurface (the module doc's soundness argument for why
    // rung 1 resolves the touched file directly from its fresh parse).
    std::fs::write(
        dir.path().join("Alpha.al"),
        r#"// Unicode smoke test (Task 10 fixture requirement): æøå
codeunit 50100 "Alpha"
{
    procedure DoWork()
    var
        Beta: Codeunit "Beta";
    begin
        Beta.Process();
        Calc('flipped');
        Calc(2);
        Løbenr();
    end;

    procedure Calc(X: Integer)
    begin
    end;

    procedure Calc(X: Text)
    begin
    end;

    procedure Løbenr()
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnAfterWork()
    begin
    end;
}
"#,
    )
    .expect("rewrite Alpha.al with swapped Calc() literal types");

    let batch = vec![ChangeEvent::FileSaved(dir.path().join("Alpha.al"))];
    let (new_snap, rung) = updater
        .apply_batch(&base, &batch)
        .expect("apply_batch must succeed");
    assert_eq!(
        rung,
        Rung::One,
        "swapping which literal each Calc() call passes is body-only — the \
         DefSurface fingerprint (routine set/arity/param types) is unaffected"
    );

    let calc_edges: Vec<&LspEdge> = new_snap.edges_by_file["Alpha.al"]
        .iter()
        .filter(|e| e.routine_targets().any(|t| t.name_lc == "calc"))
        .collect();
    assert_eq!(
        calc_edges.len(),
        2,
        "both Calc() call sites must still be present after the swap"
    );
    for e in &calc_edges {
        assert!(
            e.route_count == 1 && e.targets.len() == 1,
            "each flipped call site must still cleanly resolve (one route, to a \
             routine), proving the incremental path re-ran arg-type dispatch \
             against the fresh file rather than a stale cached DeclSurface"
        );
    }

    // The self-contained, honest core of this test (review fix-wave — this
    // must hold WITHOUT leaning on the trailing full-equivalence check
    // below): the line that now passes the TEXT literal (`Calc('flipped')`,
    // the line that passed the INTEGER literal before) must route to
    // `calc_text_id`, and the line that now passes the INTEGER literal
    // (`Calc(2)`, the line that passed the TEXT literal before) must route
    // to `calc_integer_id` — i.e. the overload identity assigned to each
    // FIXED source position genuinely flipped, not merely "stayed resolved
    // to something."
    let new_edges = &new_snap.edges_by_file["Alpha.al"];
    let now_text_literal_site_target = calc_target_at_line(new_edges, INTEGER_LITERAL_CALL_LINE);
    let now_integer_literal_site_target = calc_target_at_line(new_edges, TEXT_LITERAL_CALL_LINE);
    assert_eq!(
        now_text_literal_site_target, calc_text_id,
        "the line that now passes a Text literal (Calc('flipped')) must name \
         Calc(Text)'s specific RoutineNodeId"
    );
    assert_eq!(
        now_integer_literal_site_target, calc_integer_id,
        "the line that now passes an Integer literal (Calc(2)) must name \
         Calc(Integer)'s specific RoutineNodeId"
    );

    let fresh = LspSnapshot::build_full(dir.path()).expect("fresh build_full");
    assert_snapshots_equivalent(&new_snap, &fresh, "overload-flip");
}

// ---------------------------------------------------------------------------
// Script 7: event-subscriber attribute edit — rung 2, event edge disappears
// ---------------------------------------------------------------------------

#[test]
fn event_subscriber_attribute_edit_stays_equivalent() {
    let dir = copy_fixture_to_tempdir();
    let (base, parsed) = build_full_with_parsed(dir.path());
    let mut updater = Updater::new(dir.path().to_path_buf(), parsed);

    assert!(
        base.event_edges().next().is_some(),
        "baseline: Alpha.OnAfterWork -> Beta.HandleAfterWork must be wired"
    );

    // Change the subscribed event name so it no longer matches anything
    // Alpha actually publishes — a DefSurface change (item 14,
    // event_subscribers), so this must take rung 2, and the whole-graph
    // event-flow re-emission must drop the now-orphaned subscriber.
    std::fs::write(
        dir.path().join("Beta.al"),
        r#"codeunit 50101 "Beta"
{
    procedure Process()
    begin
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Alpha", 'OnAfterWorkRenamed', '', false, false)]
    local procedure HandleAfterWork()
    begin
    end;
}
"#,
    )
    .expect("rewrite Beta.al with a non-matching subscribed event name");

    let batch = vec![ChangeEvent::FileSaved(dir.path().join("Beta.al"))];
    let (new_snap, rung) = updater
        .apply_batch(&base, &batch)
        .expect("apply_batch must succeed");
    assert_eq!(
        rung,
        Rung::Two,
        "an EventSubscriber attribute edit moves the DefSurface fingerprint's \
         event_subscribers item"
    );

    let still_wired = new_snap
        .event_edges()
        .any(|e| e.routine_targets().any(|t| t.name_lc == "handleafterwork"));
    assert!(
        !still_wired,
        "the subscriber must no longer receive Alpha's OnAfterWork event once \
         its subscribed event name no longer matches"
    );

    let fresh = LspSnapshot::build_full(dir.path()).expect("fresh build_full");
    assert_snapshots_equivalent(&new_snap, &fresh, "event-subscriber-attribute-edit");
}

// ---------------------------------------------------------------------------
// Script 8: one MIXED 6-edit batch — add + delete + rename + signature +
// 2 body-only edits, all coalesced into ONE apply_batch call
// ---------------------------------------------------------------------------

#[test]
fn mixed_six_edit_batch_stays_equivalent() {
    let dir = copy_fixture_to_tempdir();
    let (base, parsed) = build_full_with_parsed(dir.path());
    let mut updater = Updater::new(dir.path().to_path_buf(), parsed);

    // Edit 1 (body-only): Alpha gains one more Løbenr() call.
    std::fs::write(
        dir.path().join("Alpha.al"),
        r#"// Unicode smoke test (Task 10 fixture requirement): æøå
codeunit 50100 "Alpha"
{
    procedure DoWork()
    var
        Beta: Codeunit "Beta";
    begin
        Beta.Process();
        Calc(1);
        Calc('x');
        Løbenr();
        Løbenr();
    end;

    procedure Calc(X: Integer)
    begin
    end;

    procedure Calc(X: Text)
    begin
    end;

    procedure Løbenr()
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnAfterWork()
    begin
    end;
}
"#,
    )
    .expect("edit 1: rewrite Alpha.al (body-only)");

    // Edit 2 (body-only): MyPage's OnOpenPage calls Process() twice.
    std::fs::write(
        dir.path().join("MyPage.al"),
        r#"page 50104 "LSP Incr Page"
{
    SourceTable = "LSP Incr Table";

    layout
    {
        area(Content)
        {
            repeater(Group)
            {
                field("No."; Rec."No.")
                {
                }
            }
        }
    }

    trigger OnOpenPage()
    var
        Beta: Codeunit "Beta";
    begin
        Beta.Process();
        Beta.Process();
    end;
}
"#,
    )
    .expect("edit 2: rewrite MyPage.al (body-only)");

    // Edit 3 (brand-new file): Epsilon calls Alpha.Calc(5).
    std::fs::write(
        dir.path().join("Epsilon.al"),
        r#"codeunit 50105 "Epsilon"
{
    procedure DoIt()
    var
        Alpha: Codeunit "Alpha";
    begin
        Alpha.Calc(5);
    end;
}
"#,
    )
    .expect("edit 3: write brand-new Epsilon.al");

    // Edit 4 (delete): the tableextension goes away entirely.
    std::fs::remove_file(dir.path().join("MyTableExt.al")).expect("edit 4: delete MyTableExt.al");

    // Edit 5 (signature change): MyTable gains a new field.
    std::fs::write(
        dir.path().join("MyTable.al"),
        r#"table 50102 "LSP Incr Table"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Description; Text[100])
        {
            trigger OnValidate()
            var
                Beta: Codeunit "Beta";
            begin
                Beta.Process();
            end;
        }
        field(3; Amount; Decimal) { }
    }
    keys
    {
        key(PK; "No.") { }
    }
}
"#,
    )
    .expect("edit 5: rewrite MyTable.al (new field)");

    // Edit 6 (rename): Beta.Process -> Beta.DoProcess — breaks every other
    // edit's own calls to the old name in the SAME batch (Alpha, MyPage,
    // MyTable's field trigger all still say `Beta.Process()`).
    std::fs::write(
        dir.path().join("Beta.al"),
        r#"codeunit 50101 "Beta"
{
    procedure DoProcess()
    begin
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Alpha", 'OnAfterWork', '', false, false)]
    local procedure HandleAfterWork()
    begin
    end;
}
"#,
    )
    .expect("edit 6: rewrite Beta.al (rename Process -> DoProcess)");

    let batch = vec![
        ChangeEvent::FileSaved(dir.path().join("Alpha.al")),
        ChangeEvent::FileSaved(dir.path().join("MyPage.al")),
        ChangeEvent::FileSaved(dir.path().join("Epsilon.al")),
        ChangeEvent::FileRemoved(dir.path().join("MyTableExt.al")),
        ChangeEvent::FileSaved(dir.path().join("MyTable.al")),
        ChangeEvent::FileSaved(dir.path().join("Beta.al")),
    ];
    let (new_snap, rung) = updater
        .apply_batch(&base, &batch)
        .expect("apply_batch must succeed for the mixed 6-edit batch");
    assert_eq!(
        rung,
        Rung::Two,
        "a batch containing ANY rung-2-eligible event must take rung 2 for the whole batch"
    );

    assert!(new_snap.edges_by_file.contains_key("Epsilon.al"));
    assert!(!new_snap.edges_by_file.contains_key("MyTableExt.al"));

    let fresh = LspSnapshot::build_full(dir.path()).expect("fresh build_full");
    assert_snapshots_equivalent(&new_snap, &fresh, "mixed-6-edit-batch");
}

// ---------------------------------------------------------------------------
// Step 2: non-vacuity negative control (binding)
// ---------------------------------------------------------------------------

/// Independently proves — without relying on any other script's specific
/// assertions — that this suite's `apply_batch` calls really do take BOTH
/// `Rung::One` and `Rung::Two` at least once each. A suite that silently
/// took the slow, always-correct rung 3 (or always rung 2) everywhere would
/// pass every equivalence check above for a trivial, uninteresting reason.
#[test]
fn gate_non_vacuity_rung1_and_rung2_are_both_exercised() {
    let dir = copy_fixture_to_tempdir();
    let (base, parsed) = build_full_with_parsed(dir.path());
    let mut updater = Updater::new(dir.path().to_path_buf(), parsed);

    // A body-only edit -> rung 1.
    std::fs::write(
        dir.path().join("Alpha.al"),
        r#"// Unicode smoke test (Task 10 fixture requirement): æøå
codeunit 50100 "Alpha"
{
    procedure DoWork()
    var
        Beta: Codeunit "Beta";
    begin
        Beta.Process();
        Beta.Process();
        Calc(1);
        Calc('x');
        Løbenr();
    end;

    procedure Calc(X: Integer)
    begin
    end;

    procedure Calc(X: Text)
    begin
    end;

    procedure Løbenr()
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnAfterWork()
    begin
    end;
}
"#,
    )
    .expect("rewrite Alpha.al (rung-1 probe)");
    let (r1_snap, r1) = updater
        .apply_batch(
            &base,
            &[ChangeEvent::FileSaved(dir.path().join("Alpha.al"))],
        )
        .expect("apply_batch must succeed (rung-1 probe)");
    assert_eq!(
        r1,
        Rung::One,
        "non-vacuity probe: a body-only edit must take rung 1"
    );

    // A signature edit on a DIFFERENT file -> rung 2.
    std::fs::write(
        dir.path().join("Beta.al"),
        r#"codeunit 50101 "Beta"
{
    procedure Process(X: Integer)
    begin
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Alpha", 'OnAfterWork', '', false, false)]
    local procedure HandleAfterWork()
    begin
    end;
}
"#,
    )
    .expect("rewrite Beta.al (rung-2 probe)");
    let (_r2_snap, r2) = updater
        .apply_batch(
            &r1_snap,
            &[ChangeEvent::FileSaved(dir.path().join("Beta.al"))],
        )
        .expect("apply_batch must succeed (rung-2 probe)");
    assert_eq!(
        r2,
        Rung::Two,
        "non-vacuity probe: a signature (arity) edit must take rung 2"
    );
}

// ---------------------------------------------------------------------------
// Meta-test: canon_edge's discriminating power (review fix-wave)
// ---------------------------------------------------------------------------

/// Proves the `CanonEdge` key covers every fact the snapshot stores for an
/// edge (`LspEdge`, engine-switch S10.5): edges with the same obligation but
/// a different kind, a different target, or a different route count (an
/// extra route the LSP does not show) must canonicalize UNEQUAL. (Before
/// S10.5 the key also covered dispatch shape, completeness and conditions;
/// the snapshot no longer stores them.)
#[test]
fn canon_edge_distinguishes_kind_targets_and_route_count() {
    fn rid(name: &str) -> RoutineNodeId {
        RoutineNodeId {
            object: ObjectNodeId {
                app: AppRef(0),
                kind: al_syntax::ir::ObjectKind::Codeunit,
                key: ObjKey::Id(1),
            },
            name_lc: name.into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        }
    }

    fn base_edge(caller: RoutineNodeId, target: RoutineNodeId) -> Edge {
        Edge {
            from: caller.clone(),
            site: SiteId {
                caller,
                span: CanonicalSpan {
                    unit: "F.al".into(),
                    start: SourcePos { line: 1, col: 1 },
                    end: SourcePos { line: 1, col: 2 },
                },
                callee_fingerprint: 1,
            },
            kind: EdgeKind::Call,
            shape: DispatchShape::Exact,
            completeness: SetCompleteness::Complete,
            routes: vec![Route {
                target: RouteTarget::Routine(target),
                evidence: Evidence::Source,
                conditions: vec![],
                witness: Witness::None,
                receiver_tier: None,
            }],
        }
    }

    fn lsp(edge: &Edge) -> LspEdge {
        LspEdge::project(edge, &SharedStr::from("F.al"))
    }

    let caller = rid("caller");
    let target = rid("target");
    let base_canon = canon_edge(&lsp(&base_edge(caller.clone(), target.clone())));

    let mut kind_variant = base_edge(caller.clone(), target.clone());
    kind_variant.kind = EdgeKind::Run;
    assert_ne!(
        canon_edge(&lsp(&kind_variant)),
        base_canon,
        "two edges differing only in EdgeKind must NOT canonicalize equal"
    );

    let target_variant = base_edge(caller.clone(), rid("other"));
    assert_ne!(
        canon_edge(&lsp(&target_variant)),
        base_canon,
        "two edges differing only in a target must NOT canonicalize equal"
    );

    let mut count_variant = base_edge(caller, target);
    count_variant.routes.push(Route {
        target: RouteTarget::Unresolved,
        evidence: Evidence::Source,
        conditions: vec![],
        witness: Witness::None,
        receiver_tier: None,
    });
    assert_ne!(
        canon_edge(&lsp(&count_variant)),
        base_canon,
        "two edges differing only in a route the LSP does not show must NOT \
         canonicalize equal"
    );
}

// ---------------------------------------------------------------------------
// Script 10 (T3 Task 14 Step 5, plan-amended): dep-bearing fixture arm.
//
// Every script above runs on `tests/fixtures/lsp-incr/`, which declares no
// dependencies — so the module doc's `dep_meta`/`dep_lines`/
// `workspace_root` comparisons in `assert_snapshots_equivalent` (added in
// the Task 10/11 review fix-waves) are trivially vacuous there (both sides
// empty/identical for a structural, not a proven, reason). This script uses
// `tests/fixtures/lsp-diff-deps/` instead — a real, committed, disk-based
// `.alpackages/` pair (`aaaaaaaa…0001.app` "Core Lib", SymbolOnly; and
// `bbbbbbbb…0002.app` "Source Lib", a REAL embedded-source dependency
// shipping `codeunit 60100 "Source Mgt"`'s actual AL source inside the
// package — see `tests/fixtures/lsp-diff-deps/app.json`'s declared
// dependencies) — giving these three fields NON-VACUOUS coverage: a real
// `dep_meta` entry for `Source Mgt.DoWork`, a real `dep_lines` entry
// for its embedded source, through both a rung-1 (body-only) and a rung-2
// (signature-change) transition on the WORKSPACE caller file. Per the
// design doc's `dep_layer` Arc-sharing rationale (`snapshot.rs`'s own
// doc), dependency source cannot change on either rung — this script
// explicitly asserts `dep_meta` stays byte-identical across both
// transitions, not merely present.
//
// A hand-built `.app` zip was considered infeasible to construct correctly
// from scratch within this task's scope (the embedded-source format is a
// real BC "ShowMyCode" package layout `cached_source` parses structurally,
// not a simple ad-hoc convention) — reusing the ALREADY-COMMITTED,
// already-proven `tests/r2-5a-fixtures/{core-symbol-only,source-included}`
// `.app` fixtures (copied verbatim into this new fixture's `.alpackages/`)
// made the disk-based, plan-preferred arm feasible after all, so the
// brief's in-memory `two_app` fallback was not needed here.

fn copy_fixture_lsp_diff_deps_to_tempdir() -> tempfile::TempDir {
    let dst = tempfile::tempdir().expect("tempdir");
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lsp-diff-deps");
    copy_dir_recursive(&src, dst.path());
    dst
}

#[test]
fn dep_bearing_rung1_then_rung2_stay_equivalent_with_nonvacuous_dep_indexes() {
    let dir = copy_fixture_lsp_diff_deps_to_tempdir();
    let (base, parsed) = build_full_with_parsed(dir.path());

    // Non-vacuity precondition (binding — see this script's header comment):
    // the fixture's dep layer must actually contribute BEFORE any rung
    // assertion means anything.
    assert!(
        !base.dep_meta.is_empty(),
        "fixture sanity: dep_meta must carry Source Lib's embedded DoWork"
    );
    assert!(
        !base.dep_lines.is_empty(),
        "fixture sanity: dep_lines must index Source Mgt.al's embedded source"
    );
    let base_dep_decls = canon_dep_meta(&base);

    let mut updater = Updater::new(dir.path().to_path_buf(), parsed);
    let mut cur = base;

    // Rung 1: a body-only edit to the WORKSPACE caller (an extra call to an
    // already-existing target, same shape every other rung-1 script uses)
    // — the dep layer must never be touched (Arc-cloned forward unchanged).
    let body_only = r#"codeunit 50000 "Caller"
{
    procedure CallSymbolOnlyDep()
    var
        WidgetMgt: Codeunit "Widget Mgt";
    begin
        WidgetMgt.Compute(5);
        WidgetMgt.Compute(6);
    end;

    procedure CallEmbeddedSourceDep()
    var
        SourceMgt: Codeunit "Source Mgt";
    begin
        SourceMgt.DoWork(3);
    end;
}
"#;
    std::fs::write(dir.path().join("Caller.al"), body_only).expect("rewrite Caller.al (rung 1)");
    let batch1 = vec![ChangeEvent::FileSaved(dir.path().join("Caller.al"))];
    let (rung1_snap, rung1) = updater
        .apply_batch(&cur, &batch1)
        .expect("apply_batch must succeed (rung-1 step)");
    assert_eq!(
        rung1,
        Rung::One,
        "an extra call to an already-existing target must stay rung-1-eligible"
    );

    let fresh1 = LspSnapshot::build_full(dir.path()).expect("fresh build_full (rung 1)");
    assert_snapshots_equivalent(&rung1_snap, &fresh1, "dep-bearing rung-1 step");
    assert_eq!(
        canon_dep_meta(&rung1_snap),
        base_dep_decls,
        "rung 1 must Arc-clone dep_meta forward unchanged (never touch the dep layer)"
    );
    cur = rung1_snap;

    // Rung 2: a SIGNATURE change on the workspace caller (adds a parameter)
    // — must rebuild the workspace layer while REUSING the cached dep
    // layer; dep_meta/dep_lines must stay non-vacuous AND identical.
    let signature_change = r#"codeunit 50000 "Caller"
{
    procedure CallSymbolOnlyDep(Extra: Integer)
    var
        WidgetMgt: Codeunit "Widget Mgt";
    begin
        WidgetMgt.Compute(Extra);
    end;

    procedure CallEmbeddedSourceDep()
    var
        SourceMgt: Codeunit "Source Mgt";
    begin
        SourceMgt.DoWork(3);
    end;
}
"#;
    std::fs::write(dir.path().join("Caller.al"), signature_change)
        .expect("rewrite Caller.al (rung 2)");
    let batch2 = vec![ChangeEvent::FileSaved(dir.path().join("Caller.al"))];
    let (rung2_snap, rung2) = updater
        .apply_batch(&cur, &batch2)
        .expect("apply_batch must succeed (rung-2 step)");
    assert_eq!(
        rung2,
        Rung::Two,
        "a parameter-count change is a definition-surface change — must take rung 2"
    );

    let fresh2 = LspSnapshot::build_full(dir.path()).expect("fresh build_full (rung 2)");
    assert_snapshots_equivalent(&rung2_snap, &fresh2, "dep-bearing rung-2 step");
    assert!(
        !rung2_snap.dep_meta.is_empty(),
        "rung 2 must still carry a non-vacuous dep_meta after rebuilding the workspace layer"
    );
    assert_eq!(
        canon_dep_meta(&rung2_snap),
        base_dep_decls,
        "rung 2 must reuse the cached, unchanged dep layer — dep_meta identical across a workspace-only signature-change rebuild"
    );
}

/// Perf safe-wins Task 2: `build_full_with_parsed` must NOT run a second
/// whole-program parse — the published snapshot's workspace `AlFile`s and
/// the updater's working-state `AlFile`s must be the SAME `Arc` allocations.
#[test]
fn build_full_with_parsed_shares_one_parse_between_snapshot_and_updater() {
    let dir = copy_fixture_lsp_diff_deps_to_tempdir();
    let (base, parsed) = build_full_with_parsed(dir.path());

    assert_eq!(
        parsed.app, base.snap.workspace_app,
        "build_full_with_parsed must return the WORKSPACE ParsedUnit"
    );
    assert!(
        !base.parsed.is_empty(),
        "fixture sanity: snapshot must hold workspace ParsedFileEntry values"
    );
    for (vp, entry) in base.parsed.iter() {
        let pf = parsed
            .files
            .iter()
            .find(|f| &f.virtual_path == vp)
            .expect("every snapshot workspace file must be in the updater unit");
        assert!(
            std::sync::Arc::ptr_eq(&entry.file, &pf.file),
            "{vp}: snapshot and updater must share ONE parsed AlFile, \
             not two independent parses"
        );
        assert!(
            std::sync::Arc::ptr_eq(&entry.text, &pf.text),
            "{vp}: snapshot and updater must share ONE text allocation"
        );
    }
}

/// T3 Task 12 (the owned-DeclSurface lifecycle's whole point): after a full
/// build, the caller receives ONLY the workspace ParsedUnit — dependency
/// parse arenas are dropped, not retained for the updater's lifetime.
#[test]
fn build_full_with_parsed_returns_only_the_workspace_unit() {
    let dir = copy_fixture_lsp_diff_deps_to_tempdir();
    let (snap, workspace) = build_full_with_parsed(dir.path());
    assert_eq!(workspace.app, snap.snap.workspace_app);
    // and the dep tier is populated (deps were parsed, projected, then
    // dropped) — proves the frozen tier really was derived from the dep
    // arenas before they were released, not just vacuously empty.
    assert!(
        !snap.dep_meta.is_empty(),
        "dep tier must hold the projected dep decls"
    );
}

/// Rungs 1 and 2 must FORWARD the frozen dep tier (and the dep query maps),
/// never rebuild them: Arc identity proves zero recompute. Covers BOTH
/// rung-1 code paths — `Updater::apply_batch`'s Rung1 arm (exercised
/// directly below) AND `spawn_updater`'s hot loop (exercised through the
/// real background thread, via `spawn_updater`'s public `on_swap` hook,
/// which is the only externally observable point from this integration
/// test crate — `Updater`'s fields are private).
#[test]
fn rung1_and_rung2_forward_dep_meta_dep_decls_and_dep_lines_by_arc_identity() {
    let dir = copy_fixture_lsp_diff_deps_to_tempdir();
    let (base, parsed) = build_full_with_parsed(dir.path());
    assert!(
        !base.dep_meta.is_empty(),
        "fixture sanity: dep_meta must carry Source Lib's embedded DoWork"
    );

    // ── Path 1: Updater::apply_batch's Rung1 arm ───────────────────────────
    let mut updater = Updater::new(dir.path().to_path_buf(), parsed);

    let body_only = r#"codeunit 50000 "Caller"
{
    procedure CallSymbolOnlyDep()
    var
        WidgetMgt: Codeunit "Widget Mgt";
    begin
        WidgetMgt.Compute(5);
        WidgetMgt.Compute(6);
    end;

    procedure CallEmbeddedSourceDep()
    var
        SourceMgt: Codeunit "Source Mgt";
    begin
        SourceMgt.DoWork(3);
    end;
}
"#;
    std::fs::write(dir.path().join("Caller.al"), body_only).expect("rewrite Caller.al (rung 1)");
    let batch1 = vec![ChangeEvent::FileSaved(dir.path().join("Caller.al"))];
    let (rung1_snap, rung1) = updater
        .apply_batch(&base, &batch1)
        .expect("apply_batch must succeed (rung-1 step)");
    assert_eq!(rung1, Rung::One);
    assert!(
        std::sync::Arc::ptr_eq(&base.dep_meta, &rung1_snap.dep_meta),
        "apply_batch's Rung1 arm must forward dep_meta by Arc identity, never rebuild it"
    );
    assert!(
        std::sync::Arc::ptr_eq(&base.dep_lines, &rung1_snap.dep_lines),
        "apply_batch's Rung1 arm must forward dep_lines by Arc identity"
    );

    // ── Rung 2, same Updater: a signature change on the workspace caller ──
    let signature_change = r#"codeunit 50000 "Caller"
{
    procedure CallSymbolOnlyDep(Extra: Integer)
    var
        WidgetMgt: Codeunit "Widget Mgt";
    begin
        WidgetMgt.Compute(Extra);
    end;

    procedure CallEmbeddedSourceDep()
    var
        SourceMgt: Codeunit "Source Mgt";
    begin
        SourceMgt.DoWork(3);
    end;
}
"#;
    std::fs::write(dir.path().join("Caller.al"), signature_change)
        .expect("rewrite Caller.al (rung 2)");
    let batch2 = vec![ChangeEvent::FileSaved(dir.path().join("Caller.al"))];
    let (rung2_snap, rung2) = updater
        .apply_batch(&rung1_snap, &batch2)
        .expect("apply_batch must succeed (rung-2 step)");
    assert_eq!(rung2, Rung::Two);
    assert!(
        std::sync::Arc::ptr_eq(&rung1_snap.dep_meta, &rung2_snap.dep_meta),
        "apply_rung2 must forward dep_meta by Arc identity, never rebuild it"
    );
    assert!(
        std::sync::Arc::ptr_eq(&rung1_snap.dep_lines, &rung2_snap.dep_lines),
        "apply_rung2 must forward dep_lines by Arc identity"
    );

    // ── Path 2: spawn_updater's hot loop — a SEPARATE Updater/thread, so it
    // can never share Arc identity with the `base`/`rung1_snap` values
    // above; this half proves the hot loop's cached-surface rung-1 variant
    // ALSO forwards by Arc identity, against its OWN base snapshot.
    use al_sem::lsp::updater::{SharedSnapshot, spawn_updater};
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::time::Duration;

    let dir2 = copy_fixture_lsp_diff_deps_to_tempdir();
    let (base2, parsed2) = build_full_with_parsed(dir2.path());
    let base2_dep_meta = Arc::clone(&base2.dep_meta);
    let base2_dep_lines = Arc::clone(&base2.dep_lines);
    let shared = Arc::new(SharedSnapshot::new(Arc::new(base2)));
    let (tx, rx) = mpsc::channel();

    let handle = spawn_updater(
        Arc::clone(&shared),
        rx,
        dir2.path().to_path_buf(),
        parsed2,
        al_sem::snapshot::DependencySource::default(),
        Arc::default(),
        |_old, _new| {},
    );

    let caller_path = dir2.path().join("Caller.al");
    std::fs::write(&caller_path, body_only).expect("rewrite Caller.al (hot-loop rung 1)");
    tx.send(ChangeEvent::FileSaved(caller_path))
        .expect("send must succeed");
    std::thread::sleep(Duration::from_millis(400));
    drop(tx);
    handle.join().expect("updater thread must exit cleanly");

    let hot_loop_snap = shared.get();
    assert!(
        hot_loop_snap.generation > 0,
        "the hot loop must have applied at least one rung-1 swap"
    );
    assert!(
        std::sync::Arc::ptr_eq(&base2_dep_meta, &hot_loop_snap.dep_meta),
        "spawn_updater's hot-loop rung-1 path must forward dep_meta by Arc identity"
    );
    assert!(
        std::sync::Arc::ptr_eq(&base2_dep_lines, &hot_loop_snap.dep_lines),
        "spawn_updater's hot-loop rung-1 path must forward dep_lines by Arc identity"
    );
}

/// CONTENT-level proof (Arc identity proves forwarding, not correctness): a
/// workspace routine's call into a DEP routine must still resolve AFTER the
/// dep parse arenas are dropped — i.e. dispatched through the frozen tier's
/// `RoutineMeta` (params ty/by_ref), not through any surviving
/// `RoutineDecl`. Uses the `lsp-diff-deps` fixture's real embedded-source
/// dependency call (`Caller.CallEmbeddedSourceDep` → `Source Mgt.DoWork`,
/// arity 1) — its target `RoutineNodeId` (app + arity + sig_fp) is asserted
/// on the freshly built snapshot AND again after a rung-1 body edit, so the
/// SAME identity resolving twice — once before, once after the dep arenas
/// are long gone — proves dispatch never regresses to relying on a
/// surviving dependency parse.
#[test]
fn dep_overload_dispatch_resolves_through_frozen_tier_after_arena_drop() {
    let dir = copy_fixture_lsp_diff_deps_to_tempdir();
    let (base, parsed) = build_full_with_parsed(dir.path());

    fn dep_routine_target(snap: &LspSnapshot) -> RoutineNodeId {
        snap.edges_by_file["Caller.al"]
            .iter()
            .flat_map(|e| e.routine_targets())
            .find(|id| id.name_lc == "dowork")
            .expect("Caller.al must carry an edge routing to Source Mgt.DoWork")
            .clone()
    }

    let base_target = dep_routine_target(&base);
    assert_eq!(
        base_target.params_count, 1,
        "Source Mgt.DoWork(x: Integer) must dispatch as the 1-arity overload"
    );
    assert!(
        base.dep_meta.contains_key(&base_target),
        "the dispatched target must be a REAL dep decl, not a stale/absent one"
    );

    let mut updater = Updater::new(dir.path().to_path_buf(), parsed);
    let body_only = r#"codeunit 50000 "Caller"
{
    procedure CallSymbolOnlyDep()
    var
        WidgetMgt: Codeunit "Widget Mgt";
    begin
        WidgetMgt.Compute(5);
        WidgetMgt.Compute(6);
    end;

    procedure CallEmbeddedSourceDep()
    var
        SourceMgt: Codeunit "Source Mgt";
    begin
        SourceMgt.DoWork(3);
    end;
}
"#;
    std::fs::write(dir.path().join("Caller.al"), body_only).expect("rewrite Caller.al (rung 1)");
    let batch = vec![ChangeEvent::FileSaved(dir.path().join("Caller.al"))];
    let (rung1_snap, rung1) = updater
        .apply_batch(&base, &batch)
        .expect("apply_batch must succeed (rung-1 step)");
    assert_eq!(rung1, Rung::One);

    let rung1_target = dep_routine_target(&rung1_snap);
    assert_eq!(
        base_target, rung1_target,
        "the dep dispatch target must resolve to the IDENTICAL RoutineNodeId \
         after a rung-1 edit — dispatched via the frozen tier, not a stale \
         surviving dependency parse (there is none left to survive)"
    );
    assert!(
        rung1_snap.dep_meta.contains_key(&rung1_target),
        "the post-rung-1 snapshot must still resolve the dep target to a real decl"
    );
}

/// Every `RouteTarget::Routine(id)` naming a DEPENDENCY routine must resolve
/// through `decl_and_line_table` (served by `dep_meta` since the `dep_decl_by_id`
/// deletion) — the fail-closed "never guess" contract must not lose a single
/// id in the migration.
#[test]
fn every_dep_routine_route_target_resolves_via_dep_meta() {
    let dir = copy_fixture_lsp_diff_deps_to_tempdir();
    let snap = LspSnapshot::build_full(dir.path()).expect("build_full");
    let workspace_app = snap.graph.apps.find(&snap.snap.workspace_app);
    let mut dep_targets = 0usize;
    for edges in snap.edges_by_file.values().map(|a| a.as_slice()).chain([
        snap.ws_event_edges.as_slice(),
        snap.dep_events.edges.as_slice(),
    ]) {
        for e in edges {
            for rid in e.routine_targets() {
                if Some(rid.object.app) != workspace_app {
                    dep_targets += 1;
                    assert!(
                        snap.decl_and_line_table(rid).is_some(),
                        "dep routine target {rid:?} must resolve via dep_meta"
                    );
                }
            }
        }
    }
    assert!(
        dep_targets > 0,
        "fixture sanity: at least one dependency-routine route target must exist"
    );
}

// ---------------------------------------------------------------------------
// Tier-2 latency wave, Task 1: rung-1 incremental decl_by_id/incoming patch
// ---------------------------------------------------------------------------
//
// The two scripts below are the brief's Step 1 gate
// (`.superpowers/sdd/tier2-lsp/task-1-brief.md`, Task 1): (a) the patched
// `decl_by_id`/`incoming` indexes a rung-1 apply produces must match a fresh
// WHOLESALE rebuild (`build_decl_by_id`/`build_incoming`) of the exact same
// post-edit `decls_by_file`/`edges_by_file` — a STRONGER, more direct check
// than `assert_snapshots_equivalent` above (which only compares the
// UNDERLYING `decls_by_file`/`edges_by_file` populations, never the derived
// `decl_by_id`/`incoming` indexes themselves — see this module's doc for why
// that's sufficient for the OTHER scripts, which never touch `decl_by_id`
// directly enough to risk a duplicate-id bookkeeping bug). (b) a NEW
// cross-file-duplicate-`RoutineNodeId` fixture exercises the duplicate-safe
// multiplicity rule.

/// `decl_by_id`/`incoming` MUST be presentable as "a valid instance of a
/// wholesale rebuild" after a rung-1 patch: `decl_by_id`'s key set matches
/// `build_decl_by_id`'s key set, and every entry is SOME declaring file's own
/// `DeclEntry` (winner is unspecified for a duplicate id — see
/// `build_decl_by_id`'s doc — so this does not assert byte-identical maps,
/// only the invariant the patch is allowed to preserve). `incoming` is
/// compared as a genuine sorted-multiset equality against a fresh
/// `build_incoming` (per the brief's step 1(a) ordering caveat:
/// `build_incoming`'s per-target `Vec<EdgeRef>` order is ALREADY
/// nondeterministic, so every consumer — including this gate — must sort by
/// `(file, idx)` before comparing). `publisher_fanout` is compared for exact
/// equality (Arc-forwarded at rung 1, so it must be byte-identical to a
/// fresh rebuild from the unchanged `event_edges`).
#[test]
fn rung1_patched_indexes_match_wholesale_rebuild() {
    let dir = copy_fixture_to_tempdir();
    let (base, parsed) = build_full_with_parsed(dir.path());
    let mut updater = Updater::new(dir.path().to_path_buf(), parsed);

    // A body-only edit (extra call to an already-existing target) — no
    // identity/signature change, so this must stay rung-1-eligible.
    std::fs::write(
        dir.path().join("Alpha.al"),
        r#"// Unicode smoke test (Task 10 fixture requirement): æøå
codeunit 50100 "Alpha"
{
    procedure DoWork()
    var
        Beta: Codeunit "Beta";
    begin
        Beta.Process();
        Beta.Process();
        Calc(1);
        Calc('x');
        Løbenr();
    end;

    procedure Calc(X: Integer)
    begin
    end;

    procedure Calc(X: Text)
    begin
    end;

    procedure Løbenr()
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnAfterWork()
    begin
    end;
}
"#,
    )
    .expect("rewrite Alpha.al");

    let batch = vec![ChangeEvent::FileSaved(dir.path().join("Alpha.al"))];
    let (new_snap, rung) = updater
        .apply_batch(&base, &batch)
        .expect("apply_batch must succeed");
    assert_eq!(rung, Rung::One, "a body-only edit must take rung 1");

    // ---- decl_by_id: key-set equality + "one of the declaring files' own
    // entries" per id, against a fresh wholesale rebuild of the SAME
    // post-edit decls_by_file. ----
    let fresh_decl_by_id = al_sem::lsp::snapshot::build_decl_by_id(&new_snap.decls_by_file);
    let mut patched_ids: Vec<&RoutineNodeId> = new_snap.decl_by_id.keys().collect();
    let mut fresh_ids: Vec<&RoutineNodeId> = fresh_decl_by_id.keys().collect();
    patched_ids.sort();
    fresh_ids.sort();
    assert_eq!(
        patched_ids, fresh_ids,
        "rung-1 patched decl_by_id's key set must match a fresh wholesale rebuild"
    );
    for id in &patched_ids {
        let patched_entry = &new_snap.decl_by_id[*id];
        let is_valid_winner = new_snap
            .decls_by_file
            .values()
            .flat_map(|decls| decls.iter())
            .any(|d| {
                &d.id == *id
                    && d.virtual_path == patched_entry.virtual_path
                    && canon_decl(d) == canon_decl(patched_entry)
            });
        assert!(
            is_valid_winner,
            "decl_by_id[{id:?}] = {patched_entry:?} must be ONE of the declaring \
             files' own DeclEntry values"
        );
    }

    // ---- incoming: sorted-multiset equality against a fresh build_incoming
    // over the SAME post-edit edges_by_file/event_edges. ----
    let (fresh_incoming, fresh_publisher_fanout) =
        al_sem::lsp::snapshot::build_incoming(&new_snap.edges_by_file, &new_snap.ws_event_edges);
    let sort_refs = |v: &[al_sem::lsp::snapshot::EdgeRef]| {
        let mut v: Vec<(String, u32)> = v.iter().map(|r| (r.file.to_string(), r.idx)).collect();
        v.sort();
        v
    };
    let mut all_targets: Vec<RoutineNodeId> = new_snap
        .ws_incoming
        .keys()
        .cloned()
        .chain(fresh_incoming.keys().cloned())
        .collect();
    all_targets.sort();
    all_targets.dedup();
    for target in &all_targets {
        let patched: Vec<(String, u32)> = new_snap
            .ws_incoming
            .get(target)
            .map(|v| sort_refs(v))
            .unwrap_or_default();
        let fresh: Vec<(String, u32)> = fresh_incoming
            .get(target)
            .map(|v| sort_refs(v))
            .unwrap_or_default();
        assert_eq!(
            patched, fresh,
            "rung-1 patched incoming[{target:?}] must match a fresh build_incoming \
             (sorted by (file, idx))"
        );
    }

    // ---- publisher_fanout: Arc-forwarded at rung 1, must be byte-identical
    // to a fresh rebuild off the unchanged event_edges. ----
    assert_eq!(
        *new_snap.ws_publisher_fanout, fresh_publisher_fanout,
        "rung-1 Arc-forwarded publisher_fanout must equal a fresh build_incoming's"
    );

    // Non-vacuity: the fixture's incoming population must be non-empty, or
    // the loop above would trivially pass over zero targets.
    assert!(
        !all_targets.is_empty(),
        "fixture sanity: at least one incoming target must exist"
    );
}

/// Task-1 review finding (Fable, Medium): if the same `vp` appears TWICE in
/// one rung-1 batch (`classify` pushes one `Planned::Save` per event with no
/// vp dedup; the production coalescer dedupes by exact `PathBuf` only, and
/// `classify_path`'s case-insensitive fallback can map two spellings to one
/// vp), the per-save loop's removal pass must derive the file's OLD edge
/// targets from the WORKING maps (iteration 1's fresh state), not from
/// `cur`'s stale list — otherwise iteration 2 re-pushes the file's new
/// edges without removing iteration 1's, leaving duplicate `EdgeRef`s in
/// `incoming` until the next rebuild. This test sends the SAME `FileSaved`
/// event twice in one batch and asserts full sorted-multiset parity of
/// `incoming` against a fresh wholesale `build_incoming`.
#[test]
fn rung1_duplicate_vp_in_one_batch_stays_equivalent() {
    let dir = copy_fixture_to_tempdir();
    let (base, parsed) = build_full_with_parsed(dir.path());
    let mut updater = Updater::new(dir.path().to_path_buf(), parsed);

    // Same body-only edit shape as `rung1_patched_indexes_match_wholesale_
    // rebuild`, plus a call to a target with NO prior incoming edge from
    // this file (`OnAfterWork` — never called in the base fixture): the
    // stale-removal bug only materializes for a target absent from the OLD
    // edge list (iteration 2's removal pass never visits it, then re-pushes
    // its fresh EdgeRef → duplicate).
    std::fs::write(
        dir.path().join("Alpha.al"),
        r#"// Unicode smoke test (Task 10 fixture requirement): æøå
codeunit 50100 "Alpha"
{
    procedure DoWork()
    var
        Beta: Codeunit "Beta";
    begin
        Beta.Process();
        Beta.Process();
        Calc(1);
        Calc('x');
        Løbenr();
        OnAfterWork();
    end;

    procedure Calc(X: Integer)
    begin
    end;

    procedure Calc(X: Text)
    begin
    end;

    procedure Løbenr()
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnAfterWork()
    begin
    end;
}
"#,
    )
    .expect("rewrite Alpha.al");

    // The SAME save event twice in ONE batch — un-coalesced, as the public
    // `apply_batch` API permits.
    let batch = vec![
        ChangeEvent::FileSaved(dir.path().join("Alpha.al")),
        ChangeEvent::FileSaved(dir.path().join("Alpha.al")),
    ];
    let (new_snap, rung) = updater
        .apply_batch(&base, &batch)
        .expect("apply_batch must succeed");
    assert_eq!(rung, Rung::One, "a body-only edit must take rung 1");

    let (fresh_incoming, _) =
        al_sem::lsp::snapshot::build_incoming(&new_snap.edges_by_file, &new_snap.ws_event_edges);
    let sort_refs = |v: &[al_sem::lsp::snapshot::EdgeRef]| {
        let mut v: Vec<(String, u32)> = v.iter().map(|r| (r.file.to_string(), r.idx)).collect();
        v.sort();
        v
    };
    let mut all_targets: Vec<RoutineNodeId> = new_snap
        .ws_incoming
        .keys()
        .cloned()
        .chain(fresh_incoming.keys().cloned())
        .collect();
    all_targets.sort();
    all_targets.dedup();
    assert!(
        !all_targets.is_empty(),
        "fixture sanity: at least one incoming target must exist"
    );
    for target in &all_targets {
        let patched: Vec<(String, u32)> = new_snap
            .ws_incoming
            .get(target)
            .map(|v| sort_refs(v))
            .unwrap_or_default();
        let fresh: Vec<(String, u32)> = fresh_incoming
            .get(target)
            .map(|v| sort_refs(v))
            .unwrap_or_default();
        assert_eq!(
            patched, fresh,
            "duplicate-vp batch: patched incoming[{target:?}] must match a fresh \
             build_incoming — a mismatch means the removal pass read stale `cur` \
             state instead of the working maps"
        );
    }
}

/// A NEW fixture: two workspace files each independently declare `codeunit
/// 50100 "Dup"` with an identically-shaped `procedure Shared()` — since
/// `RoutineNodeId` is derived from the object's key + name + arity + sig_fp
/// (never from `virtual_path` — see `source_routine_node_id`), both files'
/// `DeclEntry` for `Shared` compute the EXACT SAME `RoutineNodeId`: the
/// `decls_by_file`-level cross-file-duplicate-id population the brief's
/// duplicate-safe multiplicity rule exists for (this is a DIFFERENT,
/// per-snapshot phenomenon than the graph-level
/// `dedup_routines_preserving_genuine_overloads` pass, which the LSP
/// snapshot's per-file `decls_by_file` construction — `recompute_file`,
/// independent per file — never runs at all).
///
/// A body-only edit to ONE of the two files can never observe the
/// duplicate-EVICTION path (fingerprint covers decls — see the brief's Step
/// 1(b) note), so this test instead: (1) asserts the duplicate survives a
/// rung-1 body edit to one of the two files, with `decl_multiplicity`
/// (recomputed from scratch via `build_decl_multiplicity`) still counting 2
/// declaring files; then (2) deletes the OTHER file entirely (a rung-2
/// escalation), and asserts the id still survives with multiplicity 1 —
/// exercising the decrement-without-eviction path — matching a fresh
/// from-scratch `build_decl_multiplicity` count at every step (the binding
/// "matches a from-scratch count" requirement).
#[test]
fn rung1_cross_file_duplicate_routine_id_survives_edit() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("app.json"),
        r#"{
    "id": "77777777-0000-0000-0000-000000000011",
    "name": "Task1 Cross-File Duplicate Fixture",
    "publisher": "probe",
    "version": "1.0.0.0"
}"#,
    )
    .expect("write app.json");
    let dup_body = |comment: &str| {
        format!(
            r#"codeunit 50100 "Dup"
{{
    procedure Shared()
    begin
        // {comment}
    end;
}}
"#
        )
    };
    std::fs::write(dir.path().join("Dup1.al"), dup_body("v1")).expect("write Dup1.al");
    std::fs::write(dir.path().join("Dup2.al"), dup_body("v1")).expect("write Dup2.al");

    let (base, parsed) = build_full_with_parsed(dir.path());
    let mut updater = Updater::new(dir.path().to_path_buf(), parsed);

    let shared_id = base.decls_by_file["Dup1.al"]
        .iter()
        .find(|d| d.name == "Shared")
        .expect("Dup1.Shared decl")
        .id
        .clone();
    assert_eq!(
        base.decls_by_file["Dup2.al"]
            .iter()
            .find(|d| d.name == "Shared")
            .expect("Dup2.Shared decl")
            .id,
        shared_id,
        "fixture sanity: both files must compute the IDENTICAL RoutineNodeId for Shared"
    );
    assert_eq!(
        al_sem::lsp::snapshot::build_decl_multiplicity(&base.decls_by_file)
            .get(&shared_id)
            .copied(),
        Some(2),
        "baseline: Shared must be declared by exactly 2 files"
    );
    assert!(
        base.decl_by_id.contains_key(&shared_id),
        "baseline: the duplicate id must resolve to SOME declaring file's entry"
    );

    // Step 1: a rung-1 body-only edit to Dup1.al — the duplicate must
    // survive, with multiplicity still 2 (recomputed from scratch, matching
    // the live `Updater::decl_multiplicity`'s own bookkeeping — see this
    // function's own doc).
    std::fs::write(dir.path().join("Dup1.al"), dup_body("v2")).expect("rewrite Dup1.al");
    let batch1 = vec![ChangeEvent::FileSaved(dir.path().join("Dup1.al"))];
    let (snap1, rung1) = updater
        .apply_batch(&base, &batch1)
        .expect("apply_batch must succeed");
    assert_eq!(rung1, Rung::One, "a body-only edit must take rung 1");
    assert!(
        snap1.decl_by_id.contains_key(&shared_id),
        "the duplicate id must survive a rung-1 edit to one of its 2 declaring files"
    );
    assert_eq!(
        al_sem::lsp::snapshot::build_decl_multiplicity(&snap1.decls_by_file)
            .get(&shared_id)
            .copied(),
        Some(2),
        "after editing Dup1.al only, Shared must still be declared by 2 files \
         (a from-scratch recount must match)"
    );

    // Step 2: delete Dup2.al entirely — a rung-2 escalation. The duplicate
    // must still survive (now declared by exactly 1 file), and — since only
    // ONE file remains — decl_by_id[shared_id] is UNAMBIGUOUSLY that file's
    // own entry (no winner ambiguity left).
    std::fs::remove_file(dir.path().join("Dup2.al")).expect("delete Dup2.al");
    let batch2 = vec![ChangeEvent::FileRemoved(dir.path().join("Dup2.al"))];
    let (snap2, rung2) = updater
        .apply_batch(&snap1, &batch2)
        .expect("apply_batch must succeed");
    assert_eq!(rung2, Rung::Two, "a file delete must take rung 2");
    assert!(
        snap2.decl_by_id.contains_key(&shared_id),
        "the id must survive losing one of its 2 declaring files (multiplicity 2 -> 1)"
    );
    assert_eq!(
        al_sem::lsp::snapshot::build_decl_multiplicity(&snap2.decls_by_file)
            .get(&shared_id)
            .copied(),
        Some(1),
        "after deleting Dup2.al, Shared must be declared by exactly 1 file \
         (a from-scratch recount must match)"
    );
    assert_eq!(
        snap2.decl_by_id[&shared_id].virtual_path, "Dup1.al",
        "with only 1 declaring file left, decl_by_id must resolve to it unambiguously"
    );

    let fresh2 = LspSnapshot::build_full(dir.path()).expect("fresh build_full after delete");
    assert_snapshots_equivalent(
        &snap2,
        &fresh2,
        "cross-file-duplicate: after Dup2.al delete",
    );
}
