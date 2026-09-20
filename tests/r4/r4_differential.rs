//! R4 — L5 DETECTOR FINDINGS differential over the SOURCE-ONLY smoke corpus.
//!
//! For each committed al-sem golden under `tests/r4-goldens/<fixture>.r4.golden.json`,
//! run the Rust source-only L0→L3→L5 pass (`assemble_and_resolve_workspace_default(...)`
//! → `engine::l5::finding::project_r4_findings(...)` over the REGISTERED detectors)
//! over the matching `tests/r0-corpus/<fixture>` workspace.
//!
//! ## Wave gating (the ACCEPTANCE GATE)
//!
//! `ported: true` fixtures MUST byte-match their golden END-TO-END. `ported: false`
//! fixtures run cleanly and produce the registered-detector subset (empty for not-yet-
//! ported detectors). Each wave flips its fixture(s) to `ported: true` as they land.
//!
//! ## Anti-degenerate (fail-on-zero)
//!
//! Each ported detector's positive fixture MUST produce ≥1 finding AND byte-match.
//!
//! ## Negative assertions
//!
//! Each new R4-A detector (d5/d10/d11/d18/d21/d36, plus d19/d20/d29) MUST yield 0
//! findings over a neutral fixture that lacks its pattern. R4-B adds d22/d33.
//!
//! ## Divergence comparison
//!
//! The harness performs a direct strict comparison against the committed goldens:
//! any structural divergence found by `diff_value` fails the test, unconditionally,
//! for the ported subset — there is no tolerated-exception mechanism.

use std::collections::BTreeSet;
use std::path::PathBuf;

use al_sem::engine::l3::l3_workspace::assemble_and_resolve_workspace_default;
use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::{
    R4FindingsProjection, project_r4_findings, project_r4_findings_cross_app,
};
use serde_json::Value;

use crate::regen;

/// A smoke entry — the wave representative fixtures (one per substrate wave).
/// These are kept from R4-0 for continuity; the per-detector fixtures below are the
/// actual byte-match entries for R4-A.
///
/// `corpus_dir` — when `Some`, the corpus directory to run against (may differ from
/// the golden name / `fixture`). Used when one workspace produces multiple per-detector
/// goldens (e.g. d9 reuses the `ws-d8-commit-in-tx` corpus but has its own golden).
/// When `None` the corpus directory equals `fixture`.
struct Smoke {
    fixture: &'static str,
    wave: &'static str,
    detectors: &'static [&'static str],
    ported: bool,
    /// Optional: the corpus dir to run. Defaults to `fixture` when `None`.
    corpus_dir: Option<&'static str>,
}

/// The wave-representative smoke set (one per substrate wave).
const SMOKE: &[Smoke] = &[
    Smoke {
        fixture: "ws-d4-repeated-get",
        wave: "R4-A",
        detectors: &["d4-repeated-lookup-in-loop"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-d22",
        wave: "R4-B",
        detectors: &["d22-flowfield-without-calcfields"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-d12-dead-event",
        wave: "R4-C",
        detectors: &["d12-dead-integration-event"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-d8-commit-in-tx",
        wave: "R4-D",
        detectors: &["d8-commit-in-transaction"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-d3",
        wave: "R4-E",
        detectors: &["d3-missing-setloadfields"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-txn-d47-pos-http-nocommit",
        wave: "R4-F",
        detectors: &["d47-io-unsafe-txn"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-d14-dead-routine",
        wave: "R4-G",
        detectors: &["d14-dead-routine"],
        ported: true,
        corpus_dir: None,
    },
];

/// R4-H per-detector fixtures (d50 checked-run-implicit-commit). d50 is OPT-IN
/// (advisory, info/medium). ws-d50-pos → ≥1 finding (byte-matched); ws-d50-neg → 0
/// findings (byte-matched, EXEMPT from anti-degenerate ≥1 check).
const WAVE_H_POSITIVE: &[Smoke] = &[
    Smoke {
        fixture: "ws-d50-pos",
        wave: "R4-H",
        detectors: &["d50-checked-run-implicit-commit"],
        ported: true,
        corpus_dir: None,
    },
    // Issue 23 — d50's transaction-managing COUNT branch must count PHYSICAL
    // table writes, not temp-inclusive ones. FOUR mutually-independent writers
    // over the same three tables: `BufferRows` (3 temp / 0 physical) and
    // `StageMixedRows` (3 / 2) must STOP reporting; `StageRows` (3 / 3) and
    // `PostBuffers` (3 / 0, retained by the NAME branch) must keep reporting.
    // ONE combined fixture: it produces >= 1 finding, so it satisfies the
    // anti-degenerate check below without needing the negatives list.
    Smoke {
        fixture: "ws-d50-temp-gate",
        wave: "R4-H",
        detectors: &["d50-checked-run-implicit-commit"],
        ported: true,
        corpus_dir: None,
    },
    // Issue 18 — d50's MEDIUM tier and its medium→info DEMOTION. Three
    // independent codeunits: a `local procedure` committer (no root
    // classification → UNCAPPED → medium), its ONE-LINE TWIN whose committer is
    // a `trigger OnRun()` (`onrun-codeunit` is in `D50_UNTRUSTED_ROOT_KINDS` →
    // CAPPED → info), and a control carrying BOTH kinds of committer on one span
    // (the ANY quantifier short-circuits on the uncapped one → medium). Before
    // this fixture no corpus fixture put an explicit `Commit()` anywhere near a
    // checked-Run span, so `has_proven_effective_explicit_commit` was false by
    // vacuity and that list could be edited without moving a single golden.
    Smoke {
        fixture: "ws-d50-medium",
        wave: "R4-H",
        detectors: &["d50-checked-run-implicit-commit"],
        ported: true,
        corpus_dir: None,
    },
];

const WAVE_H_NEGATIVES: &[Smoke] = &[Smoke {
    fixture: "ws-d50-neg",
    wave: "R4-H",
    detectors: &["d50-checked-run-implicit-commit"],
    ported: true,
    corpus_dir: None,
}];

/// R4 CROSS-APP per-detector fixtures (d13/d16/d17). Each fixture resolves a cross-app
/// call into a COMMITTED dependency `.app` under `tests/r0-corpus/<fixture>/.alpackages/`.
/// These run through the CROSS-APP L5 pipeline (`project_r4_findings_cross_app`) which
/// reads the dep `.app` off disk — NOT the source-only `project_r4_findings` path.
const WAVE_CROSS_APP: &[Smoke] = &[
    Smoke {
        fixture: "ws-d13-internal-call",
        wave: "R4-CROSS-APP",
        detectors: &["d13-cross-app-internal-call"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-d13-member-call",
        wave: "R4-CROSS-APP",
        detectors: &["d13-cross-app-internal-call"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-d16-obsolete",
        wave: "R4-CROSS-APP",
        detectors: &["d16-obsolete-routine-call"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-d17-drift",
        wave: "R4-CROSS-APP",
        detectors: &["d17-min-version-drift"],
        ported: true,
        corpus_dir: None,
    },
];

/// BCQuality wave (d52–d64) per-detector fixtures. Each fixture contains both
/// flagged and deliberately-unflagged cases; the golden byte-match pins the
/// exact finding set.
const WAVE_BCQ: &[Smoke] = &[
    Smoke {
        fixture: "ws-d52",
        wave: "R4-BCQ",
        detectors: &["d52-bulk-write-param-no-temp-guard"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-d53",
        wave: "R4-BCQ",
        detectors: &["d53-ignored-tryfunction-result"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-d54",
        wave: "R4-BCQ",
        detectors: &["d54-publish-in-tryfunction-cone"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-d55",
        wave: "R4-BCQ",
        detectors: &["d55-event-publish-in-loop"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-d56",
        wave: "R4-BCQ",
        detectors: &["d56-clone-before-write-in-loop"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-d57",
        wave: "R4-BCQ",
        detectors: &["d57-singleinstance-growing-state"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-d58",
        wave: "R4-BCQ",
        detectors: &["d58-query-filter-after-open"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-d59",
        wave: "R4-BCQ",
        detectors: &["d59-integrationevent-var-boolean-guard"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-d60",
        wave: "R4-BCQ",
        detectors: &["d60-upgrade-loop-should-be-datatransfer"],
        ported: true,
        corpus_dir: None,
    },
    // d61 is OPT-IN (registry after d51) — still runs here via run_smoke_entry's
    // by-name selection.
    Smoke {
        fixture: "ws-d61",
        wave: "R4-BCQ",
        detectors: &["d61-ishandled-bypasses-critical-write"],
        ported: true,
        corpus_dir: None,
    },
    // d62 is OPT-IN (registry after d61) — still runs here via run_smoke_entry's
    // by-name selection.
    Smoke {
        fixture: "ws-d62",
        wave: "R4-BCQ",
        detectors: &["d62-telemetry-before-success"],
        ported: true,
        corpus_dir: None,
    },
    // d63 is OPT-IN (registry after d62) — still runs here via run_smoke_entry's
    // by-name selection.
    Smoke {
        fixture: "ws-d63",
        wave: "R4-BCQ",
        detectors: &["d63-html-concat-injection"],
        ported: true,
        corpus_dir: None,
    },
    // d64 is OPT-IN (registry after d63) — still runs here via run_smoke_entry's
    // by-name selection.
    Smoke {
        fixture: "ws-d64",
        wave: "R4-BCQ",
        detectors: &["d64-api-page-write-surface"],
        ported: true,
        corpus_dir: None,
    },
];

/// R4-G per-detector fixtures (d14 dead-routine + d46 commit-in-lifecycle).
/// d14: ws-d14-dead-routine is the SMOKE positive (1 finding) flipped above;
/// ws-interface-dispatch + ws-member-call-resolution are d14 NEGATIVES (0 findings,
/// byte-matched END-TO-END). d46: ws-txn-d46-pos (2 findings) + ws-txn-d46-neg
/// (0 findings, the explicit negative). The two 0-count goldens are byte-matched
/// END-TO-END but EXEMPT from the anti-degenerate ≥1 check (handled by NOT pushing
/// them into ported_results).
const WAVE_G_POSITIVE: &[Smoke] = &[
    // d46: Install + Upgrade lifecycle triggers each reach Commit (2 findings).
    Smoke {
        fixture: "ws-txn-d46-pos",
        wave: "R4-G",
        detectors: &["d46-commit-in-lifecycle"],
        ported: true,
        corpus_dir: None,
    },
];

/// R4-G explicit 0-count goldens (byte-matched END-TO-END, EXEMPT from ≥1).
/// - ws-interface-dispatch: d14 negative — interface dispatch keeps the callee
///   reachable, so no routine is dead.
/// - ws-member-call-resolution: d14 negative — member calls resolve to reachable
///   routines.
/// - ws-txn-d46-neg: d46 negative — a normal codeunit (no Install/Upgrade subtype)
///   that commits; the lifecycle gate suppresses it.
const WAVE_G_NEGATIVES: &[Smoke] = &[
    Smoke {
        fixture: "ws-interface-dispatch",
        wave: "R4-G",
        detectors: &["d14-dead-routine"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-member-call-resolution",
        wave: "R4-G",
        detectors: &["d14-dead-routine"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-txn-d46-neg",
        wave: "R4-G",
        detectors: &["d46-commit-in-lifecycle"],
        ported: true,
        corpus_dir: None,
    },
];

/// R4-F per-detector POSITIVE fixtures (d47/d49/d51 ordering-facts → Finding[]).
/// ws-txn-d47-pos-http-nocommit is the SMOKE positive (flipped to ported above).
/// d47: pos-http-commit-after, pos-file, advisory-deduped, advisory-post-nowrite,
/// event-pos (each 1 finding). d49: pos-modify-message, pos-modify-runmodal (1 each).
/// d51: ws-d51-pos (1, "likely"), ws-d51-jobqueue (1, "confirmed"). Each byte-matches
/// END-TO-END and produces ≥1 finding.
const WAVE_F: &[Smoke] = &[
    Smoke {
        fixture: "ws-txn-d47-pos-http-commit-after",
        wave: "R4-F",
        detectors: &["d47-io-unsafe-txn"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-txn-d47-pos-file",
        wave: "R4-F",
        detectors: &["d47-io-unsafe-txn"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-txn-d47-advisory-deduped",
        wave: "R4-F",
        detectors: &["d47-io-unsafe-txn"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-txn-d47-advisory-post-nowrite",
        wave: "R4-F",
        detectors: &["d47-io-unsafe-txn"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-txn-d47-event-pos",
        wave: "R4-F",
        detectors: &["d47-io-unsafe-txn"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-txn-d49-pos-modify-message",
        wave: "R4-F",
        detectors: &["d49-uncommitted-write-before-ui"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-txn-d49-pos-modify-runmodal",
        wave: "R4-F",
        detectors: &["d49-uncommitted-write-before-ui"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-d51-pos",
        wave: "R4-F",
        detectors: &["d51-retry-side-effect-duplication"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-d51-jobqueue",
        wave: "R4-F",
        detectors: &["d51-retry-side-effect-duplication"],
        ported: true,
        corpus_dir: None,
    },
];

/// R4-F explicit 0-count goldens (byte-matched END-TO-END, EXEMPT from the ≥1 check).
/// d47: crosshop-iobeforecommit (the KEY gradeGuarantee test — EXTERNAL_IO_BEFORE_COMMIT
/// with read-direction HTTP Get is gradeGuarantee-suppressed → 0 findings),
/// event-neg-clean, event-neg-isolated, neg-commit-between, neg-readonly, neg-temp.
/// d49: neg-commit-between, neg-no-write, neg-run-boundary, neg-temp-write.
/// d51: ws-d51-neg.
const WAVE_F_NEGATIVES: &[Smoke] = &[
    Smoke {
        fixture: "ws-txn-d47-crosshop-iobeforecommit",
        wave: "R4-F",
        detectors: &["d47-io-unsafe-txn"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-txn-d47-event-neg-clean",
        wave: "R4-F",
        detectors: &["d47-io-unsafe-txn"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-txn-d47-event-neg-isolated",
        wave: "R4-F",
        detectors: &["d47-io-unsafe-txn"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-txn-d47-neg-commit-between",
        wave: "R4-F",
        detectors: &["d47-io-unsafe-txn"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-txn-d47-neg-readonly",
        wave: "R4-F",
        detectors: &["d47-io-unsafe-txn"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-txn-d47-neg-temp",
        wave: "R4-F",
        detectors: &["d47-io-unsafe-txn"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-txn-d49-neg-commit-between",
        wave: "R4-F",
        detectors: &["d49-uncommitted-write-before-ui"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-txn-d49-neg-no-write",
        wave: "R4-F",
        detectors: &["d49-uncommitted-write-before-ui"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-txn-d49-neg-run-boundary",
        wave: "R4-F",
        detectors: &["d49-uncommitted-write-before-ui"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-txn-d49-neg-temp-write",
        wave: "R4-F",
        detectors: &["d49-uncommitted-write-before-ui"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-d51-neg",
        wave: "R4-F",
        detectors: &["d51-retry-side-effect-duplication"],
        ported: true,
        corpus_dir: None,
    },
];

/// R4-A per-detector positive fixtures (ported: true = byte-match required).
const WAVE_A: &[Smoke] = &[
    // d5: loop-and-Modify could be ModifyAll
    Smoke {
        fixture: "ws-d5-modifyall",
        wave: "R4-A",
        detectors: &["d5-set-based-opportunity"],
        ported: true,
        corpus_dir: None,
    },
    // d10: self-modifying loop
    Smoke {
        fixture: "ws-d10-self-mod",
        wave: "R4-A",
        detectors: &["d10-self-modifying-loop"],
        ported: true,
        corpus_dir: None,
    },
    // d11: modify without get (no-get positive)
    Smoke {
        fixture: "ws-d11-no-get",
        wave: "R4-A",
        detectors: &["d11-modify-without-get"],
        ported: true,
        corpus_dir: None,
    },
    // d11: modify without get (modifyall+init, second positive fixture)
    Smoke {
        fixture: "ws-d11-modifyall-init",
        wave: "R4-A",
        detectors: &["d11-modify-without-get"],
        ported: true,
        corpus_dir: None,
    },
    // d18: constant filter in loop
    Smoke {
        fixture: "ws-d18",
        wave: "R4-A",
        detectors: &["d18-constant-filter-in-loop"],
        ported: true,
        corpus_dir: None,
    },
    // d21: read without load
    Smoke {
        fixture: "ws-d21",
        wave: "R4-A",
        detectors: &["d21-read-without-load"],
        ported: true,
        corpus_dir: None,
    },
    // d36: SetLoadFields placed after the load
    Smoke {
        fixture: "ws-d36",
        wave: "R4-A",
        detectors: &["d36-late-setloadfields"],
        ported: true,
        corpus_dir: None,
    },
    // d19: unused procedure parameter
    Smoke {
        fixture: "ws-d19",
        wave: "R4-A",
        detectors: &["d19-unused-parameter"],
        ported: true,
        corpus_dir: None,
    },
    // d20: unreachable statement after unconditional exit
    Smoke {
        fixture: "ws-d20",
        wave: "R4-A",
        detectors: &["d20-unreachable-after-exit"],
        ported: true,
        corpus_dir: None,
    },
    // d29: event subscriber mutates the inbound record
    Smoke {
        fixture: "ws-d29",
        wave: "R4-A",
        detectors: &["d29-subscriber-modify-on-event-record"],
        ported: true,
        corpus_dir: None,
    },
];

/// R4-B per-detector positive fixtures (metadata — tableById field metadata).
const WAVE_B: &[Smoke] = &[
    // d33: unfiltered DeleteAll / ModifyAll
    Smoke {
        fixture: "ws-d33",
        wave: "R4-B",
        detectors: &["d33-unfiltered-bulk-write"],
        ported: true,
        corpus_dir: None,
    },
];

/// R4-C per-detector positive fixtures (event/call-graph — eventGraph edges +
/// combined-graph SCC). d12's smoke entry already byte-matches above; d7/d38 are the
/// new per-detector entries.
const WAVE_C: &[Smoke] = &[
    // d7: event-subscriber chain forms a cycle (combined-graph SCC + event-dispatch).
    Smoke {
        fixture: "ws-d7-event-cycle",
        wave: "R4-C",
        detectors: &["d7-recursive-event-expansion"],
        ported: true,
        corpus_dir: None,
    },
    // d38: primary subscriber bound to an [Obsolete] publisher event (2 findings).
    Smoke {
        fixture: "ws-d38",
        wave: "R4-C",
        detectors: &["d38-subscriber-to-obsolete-event"],
        ported: true,
        corpus_dir: None,
    },
];

/// R4-D per-detector positive fixtures (transaction-span / commit-reachability).
/// d8's smoke entry already byte-matches above; d9/d34/d35 are the new per-detector
/// entries. d9 reuses the ws-d8-commit-in-tx corpus (same workspace, different golden).
const WAVE_D: &[Smoke] = &[
    // d9: transaction span summary (info-level; reuses d8's corpus dir).
    Smoke {
        fixture: "ws-d8-commit-in-tx-d9",
        wave: "R4-D",
        detectors: &["d9-transaction-span-summary"],
        ported: true,
        corpus_dir: Some("ws-d8-commit-in-tx"),
    },
    // d34: Commit inside a loop (direct + transitive; 3 findings).
    Smoke {
        fixture: "ws-d34",
        wave: "R4-D",
        detectors: &["d34-commit-in-loop"],
        ported: true,
        corpus_dir: None,
    },
    // d35: Commit reachable from event subscriber (direct + transitive; 2 findings).
    Smoke {
        fixture: "ws-d35",
        wave: "R4-D",
        detectors: &["d35-commit-in-event-subscriber"],
        ported: true,
        corpus_dir: None,
    },
    // d32: Boolean parameter always passed the same literal (1 finding).
    Smoke {
        fixture: "ws-d32",
        wave: "R4-D",
        detectors: &["d32-constant-boolean-parameter"],
        ported: true,
        corpus_dir: None,
    },
];

/// R4-E per-detector positive fixtures (record-flow — parameterRoles). ws-d3's smoke
/// entry already byte-matches above; d37/d39 are the new per-detector entries.
/// d37: ws-d37 (3 findings — Validate without persist). d39: ws-d39 (1 finding —
/// record left dirty across a call chain via the reverse call graph).
const WAVE_E: &[Smoke] = &[
    Smoke {
        fixture: "ws-d37",
        wave: "R4-E",
        detectors: &["d37-validate-without-persist"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-d39",
        wave: "R4-E",
        detectors: &["d39-record-left-dirty-across-chain"],
        ported: true,
        corpus_dir: None,
    },
];

/// R4-E2 per-detector positive fixtures (transitive record-flow — parameterRoles +
/// resolved call edge + the source-ordered load/filter scan). d40: ws-d40 (2 findings —
/// medium read + high mutate, OPT-IN detector). d41: ws-d41 (1 finding — filter lost
/// across a Reset helper). d42: ws-d42 (1 finding — narrowed load misses the field the
/// callee reads). Each byte-matches END-TO-END.
const WAVE_E2: &[Smoke] = &[
    Smoke {
        fixture: "ws-d40",
        wave: "R4-E2",
        detectors: &["d40-transitive-load-missing"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-d41",
        wave: "R4-E2",
        detectors: &["d41-transitive-filter-loss"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-d42",
        wave: "R4-E2",
        detectors: &["d42-cross-call-wrong-setloadfields"],
        ported: true,
        corpus_dir: None,
    },
];

/// D1 per-detector positive fixtures (path-walker substrate end-to-end). d1 is the
/// MOST COMPLEX detector — its byte-match validates `walk_evidence` +
/// `merge_by_terminal` + `describe_table` + `pick_actionable_anchor` together.
/// ws-d1 (3 findings, one WITH additionalPaths), ws-d1-multi-caller (1 finding, 2
/// additionalPaths — validates merge_by_terminal), ws-d1-setup-singleton (3),
/// ws-d1-uncertain-path (1 finding WITH a non-empty `confidence.evidence` — see
/// below). ws-d1-dep-terminal (0 findings — the explicit negative) is byte-matched
/// separately below (its 0-count golden is exempt from the anti-degenerate ≥1
/// check).
///
/// **`ws-d1-uncertain-path` covers `Finding.confidence.evidence` for d1**, and
/// `ws-d2-uncertain` (in `WAVE_D2` below) does the same for d2 — together they are
/// the only goldens in this repository that cover the field at all. Until
/// `ws-d2-uncertain` was added, d1's was the ONLY one, which left d2's half of the
/// same substrate (`sub_summary_uncertainties` borrows straight out of
/// `ctx.uncertainties_by_node`) with no coverage that could fail. Every other
/// fixture's d1 winner path is
/// certain, so `to_confidence` takes its `is_empty()` fast path and the field
/// serializes as `[]`; and no `alsem analyze` golden family can EVER cover it,
/// because `FindingSummary` projects only `{level, cappedBy}`. Its workspace
/// (`tests/r0-corpus/ws-d1-uncertain-path/src/chain.al`, which documents the shape
/// in full) is built so the emitted evidence pins ORDER (byte-sorted key order
/// differs from path-discovery order), DE-DUPLICATION (one uncertainty spans two
/// nodes of the same path) and RESOLUTION (four distinct entries, so an off-by-one
/// in the id → value mapping shows). Byte-comparing it here is what keeps
/// `d1_cohort.rs`'s interned `UncertaintyTable` — and anything that later rewrites
/// how `Evidence` is built from it — honest.
const WAVE_D1: &[Smoke] = &[
    Smoke {
        fixture: "ws-d1",
        wave: "R4-D1",
        detectors: &["d1-db-op-in-loop"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-d1-multi-caller",
        wave: "R4-D1",
        detectors: &["d1-db-op-in-loop"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-d1-setup-singleton",
        wave: "R4-D1",
        detectors: &["d1-db-op-in-loop"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-d1-uncertain-path",
        wave: "R4-D1",
        detectors: &["d1-db-op-in-loop"],
        ported: true,
        corpus_dir: None,
    },
];

/// D2 per-detector positive fixture (event-fanout-in-loop — the complement of d1).
/// ws-d2 (1 finding, WITH additionalPaths — two loops publish the same event, folded
/// by merge_by_terminal). Validates the event-dispatch-following WalkPolicy + the
/// subscriber DB-effect terminal + merge_by_terminal on rootCauseKey `d2/{eventId}`.
/// `ws-d2-uncertain` (1 finding WITH a non-empty `confidence.evidence`) is the d2
/// counterpart of `ws-d1-uncertain-path`: its subscriber both touches the database
/// AND dispatches through an interface, so `sub_summary_uncertainties` returns a
/// non-empty set and d2's `to_confidence` leaves its `is_empty()` fast path. See
/// `tests/r0-corpus/ws-d2-uncertain/src/subscriber.al`.
///
/// **What it pins, measured by breaking it** (not asserted from reading the code):
/// deleting d2's `dedupe_uncertainties` call makes this golden fail with four
/// evidence entries instead of two, and NO other fixture moves. What it does NOT
/// pin: dropping the `sub_summary_uncertainties(ctx, ..)` contribution entirely
/// leaves this golden GREEN, because d2 collects the same uncertainties a second
/// time through the subscriber walk and the dedupe collapses them. So a change
/// that breaks ONLY the direct `ctx.uncertainties_by_node` borrow — which is
/// exactly what a substrate migration touches — would not be caught here. Pinning
/// that would need a subscriber whose own uncertainty set is not re-derivable from
/// its walk.
const WAVE_D2: &[Smoke] = &[
    Smoke {
        fixture: "ws-d2",
        wave: "R4-D2",
        detectors: &["d2-event-fanout-in-loop"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-d2-uncertain",
        wave: "R4-D2",
        detectors: &["d2-event-fanout-in-loop"],
        ported: true,
        corpus_dir: None,
    },
];

/// D48 per-detector positive fixture (http/file IO inside a loop). ws-txn-d48-pos
/// (2 findings: one transitive HTTP Send via a call chain, one direct FILE IO).
/// Validates the capability-fact IoTerminal (resource_kind http/file, witness
/// callsite, HttpExtra method) + the in-loop call-chain walk.
const WAVE_D48: &[Smoke] = &[Smoke {
    fixture: "ws-txn-d48-pos",
    wave: "R4-D48",
    detectors: &["d48-io-in-loop"],
    ported: true,
    corpus_dir: None,
}];

/// ws-txn-d48-neg: the explicit D48 negative — a 0-finding workspace (http/file IO
/// present but NOT inside any loop) with a committed 0-count golden. Byte-matched
/// END-TO-END but EXEMPT from the anti-degenerate ≥1 check.
const WAVE_D48_NEGATIVE: Smoke = Smoke {
    fixture: "ws-txn-d48-neg",
    wave: "R4-D48",
    detectors: &["d48-io-in-loop"],
    ported: true,
    corpus_dir: None,
};

/// R4-EVENT per-detector positive fixtures (event-flow substrate: d43/d44/d45).
/// d43: 6 fixtures totaling 7 findings (ws-event-ishandled = 1, -conditional-set = ?,
/// -exit, -helper, -multi-dispatch, -nested-guard). d44: 2 fixtures totaling 2
/// (ws-event-multi-sub-overlap = 1, ws-event-read-after-write = 1). d45: 2 fixtures
/// totaling 3 (ws-event-d45-deep = 2, ws-event-transitive-exposure = 1). Each
/// byte-matches END-TO-END.
const WAVE_R4_EVENT: &[Smoke] = &[
    // --- d43: event-ishandled-skip (6 fixtures) ---
    Smoke {
        fixture: "ws-event-ishandled",
        wave: "R4-EVENT",
        detectors: &["d43-event-ishandled-skip"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-event-ishandled-conditional-set",
        wave: "R4-EVENT",
        detectors: &["d43-event-ishandled-skip"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-event-ishandled-exit",
        wave: "R4-EVENT",
        detectors: &["d43-event-ishandled-skip"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-event-ishandled-helper",
        wave: "R4-EVENT",
        detectors: &["d43-event-ishandled-skip"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-event-ishandled-multi-dispatch",
        wave: "R4-EVENT",
        detectors: &["d43-event-ishandled-skip"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-event-ishandled-nested-guard",
        wave: "R4-EVENT",
        detectors: &["d43-event-ishandled-skip"],
        ported: true,
        corpus_dir: None,
    },
    // --- d44: event-multi-subscriber-overlap (2 fixtures) ---
    Smoke {
        fixture: "ws-event-multi-sub-overlap",
        wave: "R4-EVENT",
        detectors: &["d44-event-multi-subscriber-overlap"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-event-read-after-write",
        wave: "R4-EVENT",
        detectors: &["d44-event-multi-subscriber-overlap"],
        ported: true,
        corpus_dir: None,
    },
    // --- d45: event-transitive-table-exposure (2 fixtures, 3 findings) ---
    Smoke {
        fixture: "ws-event-d45-deep",
        wave: "R4-EVENT",
        detectors: &["d45-event-transitive-table-exposure"],
        ported: true,
        corpus_dir: None,
    },
    Smoke {
        fixture: "ws-event-transitive-exposure",
        wave: "R4-EVENT",
        detectors: &["d45-event-transitive-table-exposure"],
        ported: true,
        corpus_dir: None,
    },
];

/// ws-d1-dep-terminal: the explicit D1 negative — a 0-finding workspace with a
/// committed 0-count golden. Byte-matched END-TO-END (so the projection envelope's
/// `findingCount: 0` shape is asserted) but EXEMPT from the anti-degenerate ≥1
/// check that the positive fixtures must satisfy.
const WAVE_D1_NEGATIVE: Smoke = Smoke {
    fixture: "ws-d1-dep-terminal",
    wave: "R4-D1",
    detectors: &["d1-db-op-in-loop"],
    ported: true,
    corpus_dir: None,
};

/// A negative assertion: the given detector must produce 0 findings over the
/// neutral fixture (which lacks the detector's triggering pattern).
struct NegativeAssertion {
    detector: &'static str,
    neutral_fixture: &'static str,
}

/// One neutral per detector — pick a corpus fixture that CONTAINS the detector's
/// op-family but in a safe arrangement, so the detector genuinely runs its logic and
/// emits 0. The per-detector rationale is given below.
const NEGATIVES: &[NegativeAssertion] = &[
    // d5: ws-d18 has for-loops (no repeat..until / Next()) and SetRange/SetFilter —
    // but no Modify inside any loop, so the loop-and-Modify pattern never fires.
    NegativeAssertion {
        detector: "d5-set-based-opportunity",
        neutral_fixture: "ws-d18",
    },
    // d10: ws-d36 has Get/SetLoadFields ops but no loops at all — the self-modifying
    // loop detector requires Next() inside a repeat..until body and finds none.
    NegativeAssertion {
        detector: "d10-self-modifying-loop",
        neutral_fixture: "ws-d36",
    },
    // d11: ws-d10-self-mod has Modify on Customer but FindSet() (a LOAD_OP) precedes
    // every Modify — the "loaded before" check suppresses all instances.
    NegativeAssertion {
        detector: "d11-modify-without-get",
        neutral_fixture: "ws-d10-self-mod",
    },
    // d18: ws-d5-modifyall has SetRange outside the loop only; no SetRange/SetFilter
    // appears inside any loop body, so the constant-filter pattern never triggers.
    NegativeAssertion {
        detector: "d18-constant-filter-in-loop",
        neutral_fixture: "ws-d5-modifyall",
    },
    // d21: ws-d36 contains Get/SetLoadFields but no TestField/CalcFields/CalcSums —
    // the reading ops that d21 checks for are entirely absent.
    NegativeAssertion {
        detector: "d21-read-without-load",
        neutral_fixture: "ws-d36",
    },
    // d36: ws-d18 has SetRange/SetFilter inside loops and FindFirst — but no
    // SetLoadFields or AddLoadFields appear anywhere, so the late-setloadfields
    // pattern is never exercised.
    NegativeAssertion {
        detector: "d36-late-setloadfields",
        neutral_fixture: "ws-d18",
    },
    // d19: ws-d11-no-get has a procedure `FromParam(var Customer: Record Customer)`
    // whose record parameter IS referenced (Customer.Modify()) — the detector runs
    // its identifier-reference check on a parameterized procedure and finds every
    // param used, so it emits 0.
    NegativeAssertion {
        detector: "d19-unused-parameter",
        neutral_fixture: "ws-d11-no-get",
    },
    // d20: ws-d36 has Get/SetLoadFields procedures but no Exit/Error/CurrReport.Quit
    // followed by a statement — the body DFS records no unreachable pair, so 0.
    NegativeAssertion {
        detector: "d20-unreachable-after-exit",
        neutral_fixture: "ws-d36",
    },
    // d29: ws-d11-no-get contains Modify on a record (the MUTATING_OPS family) but
    // has NO [EventSubscriber] routine at all — the subscriber-kind gate suppresses
    // every candidate, so 0.
    NegativeAssertion {
        detector: "d29-subscriber-modify-on-event-record",
        neutral_fixture: "ws-d11-no-get",
    },
    // d22: ws-d11-no-get has field accesses on "No." and "Last Date Modified" (Normal
    // fields), but neither field is declared FlowField — the fieldClass gate drops all
    // accesses and the detector emits 0.
    NegativeAssertion {
        detector: "d22-flowfield-without-calcfields",
        neutral_fixture: "ws-d11-no-get",
    },
    // d33: ws-d22 contains Get/CalcFields ops but NO DeleteAll or ModifyAll
    // calls — the bulk-op gate never fires, so the detector emits 0.
    NegativeAssertion {
        detector: "d33-unfiltered-bulk-write",
        neutral_fixture: "ws-d22",
    },
    // d7: ws-d38 has event-dispatch edges (publisher → subscriber) but the
    // subscribers don't re-publish, so the combined graph has NO SCC of size >= 2 —
    // Tarjan finds only singletons and the detector emits 0.
    NegativeAssertion {
        detector: "d7-recursive-event-expansion",
        neutral_fixture: "ws-d38",
    },
    // d12: ws-d38 declares three [IntegrationEvent] publishers, but EACH has at least
    // one [EventSubscriber] in the workspace — subsByEvent > 0 for every event, so the
    // dead-event gate never fires and the detector emits 0.
    NegativeAssertion {
        detector: "d12-dead-integration-event",
        neutral_fixture: "ws-d38",
    },
    // d38: ws-d12-dead-event has an [IntegrationEvent] publisher but NO subscriber at
    // all — there are no resolved edges to walk, so the obsolete-publisher check is
    // never reached and the detector emits 0.
    NegativeAssertion {
        detector: "d38-subscriber-to-obsolete-event",
        neutral_fixture: "ws-d12-dead-event",
    },
    // d8: ws-d4-repeated-get has no Commit operations at all — no transaction spans
    // are seeded, so the posting-span check never fires and the detector emits 0.
    NegativeAssertion {
        detector: "d8-commit-in-transaction",
        neutral_fixture: "ws-d4-repeated-get",
    },
    // d9: ws-d4-repeated-get has no transaction spans (no Commit) — the
    // span-summary check never fires and the detector emits 0.
    NegativeAssertion {
        detector: "d9-transaction-span-summary",
        neutral_fixture: "ws-d4-repeated-get",
    },
    // d34: ws-d35 has a Commit inside an event subscriber but that subscriber's body
    // has no loop at all — the in-loop gate suppresses every commit operation, so 0.
    NegativeAssertion {
        detector: "d34-commit-in-loop",
        neutral_fixture: "ws-d35",
    },
    // d35: ws-d34 has Commits inside loops and a Persist callee that commits, but NO
    // [EventSubscriber] routine — the subscriber-kind gate suppresses every
    // candidate, so 0.
    NegativeAssertion {
        detector: "d35-commit-in-event-subscriber",
        neutral_fixture: "ws-d34",
    },
    // d32: ws-d19 has a `local` event-subscriber (kind == "event-subscriber", not
    // "procedure") and public procedures with no Boolean params — the kind gate and
    // the boolean-param gate both suppress every candidate, so 0.
    NegativeAssertion {
        detector: "d32-constant-boolean-parameter",
        neutral_fixture: "ws-d19",
    },
    // d2: ws-d4-repeated-get has a repeat..until loop with in-loop Get ops but NO
    // event publish inside any loop — the publisher-routine gate on the in-loop
    // callsite never matches, so the event-fanout detector emits 0.
    NegativeAssertion {
        detector: "d2-event-fanout-in-loop",
        neutral_fixture: "ws-d4-repeated-get",
    },
    // d43: ws-event-multi-sub-overlap has event subscribers but NO IsHandled dispatch
    // guard anywhere (no conditionReferences) — the substrate guard bails (or, even
    // reaching the site loop, no dispatch site has a post-call IsHandled guard), so 0.
    NegativeAssertion {
        detector: "d43-event-ishandled-skip",
        neutral_fixture: "ws-event-multi-sub-overlap",
    },
    // d44: ws-event-transitive-exposure has a single subscriber per event (no two
    // subscribers writing the same table, no reader-after-writer) — the overlap and
    // read-after-write gates never fire, so 0.
    NegativeAssertion {
        detector: "d44-event-multi-subscriber-overlap",
        neutral_fixture: "ws-event-transitive-exposure",
    },
    // d45: ws-event-ishandled has an event publisher whose subscriber sets IsHandled
    // but writes NO table — the transitive-write aggregation finds no writer-subscriber
    // table, so the per-(publisher,table) loop never fires and the detector emits 0.
    NegativeAssertion {
        detector: "d45-event-transitive-table-exposure",
        neutral_fixture: "ws-event-ishandled",
    },
    // d3: ws-d36 has Get/FindSet retrievals WITH SetLoadFields ops — deriveLoadStates
    // runs the full load-state machine, but every retrieval's loaded set covers its
    // accessed fields (no uncovered same-routine field access in any window), so the
    // missing/incomplete determination is silent and the detector emits 0.
    NegativeAssertion {
        detector: "d3-missing-setloadfields",
        neutral_fixture: "ws-d36",
    },
    // d37: ws-d39 contains a Validate op, but it is on a BY-VAR PARAMETER record
    // (ValidatesAndExits validates its `var Customer` parameter) — the parameter-record
    // suppression fires (the caller may persist after returning), so the detector
    // exercises its gate and emits 0.
    NegativeAssertion {
        detector: "d37-validate-without-persist",
        neutral_fixture: "ws-d39",
    },
    // d39: ws-d37 has Validate-without-persist routines (so parameterRoles are
    // computed), but NONE forward a Validate-dirty record by-var across a call chain
    // to a primary callee with dirtyAtExit === "yes" — the reverse-call-graph walk
    // finds no qualifying caller/callee pair, so the detector emits 0.
    NegativeAssertion {
        detector: "d39-record-left-dirty-across-chain",
        neutral_fixture: "ws-d37",
    },
    // d40: ws-d41 forwards records by-var to ResettingHelper, but that callee resets
    // filters — it does NOT require its parameter loaded at entry
    // (requiresLoadedAtEntry !== "yes") — so the transitive-load gate never fires and
    // the detector emits 0.
    NegativeAssertion {
        detector: "d40-transitive-load-missing",
        neutral_fixture: "ws-d41",
    },
    // d41: ws-d40 forwards records by-var to MutatingHelper / ReadingHelper, but
    // neither callee calls Reset (resetsFiltersOnParam !== "yes") — the filter-loss
    // gate never fires, so the detector emits 0.
    NegativeAssertion {
        detector: "d41-transitive-filter-loss",
        neutral_fixture: "ws-d40",
    },
    // d42: ws-d40 forwards records that were never narrowed (no SetLoadFields /
    // AddLoadFields) — the caller-side load is "full", so the callerFull skip fires
    // for every binding and the detector emits 0.
    NegativeAssertion {
        detector: "d42-cross-call-wrong-setloadfields",
        neutral_fixture: "ws-d40",
    },
    // d52: ws-e2e contains no DeleteAll/ModifyAll calls at all (grep-verified) — the
    // bulk-op scan over every routine's record_operations never matches BULK_OPS, so
    // candidates_considered stays 0 and the detector emits 0.
    NegativeAssertion {
        detector: "d52-bulk-write-param-no-temp-guard",
        neutral_fixture: "ws-e2e",
    },
    // d53: ws-e2e has no [TryFunction] procedures at all (grep-verified) — the
    // resolved-callee attribute check never matches, so candidates_considered
    // stays 0 and the detector emits 0.
    NegativeAssertion {
        detector: "d53-ignored-tryfunction-result",
        neutral_fixture: "ws-e2e",
    },
    // d54: ws-e2e has no [TryFunction] procedures at all (grep-verified) — the
    // routine scan over attributes_parsed never matches, so candidates_considered
    // stays 0 and the detector emits 0.
    NegativeAssertion {
        detector: "d54-publish-in-tryfunction-cone",
        neutral_fixture: "ws-e2e",
    },
    // d55: ws-e2e is NOT usable here — it declares OnAfterRunIteration as an
    // IntegrationEvent AND calls it inside RunBatch's `for` loop, so d55 would
    // fire on it (grep-verified). ws-d41 has no IntegrationEvent/BusinessEvent
    // declaration at all (grep-verified), so no resolved callee can ever have
    // kind=="event-publisher" — candidates_considered stays 0 and the detector
    // emits 0.
    NegativeAssertion {
        detector: "d55-event-publish-in-loop",
        neutral_fixture: "ws-d41",
    },
    // d56: ws-e2e has no whole-record copy between two record variables anywhere
    // (grep-verified: the only assignment is the field-level
    // `Customer.Address := Customer.Name`) — the var_assignments scan never finds
    // a record-to-record rhs_identifier match, so candidates_considered stays 0
    // and the detector emits 0.
    NegativeAssertion {
        detector: "d56-clone-before-write-in-loop",
        neutral_fixture: "ws-e2e",
    },
    // d57: ws-e2e declares no SingleInstance codeunit at all (grep-verified) — the
    // si_objects set is empty, so the detector short-circuits to 0 findings before
    // ever scanning a routine.
    NegativeAssertion {
        detector: "d57-singleinstance-growing-state",
        neutral_fixture: "ws-e2e",
    },
    // d58: ws-e2e declares no Query-typed variable at all (grep-verified) — the
    // query_vars set is empty for every routine, so the detector short-circuits
    // to 0 findings before ever scanning a call site.
    NegativeAssertion {
        detector: "d58-query-filter-after-open",
        neutral_fixture: "ws-e2e",
    },
    // d59: ws-e2e's only IntegrationEvent publisher, OnAfterRunIteration, declares
    // ZERO parameters (grep-verified) — the var-Boolean-parameter scan never finds
    // a candidate, so candidates_considered stays 0 and the detector emits 0.
    NegativeAssertion {
        detector: "d59-integrationevent-var-boolean-guard",
        neutral_fixture: "ws-e2e",
    },
    // d60: ws-e2e declares no Codeunit with `Subtype = Upgrade`/`Install` at all
    // (grep-verified) — the lifecycle_objects set is empty, so the detector
    // short-circuits to 0 findings before ever scanning a routine.
    NegativeAssertion {
        detector: "d60-upgrade-loop-should-be-datatransfer",
        neutral_fixture: "ws-e2e",
    },
    // d61: ws-e2e's only IntegrationEvent publisher, OnAfterRunIteration, declares
    // ZERO parameters (grep-verified, same fact d59 relies on) — no var Boolean
    // IsHandled-shaped formal can ever exist on it, so the publisher scan never
    // matches, publisher_meta stays empty, and the detector short-circuits to 0
    // findings before scanning any routine.
    NegativeAssertion {
        detector: "d61-ishandled-bypasses-critical-write",
        neutral_fixture: "ws-e2e",
    },
    // d62: ws-e2e declares no "Feature Telemetry" typed variable at all
    // (grep-verified) — the ft_vars per-routine filter never matches, so the
    // detector short-circuits to 0 findings before scanning any call site.
    NegativeAssertion {
        detector: "d62-telemetry-before-success",
        neutral_fixture: "ws-e2e",
    },
    // d63: ws-e2e contains no single-quoted literal with an HTML-tag-ish
    // `<x`/`</x` sequence concatenated with `+` (grep-verified) — the
    // looks_like_html_concat argument-text scan never matches any call site,
    // so the detector short-circuits to 0 findings.
    NegativeAssertion {
        detector: "d63-html-concat-injection",
        neutral_fixture: "ws-e2e",
    },
    // d64: ws-e2e declares NO `PageType` property at all (grep-verified) — the
    // API-page candidate scan never matches any object, so
    // candidates_considered stays 0 and the detector short-circuits to 0
    // findings before ever inspecting a write-surface property.
    NegativeAssertion {
        detector: "d64-api-page-write-surface",
        neutral_fixture: "ws-e2e",
    },
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn goldens_dir() -> PathBuf {
    repo_root().join("tests").join("r4-goldens")
}

fn corpus_dir() -> PathBuf {
    repo_root().join("tests").join("r0-corpus")
}

/// `r4-goldens/manifest.json`'s `fixtureCount` was read by no test (Task
/// T0.6 — a silently deleted golden would pass unnoticed). Checks `>=`, not
/// `==`: `fixtureCount` is a frozen al-sem-era provenance floor (52), and the
/// Rust engine's own R4-A..R4-H per-detector waves have since grown the
/// corpus well past it — an exact-equality check would break the moment a
/// wave legitimately gains a fixture.
#[test]
fn manifest_fixture_count_floor() {
    let manifest_path = goldens_dir().join("manifest.json");
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&manifest_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", manifest_path.display())),
    )
    .unwrap_or_else(|e| panic!("{} not valid JSON: {e}", manifest_path.display()));
    let claimed = manifest
        .get("fixtureCount")
        .and_then(|v| v.as_u64())
        .expect("manifest missing fixtureCount") as usize;
    let discovered = std::fs::read_dir(goldens_dir())
        .unwrap_or_else(|e| panic!("read {}: {e}", goldens_dir().display()))
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".r4.golden.json"))
        .count();
    assert!(
        discovered >= claimed,
        "r4-goldens/manifest.json claims fixtureCount={claimed} but only {discovered} \
         `.r4.golden.json` file(s) were found — a golden may have been silently deleted"
    );
}

#[derive(Debug, Clone)]
struct Divergence {
    fixture: String,
    path: String,
    golden_value: String,
    rust_value: String,
}

fn compact(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| format!("{v:?}"))
}

/// Recursively diff two values POSITIONALLY (both sides already canonically sorted).
fn diff_value(fixture: &str, path: &str, golden: &Value, rust: &Value, out: &mut Vec<Divergence>) {
    match (golden, rust) {
        (Value::Object(g), Value::Object(r)) => {
            for (k, gv) in g {
                let child = format!("{path}.{k}");
                match r.get(k) {
                    Some(rv) => diff_value(fixture, &child, gv, rv, out),
                    None => out.push(Divergence {
                        fixture: fixture.to_string(),
                        path: format!("{child}:MISSING_IN_RUST"),
                        golden_value: compact(gv),
                        rust_value: "<absent>".to_string(),
                    }),
                }
            }
            for (k, rv) in r {
                if !g.contains_key(k) {
                    out.push(Divergence {
                        fixture: fixture.to_string(),
                        path: format!("{path}.{k}:EXTRA_IN_RUST"),
                        golden_value: "<absent>".to_string(),
                        rust_value: compact(rv),
                    });
                }
            }
        }
        (Value::Array(g), Value::Array(r)) => {
            if g.len() != r.len() {
                out.push(Divergence {
                    fixture: fixture.to_string(),
                    path: format!("{path}:LENGTH"),
                    golden_value: g.len().to_string(),
                    rust_value: r.len().to_string(),
                });
            }
            let n = g.len().min(r.len());
            for i in 0..n {
                diff_value(fixture, &format!("{path}[{i}]"), &g[i], &r[i], out);
            }
            for (i, gv) in g.iter().enumerate().skip(n) {
                out.push(Divergence {
                    fixture: fixture.to_string(),
                    path: format!("{path}[{i}]:MISSING_IN_RUST"),
                    golden_value: compact(gv),
                    rust_value: "<absent>".to_string(),
                });
            }
            for (i, rv) in r.iter().enumerate().skip(n) {
                out.push(Divergence {
                    fixture: fixture.to_string(),
                    path: format!("{path}[{i}]:EXTRA_IN_RUST"),
                    golden_value: "<absent>".to_string(),
                    rust_value: compact(rv),
                });
            }
        }
        _ => {
            if golden != rust {
                out.push(Divergence {
                    fixture: fixture.to_string(),
                    path: path.to_string(),
                    golden_value: compact(golden),
                    rust_value: compact(rust),
                });
            }
        }
    }
}

/// Run the Rust source-only L5 pass for one fixture over the REGISTERED detectors,
/// projecting the envelope with the given detector list (findings filtered to that set).
///
/// `golden_name` is the `fixtureName` stamped into the projection envelope (must match
/// the golden file's `fixtureName` field for byte parity). `source_dir` is the corpus
/// directory to actually parse — when a golden covers a subset of a larger workspace
/// (e.g. d9 reuses `ws-d8-commit-in-tx`), these two differ.
fn run_rust(golden_name: &str, source_dir: &str, detector_names: &[&str]) -> R4FindingsProjection {
    let fixture_dir = corpus_dir().join(source_dir);
    assert!(
        fixture_dir.is_dir(),
        "R4 golden for {golden_name} has no matching in-repo fixture at {} (offline corpus incomplete)",
        fixture_dir.display()
    );
    let names: Vec<String> = detector_names.iter().map(|s| s.to_string()).collect();
    let detectors = registered_detectors();
    match assemble_and_resolve_workspace_default(&fixture_dir) {
        Some(resolved) => project_r4_findings(&resolved, &detectors, golden_name, &names),
        None => R4FindingsProjection {
            fixture_name: golden_name.to_string(),
            detectors: names,
            finding_count: 0,
            findings: vec![],
            loop_catalog: Vec::new(),
            loop_set_registry: None,
        },
    }
}

/// Run the Rust CROSS-APP L5 pass for one fixture: build the cross-app L4 base
/// (reading the committed dep `.app`(s) from `<fixture>/.alpackages/`), run the
/// registered detectors in cross-app mode, project the envelope filtered to the named
/// detectors. Model-instance id `"r0"` matches the al-sem dump convention.
fn run_rust_cross_app(
    golden_name: &str,
    source_dir: &str,
    detector_names: &[&str],
) -> R4FindingsProjection {
    let fixture_dir = corpus_dir().join(source_dir);
    assert!(
        fixture_dir.is_dir(),
        "R4 cross-app golden for {golden_name} has no matching in-repo fixture at {}",
        fixture_dir.display()
    );
    let names: Vec<String> = detector_names.iter().map(|s| s.to_string()).collect();
    let detectors = registered_detectors();
    project_r4_findings_cross_app(&fixture_dir, "r0", &detectors, golden_name, &names)
}

/// Run a single CROSS-APP entry. All 4 cross-app fixtures (d13-internal-call,
/// d13-member-call, d16-obsolete, d17-drift) byte-match their goldens END-TO-END:
///   - d13 carries no dep id in its rootCauseKey → fingerprint is stable.
///   - d16's rootCauseKey embeds the dep callee's INTERNAL id, BUT the Rust engine
///     stabilizes it via `FingerprintIndex::build` (the all-routines internal-id →
///     stable-id substitution) — so the fingerprint DOES byte-match.
///   - d17 carries no dep id in its rootCauseKey → fingerprint is stable.
///
/// Divergences are routed into `all_divergences` for the harness's direct strict
/// comparison at the end of the test — any divergence fails the run.
///
/// Returns `(byte_matched, finding_count)`. The anti-degenerate ≥1 check still
/// applies; the byte-match flag is true only when the fixture has zero structural
/// divergences from its golden.
fn run_cross_app_entry(
    smoke: &Smoke,
    all_divergences: &mut Vec<Divergence>,
) -> Option<(bool, usize)> {
    let golden_path = goldens_dir().join(format!("{}.r4.golden.json", smoke.fixture));
    assert!(
        golden_path.is_file(),
        "missing R4 cross-app golden: {}",
        golden_path.display()
    );
    let golden_text = std::fs::read_to_string(&golden_path)
        .unwrap_or_else(|e| panic!("read R4 golden {}: {e}", golden_path.display()));
    let golden_json: Value = serde_json::from_str(&golden_text)
        .unwrap_or_else(|e| panic!("R4 golden {} not valid JSON: {e}", golden_path.display()));
    let _: R4FindingsProjection = serde_json::from_value(golden_json.clone())
        .unwrap_or_else(|e| panic!("R4 golden {} not R4FindingsProjection: {e}", smoke.fixture));

    let source_dir = smoke.corpus_dir.unwrap_or(smoke.fixture);
    let rust = run_rust_cross_app(smoke.fixture, source_dir, smoke.detectors);

    if maybe_regen_r4(&golden_path, &rust) {
        return Some((true, rust.finding_count));
    }

    let rust_text = pretty_with_newline(&rust);
    let byte_matched = rust_text == golden_text;
    let count = rust.finding_count;
    if byte_matched {
        return Some((true, count));
    }

    // Diff into a local bucket; any divergence fails the run via the harness's
    // direct strict comparison at the end of the test.
    let mut local: Vec<Divergence> = Vec::new();
    let rust_json = serde_json::to_value(&rust).expect("rust → value");
    diff_value(smoke.fixture, "", &golden_json, &rust_json, &mut local);
    let no_divergences = local.is_empty();
    all_divergences.extend(local);
    Some((no_divergences, count))
}

/// Pretty-serialize + trailing newline — the exact on-disk golden form.
fn pretty_with_newline(proj: &R4FindingsProjection) -> String {
    let mut s = serde_json::to_string_pretty(proj).expect("serialize R4 projection");
    s.push('\n');
    s
}

/// REGEN path (temp-state epoch rebaseline, Task 16). When `REGEN_TEMP_GOLDENS`
/// is set, write the ENGINE-produced projection (in the exact on-disk golden
/// form) to the golden file instead of comparing — the goldens are Rust-owned
/// baselines (TS oracle retired). Returns `true` when a regen write happened.
fn maybe_regen_r4(golden_path: &std::path::Path, rust: &R4FindingsProjection) -> bool {
    if !regen::regen_mode() {
        return false;
    }
    std::fs::write(golden_path, pretty_with_newline(rust))
        .unwrap_or_else(|e| panic!("regen write {}: {e}", golden_path.display()));
    eprintln!("REGEN r4 golden: {}", golden_path.display());
    true
}

/// The subset of a golden `Value`'s findings whose `detector` is in `names`.
fn finding_subset(golden: &Value, names: &BTreeSet<String>) -> Vec<Value> {
    golden
        .get("findings")
        .and_then(|f| f.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|f| {
                    f.get("detector")
                        .and_then(|d| d.as_str())
                        .map(|d| names.contains(d))
                        .unwrap_or(false)
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Run a single smoke entry through the acceptance gate or deferred path.
fn run_smoke_entry(
    smoke: &Smoke,
    registered_names: &BTreeSet<String>,
    all_divergences: &mut Vec<Divergence>,
) -> Option<(bool, usize)> {
    let golden_path = goldens_dir().join(format!("{}.r4.golden.json", smoke.fixture));
    assert!(
        golden_path.is_file(),
        "missing R4 golden: {}",
        golden_path.display()
    );
    let golden_text = std::fs::read_to_string(&golden_path)
        .unwrap_or_else(|e| panic!("read R4 golden {}: {e}", golden_path.display()));
    let golden_json: Value = serde_json::from_str(&golden_text)
        .unwrap_or_else(|e| panic!("R4 golden {} not valid JSON: {e}", golden_path.display()));
    // Shape guard.
    let _: R4FindingsProjection = serde_json::from_value(golden_json.clone())
        .unwrap_or_else(|e| panic!("R4 golden {} not R4FindingsProjection: {e}", smoke.fixture));

    let source_dir = smoke.corpus_dir.unwrap_or(smoke.fixture);
    let rust = run_rust(smoke.fixture, source_dir, smoke.detectors);

    if smoke.ported {
        if maybe_regen_r4(&golden_path, &rust) {
            return Some((true, rust.finding_count));
        }
        let rust_text = pretty_with_newline(&rust);
        let byte_matched = rust_text == golden_text;
        if !byte_matched {
            let rust_json = serde_json::to_value(&rust).expect("rust → value");
            diff_value(smoke.fixture, "", &golden_json, &rust_json, all_divergences);
        }
        let count = rust.finding_count;
        assert_eq!(
            rust_text, golden_text,
            "R4 ACCEPTANCE GATE: {} ({}) did NOT byte-match its golden",
            smoke.fixture, smoke.wave
        );
        Some((byte_matched, count))
    } else {
        let golden_subset = finding_subset(&golden_json, registered_names);
        let rust_json = serde_json::to_value(&rust).expect("rust → value");
        let rust_subset = finding_subset(&rust_json, registered_names);
        diff_value(
            smoke.fixture,
            ".findings(registered-subset)",
            &Value::Array(golden_subset.clone()),
            &Value::Array(rust_subset.clone()),
            all_divergences,
        );
        eprintln!(
            "R4 {} ({}): deferred to wave {} — registered-subset findings: {} (golden subset: {})",
            smoke.fixture,
            smoke.wave,
            smoke.wave,
            rust_subset.len(),
            golden_subset.len(),
        );
        None
    }
}

#[test]
fn differential_r4_findings_match_goldens() {
    let registered_names: BTreeSet<String> = registered_detectors()
        .iter()
        .map(|d| d.name.clone())
        .collect();

    let mut all_divergences: Vec<Divergence> = Vec::new();

    // Track per-fixture byte-match results for anti-degenerate checks.
    let mut ported_results: Vec<(&'static str, bool, usize)> = Vec::new();

    // --- Smoke entries (wave representatives) ----------------------------------
    for smoke in SMOKE {
        if let Some((matched, count)) =
            run_smoke_entry(smoke, &registered_names, &mut all_divergences)
        {
            ported_results.push((smoke.fixture, matched, count));
        }
    }

    // --- R4-A per-detector fixtures -------------------------------------------
    for smoke in WAVE_A {
        if let Some((matched, count)) =
            run_smoke_entry(smoke, &registered_names, &mut all_divergences)
        {
            ported_results.push((smoke.fixture, matched, count));
        }
    }

    // --- R4-B per-detector fixtures -------------------------------------------
    for smoke in WAVE_B {
        if let Some((matched, count)) =
            run_smoke_entry(smoke, &registered_names, &mut all_divergences)
        {
            ported_results.push((smoke.fixture, matched, count));
        }
    }

    // --- R4-C per-detector fixtures -------------------------------------------
    for smoke in WAVE_C {
        if let Some((matched, count)) =
            run_smoke_entry(smoke, &registered_names, &mut all_divergences)
        {
            ported_results.push((smoke.fixture, matched, count));
        }
    }

    // --- R4-D per-detector fixtures -------------------------------------------
    for smoke in WAVE_D {
        if let Some((matched, count)) =
            run_smoke_entry(smoke, &registered_names, &mut all_divergences)
        {
            ported_results.push((smoke.fixture, matched, count));
        }
    }

    // --- R4-E per-detector fixtures (record-flow — parameterRoles) ------------
    for smoke in WAVE_E {
        if let Some((matched, count)) =
            run_smoke_entry(smoke, &registered_names, &mut all_divergences)
        {
            ported_results.push((smoke.fixture, matched, count));
        }
    }

    // --- R4-E2 per-detector fixtures (transitive record-flow: d40/d41/d42) -----
    for smoke in WAVE_E2 {
        if let Some((matched, count)) =
            run_smoke_entry(smoke, &registered_names, &mut all_divergences)
        {
            ported_results.push((smoke.fixture, matched, count));
        }
    }

    // --- D1 per-detector fixtures (path-walker substrate) ---------------------
    for smoke in WAVE_D1 {
        if let Some((matched, count)) =
            run_smoke_entry(smoke, &registered_names, &mut all_divergences)
        {
            ported_results.push((smoke.fixture, matched, count));
        }
    }
    // The D1 explicit negative: byte-match its 0-count golden END-TO-END, but do
    // NOT push it into ported_results (it is exempt from the anti-degenerate ≥1).
    run_smoke_entry(&WAVE_D1_NEGATIVE, &registered_names, &mut all_divergences);

    // --- D2 per-detector fixture (event-fanout-in-loop) -----------------------
    for smoke in WAVE_D2 {
        if let Some((matched, count)) =
            run_smoke_entry(smoke, &registered_names, &mut all_divergences)
        {
            ported_results.push((smoke.fixture, matched, count));
        }
    }

    // --- D48 per-detector fixture (http/file IO in loop) ----------------------
    for smoke in WAVE_D48 {
        if let Some((matched, count)) =
            run_smoke_entry(smoke, &registered_names, &mut all_divergences)
        {
            ported_results.push((smoke.fixture, matched, count));
        }
    }
    // The D48 explicit negative: byte-match its 0-count golden END-TO-END, but do
    // NOT push it into ported_results (exempt from the anti-degenerate ≥1).
    run_smoke_entry(&WAVE_D48_NEGATIVE, &registered_names, &mut all_divergences);

    // --- R4-EVENT per-detector fixtures (event-flow substrate: d43/d44/d45) ----
    for smoke in WAVE_R4_EVENT {
        if let Some((matched, count)) =
            run_smoke_entry(smoke, &registered_names, &mut all_divergences)
        {
            ported_results.push((smoke.fixture, matched, count));
        }
    }

    // --- R4-G positives (d46; d14's positive is the SMOKE entry above) ---------
    for smoke in WAVE_G_POSITIVE {
        if let Some((matched, count)) =
            run_smoke_entry(smoke, &registered_names, &mut all_divergences)
        {
            ported_results.push((smoke.fixture, matched, count));
        }
    }
    // R4-G explicit 0-count goldens (d14 + d46 negatives): byte-match END-TO-END
    // but do NOT push into ported_results (EXEMPT from the anti-degenerate ≥1).
    for smoke in WAVE_G_NEGATIVES {
        run_smoke_entry(smoke, &registered_names, &mut all_divergences);
    }

    // --- R4-H positives (d50 checked-run-implicit-commit) ---------------------
    for smoke in WAVE_H_POSITIVE {
        if let Some((matched, count)) =
            run_smoke_entry(smoke, &registered_names, &mut all_divergences)
        {
            ported_results.push((smoke.fixture, matched, count));
        }
    }
    // R4-H explicit 0-count goldens (d50 negative): byte-match END-TO-END but
    // do NOT push into ported_results (EXEMPT from the anti-degenerate ≥1).
    for smoke in WAVE_H_NEGATIVES {
        run_smoke_entry(smoke, &registered_names, &mut all_divergences);
    }

    // --- R4-F positives (d47/d49/d51; the d47 smoke positive is above) ---------
    for smoke in WAVE_F {
        if let Some((matched, count)) =
            run_smoke_entry(smoke, &registered_names, &mut all_divergences)
        {
            ported_results.push((smoke.fixture, matched, count));
        }
    }
    // R4-F explicit 0-count goldens (d47/d49/d51 negatives, incl. the crosshop
    // gradeGuarantee-suppression test): byte-match END-TO-END but do NOT push into
    // ported_results (EXEMPT from the anti-degenerate ≥1).
    for smoke in WAVE_F_NEGATIVES {
        run_smoke_entry(smoke, &registered_names, &mut all_divergences);
    }

    // --- R4-BCQ positives (d52–d64, BCQuality wave) ----------------------------
    for smoke in WAVE_BCQ {
        if let Some((matched, count)) =
            run_smoke_entry(smoke, &registered_names, &mut all_divergences)
        {
            ported_results.push((smoke.fixture, matched, count));
        }
    }

    // --- R4 CROSS-APP positives (d13/d16/d17 — committed dep .app in .alpackages) -
    for smoke in WAVE_CROSS_APP {
        if let Some((matched, count)) = run_cross_app_entry(smoke, &mut all_divergences) {
            ported_results.push((smoke.fixture, matched, count));
        }
    }

    // --- Anti-degenerate: all ported fixtures byte-matched AND had ≥1 finding -
    for (fixture, byte_matched, count) in &ported_results {
        assert!(
            *byte_matched,
            "R4 anti-degenerate: {fixture} did NOT byte-match (acceptance gate failed)"
        );
        assert!(
            *count >= 1,
            "R4 anti-degenerate: {fixture} produced {count} findings — expected ≥1"
        );
    }

    // --- Negative assertions: each new R4-A detector → 0 findings on neutral --
    for neg in NEGATIVES {
        let neutral_dir = corpus_dir().join(neg.neutral_fixture);
        assert!(
            neutral_dir.is_dir(),
            "R4 negative: neutral fixture {} not found",
            neg.neutral_fixture
        );
        let detectors = registered_detectors();
        let names = vec![neg.detector.to_string()];
        let rust = match assemble_and_resolve_workspace_default(&neutral_dir) {
            Some(resolved) => {
                project_r4_findings(&resolved, &detectors, neg.neutral_fixture, &names)
            }
            None => R4FindingsProjection {
                fixture_name: neg.neutral_fixture.to_string(),
                detectors: names.clone(),
                finding_count: 0,
                findings: vec![],
                loop_catalog: Vec::new(),
                loop_set_registry: None,
            },
        };
        assert_eq!(
            rust.finding_count, 0,
            "R4 negative assertion FAILED: detector {} produced {} finding(s) on neutral fixture {} \
             (expected 0)",
            neg.detector, rust.finding_count, neg.neutral_fixture
        );
        eprintln!(
            "R4 negative: {} on {} → {} findings (expected 0) ✓",
            neg.detector, neg.neutral_fixture, rust.finding_count
        );
    }

    // --- Strict divergence check ---------------------------------------------
    all_divergences
        .sort_by(|a, b| (a.fixture.as_str(), &a.path).cmp(&(b.fixture.as_str(), &b.path)));

    let mut failure = String::new();
    if !all_divergences.is_empty() {
        failure.push_str(&format!(
            "\n{} divergence(s) found:\n",
            all_divergences.len()
        ));
        for d in &all_divergences {
            failure.push_str(&format!(
                "  [{}] {}\n      golden = {}\n      rust   = {}\n",
                d.fixture, d.path, d.golden_value, d.rust_value
            ));
        }
    }

    assert!(
        failure.is_empty(),
        "R4 findings differential FAILED:{failure}"
    );

    // --- A8 (issue 23): the ws-d50-temp-gate fixture is GENUINELY executed ----
    // R4 drives an EXPLICIT named Smoke list, so a fixture that exists on disk
    // with a seed golden but no entry is never run at all, and this test stays
    // green while testing nothing. This assertion is what removing the
    // `ws-d50-temp-gate` entry from `WAVE_H_POSITIVE` must fail — deleting the
    // entry AND the per-fixture checks together is exactly why "remove it and
    // watch the silence" proves nothing.
    let temp_gate_runs = ported_results
        .iter()
        .filter(|(fixture, _, _)| *fixture == "ws-d50-temp-gate")
        .count();
    assert_eq!(
        temp_gate_runs,
        1,
        "ws-d50-temp-gate must appear EXACTLY ONCE in the completed ported results; \
         found {temp_gate_runs}. Ported fixtures: {:?}",
        ported_results
            .iter()
            .map(|(f, _, _)| *f)
            .collect::<Vec<_>>()
    );

    eprintln!(
        "R4 differential: {} smoke + {} R4-A + {} R4-B + {} R4-C + {} R4-D + {} D1 wave fixture(s) \
         (+1 D1 negative); {} ported (all byte-matched); {} negatives passed; {} deferred.",
        SMOKE.len(),
        WAVE_A.len(),
        WAVE_B.len(),
        WAVE_C.len(),
        WAVE_D.len(),
        WAVE_D1.len(),
        ported_results.len(),
        NEGATIVES.len(),
        SMOKE.iter().filter(|s| !s.ported).count(),
    );
}
