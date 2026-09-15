//! Issue 23 — d50's transaction-managing COUNT branch USED TO count
//! TEMP-INCLUSIVE table writes; it now counts PHYSICAL ones, as d8 does. These
//! tests are what pin that, so they are written against the defect they fixed —
//! read the past tense below as describing the bug, not the current code.
//!
//! `d50.rs`'s `is_transaction_managing` has two branches. The NAME branch
//! (`^(Post|Apply|Release)[A-Z]`) is correct and stays. The COUNT branch USED TO
//! call `ConeDerivedStore::writes_tables_count_of`, which is documented
//! temp-INCLUSIVE (`cone_derived.rs:23-24`); d8 calls
//! `writes_physical_tables_count_of` (`cone_derived.rs:25-27`), which excludes
//! `fact_is_known_temp` facts. A routine whose only "writes" are to `temporary`
//! records dirties nothing an implicit commit could split, so counting it as a
//! transaction manager is a false positive by construction.
//!
//! Every test here drives the REAL assembled `ws-d50-temp-gate` workspace. Five
//! of the seven also drive the REGISTERED detector; `a5_...` and `a9_...`
//! deliberately do not — they assert the substrate preconditions (per-writer
//! counts, and span membership) that the other five rest on, by reading
//! `ctx.cone_derived` and `ctx.transaction_spans` directly. The THREE A1 unit
//! tests live beside d50's own native oracles
//! (`src/engine/l5/detectors/d50.rs`).
//!
//! ## Reading a finding's SUBJECT
//!
//! d50's `primary_location` is the checked-Run CALLSITE anchor, whose
//! `enclosing_routine_id` is the SEED routine — the writer that owns the
//! `if Codeunit.Run(...) then;` (`d50.rs:217-235`, `:226`). Every assertion
//! below identifies a finding by that field and never by the manager evidence
//! step, so a change to which routine d50 picks as manager, or to its evidence
//! wording, can never read as suppression.
//!
//! ## The isolation contract, executably (A9)
//!
//! `span_of` and `a9_each_checked_span_holds_exactly_its_own_writer` assert that
//! each writer's CheckedRunImplicit span contains EXACTLY that writer. Without
//! it, a common AL driver calling two writers would inherit `StageRows`' three
//! PHYSICAL writes through its forward capability cone, appear in `BufferRows`'
//! BACKWARD span as an accepted manager, and keep d50 reporting at `BufferRows`'
//! callsite after the fix — an A2 failure with nothing to do with the change.

use std::path::{Path, PathBuf};

use al_sem::engine::l3::l3_workspace::{
    L3Resolved, L3Routine, assemble_and_resolve_workspace_default,
};
use al_sem::engine::l5::detector_context::{DetectorContext, build_detector_context};
use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::{run_detectors, substrate};
use al_sem::engine::l5::transaction_spans::{SeedKind, TransactionSpan};

const FIXTURE: &str = "ws-d50-temp-gate";
const DETECTOR: &str = "d50-checked-run-implicit-commit";
/// `d50.rs`'s `TRANSACTION_THRESHOLD_TABLES` (private there).
const THRESHOLD: usize = 3;

/// The four writer procedures, and the inclusive / physical write counts the
/// fixture is built to produce. Per-routine `temporary` declarations decide
/// temp-ness; the cone counts are per routine, so sharing the three table
/// OBJECTS across all four writers cannot bleed.
///
/// `(routine name, temp-INCLUSIVE writes, PHYSICAL writes)`
const WRITERS: &[(&str, usize, usize)] = &[
    ("BufferRows", 3, 0),
    ("StageRows", 3, 3),
    ("StageMixedRows", 3, 2),
    ("PostBuffers", 3, 0),
];

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("r0-corpus")
        .join(FIXTURE)
}

fn resolve_fixture() -> L3Resolved {
    let dir = fixture_dir();
    assert!(
        dir.is_dir(),
        "fixture workspace {} is missing",
        dir.display()
    );
    assemble_and_resolve_workspace_default(&dir)
        .unwrap_or_else(|| panic!("workspace assembly returned None for {}", dir.display()))
}

/// The assembled routine id for `name`, asserting the assembly actually produced
/// exactly one routine with that name and that it has a body. This is the
/// anti-degenerate guard: a fixture that failed to parse would assemble zero
/// routines and every "expects no finding" case would pass while analysing
/// nothing.
fn routine_id(resolved: &L3Resolved, name: &str) -> String {
    let matches: Vec<&L3Routine> = resolved
        .workspace
        .routines
        .iter()
        .filter(|r| r.name == name)
        .collect();
    let assembled: Vec<&str> = resolved
        .workspace
        .routines
        .iter()
        .map(|r| r.name.as_str())
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "expected exactly 1 assembled routine named {name:?}; assembled routines: {assembled:?}"
    );
    let r = matches[0];
    assert!(
        r.body_available,
        "routine {name:?} assembled without a body — no callsite, no seed, and every \
         assertion below would be vacuous"
    );
    r.id.clone()
}

/// The assembled name for a routine id, for failure messages.
fn name_of(resolved: &L3Resolved, routine_id: &str) -> String {
    resolved
        .workspace
        .routines
        .iter()
        .find(|r| r.id == routine_id)
        .map(|r| r.name.clone())
        .unwrap_or_else(|| format!("<unknown {routine_id}>"))
}

/// Run the REGISTERED d50 over the fixture.
fn d50_findings(resolved: &L3Resolved) -> Vec<Finding> {
    let selected: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == DETECTOR)
        .collect();
    assert_eq!(
        selected.len(),
        1,
        "{DETECTOR} must be registered exactly once"
    );
    run_detectors(resolved, &selected).findings
}

/// The d50 findings whose PRIMARY CHECKED-CALLSITE anchor is enclosed by
/// `routine_id` — the finding's subject (`d50.rs:217-235`).
fn findings_at<'f>(findings: &'f [Finding], routine_id: &str) -> Vec<&'f Finding> {
    findings
        .iter()
        .filter(|f| f.primary_location.enclosing_routine_id == routine_id)
        .collect()
}

/// A compact rendering of every finding's subject, for failure messages.
fn subjects(resolved: &L3Resolved, findings: &[Finding]) -> Vec<String> {
    findings
        .iter()
        .map(|f| {
            format!(
                "{} (line {}, {})",
                name_of(resolved, &f.primary_location.enclosing_routine_id),
                f.primary_location.start_line,
                f.severity
            )
        })
        .collect()
}

/// The UNIQUE `CheckedRunImplicit` span seeded in `routine_id`.
fn span_of<'c>(ctx: &'c DetectorContext<'_>, routine_id: &str) -> &'c TransactionSpan {
    let spans: Vec<&TransactionSpan> = ctx
        .transaction_spans
        .iter()
        .filter(|s| {
            s.seed_kind == SeedKind::CheckedRunImplicit && s.commit_routine_id == routine_id
        })
        .collect();
    assert_eq!(
        spans.len(),
        1,
        "expected exactly 1 CheckedRunImplicit span seeded in {routine_id}, found {}",
        spans.len()
    );
    spans[0]
}

// ---------------------------------------------------------------------------
// A5 — the COUNT path is genuinely reached, and reaches a POSITIVE result.
// ---------------------------------------------------------------------------

/// Neither non-posting writer's name can match d50's `POSTING_NAME_RE`
/// (`^(Post|Apply|Release)[A-Z]`), so the NAME branch cannot short-circuit and
/// the COUNT branch is what decides them. `posting_name_matches` is private to
/// `d50.rs`, so this asserts the property directly: the names do not begin with
/// any of the three prefixes. Stated honestly, it is a check on the `WRITERS`
/// literal — `name_of(routine_id(name))` round-trips to the same string by
/// construction, so this is not an independent read of the assembled name. It
/// still has teeth: rename the AL procedure and `routine_id` panics before the
/// prefix assertion is ever reached.
///
/// And the counting path must be able to reach a POSITIVE result: each writer's
/// three resolved table ids are DISTINCT (both accessors return sorted,
/// DEDUPLICATED windows, so a fixture writing one table three times would count
/// 1, never 3).
#[test]
fn a5_count_path_is_reached_and_counts_three_distinct_tables() {
    let resolved = resolve_fixture();
    let ctx = build_detector_context(&resolved, substrate::ALL);

    for (name, inclusive, physical) in WRITERS {
        let id = routine_id(&resolved, name);
        let assembled_name = name_of(&resolved, &id);

        if *name == "PostBuffers" {
            assert!(
                assembled_name.starts_with("Post")
                    && assembled_name
                        .chars()
                        .nth(4)
                        .is_some_and(|c| c.is_ascii_uppercase()),
                "PostBuffers must match ^Post[A-Z] so A6 exercises the NAME branch; \
                 assembled name is {assembled_name:?}"
            );
        } else {
            for prefix in ["Post", "Apply", "Release"] {
                assert!(
                    !assembled_name.starts_with(prefix),
                    "writer {assembled_name:?} must not begin with {prefix:?} — the NAME \
                     branch would short-circuit `is_transaction_managing` and the COUNT \
                     branch under test would never be consulted"
                );
            }
        }

        let ids = ctx.cone_derived.writes_tables_of(&id);
        assert_eq!(
            ids.len(),
            *inclusive,
            "{name}: temp-INCLUSIVE written-table ids = {ids:?}, expected {inclusive} distinct"
        );
        let mut distinct = ids.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            ids.len(),
            "{name}: the accessors count DEDUPLICATED ids; the fixture must write \
             {inclusive} DISTINCT tables, got {ids:?}"
        );
        assert_eq!(
            ctx.cone_derived.writes_tables_count_of(&id),
            *inclusive,
            "{name}: writes_tables_count_of must agree with writes_tables_of"
        );
        assert_eq!(
            ctx.cone_derived.writes_physical_tables_count_of(&id),
            *physical,
            "{name}: PHYSICAL written-table count; physical ids = {:?}",
            ctx.cone_derived.writes_physical_tables_of(&id)
        );
    }

    // The positive result the count path must be able to reach: StageRows is at
    // the threshold on BOTH definitions, so the gate is not trivially false.
    let stage = routine_id(&resolved, "StageRows");
    assert!(
        ctx.cone_derived.writes_tables_count_of(&stage) >= THRESHOLD
            && ctx.cone_derived.writes_physical_tables_count_of(&stage) >= THRESHOLD,
        "StageRows must clear TRANSACTION_THRESHOLD_TABLES on both definitions"
    );
}

// ---------------------------------------------------------------------------
// A9 — the isolation contract, executably.
// ---------------------------------------------------------------------------

/// Each writer owns its OWN checked `Codeunit.Run`, and its span contains
/// EXACTLY that writer. No writer calls another, and no common AL driver calls
/// more than one — if one did, it would inherit `StageRows`' three PHYSICAL
/// writes through its forward capability cone and be accepted as a manager in
/// `BufferRows`' backward span, so A2 would fail for a reason unrelated to the
/// gate under test.
#[test]
fn a9_each_checked_span_holds_exactly_its_own_writer() {
    let resolved = resolve_fixture();
    let ctx = build_detector_context(&resolved, substrate::ALL);

    let checked: Vec<&TransactionSpan> = ctx
        .transaction_spans
        .iter()
        .filter(|s| s.seed_kind == SeedKind::CheckedRunImplicit)
        .collect();
    assert_eq!(
        checked.len(),
        WRITERS.len(),
        "expected exactly {} CheckedRunImplicit seeds (one per writer), found {}",
        WRITERS.len(),
        checked.len()
    );

    for (name, _, _) in WRITERS {
        let id = routine_id(&resolved, name);
        let span = span_of(&ctx, &id);
        let membership: Vec<String> = span
            .routines_in_span
            .iter()
            .map(|rid| name_of(&resolved, rid))
            .collect();
        assert_eq!(
            span.routines_in_span,
            vec![id.clone()],
            "{name}'s checked-run span must contain EXACTLY {name} — the isolation \
             contract. Membership: {membership:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A2 — the temp-only writer must not report.
// ---------------------------------------------------------------------------

/// `BufferRows` writes three tables, all declared `Record "..." temporary`:
/// temp-inclusive 3, physical 0. Nothing it writes can be split by an implicit
/// commit, so it is not a transaction manager and its own checked-Run callsite
/// must produce NO d50 finding.
#[test]
fn a2_temp_only_writer_produces_no_finding() {
    let resolved = resolve_fixture();
    let ctx = build_detector_context(&resolved, substrate::ALL);
    let id = routine_id(&resolved, "BufferRows");

    // Non-vacuity: the seed EXISTS. An absent finding must be the gate's
    // decision, not a missing checked-Run callsite.
    let span = span_of(&ctx, &id);
    assert_eq!(
        span.routines_in_span,
        vec![id.clone()],
        "BufferRows' span must be exactly itself before this case means anything"
    );
    assert_eq!(
        ctx.cone_derived.writes_tables_count_of(&id),
        3,
        "precondition: BufferRows writes 3 tables temp-INCLUSIVE"
    );
    assert_eq!(
        ctx.cone_derived.writes_physical_tables_count_of(&id),
        0,
        "precondition: BufferRows writes 0 PHYSICAL tables"
    );

    let findings = d50_findings(&resolved);
    let at = findings_at(&findings, &id);
    assert!(
        at.is_empty(),
        "BufferRows writes 3 TEMPORARY records and 0 physical ones — d50's \
         transaction-managing gate must count PHYSICAL writes (as d8 does), so its \
         checked-Run callsite must produce no finding. Got {} finding(s) there; all \
         d50 subjects: {:?}",
        at.len(),
        subjects(&resolved, &findings)
    );
}

// ---------------------------------------------------------------------------
// A7 — the same-shape mixed control.
// ---------------------------------------------------------------------------

/// `StageMixedRows` is a same-shape clone of `StageRows` differing ONLY in the
/// third local record declaration carrying `temporary`: temp-inclusive 3,
/// physical 2. The narrowing is not limited to temp-ONLY routines — two physical
/// writes plus one temporary also drops below the threshold of 3.
#[test]
fn a7_mixed_writer_below_physical_threshold_produces_no_finding() {
    let resolved = resolve_fixture();
    let ctx = build_detector_context(&resolved, substrate::ALL);
    let id = routine_id(&resolved, "StageMixedRows");

    let span = span_of(&ctx, &id);
    assert_eq!(
        span.routines_in_span,
        vec![id.clone()],
        "StageMixedRows' span must be exactly itself before this case means anything"
    );
    assert_eq!(
        ctx.cone_derived.writes_tables_count_of(&id),
        3,
        "precondition: StageMixedRows writes 3 tables temp-INCLUSIVE — it is ABOVE the \
         threshold on the current definition, which is why it reports today"
    );
    assert_eq!(
        ctx.cone_derived.writes_physical_tables_count_of(&id),
        2,
        "precondition: StageMixedRows writes exactly 2 PHYSICAL tables — BELOW the \
         threshold of {THRESHOLD}"
    );

    let findings = d50_findings(&resolved);
    let at = findings_at(&findings, &id);
    assert!(
        at.is_empty(),
        "StageMixedRows writes only 2 PHYSICAL tables — below \
         TRANSACTION_THRESHOLD_TABLES ({THRESHOLD}) — so its checked-Run callsite must \
         produce no finding. Got {} finding(s) there; all d50 subjects: {:?}",
        at.len(),
        subjects(&resolved, &findings)
    );
}

// ---------------------------------------------------------------------------
// A3 / A6 — the retained population.
// ---------------------------------------------------------------------------

/// `StageRows` writes three PHYSICAL tables — it clears the threshold on BOTH
/// the old and the new definition, so it reports before AND after. The
/// suppression-direction control: without it, "suppress everything" would pass
/// A2 and A7.
#[test]
fn a3_physical_writer_still_reports() {
    let resolved = resolve_fixture();
    let ctx = build_detector_context(&resolved, substrate::ALL);
    let id = routine_id(&resolved, "StageRows");

    assert_eq!(
        ctx.cone_derived.writes_physical_tables_count_of(&id),
        3,
        "precondition: StageRows writes 3 PHYSICAL tables"
    );

    let findings = d50_findings(&resolved);
    let at = findings_at(&findings, &id);
    assert_eq!(
        at.len(),
        1,
        "StageRows writes 3 PHYSICAL tables and must STILL report at its own \
         checked-Run callsite. All d50 subjects: {:?}",
        subjects(&resolved, &findings)
    );
}

/// `PostBuffers` has `BufferRows`' writes — three TEMPORARY records, physical 0
/// — but a posting-style name. The NAME branch is untouched by this change, so
/// it must report before AND after. This is what keeps the fix scoped to the
/// COUNT branch.
#[test]
fn a6_posting_named_temp_only_writer_still_reports_via_the_name_branch() {
    let resolved = resolve_fixture();
    let ctx = build_detector_context(&resolved, substrate::ALL);
    let id = routine_id(&resolved, "PostBuffers");

    assert_eq!(
        ctx.cone_derived.writes_physical_tables_count_of(&id),
        0,
        "precondition: PostBuffers writes 0 PHYSICAL tables, so the COUNT branch cannot \
         make it a manager — only the NAME branch can"
    );

    let findings = d50_findings(&resolved);
    let at = findings_at(&findings, &id);
    assert_eq!(
        at.len(),
        1,
        "PostBuffers matches ^Post[A-Z]; the NAME branch is unchanged, so it must still \
         report even with zero physical writes. All d50 subjects: {:?}",
        subjects(&resolved, &findings)
    );
}

// ---------------------------------------------------------------------------
// The whole-fixture population.
// ---------------------------------------------------------------------------

/// The population, stated once: after the fix the fixture yields EXACTLY the two
/// retained subjects. Before it, all four report. Asserted by subject SET, never
/// by a bare count, so a suppression plus an unrelated new finding cannot cancel
/// out.
#[test]
fn fixture_population_is_exactly_stage_rows_and_post_buffers() {
    let resolved = resolve_fixture();
    let findings = d50_findings(&resolved);

    let mut got: Vec<String> = findings
        .iter()
        .map(|f| name_of(&resolved, &f.primary_location.enclosing_routine_id))
        .collect();
    got.sort();

    assert_eq!(
        got,
        vec!["PostBuffers".to_string(), "StageRows".to_string()],
        "d50 over {FIXTURE} must report at exactly the two checked-Run callsites whose \
         routine is transaction-managing on PHYSICAL writes (StageRows) or by NAME \
         (PostBuffers). Full findings: {:#?}",
        findings
            .iter()
            .map(|f| (&f.id, &f.severity, &f.root_cause))
            .collect::<Vec<_>>()
    );
}
