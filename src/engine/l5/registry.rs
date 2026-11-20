//! The detector registry + `run_detectors` — port of al-sem
//! `src/detectors/registry.ts`.
//!
//! `run_detectors` builds the shared `DetectorContext`, runs each detector
//! (an `Err` becomes a diagnostic so one detector cannot kill the run — the real,
//! abort-safe contract; see the `run_detectors` doc comment), applies the
//! role-scoping filter, then sorts the combined Finding[] by
//! `(detector compareNatural, primaryLocationKey compareStrings, rootCauseKey
//! compareStrings)` over the INTERNAL ids. Per-detector dedup-by-id happens INSIDE
//! each detector, not here.
//!
//! `compare_natural` is ported exactly from al-sem `uncertainty-util.ts`
//! (digit-runs numeric, letter-runs lexicographic; "d2" < "d10"). `compareStrings`
//! is plain `str::cmp` (byte order), used inline.

use crate::engine::l5::detector_context::{DetectorContext, build_detector_context};
use crate::engine::l5::finding::{D1CohortIndex, Finding};
use crate::engine::perf_trace as pt;
use crate::program::model::workspace::Model;
use rayon::prelude::*;

/// Substrate demand bits (W1.0 demand-driven detector substrate).
///
/// `build_detector_context` ALWAYS builds the cheap, many-consumer CORE surface —
/// the model's calls and event graph, combined graph, reverse graph, entry
/// points, reachable roots, all borrowed indexes (routine/object/table/call-site
/// maps), `resolved_call_edge_by_callsite`, `uncertainty_edges_by_from`,
/// `dependency_uncertainty_edges_by_from` (#287), `upgraded_bindings_by_callsite`, `event_flow_indexes`, `cross_extension_subscribers`
/// (T3), `fingerprint_index` (T1), and `root_classifications_by_routine`. The four
/// EXPENSIVE substrates below are built only when some selected detector demands them;
/// `run_detectors` folds every detector's `requires` into the union it passes here.
/// No bit materializes raw inherited facts: a `View`'s witness request does
/// (policy check's, #160 step 6 Task 3).
///
/// Skipped substrates leave their ctx fields EMPTY (`HashMap::new()`/`Vec::new()`/
/// `Default::default()`) — the field TYPES are unchanged, so no detector needs to
/// change. The per-detector full-vs-minimal parity test is the enforcement: an
/// under-declared `requires` produces a finding divergence and fails the test.
///
/// A full/preset/all-detector run demands `ALL`, so the whole context — and thus the
/// entire report — is byte-identical to the pre-W1.0 eager build. The ONLY permitted
/// output change (decision (a), user-approved) is that a selection NOT demanding
/// `CORE_SUMMARIES` emits no summarize cap-hit diagnostics (they are harvested from
/// the same `compute_summaries_v2` call this substrate gates).
pub mod substrate {
    /// Capability cones + the per-routine `FullRoutineSummary` map (`ctx.summaries`).
    pub const SUMMARIES: u32 = 1 << 0;
    /// The closed-form v2 CORE summaries over the relation's SCC (no Tarjan of
    /// its own since #160 step 4: `ExecRelation::new` computes the SCC) →
    /// `ctx.uncertainties_by_node`, `ctx.parameter_roles_by_routine`, and the
    /// `summarize_diagnostics` cap-hit set (from the `parameter_roles`-only
    /// JACOBI fixpoint — the only fixpoint remaining since Task B1).
    pub const CORE_SUMMARIES: u32 = 1 << 1;
    /// Transaction spans (`ctx.transaction_spans`). Requires SUMMARIES internally —
    /// `compute_transaction_spans` folds over the summaries map — so the summaries
    /// block is built whenever this bit is set (see `build_detector_context`).
    pub const TRANSACTION_SPANS: u32 = 1 << 2;
    /// Closed-world proven-temp params (`ctx.closed_world_temp_params`).
    pub const CLOSED_WORLD_TEMP: u32 = 1 << 3;
    // `1 << 4` was policy's raw-cone bit, deleted by #160 step 6 Task 3:
    // policy reads the view's witness pass.
    /// ⟨Task 6⟩ The [`crate::engine::l4::reverse_index::ReverseEffectIndex`]
    /// transpose over the solve's bundle (`L4Analysis::bundle`), exposed as
    /// `ctx.reverse_effect_index`.
    ///
    /// **QUERY-ONLY, deliberately NOT part of [`ALL`], and — unlike every other
    /// bit here — NOT a substrate a detector may declare in `requires`.** The
    /// reason is less obvious, so read this before copying the pattern:
    ///
    /// `tests/gap/gap_detector_substrate_parity.rs` licenses each detector by
    /// building a "full" context from `substrate::ALL` and a "minimal" one from
    /// `det.requires`, then asserting identical findings. A detector declaring a
    /// bit OUTSIDE `ALL` inverts that comparison — the *minimal* context would
    /// have the substrate and the *full* one would not — so the detector's
    /// findings would differ and the test would fail while PRODUCTION stayed
    /// correct (`run_detectors` passes `demanded`, the fold of every selected
    /// detector's `requires`, which does include the bit). A failing license
    /// test that indicts correct code is worse than no test.
    ///
    /// **The one-line unlock**, when a detector genuinely needs this: change
    /// that test's full context to `substrate::ALL | det.requires`. Do that
    /// FIRST; do not add the bit to `ALL` (that would charge every `ALL`
    /// caller — the parity tests' full contexts and `tools/census-probe` — for
    /// a transpose none of them reads).
    ///
    /// Today's consumer is `alsem query`, which owns its own pipeline and never
    /// builds a `DetectorContext` at all — so this bit costs the analyze path
    /// nothing by construction. It exists for the eventual detector / LSP-hover
    /// path. Implies [`CORE_SUMMARIES`] (there is no bundle to transpose
    /// without it) — set both.
    pub const DB_EFFECT_REVERSE_INDEX: u32 = 1 << 5;
    /// ⟨2026-10-04⟩ The L4.5 ordering facts (`ctx.get_ordering_facts()`, read by
    /// d47/d49/d51). #160 step 2: the context's builder fills the ordering slot
    /// (`detector_context::fill_ordering`) ONCE, BEFORE the parallel detector
    /// loop, iff the mask has this bit; without it the slot stays empty.
    ///
    /// Computing them lazily INSIDE that loop deadlocked (`r4_differential` hung
    /// for 10 minutes at 0 % CPU; a dump confirmed it). The thread filling a
    /// `OnceLock` ran a rayon `par_iter`; while it waited for its halves, rayon
    /// made it steal queued jobs — including a half of the OUTER detector
    /// loop. That half's other half sat on another thread running d49, which
    /// blocked on the same `OnceLock`. Neither could finish. There is no lazy
    /// fill any more: `get_ordering_facts` panics on an empty slot, in every
    /// build. That shows a missing declaration only when no OTHER selected
    /// detector declares the bit, so the registry test
    /// `every_detector_alone_declares_what_it_reads` runs each registered
    /// detector by itself.
    ///
    /// Not in [`ALL`]: a context built from `ALL` reads no ordering facts and
    /// so never pays for them.
    pub const ORDERING_FACTS: u32 = 1 << 6;
    /// ⟨Intent evidence step 6⟩ The per-field writer index
    /// ([`crate::engine::l5::rules_index::RulesIndex`]: every table field the
    /// primary app writes, each writer with the rules in force at each write),
    /// `ctx.get_rules_index()`, read by d65. Filled ONCE, before the parallel
    /// detector loop (`detector_context::fill_rules`), iff the mask has this
    /// bit; `get_rules_index` panics on an empty slot, in every build, like
    /// `get_ordering_facts`.
    ///
    /// It is built from the routines' own rules, a level-1 fact
    /// (`ViewNeeds::rules`, built at model assembly): the command that demands
    /// this bit must build its model with them. `alsem analyze` does when a
    /// selected detector requires this bit
    /// (`registry::assembly_needs`); filling it from a model built without
    /// rules panics ("rules not built: declare ViewNeeds::rules"). Its cost is
    /// one cross-call rules walk (`l5::cross_rules`, bounded by its budget).
    /// It does not imply `ORDERING_FACTS`: statement order comes from each
    /// site's control context and anchor.
    ///
    /// Not in [`ALL`]: the many `build_detector_context(.., ALL)` callers build
    /// models without rules and read no index.
    pub const RULES: u32 = 1 << 7;
    /// Every substrate — the eager, pre-W1.0 behavior. Since #160 step 2 no
    /// command passes it: each builds its context in a `View` at the union of
    /// its runs' `requires`. The callers left are the parity tests' "full"
    /// contexts and `tools/census-probe`.
    ///
    /// This is an EXPLICIT OR list, not "all bits set". ⟨Task 6⟩
    /// [`DB_EFFECT_REVERSE_INDEX`] is excluded: it would charge every `ALL`
    /// caller for a transpose none of them reads, and it springs the
    /// detector-parity trap — see its own doc.
    pub const ALL: u32 = SUMMARIES | CORE_SUMMARIES | TRANSACTION_SPANS | CLOSED_WORLD_TEMP;
}

/// The level-1 facts a model must hold for `detectors` to run on it: the
/// rules (`ViewNeeds::rules`) when one of them requires [`substrate::RULES`],
/// else the base rows. Every builder that runs a detector list it did not
/// choose (the r4 projections, `aldump`'s detector modes, the switch dump)
/// builds its model with these needs, so a rules-reading detector never meets
/// a model without rules.
#[must_use]
pub fn assembly_needs(detectors: &[Detector]) -> crate::program::model::workspace::AssemblyNeeds {
    let rules = detectors.iter().any(|d| d.requires & substrate::RULES != 0);
    crate::program::model::workspace::AssemblyNeeds {
        rules,
        ..crate::program::model::workspace::AssemblyNeeds::BASE
    }
}

/// A diagnostic emitted when a detector fails — returns `Err`, or (debug builds
/// only) panics — (stage = "detect").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: String,
    pub stage: String,
    pub message: String,
}

/// Per-detector stats (al-sem `DetectorStats`).
///
/// `skipped` is a `BTreeMap<String, u64>` that serializes as a JSON object with keys
/// in alphabetical order (BTreeMap gives this for free). A key is inserted ONLY when
/// its count is > 0 — this present-iff-nonzero rule is UNIVERSAL across all detectors
/// (d43 was normalized to it too; its golden shows `{}`). An empty map serializes as `{}`.
///
/// Serialization contract (canonical sorted-key JSON):
///   - Keys are sorted alphabetically (`BTreeMap` iteration order).
///   - Field order in the JSON object is: `candidatesConsidered`, `detector`,
///     `findingsEmitted`, `skipped` — exactly the alphabetical order of those names.
///   - 2-space indent, trailing newline on the array.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectorStats {
    pub detector: String,
    pub candidates_considered: usize,
    pub findings_emitted: usize,
    /// Skip counters. Insert via the `add_skip` method (present-iff-nonzero). Keys must
    /// match the taxonomy exactly.
    pub skipped: std::collections::BTreeMap<String, u64>,
    /// Internal cut counters (TLS-106): a walk that stopped short of its whole
    /// answer (a node budget, a relay cap). Never serialized, so no output
    /// changes; read by [`RunOutput::ran_completely`] through
    /// [`Incomplete::Cut`]. Insert via [`DetectorStats::add_cut`].
    pub cuts: std::collections::BTreeMap<&'static str, u64>,
}

impl DetectorStats {
    /// Create a new `DetectorStats` with an empty skipped map.
    pub fn new(
        detector: impl Into<String>,
        candidates_considered: usize,
        findings_emitted: usize,
    ) -> Self {
        Self {
            detector: detector.into(),
            candidates_considered,
            findings_emitted,
            skipped: std::collections::BTreeMap::new(),
            cuts: std::collections::BTreeMap::new(),
        }
    }

    /// Add `n` to a skip counter, inserting it if absent. Only inserts when `n > 0`.
    pub fn add_skip(&mut self, key: &str, n: u64) {
        if n > 0 {
            *self.skipped.entry(key.to_string()).or_insert(0) += n;
        }
    }

    /// Add `n` to an internal cut counter (see `cuts`). Only inserts when `n > 0`.
    pub fn add_cut(&mut self, key: &'static str, n: u64) {
        if n > 0 {
            *self.cuts.entry(key).or_insert(0) += n;
        }
    }

    /// Serialize this stats object to a `serde_json::Value` with alphabetically-sorted
    /// keys, matching the al-sem `sortedReplacer` output. The field order is the
    /// alphabetical key sort: `candidatesConsidered`, `detector`, `findingsEmitted`, `skipped`.
    ///
    /// Correctness does NOT depend on the insertion order below: `serde_json::Map` is
    /// `BTreeMap`-backed (this crate does not enable the `preserve_order` feature), so it
    /// sorts keys automatically on serialization. If a future maintainer enables
    /// `preserve_order` (making `Map` an `IndexMap`), THIS code would then emit keys in
    /// insertion order — and would need explicit alphabetical insertion (which it already
    /// happens to do) plus the `skipped_obj` map to be sorted too.
    pub fn to_json_value(&self) -> serde_json::Value {
        let skipped_obj: serde_json::Map<String, serde_json::Value> = self
            .skipped
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::Number((*v).into())))
            .collect();
        let mut obj = serde_json::Map::new();
        obj.insert(
            "candidatesConsidered".to_string(),
            serde_json::Value::Number(self.candidates_considered.into()),
        );
        obj.insert(
            "detector".to_string(),
            serde_json::Value::String(self.detector.clone()),
        );
        obj.insert(
            "findingsEmitted".to_string(),
            serde_json::Value::Number(self.findings_emitted.into()),
        );
        obj.insert(
            "skipped".to_string(),
            serde_json::Value::Object(skipped_obj),
        );
        serde_json::Value::Object(obj)
    }
}

/// Serialize a `Vec<DetectorStats>` to a canonical sorted-key JSON string: 2-space
/// indent, trailing newline, all object keys in alphabetical order. This is the format
/// the al-sem golden files use (JSON.stringify with sortedReplacer + 2-space indent +
/// trailing newline).
///
/// Delegates to `format_json::serialize_document_value` (the single canonical
/// serializer) so stats and envelopes are always byte-consistent.
pub fn serialize_detector_stats(stats: &[DetectorStats]) -> String {
    let arr: Vec<serde_json::Value> = stats.iter().map(|s| s.to_json_value()).collect();
    let val = serde_json::Value::Array(arr);
    crate::engine::gate::format_json::serialize_document_value(val)
}

/// A detector's output.
///
/// Most detectors construct this as `DetectorOutput { findings, stats }` (no diagnostics).
/// The `diagnostics` field defaults to `vec![]` via the `Default` partial support:
/// use `DetectorOutput { findings, stats, ..DetectorOutput::empty() }` or the
/// two-field shorthand `{ findings, stats }` — BUT note the struct is not `Default`
/// (requires `Finding`/`DetectorStats` Default impls). For detectors that emit
/// diagnostics (e.g. d43 substrate guard), populate the field explicitly.
#[derive(Debug, PartialEq, Eq)]
pub struct DetectorOutput {
    pub findings: Vec<Finding>,
    pub stats: DetectorStats,
    /// Non-panic diagnostics emitted by the detector (e.g. d43 substrate guard warning).
    /// Propagates to `RunOutput.diagnostics` and thence to the JSON envelope.
    /// Omit in struct literals when empty — existing `{ findings, stats }` constructions
    /// must be updated to `{ findings, stats, diagnostics: vec![] }`. The helper
    /// `DetectorOutput::no_diag(findings, stats)` reduces boilerplate for detectors
    /// that never emit diagnostics.
    pub diagnostics: Vec<Diagnostic>,
    /// The run-level d1 cohort decompression index (loop catalog + loop-set
    /// registry) — `Some` ONLY on `detect_d1`'s output, `None` for every other
    /// detector. `run_each` lifts the (at most one) `Some` into `RunOutput` so the
    /// R4 projection can serialize it alongside the findings; the compressed d1
    /// report's `cohort_contexts[].loop_set` handles are meaningless without it.
    pub d1_cohort_index: Option<D1CohortIndex>,
}

impl DetectorOutput {
    /// Convenience constructor for the common case: no diagnostics.
    pub fn no_diag(findings: Vec<Finding>, stats: DetectorStats) -> Self {
        DetectorOutput {
            findings,
            stats,
            diagnostics: vec![],
            d1_cohort_index: None,
        }
    }
}

/// A recoverable detector failure. THE isolation contract (see `run_detectors`):
/// every detector returns `Result<DetectorOutput, DetectorError>`, and `run_each`
/// turns an `Err` into the `Detector "<name>" threw: <msg>` warning diagnostic —
/// the SAME message format a caught panic produces, so callers cannot tell the two
/// apart. `Display` supplies `<msg>`.
#[derive(Debug, Clone)]
pub struct DetectorError(String);

impl DetectorError {
    pub fn new(msg: impl Into<String>) -> Self {
        DetectorError(msg.into())
    }
}

impl std::fmt::Display for DetectorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DetectorError {}

/// A detector: a pure query over the resolved model + shared context. The closure
/// receives `(resolved, ctx)`; al-sem also passes the combined graph, which the
/// ctx already carries (`ctx.graph`), so detectors read it from there.
///
/// Returns `Result` — this IS the isolation contract (see `run_detectors`): an
/// `Err` degrades to a warning diagnostic while every other detector still runs,
/// and unlike `catch_unwind` this works identically under `panic = "abort"`
/// (`[profile.release]`, Cargo.toml), the profile every shipped binary uses.
#[derive(Clone)]
pub struct Detector {
    pub name: String,
    pub run: fn(&Model, &DetectorContext) -> Result<DetectorOutput, DetectorError>,
    /// The substrate bits (see `substrate`) this detector reads from the context.
    /// `run_detectors` folds every selected detector's `requires` into the union it
    /// hands to `build_detector_context`, so a substrate is built iff some selected
    /// detector demands it. Over-inclusive is SAFE (just less skipping);
    /// under-inclusive is caught by the full-vs-minimal parity test.
    pub requires: u32,
    /// The PascalCase SARIF rule name (`tool.driver.rules[].name`). Not derivable
    /// from `name`: `d3-missing-setloadfields` is `MissingSetLoadFields`.
    pub rule_name: &'static str,
    /// A one-line summary (`tool.driver.rules[].shortDescription.text`).
    pub short_description: &'static str,
    /// What the detector is about (ENG-168): `alsem analyze --focus` keeps only
    /// the named concerns' findings and counts the rest. Required, so every
    /// detector states one; the registry is its one home.
    pub concern: Concern,
}

/// What a detector is about (ENG-168, spec
/// `docs/superpowers/specs/2026-11-20-eng168-analyze-focus-design.md` §3.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Concern {
    /// How fast or how costly: SQL round trips, rows loaded, work repeated in
    /// loops, memory held.
    Performance,
    /// What the code does: data written or lost, transactions and commits,
    /// events, errors, exposure and permissions, values read wrong.
    Behaviour,
    /// How the code is kept: dead or unreachable code, unused parameters,
    /// obsolete or internal APIs, version drift. A risk for the next build, not
    /// a change in what runs.
    Maintainability,
}

impl Concern {
    /// Every concern, in `--focus` order.
    pub const ALL: [Concern; 3] = [
        Concern::Performance,
        Concern::Behaviour,
        Concern::Maintainability,
    ];

    /// The `--focus` value and the JSON spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Concern::Performance => "performance",
            Concern::Behaviour => "behaviour",
            Concern::Maintainability => "maintainability",
        }
    }

    /// Parse a `--focus` value (`behavior` is accepted for `behaviour`).
    pub fn parse(s: &str) -> Option<Concern> {
        match s.trim() {
            "performance" => Some(Concern::Performance),
            "behaviour" | "behavior" => Some(Concern::Behaviour),
            "maintainability" => Some(Concern::Maintainability),
            _ => None,
        }
    }
}

/// The combined output of `run_detectors`.
pub struct RunOutput {
    pub findings: Vec<Finding>,
    /// Aligned with `findings`: whether each is in the primary scope, by the
    /// predicate `--scope primary` drops by (`in_primary_scope` cross-app,
    /// the role rule single-app). All `true` for a `--scope primary` run,
    /// which kept only those (TLS-106: a baseline entry's scope).
    pub finding_is_primary: Vec<bool>,
    /// `(detector, fingerprint)` of every finding the primary-scope tail
    /// dropped (empty at `--scope all`). With `findings` it is the complete
    /// set the detectors emitted: a baseline update tests presence against
    /// both, so a finding that drifted out of the primary scope is still
    /// present (TLS-106).
    pub scoped_out: Vec<(String, String)>,
    pub diagnostics: Vec<Diagnostic>,
    pub detector_stats: Vec<DetectorStats>,
    /// The L4 "summarizeDiagnostics" source (TS-order slot 3 — see
    /// `gate/run.rs`'s `analyzer_diagnostics_of_run` doc) — presently just the
    /// JACOBI fixed-point cap-hit. Kept SEPARATE from `diagnostics` (the
    /// detect-stage source, slot 6) so the gate boundary can place each in its
    /// documented TS-concat position rather than collapsing both into "detect".
    /// Empty whenever every SCC converges (additive).
    pub summarize_diagnostics: Vec<Diagnostic>,
    /// The d1 detector's run-level cohort decompression index (loop catalog +
    /// loop-set registry), if d1 ran. `None` when d1 was not among the selected
    /// detectors (or it failed). Consumed by the R4 projection to serialize the
    /// catalog/registry envelope so a JSON consumer can decompress each d1
    /// finding's `cohort_contexts[].loop_set`.
    pub d1_cohort_index: Option<D1CohortIndex>,
}

impl RunOutput {
    /// The run's buffers, shallow (#277 D11's `runs` census): findings,
    /// diagnostics, stats and summarize diagnostics by `len × size_of`.
    pub fn census_tally(&self) -> crate::engine::census::Tally {
        let mut t = crate::engine::census::Tally::default();
        t.buf::<Finding>(self.findings.len())
            .buf::<Diagnostic>(self.diagnostics.len())
            .buf::<DetectorStats>(self.detector_stats.len())
            .buf::<Diagnostic>(self.summarize_diagnostics.len());
        t
    }

    /// Whether `detector` ran COMPLETELY in this run (TLS-106: only then may a
    /// baseline update treat a fingerprint it did not report as gone): it has a
    /// stats row (a detector that errors leaves only a diagnostic) and none of
    /// its [`incompleteness_signals`] fired. A detector with no stated signals
    /// (not registered) never ran completely, and none did when the L4
    /// summaries are partial (a non-empty `summarize_diagnostics`: a fixed
    /// point that did not converge).
    pub fn ran_completely(&self, detector: &str) -> bool {
        let Some(signals) = incompleteness_signals(detector) else {
            return false;
        };
        if !self.summarize_diagnostics.is_empty() {
            return false;
        }
        let Some(stats) = self.detector_stats.iter().find(|s| s.detector == detector) else {
            return false;
        };
        let prefix = format!("{detector}:");
        !signals.iter().any(|s| match s {
            Incomplete::Skip(key) => stats.skipped.get(*key).is_some_and(|n| *n > 0),
            Incomplete::Cut(key) => stats.cuts.get(key).is_some_and(|n| *n > 0),
            Incomplete::Diagnostic => self
                .diagnostics
                .iter()
                .any(|d| d.stage == "detect" && d.message.starts_with(&prefix)),
        })
    }
}

/// A sign that a detector returned `Ok` with less than its whole answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Incomplete {
    /// This `skipped` counter of its stats row is non-zero (a cap or a
    /// truncated walk dropped findings). Only counters the output already
    /// carried before TLS-106.
    Skip(&'static str),
    /// This internal cut counter (`DetectorStats::cuts`, never serialized) is
    /// non-zero.
    Cut(&'static str),
    /// It emitted a detect-stage diagnostic starting `<detector>:` (d43's
    /// substrate guard bails with one).
    Diagnostic,
}

/// Every registered detector's incompleteness signals, in this ONE place
/// (TLS-106). Each detector is named: `&[]` states it has none. `None` for a
/// name not listed, which [`RunOutput::ran_completely`] reads as incomplete.
///
/// Not listed here because the run's inputs cover them: a routine skipped as
/// `parseIncomplete` sits in a recovered file, and a recovered file stops all
/// pruning (`preflight::inputs_whole`). Fixed depth limits (d39's helper
/// depth, the path walker's depth 20, a statement tree's depth 256) bound a
/// finding's own path or routine, the same in every run, so they are no
/// signal; a node budget is, because what fills it is other code. d5's and
/// d60's `traversal` skip is a candidate ruled out (no whole-set advance, or
/// a broken traversal), not a walk that gave up. d1's production walk has no
/// budget (its `BOUNDS` is test-only).
pub fn incompleteness_signals(detector: &str) -> Option<&'static [Incomplete]> {
    use crate::engine::l5::detectors::*;
    match detector {
        // Caps on findings per event / try routine.
        d44::DETECTOR => Some(&[Incomplete::Skip("outputCapped")]),
        d54::DETECTOR => Some(&[Incomplete::Skip("cappedPerTryRoutine")]),
        // The per-publisher cap, and the relay walk's node cap.
        d45::DETECTOR => Some(&[
            Incomplete::Skip("outputCapped"),
            Incomplete::Cut("relayNodeCap"),
        ]),
        // The path walker's node budget.
        d2::DETECTOR | d46::DETECTOR | d48::DETECTOR => Some(&[Incomplete::Cut("nodeBudgetCut")]),
        // The cross-call rules walk stopped at its budget.
        d65::DETECTOR => Some(&[Incomplete::Skip("crossCallTruncated")]),
        // The conditionReferences substrate guard.
        d43::DETECTOR => Some(&[Incomplete::Diagnostic]),
        d1::DETECTOR
        | d3::DETECTOR
        | d4::DETECTOR
        | d5::DETECTOR
        | d7::DETECTOR
        | d8::DETECTOR
        | d9::DETECTOR
        | d10::DETECTOR
        | d11::DETECTOR
        | d12::DETECTOR
        | d13::DETECTOR
        | d14::DETECTOR
        | d16::DETECTOR
        | d17::DETECTOR
        | d18::DETECTOR
        | d19::DETECTOR
        | d20::DETECTOR
        | d21::DETECTOR
        | d22::DETECTOR
        | d29::DETECTOR
        | d32::DETECTOR
        | d33::DETECTOR
        | d34::DETECTOR
        | d35::DETECTOR
        | d36::DETECTOR
        | d37::DETECTOR
        | d38::DETECTOR
        | d39::DETECTOR
        | d40::DETECTOR
        | d41::DETECTOR
        | d42::DETECTOR
        | d47::DETECTOR
        | d49::DETECTOR
        | d50::DETECTOR
        | d51::DETECTOR
        | d52::DETECTOR
        | d53::DETECTOR
        | d55::DETECTOR
        | d56::DETECTOR
        | d57::DETECTOR
        | d58::DETECTOR
        | d59::DETECTOR
        | d60::DETECTOR
        | d61::DETECTOR
        | d62::DETECTOR
        | d63::DETECTOR
        | d64::DETECTOR => Some(&[]),
        _ => None,
    }
}

/// Convert an L4 `SummarizeDiagnostic` into the shared `l5::registry::Diagnostic`
/// shape — the same seam-conversion `gate/run.rs` already does for
/// `root_classification::InfraDiagnostic`.
fn from_summarize_diagnostic(
    d: &crate::engine::l4::summary_runner::SummarizeDiagnostic,
) -> Diagnostic {
    Diagnostic {
        severity: d.severity.clone(),
        stage: d.stage.clone(),
        message: d.message.clone(),
    }
}

/// `primaryLocationKey(f) = ${sourceUnitId}:${startLine}:${startColumn}` over the
/// INTERNAL anchor.
fn primary_location_key(f: &Finding) -> String {
    let a = &f.primary_location;
    format!("{}:{}:{}", a.source_unit_id, a.start_line, a.start_column)
}

/// Run every registered detector in isolation, then role-scope + sort.
///
/// Isolation is a `Result` CONTRACT (see `Detector::run` / `run_each`): every
/// detector returns `Result<DetectorOutput, DetectorError>`, and an `Err` becomes a
/// `Diagnostic(stage: "detect")` while the rest still run. This holds under BOTH
/// panic=unwind (`cargo test`) and the shipped `[profile.release] panic = "abort"`
/// (Cargo.toml) — it never depends on unwinding. `run_each` ALSO wraps each call in
/// `catch_unwind` as debug-build-only defense-in-depth (a detector that panics
/// despite the contract still degrades to the identical diagnostic under
/// panic=unwind); that wrapper is INERT in an abort release binary — `catch_unwind`
/// never catches anything there — so it must never be relied on as the real
/// guarantee.
pub fn run_detectors(resolved: &Model, detectors: &[Detector]) -> RunOutput {
    // W1.0 demand-driven substrate: build only the expensive substrates some selected
    // detector actually reads. A full/preset/all-detector selection unions to
    // `substrate::ALL`, so the context — and the whole report — stays byte-identical.
    let demanded = detectors.iter().fold(0u32, |acc, d| acc | d.requires);
    // `context.build_total` brackets the WHOLE context build so its own
    // unspanned stretches show up as this span's self time rather than as
    // `l4_l5.run_detectors`'. `build_detector_context` opens eight `context.*`
    // spans internally, but they do not tile it — a 4-run 8020 profile left
    // 9.3 % of the whole run (≈ 7.1 s) in `l4_l5.run_detectors`' self time with
    // nothing naming it.
    let owned = {
        let _s = pt::span("context", "context.build_total");
        build_detector_context(resolved, demanded)
    };
    let ctx = owned.ctx();
    let run = finish_run(
        resolved,
        &ctx,
        detectors,
        None,
        true,
        &ctx.summarize_diagnostics,
    );

    // The context dies here either way — it is a local of this function and
    // borrows nothing that outlives it. Dropping it explicitly, inside a span,
    // prices the teardown of the cones/summaries/spans substrate instead of
    // leaving it in this function's unattributed self time. No `Drop` impl is
    // involved (the engine's only two are `perf_trace`'s guards), so the
    // reorder is pure deallocation and not observable.
    {
        let _s = pt::span("context", "context.ctx_drop");
        drop(ctx);
        drop(owned);
    }
    run
}

/// #160 step 2 (spec §4.4.3 rule 3): one run over a built context —
/// [`run_each`], then the role-scoping tail — shared by [`run_detectors`],
/// [`run_detectors_cross_app`] and `session::View::prepare`, so the paths
/// cannot drift. `cross` is the cross-app base (its dependency roles), `None`
/// single-app (every routine primary; `scope_primary` is then moot).
/// `summarize` is the run's own summarize source (rule 3: from its mask).
pub(crate) fn finish_run(
    resolved: &Model,
    ctx: &DetectorContext,
    detectors: &[Detector],
    cross: Option<&crate::engine::l4::capability_cone::R3a5CrossAppBase>,
    scope_primary: bool,
    summarize: &[crate::engine::l4::summary_runner::SummarizeDiagnostic],
) -> RunOutput {
    let summarize_diagnostics: Vec<Diagnostic> =
        summarize.iter().map(from_summarize_diagnostic).collect();
    let (findings, diagnostics, detector_stats, d1_cohort_index) =
        run_each(resolved, ctx, detectors);
    let Scoped {
        findings,
        primary: finding_is_primary,
        scoped_out,
    } = match cross {
        None => {
            // Role-scoping filter (registry.ts:161-172), by THE role rule. A
            // single-app model holds only the workspace's routines, so every
            // role is "primary" and the rule keeps everything. Single-app keeps
            // the role rule alone (no `dep:` location check), so its output is
            // unchanged by construction (TLS-106 review r2); it filters and
            // labels by that one rule.
            let _s = pt::span("l4_l5", "l4_l5.role_scope_and_sort");
            let role_by_routine: std::collections::HashMap<&str, &str> = resolved
                .workspace
                .routines
                .iter()
                .map(|r| (r.id.as_str(), resolved.routine_role(r).as_str()))
                .collect();
            scope_and_sort(findings, &role_by_routine, true, has_primary_role)
        }
        Some(base) => cross_app_scope(base, findings, scope_primary),
    };
    RunOutput {
        findings,
        finding_is_primary,
        scoped_out,
        diagnostics,
        detector_stats,
        summarize_diagnostics,
        d1_cohort_index,
    }
}

/// CROSS-APP variant of `run_detectors`: build the cross-app context from the
/// pre-assembled `R3a5CrossAppBase`, run every detector, then role-scope with
/// `dep_routine_ids`-derived roles (`"dependency"` for dep routines, `"primary"`
/// else) so dep-anchored findings are dropped by the existing scope filter. d13/d16
/// already gate `roleOf(caller)` internally via `ctx.dep_routine_ids`; the scope
/// filter is the second, anchor-based safety net (registry.ts:161-172 parity).
pub(crate) fn run_detectors_cross_app(
    base: &crate::engine::l4::capability_cone::R3a5CrossAppBase,
    detectors: &[Detector],
    scope_primary: bool,
) -> RunOutput {
    // One builder for both modes (engine-switch S8.1), demand-gated like
    // `run_detectors`.
    let demanded = detectors.iter().fold(0u32, |acc, d| acc | d.requires);
    let owned = {
        let _s = pt::span("crossapp", "crossapp.context_build");
        crate::engine::l5::detector_context::build_detector_context_with(
            &base.resolved,
            demanded,
            Some(base),
        )
    };
    let ctx = owned.ctx();
    // The detectors close over `(resolved, ctx)`: the base's own cross-app model
    // (engine-switch S7.6), with its program calls and root classifications. It
    // used to be a throwaway copy without calls, so a detector reading its calls
    // fell back to L3's resolver (deleted in engine-switch S9.6).
    let run = finish_run(
        &base.resolved,
        &ctx,
        detectors,
        Some(base),
        scope_primary,
        &ctx.summarize_diagnostics,
    );
    {
        let _s = pt::span("crossapp", "crossapp.ctx_drop");
        drop(ctx);
        drop(owned);
    }
    run
}

/// The cross-app role-scoping tail of [`finish_run`].
fn cross_app_scope(
    base: &crate::engine::l4::capability_cone::R3a5CrossAppBase,
    findings: Vec<Finding>,
    scope_primary: bool,
) -> Scoped {
    // role_by_routine: dep routines → "dependency", else "primary". An
    // object-anchored finding (d64 names its page) carries the OBJECT id there, so
    // objects are roled by their app too (engine-switch S7.4: the cross-app model
    // holds dependency objects, and a Microsoft test library's API page is not the
    // workspace's to fix).
    let model = &base.resolved;
    let role_by_routine: std::collections::HashMap<&str, &str> = base
        .ws_routines()
        .iter()
        .map(|r| (r.id.as_str(), model.routine_role(r).as_str()))
        .chain(
            base.objects()
                .iter()
                .map(|o| (o.id.as_str(), model.object_role(o).as_str())),
        )
        .collect();
    // A finding whose primary location is in dependency source (`dep:` unit) is
    // out of scope even when its enclosing routine is the workspace's: the user
    // cannot change that line (engine-switch S7.6 triage: d47 anchored an IO fact
    // inside the System Application's `Temp Blob`). An actionable anchor in the
    // workspace keeps it.
    let _s = pt::span("crossapp", "crossapp.role_scope");
    // `--scope all` (alsem analyze) keeps dependency-anchored findings: only
    // the sort applies, and each finding is labelled by the same predicate.
    scope_and_sort(findings, &role_by_routine, scope_primary, in_primary_scope)
}

/// A run's findings after the scope tail.
struct Scoped {
    findings: Vec<Finding>,
    /// Aligned with `findings`: the path's primary predicate.
    primary: Vec<bool>,
    /// `(detector, fingerprint)` of each finding the tail dropped.
    scoped_out: Vec<(String, String)>,
}

/// The scope predicate of a path: [`in_primary_scope`] cross-app,
/// [`has_primary_role`] single-app.
type ScopePredicate = fn(&Finding, &std::collections::HashMap<&str, &str>) -> bool;

/// The scope tail: with `drop_dependency`, drop every finding outside
/// `is_primary` (remembering its fingerprint); label the rest by the same
/// predicate; then the stable sort. One predicate for the filter and the
/// label on each path (TLS-106).
fn scope_and_sort(
    findings: Vec<Finding>,
    role_by_routine: &std::collections::HashMap<&str, &str>,
    drop_dependency: bool,
    is_primary: ScopePredicate,
) -> Scoped {
    let mut kept = Vec::with_capacity(findings.len());
    let mut primary = Vec::with_capacity(findings.len());
    let mut scoped_out = Vec::new();
    for f in findings {
        let p = is_primary(&f, role_by_routine);
        if p || !drop_dependency {
            kept.push((p, f));
        } else {
            scoped_out.push((f.detector.clone(), finding_fingerprint(&f)));
        }
    }
    sort_by_detector_location_cause(
        &mut kept,
        |(_, f)| &f.detector,
        |(_, f)| primary_location_key(f),
        |(_, f)| &f.root_cause_key,
    );
    let findings = kept
        .into_iter()
        .map(|(p, f)| {
            primary.push(p);
            f
        })
        .collect();
    Scoped {
        findings,
        primary,
        scoped_out,
    }
}

/// A finding's fingerprint as the gate projects it (`finding.fingerprint ??
/// finding.id`, `projection::project_finding`).
pub(crate) fn finding_fingerprint(f: &Finding) -> String {
    f.fingerprint.clone().unwrap_or_else(|| f.id.clone())
}

thread_local! {
    static IN_DETECTOR_LOOP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Marks the current thread as running a detector inside `run_each`'s parallel
/// loop, restoring the previous value on drop (a thread that steals a second
/// detector while waiting nests these).
struct InDetectorLoop(bool);

impl InDetectorLoop {
    fn enter() -> Self {
        InDetectorLoop(IN_DETECTOR_LOOP.with(|f| f.replace(true)))
    }
}

impl Drop for InDetectorLoop {
    fn drop(&mut self) {
        IN_DETECTOR_LOOP.with(|f| f.set(self.0));
    }
}

/// True while this thread is running a detector inside `run_each`'s parallel
/// loop. A lazy substrate must not be FILLED there — see
/// `substrate::ORDERING_FACTS`.
pub(crate) fn in_detector_loop() -> bool {
    IN_DETECTOR_LOOP.with(|f| f.get())
}

/// Run each detector in isolation via the `Result` contract (see `run_detectors`'s
/// doc comment for the full guarantee), collecting findings + stats.
#[allow(clippy::type_complexity)]
fn run_each(
    resolved: &Model,
    ctx: &DetectorContext,
    detectors: &[Detector],
) -> (
    Vec<Finding>,
    Vec<Diagnostic>,
    Vec<DetectorStats>,
    Option<D1CohortIndex>,
) {
    let _total_span = pt::span("detector", "detectors.total");

    // ── Run every detector IN PARALLEL, then fold sequentially ───────────────
    //
    // `detector.run` is a plain `fn(&Model, &DetectorContext)` — both
    // arguments are immutable shared borrows and no detector writes through
    // them, so the run phase has no shared mutable state at all. What DOES have
    // to stay ordered is the accumulation below: `findings` order survives into
    // the output through `scope_and_sort`, which is a STABLE sort, so two
    // findings tying on (detector, primaryLocationKey, rootCauseKey) keep their
    // insertion order; `diagnostics` and `detector_stats` are emitted in
    // detector order into the JSON envelope.
    //
    // Both properties are preserved exactly: `par_iter().collect()` on an
    // INDEXED parallel iterator yields results in ITERATION order regardless of
    // completion order, and the fold below then walks that Vec sequentially in
    // the same order the old `for` loop used. This is the same guarantee (and
    // the same reasoning) `resolve_full_program_from_parts` already relies on
    // for its per-file resolution.
    //
    // Runs on a dedicated big-stack pool rather than the rayon global pool: d1's
    // cohort walk and the CFG walker recurse over real Base Application source,
    // the same hazard `snapshot::parse` and the resolver already route around
    // (`crate::big_stack`).
    let pool = crate::big_stack::big_stack_pool();

    // The ordering slot was filled by the context's builder, before this loop
    // (`substrate::ORDERING_FACTS`); nothing is filled in here.
    type RunOutcome = std::thread::Result<Result<DetectorOutput, DetectorError>>;
    let outcomes: Vec<RunOutcome> = pool.install(|| {
        detectors
            .par_iter()
            .map(|detector| {
                // Dynamic per-detector span name (`detector.<name>`) — the
                // `TraceName::Owned` `String` variant. Built ONLY when Stages
                // tracing is enabled: `pt::span`'s `name: impl Into<TraceName>`
                // argument is evaluated by the CALLER before the function's own
                // internal `tracer()` gate runs, so an unconditional `format!`
                // would allocate on every detector even with tracing off.
                //
                // These spans now OVERLAP in the trace (one per worker thread,
                // each on its own tid) instead of tiling `detectors.total`. That
                // is the honest picture of what the process does; a self-time
                // reading of `detectors.total` is no longer meaningful and the
                // wall-clock floor is the SLOWEST detector, not the sum.
                let _detector_span = if pt::enabled(pt::Detail::Stages) {
                    Some(pt::span("detector", format!("detector.{}", detector.name)))
                } else {
                    None
                };
                // `catch_unwind` here is debug-build-only defense-in-depth (see
                // the `run_detectors` doc comment) — it is INERT under
                // `panic = "abort"`. The real, abort-safe isolation is the
                // `Result` returned by `detector.run` itself, handled in the
                // `Ok(Err(e))` arm below with the identical diagnostic shape a
                // caught panic produces.
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _in_loop = InDetectorLoop::enter();
                    (detector.run)(resolved, ctx)
                }))
            })
            .collect()
    });

    let mut findings: Vec<Finding> = Vec::new();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut detector_stats: Vec<DetectorStats> = Vec::new();
    // At most ONE detector (d1) produces a cohort index; keep the (single) `Some`.
    let mut d1_cohort_index: Option<D1CohortIndex> = None;

    for (detector, outcome) in detectors.iter().zip(outcomes) {
        match outcome {
            Ok(Ok(output)) => {
                // STRUCTS: findings count alongside the span's own RSS delta.
                // Emitted HERE rather than inside the parallel closure so its
                // position in the trace stays deterministic (detector order)
                // even though the runs themselves interleave.
                pt::instant_lazy("detector", "detector.result", || {
                    serde_json::json!({
                        "detector": detector.name.as_str(),
                        "findings": output.findings.len(),
                    })
                });
                findings.extend(output.findings);
                // Collect detector-emitted diagnostics (non-error; d43 substrate guard etc.)
                diagnostics.extend(output.diagnostics);
                detector_stats.push(output.stats);
                if let Some(idx) = output.d1_cohort_index {
                    d1_cohort_index = Some(idx);
                }
            }
            Ok(Err(e)) => {
                diagnostics.push(Diagnostic {
                    severity: "warning".to_string(),
                    stage: "detect".to_string(),
                    message: format!("Detector \"{}\" threw: {e}", detector.name),
                });
            }
            Err(panic_payload) => {
                let msg = panic_payload
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic_payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "panic".to_string());
                diagnostics.push(Diagnostic {
                    severity: "warning".to_string(),
                    stage: "detect".to_string(),
                    message: format!("Detector \"{}\" threw: {msg}", detector.name),
                });
            }
        }
    }
    (findings, diagnostics, detector_stats, d1_cohort_index)
}

/// The role rule: the finding's enclosing routine (or object), or its
/// actionable anchor's, is primary. A routine absent from the map is primary.
fn has_primary_role(f: &Finding, role_by_routine: &std::collections::HashMap<&str, &str>) -> bool {
    let primary_role = role_by_routine
        .get(f.primary_location.enclosing_routine_id.as_str())
        .copied()
        .unwrap_or("primary");
    if primary_role == "primary" {
        return true;
    }
    if let Some(anchor) = &f.actionable_anchor {
        let anchor_role = role_by_routine
            .get(anchor.enclosing_routine_id.as_str())
            .copied()
            .unwrap_or("primary");
        if anchor_role == "primary" {
            return true;
        }
    }
    false
}

/// THE primary-scope predicate, the one `--scope primary` drops by and the one
/// [`RunOutput::finding_is_primary`] labels with (a baseline entry's scope,
/// TLS-106): the role rule, and a location outside dependency source (`dep:`)
/// unless the actionable anchor is in the workspace.
fn in_primary_scope(f: &Finding, role_by_routine: &std::collections::HashMap<&str, &str>) -> bool {
    let dep_located = f.primary_location.source_unit_id.starts_with("dep:")
        && f.actionable_anchor
            .as_ref()
            .is_none_or(|a| a.source_unit_id.starts_with("dep:"));
    !dep_located && has_primary_role(f, role_by_routine)
}

/// Port of al-sem `compareNatural`: split each string into runs of digits and
/// non-digits; compare digit runs numerically and non-digit runs by byte order.
/// On a prefix tie, the shorter token list sorts first. ("d2" < "d10".)
pub fn compare_natural(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let pa = tokenize(a);
    let pb = tokenize(b);
    let len = pa.len().min(pb.len());
    for i in 0..len {
        let ta = &pa[i];
        let tb = &pb[i];
        let a_is_num = ta.chars().next().is_some_and(|c| c.is_ascii_digit());
        let b_is_num = tb.chars().next().is_some_and(|c| c.is_ascii_digit());
        if a_is_num && b_is_num {
            // Compare numerically. al-sem uses parseInt (base 10) into a JS
            // number; corpus ids stay well within u128, so parse into u128.
            let na: u128 = ta.parse().unwrap_or(0);
            let nb: u128 = tb.parse().unwrap_or(0);
            if na != nb {
                return na.cmp(&nb);
            }
        } else if a_is_num != b_is_num {
            // A numeric token sorts before a non-numeric token.
            return if a_is_num {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        } else if ta != tb {
            return ta.as_str().cmp(tb.as_str());
        }
    }
    pa.len().cmp(&pb.len())
}

/// Split a string into maximal runs of digits / non-digits, matching the JS regex
/// `/(\d+)|(\D+)/g`.
fn tokenize(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_is_digit: Option<bool> = None;
    for ch in s.chars() {
        let is_digit = ch.is_ascii_digit();
        match cur_is_digit {
            Some(d) if d == is_digit => cur.push(ch),
            Some(_) => {
                out.push(std::mem::take(&mut cur));
                cur.push(ch);
                cur_is_digit = Some(is_digit);
            }
            None => {
                cur.push(ch);
                cur_is_digit = Some(is_digit);
            }
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// TLS-27: each name's rank under [`compare_natural`]: an exact monotone image of its
/// order, so `rank[a].cmp(&rank[b]) == compare_natural(a, b)` for every pair. Names that
/// compare Equal without being the same string (`"d1"` and `"d01"`: their number tokens
/// are both 1) share a rank, so a later tie-break decides between them, as before.
fn natural_ranks<'a>(
    names: impl IntoIterator<Item = &'a str>,
) -> std::collections::HashMap<String, usize> {
    let mut distinct: Vec<&str> = names
        .into_iter()
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    distinct.sort_by(|a, b| compare_natural(a, b));
    let mut ranks = std::collections::HashMap::with_capacity(distinct.len());
    let mut rank = 0usize;
    for (i, name) in distinct.iter().enumerate() {
        if i > 0 && compare_natural(distinct[i - 1], name) != std::cmp::Ordering::Equal {
            rank += 1;
        }
        ranks.insert((*name).to_string(), rank);
    }
    ranks
}

/// TLS-27: the findings' order: detector by [`compare_natural`], then the primary
/// location key, then the root-cause key. A STABLE sort, so findings tying on all three
/// keep their order. The same order as comparing
/// `compare_natural(detector).then(location_key.cmp).then(root_cause_key.cmp)` on every
/// pair. But each item's key is built once, not twice per comparison: the detector as its
/// [`natural_ranks`] rank, the location key as its string, the root-cause key copied.
/// `sort_by_cached_key` sorts the keys with their indices and then permutes the items in
/// place, so no second buffer of items is held (code review, 2026-11-12).
///
/// The location key stays the STRING `"{unit}:{line}:{column}"`, compared as text. That
/// is not the order of `(unit, line, column)` compared field by field: `"a1:…"` sorts
/// before `"a:…"`, because `'1' < ':'`.
pub(crate) fn sort_by_detector_location_cause<T>(
    items: &mut [T],
    detector: impl Fn(&T) -> &str,
    location_key: impl Fn(&T) -> String,
    root_cause_key: impl Fn(&T) -> &str,
) {
    let ranks = natural_ranks(items.iter().map(&detector));
    items.sort_by_cached_key(|t| {
        (
            ranks[detector(t)],
            location_key(t),
            root_cause_key(t).to_owned(),
        )
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Cross-app scope: a finding located in dependency source is out
    // -----------------------------------------------------------------------

    static DEP_LOCATED: std::sync::OnceLock<Finding> = std::sync::OnceLock::new();

    fn dep_located_probe(_: &Model, _: &DetectorContext) -> Result<DetectorOutput, DetectorError> {
        let f = DEP_LOCATED.get().expect("template set").clone();
        Ok(DetectorOutput::no_diag(
            vec![f],
            DetectorStats::new("probe", 1, 1),
        ))
    }

    /// Engine-switch S7.6 triage fix, hand-stated: a finding whose ENCLOSING
    /// routine is the workspace's but whose primary location is dependency source
    /// (`dep:` unit) is dropped by `run_detectors_cross_app` — the user cannot
    /// change that line (CDO d47 located an IO fact inside the System
    /// Application's `Temp Blob`). The finding is a real d13 finding with only its
    /// location moved.
    ///
    /// Discrimination (2026-10-06): removing the `dep:` location filter keeps the
    /// probe's finding (`left: 1`); restored, it passes.
    #[test]
    fn cross_app_scope_drops_a_finding_located_in_dependency_source() {
        let ws = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/r0-corpus/ws-d13-internal-call");
        let inputs = crate::program::inputs::WorkspaceInputs::read(&ws, &Default::default());
        let base = crate::engine::l4::capability_cone::build_cross_app_base(&inputs)
            .expect("cross-app base");
        let d13 = crate::engine::l5::detectors::registered_detectors()
            .into_iter()
            .find(|d| d.name == "d13-cross-app-internal-call")
            .unwrap();
        let real = run_detectors_cross_app(&base, std::slice::from_ref(&d13), true);
        assert_eq!(real.findings.len(), 1, "the fixture's d13 finding");
        let mut moved = real.findings[0].clone();
        assert!(moved.primary_location.source_unit_id.starts_with("ws:"));
        moved.primary_location.source_unit_id = "dep:dddd:src/Lib.al".to_string();
        DEP_LOCATED.set(moved).unwrap();
        let probe = Detector {
            name: "probe".to_string(),
            run: dep_located_probe,
            requires: d13.requires,
            rule_name: "Probe",
            short_description: "test probe",
            concern: Concern::Behaviour,
        };
        let out = run_detectors_cross_app(&base, std::slice::from_ref(&probe), true);
        assert_eq!(out.findings.len(), 0);
    }

    static ROLE_PROBE: std::sync::OnceLock<Vec<Finding>> = std::sync::OnceLock::new();

    fn role_probe(_: &Model, _: &DetectorContext) -> Result<DetectorOutput, DetectorError> {
        let f = ROLE_PROBE.get().expect("template set").clone();
        Ok(DetectorOutput::no_diag(
            f,
            DetectorStats::new("probe", 1, 1),
        ))
    }

    /// T2 (#220 spec §5): `cross_app_scope` decides roles by THE role rule
    /// (`Model::routine_role` / `object_role`). Three hand-stated findings
    /// (a real d13 finding with its anchor moved): one anchored on a
    /// DEPENDENCY OBJECT id, one on a DEPENDENCY ROUTINE id, and one located
    /// in `dep:` source with a primary actionable anchor. `--scope primary`
    /// drops the first two and keeps the third; `--scope all` keeps all
    /// three. The `dep:` test above stays.
    ///
    /// Break: `role_of_app` always `Primary` -> both dependency-anchored
    /// findings are kept under `--scope primary`, fails.
    #[test]
    fn cross_app_scope_uses_role() {
        let ws = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/r0-corpus/ws-d13-internal-call");
        let inputs = crate::program::inputs::WorkspaceInputs::read(&ws, &Default::default());
        let base = crate::engine::l4::capability_cone::build_cross_app_base(&inputs)
            .expect("cross-app base");
        let primary = base
            .resolved
            .primary_app
            .as_ref()
            .expect("primary app")
            .app_guid
            .clone();
        let dep_object = base
            .objects()
            .iter()
            .find(|o| !o.app_guid.eq_ignore_ascii_case(&primary))
            .expect("precondition: the model holds a dependency object")
            .id
            .clone();
        let dep_routine = base
            .ws_routines()
            .iter()
            .find(|r| !r.app_guid.eq_ignore_ascii_case(&primary))
            .expect("precondition: the model holds a dependency routine")
            .id
            .clone();
        let primary_routine = base
            .ws_routines()
            .iter()
            .find(|r| r.app_guid.eq_ignore_ascii_case(&primary))
            .expect("precondition: a primary routine")
            .id
            .clone();
        let d13 = crate::engine::l5::detectors::registered_detectors()
            .into_iter()
            .find(|d| d.name == "d13-cross-app-internal-call")
            .unwrap();
        let real = run_detectors_cross_app(&base, std::slice::from_ref(&d13), true);
        assert_eq!(real.findings.len(), 1, "the fixture's d13 finding");
        let template = real.findings[0].clone();
        let mut on_object = template.clone();
        on_object.root_cause_key = "on-dep-object".to_string();
        on_object.primary_location.enclosing_routine_id = dep_object;
        on_object.actionable_anchor = None;
        let mut on_routine = template.clone();
        on_routine.root_cause_key = "on-dep-routine".to_string();
        on_routine.primary_location.enclosing_routine_id = dep_routine;
        on_routine.actionable_anchor = None;
        let mut dep_located = template.clone();
        dep_located.root_cause_key = "dep-located".to_string();
        dep_located.primary_location.source_unit_id = "dep:dddd:src/Lib.al".to_string();
        let mut anchor = template.primary_location.clone();
        anchor.enclosing_routine_id = primary_routine;
        assert!(anchor.source_unit_id.starts_with("ws:"));
        dep_located.actionable_anchor = Some(anchor);
        ROLE_PROBE
            .set(vec![on_object, on_routine, dep_located])
            .unwrap();
        let probe = Detector {
            name: "probe".to_string(),
            run: role_probe,
            requires: d13.requires,
            rule_name: "Probe",
            short_description: "test probe",
            concern: Concern::Behaviour,
        };
        let keys = |scope_primary: bool| -> Vec<String> {
            let mut k: Vec<String> =
                run_detectors_cross_app(&base, std::slice::from_ref(&probe), scope_primary)
                    .findings
                    .into_iter()
                    .map(|f| f.root_cause_key)
                    .collect();
            k.sort();
            k
        };
        assert_eq!(
            keys(true),
            vec!["dep-located".to_string()],
            "--scope primary"
        );
        assert_eq!(
            keys(false),
            vec![
                "dep-located".to_string(),
                "on-dep-object".to_string(),
                "on-dep-routine".to_string()
            ],
            "--scope all"
        );
        // TLS-106: each finding's label is the predicate `--scope primary`
        // drops by. A primary run keeps only primaries; an all run labels the
        // two dependency-anchored findings dependency.
        //
        // Mutation (2026-11-20): `cross_app_scope`'s labels made
        // `findings.iter().map(|_| true)` -> the `--scope all` labels read all
        // primary (red); restored, green.
        let labels = |scope_primary: bool| -> Vec<(String, bool)> {
            let out = run_detectors_cross_app(&base, std::slice::from_ref(&probe), scope_primary);
            let mut l: Vec<(String, bool)> = out
                .findings
                .into_iter()
                .map(|f| f.root_cause_key)
                .zip(out.finding_is_primary)
                .collect();
            l.sort();
            l
        };
        assert_eq!(labels(true), vec![("dep-located".to_string(), true)]);
        // The primary tail remembers what it dropped (TLS-106: presence before
        // the scope tail).
        //
        // Mutation (2026-11-20): `scoped_out.push` removed from
        // `scope_and_sort` -> `scoped_out` is empty (red); restored, green.
        let out = run_detectors_cross_app(&base, std::slice::from_ref(&probe), true);
        assert_eq!(out.scoped_out.len(), 2, "on-dep-object, on-dep-routine");
        assert!(
            out.scoped_out
                .iter()
                .all(|(d, _)| d == "d13-cross-app-internal-call")
        );
        assert_eq!(
            labels(false),
            vec![
                ("dep-located".to_string(), true),
                ("on-dep-object".to_string(), false),
                ("on-dep-routine".to_string(), false)
            ]
        );
    }

    fn stats_with(detector: &str, skip: Option<&str>) -> DetectorStats {
        let mut s = DetectorStats::new(detector, 1, 0);
        if let Some(key) = skip {
            s.add_skip(key, 1);
        }
        s
    }

    fn run_of(stats: Vec<DetectorStats>, diagnostics: Vec<Diagnostic>) -> RunOutput {
        RunOutput {
            findings: vec![],
            finding_is_primary: vec![],
            scoped_out: vec![],
            diagnostics,
            detector_stats: stats,
            summarize_diagnostics: vec![],
            d1_cohort_index: None,
        }
    }

    /// TLS-106: every registered detector states its incompleteness signals
    /// (none is a statement), so a new detector cannot be pruned on before
    /// someone has said what makes its run incomplete.
    ///
    /// Mutation (2026-11-20): the `d64::DETECTOR` arm removed -> d64 has no
    /// statement (red); restored, green.
    #[test]
    fn every_registered_detector_states_its_incompleteness_signals() {
        let missing: Vec<String> = crate::engine::l5::detectors::registered_detectors()
            .into_iter()
            .map(|d| d.name)
            .filter(|n| incompleteness_signals(n).is_none())
            .collect();
        assert!(missing.is_empty(), "no statement for {missing:?}");
    }

    /// TLS-106: a detector ran completely only with a stats row and no fired
    /// signal: d65's `crossCallTruncated`, d43's substrate diagnostic, a
    /// detector that errored (no row), an unlisted name.
    ///
    /// Mutation (2026-11-20): `ran_completely` returns `true` once the stats
    /// row is found, ignoring the signals -> the truncated d65 and the d43
    /// warning read complete (red); restored, green.
    #[test]
    fn ran_completely_reads_the_stats_row_and_the_signals() {
        use crate::engine::l5::detectors::{d1, d43, d65};
        let d43_warning = Diagnostic {
            severity: "warning".to_string(),
            stage: "detect".to_string(),
            message: format!("{}: conditionReferences substrate empty", d43::DETECTOR),
        };
        let complete = run_of(
            vec![
                stats_with(d1::DETECTOR, Some("tempRecord")),
                stats_with(d65::DETECTOR, Some("tooFewWriters")),
                stats_with(d43::DETECTOR, None),
                stats_with("probe", None),
            ],
            vec![],
        );
        assert!(complete.ran_completely(d1::DETECTOR));
        assert!(complete.ran_completely(d65::DETECTOR));
        assert!(complete.ran_completely(d43::DETECTOR));
        assert!(!complete.ran_completely("probe"), "unlisted name");
        let incomplete = run_of(
            vec![
                stats_with(d65::DETECTOR, Some("crossCallTruncated")),
                stats_with(d43::DETECTOR, None),
            ],
            vec![d43_warning],
        );
        assert!(!incomplete.ran_completely(d65::DETECTOR), "truncated");
        assert!(
            !incomplete.ran_completely(d43::DETECTOR),
            "substrate warning"
        );
        assert!(!incomplete.ran_completely(d1::DETECTOR), "errored: no row");
    }

    /// TLS-106: an internal cut (d46's node budget, d45's relay cap) makes a
    /// detector incomplete without touching its serialized stats, and a
    /// non-empty `summarize_diagnostics` makes every detector incomplete.
    ///
    /// Mutation (2026-11-20): the `Incomplete::Cut` arm made `false` -> the
    /// cut d46 and d45 read complete (red); restored, green.
    /// Mutation (2026-11-20): the `summarize_diagnostics` test removed from
    /// `ran_completely` -> d1 reads complete beside a partial summary (red);
    /// restored, green.
    #[test]
    fn internal_cuts_and_partial_summaries_are_incomplete() {
        use crate::engine::l5::detectors::{d1, d45, d46};
        let mut d46_cut = stats_with(d46::DETECTOR, None);
        d46_cut.add_cut("nodeBudgetCut", 1);
        let mut d45_cut = stats_with(d45::DETECTOR, None);
        d45_cut.add_cut("relayNodeCap", 1);
        assert_eq!(
            d46_cut.to_json_value(),
            stats_with(d46::DETECTOR, None).to_json_value(),
            "a cut is never serialized"
        );
        let run = run_of(
            vec![d46_cut, d45_cut, stats_with(d1::DETECTOR, None)],
            vec![],
        );
        assert!(!run.ran_completely(d46::DETECTOR), "node budget");
        assert!(!run.ran_completely(d45::DETECTOR), "relay cap");
        assert!(run.ran_completely(d1::DETECTOR));
        let mut partial = run_of(vec![stats_with(d1::DETECTOR, None)], vec![]);
        partial.summarize_diagnostics = vec![Diagnostic {
            severity: "warning".to_string(),
            stage: "summarize".to_string(),
            message: "fixed point capped".to_string(),
        }];
        assert!(!partial.ran_completely(d1::DETECTOR), "partial summaries");
    }

    // -----------------------------------------------------------------------
    // Parallel detector execution must not change ORDER
    // -----------------------------------------------------------------------

    fn ordering_output(name: &str) -> DetectorOutput {
        DetectorOutput {
            findings: vec![],
            stats: DetectorStats::new(name, 0, 0),
            diagnostics: vec![Diagnostic {
                severity: "warning".to_string(),
                stage: "detect".to_string(),
                message: name.to_string(),
            }],
            d1_cohort_index: None,
        }
    }

    /// Sleeps, so it CANNOT complete before `ordering_fast` — this is the
    /// hand-stated precondition that makes the test discriminate.
    fn ordering_slow(_r: &Model, _c: &DetectorContext) -> Result<DetectorOutput, DetectorError> {
        std::thread::sleep(std::time::Duration::from_millis(200));
        Ok(ordering_output("slow"))
    }

    fn ordering_fast(_r: &Model, _c: &DetectorContext) -> Result<DetectorOutput, DetectorError> {
        Ok(ordering_output("fast"))
    }

    /// Reads the facts the way d47/d49/d51 do.
    fn ordering_facts_reader(
        _r: &Model,
        c: &DetectorContext,
    ) -> Result<DetectorOutput, DetectorError> {
        c.get_ordering_facts();
        Ok(ordering_output("read"))
    }

    /// A detector that READS the ordering facts without declaring
    /// `substrate::ORDERING_FACTS` fails loudly, in every build: at mask 0 the
    /// shim fills no ordering slot, `get_ordering_facts` panics with a text
    /// naming the bit, and `run_each` turns that into the diagnostic.
    ///
    /// DISCRIMINATION PROOF (#160 Task 21): replacing the `expect` in
    /// `DetectorContext::get_ordering_facts` with a fall-back to an empty map
    /// makes this FAIL (the reader returns "read"); restoring it passes.
    #[test]
    fn undeclared_ordering_facts_read_inside_the_loop_fails_loudly() {
        let resolved = empty_resolved();
        let owned = build_detector_context(&resolved, 0);
        let ctx = owned.ctx();
        let detectors = vec![Detector {
            name: "reader".to_string(),
            run: ordering_facts_reader,
            requires: 0,
            rule_name: "Reader",
            short_description: "test reader",
            concern: Concern::Behaviour,
        }];

        let (_findings, diagnostics, _stats, _idx) = run_each(&resolved, &ctx, &detectors);

        assert!(
            diagnostics
                .iter()
                .any(|d| d.message.contains("substrate::ORDERING_FACTS")),
            "{diagnostics:?}"
        );
    }

    /// A workspace with one table field written by a routine that has a rule
    /// before the write (intent-evidence step 6): enough for a detector that
    /// reads the rules index, or a routine's rules, to reach the read.
    const ONE_WRITER: &[(&str, &str)] = &[
        (
            "src/T.al",
            "table 50990 \"Needs T\" { fields { field(1; \"No.\"; Code[20]) { } \
             field(2; Status; Option) { OptionMembers = Open,Released; } \
             field(3; Amount; Decimal) { } } }",
        ),
        (
            "src/C.al",
            "codeunit 50991 \"Needs C\" { procedure SetAmount(var T: Record \"Needs T\") \
             begin T.TestField(Status, T.Status::Open); T.Amount := 1; T.Modify(); end; }",
        ),
    ];

    /// The [`ONE_WRITER`] model built with `needs`.
    fn one_writer(needs: crate::program::model::workspace::AssemblyNeeds) -> Model {
        let files: Vec<(String, String)> = ONE_WRITER
            .iter()
            .map(|(p, s)| (p.to_string(), s.to_string()))
            .collect();
        crate::program::model::program_calls::assemble_and_resolve_inline_program_with(
            &files,
            "99999999-0000-0000-0000-0000000ee651",
            needs,
        )
    }

    /// Pins the REAL declarations, not just the mechanism: every registered
    /// detector runs ALONE through `run_each`, so no other detector's `requires`
    /// can trigger the fill on its behalf. A detector that reads a substrate
    /// slot without declaring its bit finds the slot empty, panics in the
    /// accessor and shows up as a "threw" diagnostic naming the bit:
    ///
    /// - `substrate::ORDERING_FACTS` (`get_ordering_facts`): d47/d49/d51 read
    ///   the facts on their first line;
    /// - `substrate::RULES` (`get_rules_index`): d65 reads the index on its
    ///   first line;
    /// - `ViewNeeds::rules` (`ModelRoutine::rules`): a detector reading a
    ///   routine's rules directly. Each detector's model is built with what its
    ///   `requires` asks for (`assembly_needs`), so one that reads rules without
    ///   declaring `RULES` meets a model without them.
    ///
    /// The model has one writer with a rule ([`ONE_WRITER`]), so a per-routine
    /// read is reached (with zero routines, rules are never read).
    ///
    /// DISCRIMINATION PROOFS: (2026-10-04) setting d47's `requires` back to
    /// `0` in `detectors/mod.rs` makes this FAIL naming d47; restoring it
    /// passes. (#160 Task 21: skipping `fill_ordering` in the shim makes it fail
    /// naming d47/d49/d51.) (2026-10-30, intent-evidence step 6) setting d65's
    /// `requires` to `0` makes this FAIL naming d65 (the RULES needle);
    /// restoring it passes.
    #[cfg(debug_assertions)]
    #[test]
    fn every_detector_alone_declares_what_it_reads() {
        let with_rules = one_writer(crate::program::model::workspace::AssemblyNeeds::RULES);
        let without = one_writer(crate::program::model::workspace::AssemblyNeeds::BASE);
        assert!(
            with_rules
                .workspace
                .routines
                .iter()
                .any(|r| !r.rules().is_empty()),
            "precondition: a routine with a rule"
        );
        let needles = [
            "substrate::ORDERING_FACTS",
            "substrate::RULES",
            "ViewNeeds::rules",
        ];
        let mut offenders = Vec::new();
        for detector in crate::engine::l5::detectors::registered_detectors() {
            let resolved = if assembly_needs(std::slice::from_ref(&detector)).rules {
                &with_rules
            } else {
                &without
            };
            let owned = build_detector_context(resolved, detector.requires);
            let ctx = owned.ctx();
            let name = detector.name.clone();
            let (_f, diagnostics, _s, _i) = run_each(resolved, &ctx, &[detector]);
            for needle in needles {
                if diagnostics.iter().any(|d| d.message.contains(needle)) {
                    offenders.push(format!("{name} ({needle})"));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "undeclared substrate / view-need readers: {offenders:?}"
        );
    }

    /// Filling `RULES` from a model built without rules panics (the
    /// `declared-needs` skill, checklist step 6): "not built" never reads as
    /// "no rules".
    #[test]
    #[should_panic(expected = "rules not built: declare ViewNeeds::rules")]
    fn filling_rules_from_a_model_without_rules_panics() {
        let without = one_writer(crate::program::model::workspace::AssemblyNeeds::BASE);
        let _ = build_detector_context(&without, substrate::RULES);
    }

    /// `run_each` runs detectors in PARALLEL, so completion order and iteration
    /// order are different things. Everything it accumulates is order-bearing:
    /// `detector_stats` and `diagnostics` are serialized into the JSON envelope
    /// in this order, and `findings` order survives into the output through
    /// `scope_and_sort`, which is a STABLE sort (findings tying on
    /// detector + primaryLocationKey + rootCauseKey keep insertion order —
    /// `d55-event-publish-in-loop` is a live example of such ties).
    ///
    /// PRECONDITION, hand-stated rather than hoped for: the FIRST detector
    /// sleeps 200 ms and the second returns immediately, so completion order is
    /// necessarily the REVERSE of iteration order. A fold that used completion
    /// order would therefore produce `["fast", "slow"]`.
    ///
    /// DISCRIMINATION PROOF (recorded): replacing the indexed
    /// `par_iter().collect()` with an unordered accumulation, or swapping the
    /// zip for completion order, makes this FAIL with `["fast", "slow"]`;
    /// restoring it passes.
    ///
    /// The sleep's role, measured rather than assumed: with the fold broken AND
    /// the sleep deleted the test still FAILED on this machine, so the sleep is
    /// not what makes the defect detectable here — rayon happened to distribute
    /// two items in a different order anyway. It stays because that is
    /// SCHEDULING LUCK, not a guarantee: with two equally-fast detectors a
    /// completion-ordered fold could coincidentally match iteration order and the
    /// row would pass while broken. The sleep makes the reversal deterministic.
    #[test]
    fn detector_results_fold_in_detector_order_not_completion_order() {
        let resolved = empty_resolved();
        let owned = build_detector_context(&resolved, 0);
        let ctx = owned.ctx();
        let detectors = vec![
            Detector {
                name: "slow".to_string(),
                run: ordering_slow,
                requires: 0,
                rule_name: "Slow",
                short_description: "test slow",
                concern: Concern::Behaviour,
            },
            Detector {
                name: "fast".to_string(),
                run: ordering_fast,
                requires: 0,
                rule_name: "Fast",
                short_description: "test fast",
                concern: Concern::Behaviour,
            },
        ];

        let (_findings, diagnostics, stats, _idx) = run_each(&resolved, &ctx, &detectors);

        assert_eq!(
            stats
                .iter()
                .map(|s| s.detector.as_str())
                .collect::<Vec<_>>(),
            ["slow", "fast"],
            "detector_stats must follow detector order, not completion order"
        );
        assert_eq!(
            diagnostics
                .iter()
                .map(|d| d.message.as_str())
                .collect::<Vec<_>>(),
            ["slow", "fast"],
            "diagnostics must follow detector order, not completion order"
        );
    }

    use crate::engine::l5::finding::{Finding, FindingConfidence, SourceAnchor};
    use crate::engine::l5::test_support::empty_resolved;
    use std::cmp::Ordering;

    fn test_finding(id: &str) -> Finding {
        Finding {
            id: id.to_string(),
            root_cause_key: id.to_string(),
            detector: "test-ok-detector".to_string(),
            title: "test finding".into(),
            root_cause: "test root cause".to_string(),
            severity: "info".to_string(),
            confidence: FindingConfidence {
                level: "likely".to_string(),
                capped_by: None,
                evidence: vec![],
            },
            primary_location: SourceAnchor {
                source_unit_id: "u0".to_string(),
                start_line: 1,
                start_column: 1,
                end_line: 1,
                end_column: 1,
                enclosing_routine_id: "r0".to_string(),
                syntax_kind: "call".to_string(),
                normalized_text_hash: None,
                leading_context_hash: None,
                trailing_context_hash: None,
            },
            evidence_path: vec![],
            additional_paths: None,
            affected_objects: vec![],
            affected_tables: vec![],
            fix_options: vec![],
            provenance: vec![],
            actionable_anchor: None,
            fingerprint: None,
            event_kind: None,
            cross_extension_subscribers: None,
            cohort_contexts: None,
            loose_subject: None,
        }
    }

    fn ok_detector(
        _resolved: &Model,
        _ctx: &DetectorContext,
    ) -> Result<DetectorOutput, DetectorError> {
        Ok(DetectorOutput::no_diag(
            vec![test_finding("ok-detector-finding")],
            DetectorStats::new("test-ok-detector", 1, 1),
        ))
    }

    /// THE MISSING TEST (T2.3): a detector that returns `Err` — the abort-safe
    /// isolation path — degrades to the exact warning-diagnostic format while every
    /// other registered detector still runs to completion.
    fn err_detector(
        _resolved: &Model,
        _ctx: &DetectorContext,
    ) -> Result<DetectorOutput, DetectorError> {
        Err(DetectorError::new("boom"))
    }

    /// A detector that panics despite the `Result` contract. Only reachable via the
    /// debug-build-only `catch_unwind` backstop (see `run_each`) — this test runs
    /// under `cargo test`, which unwinds (unlike the shipped `panic = "abort"`
    /// release profile), so it exercises that backstop specifically.
    fn panic_detector(
        _resolved: &Model,
        _ctx: &DetectorContext,
    ) -> Result<DetectorOutput, DetectorError> {
        panic!("boom-panic");
    }

    #[test]
    fn err_returning_detector_degrades_to_warning_others_still_run() {
        let resolved = empty_resolved();
        let detectors = vec![
            Detector {
                name: "d-ok".to_string(),
                run: ok_detector,
                requires: substrate::ALL,
                rule_name: "Ok",
                short_description: "test ok",
                concern: Concern::Behaviour,
            },
            Detector {
                name: "d-err".to_string(),
                run: err_detector,
                requires: substrate::ALL,
                rule_name: "Err",
                short_description: "test err",
                concern: Concern::Behaviour,
            },
        ];
        let out = run_detectors(&resolved, &detectors);

        assert_eq!(
            out.findings.len(),
            1,
            "the ok detector's finding must still appear"
        );
        assert_eq!(out.findings[0].id, "ok-detector-finding");

        assert_eq!(out.diagnostics.len(), 1);
        assert_eq!(out.diagnostics[0].severity, "warning");
        assert_eq!(out.diagnostics[0].stage, "detect");
        assert_eq!(
            out.diagnostics[0].message, "Detector \"d-err\" threw: boom",
            "exact message format is relied on by consumers (may be golden-pinned)"
        );

        assert_eq!(
            out.detector_stats.len(),
            1,
            "the failing detector never reaches the stats-push line"
        );
    }

    #[test]
    fn panicking_detector_degrades_to_warning_others_still_run() {
        let resolved = empty_resolved();
        let detectors = vec![
            Detector {
                name: "d-ok".to_string(),
                run: ok_detector,
                requires: substrate::ALL,
                rule_name: "Ok",
                short_description: "test ok",
                concern: Concern::Behaviour,
            },
            Detector {
                name: "d-panic".to_string(),
                run: panic_detector,
                requires: substrate::ALL,
                rule_name: "Panic",
                short_description: "test panic",
                concern: Concern::Behaviour,
            },
        ];
        let out = run_detectors(&resolved, &detectors);

        assert_eq!(
            out.findings.len(),
            1,
            "the ok detector's finding must still appear"
        );
        assert_eq!(out.findings[0].id, "ok-detector-finding");

        assert_eq!(out.diagnostics.len(), 1);
        assert_eq!(out.diagnostics[0].severity, "warning");
        assert_eq!(out.diagnostics[0].stage, "detect");
        assert_eq!(
            out.diagnostics[0].message, "Detector \"d-panic\" threw: boom-panic",
            "a caught panic and a returned Err must produce the IDENTICAL message shape"
        );
    }

    /// #220 review u-1: the SINGLE-APP arm of `finish_run` roles its routines by
    /// THE role rule. Hand-stated: a single-app model whose one routine (`r0`,
    /// the finding's anchor) carries an app guid other than the primary's. The
    /// finding is dropped; with the row back on the primary app it is kept.
    /// Break: the arm's constant `"primary"` -> the finding is kept, fails.
    #[test]
    fn single_app_scope_uses_role() {
        let run_with_row_app = |app: &str| {
            let mut m = empty_resolved();
            m.primary_app = Some(crate::engine::gate::app_attribution::App {
                app_guid: "app".to_string(),
                publisher: "probe".to_string(),
                name: "Ws".to_string(),
                version: "1.0.0.0".to_string(),
            });
            let mut r = crate::engine::l5::test_support::routine("r0", "procedure");
            r.app_guid = app.to_string();
            m.workspace.routines.push(r);
            let detectors = vec![Detector {
                name: "d-ok".to_string(),
                run: ok_detector,
                requires: substrate::ALL,
                rule_name: "Ok",
                short_description: "test ok",
                concern: Concern::Behaviour,
            }];
            run_detectors(&m, &detectors).findings.len()
        };
        assert_eq!(run_with_row_app("APP"), 1, "precondition: a primary row");
        assert_eq!(run_with_row_app("dep"), 0, "a dependency row's finding");
    }

    #[test]
    fn natural_orders_detectors_numerically() {
        assert_eq!(compare_natural("d2", "d10"), Ordering::Less);
        assert_eq!(compare_natural("d10", "d2"), Ordering::Greater);
        assert_eq!(compare_natural("d4", "d4"), Ordering::Equal);
        assert_eq!(
            compare_natural("d4-repeated-lookup-in-loop", "d4-repeated-lookup-in-loop"),
            Ordering::Equal
        );
    }

    #[test]
    fn natural_numeric_token_before_alpha_token() {
        // "1a" vs "a1": first tokens are "1" (num) and "a" (non-num) → num first.
        assert_eq!(compare_natural("1a", "a1"), Ordering::Less);
    }

    #[test]
    fn natural_shorter_prefix_first() {
        assert_eq!(compare_natural("d4", "d4x"), Ordering::Less);
    }

    /// The names under test: real detector shapes plus the corners of
    /// `compare_natural` (numeric-equal spellings, a numeric first token,
    /// prefixes, the empty name).
    const RANK_NAMES: [&str; 14] = [
        "d10",
        "d2",
        "d02",
        "d1",
        "d01",
        "d1a",
        "d4x",
        "d4",
        "1a",
        "a1",
        "d",
        "",
        "d4-repeated-lookup-in-loop",
        "d55-event-publish-in-loop",
    ];

    /// TLS-27: the rank is an EXACT monotone image of `compare_natural`: for
    /// every pair, comparing ranks gives what `compare_natural` gives, Equal
    /// included (`d1`/`d01` and `d2`/`d02` share a rank).
    ///
    /// DISCRIMINATION (2026-11-12, redone after review): giving each distinct
    /// name its own rank (the sorted position, no Equal sharing) fails on
    /// `"d2"` vs `"d02"` (not Equal), and
    /// `decorated_sort_equals_the_old_comparator` fails with it; restored,
    /// both pass.
    #[test]
    fn natural_ranks_are_an_exact_monotone_image_of_compare_natural() {
        let ranks = natural_ranks(RANK_NAMES.iter().copied());
        assert_eq!(
            compare_natural("d01", "d1"),
            Ordering::Equal,
            "precondition"
        );
        for a in RANK_NAMES {
            for b in RANK_NAMES {
                assert_eq!(
                    ranks[a].cmp(&ranks[b]),
                    compare_natural(a, b),
                    "{a:?} vs {b:?}"
                );
            }
        }
    }

    fn located(id: &str, detector: &str, unit: &str, line: u32, cause: &str) -> Finding {
        let mut f = test_finding(id);
        f.detector = detector.to_string();
        f.primary_location.source_unit_id = unit.to_string();
        f.primary_location.start_line = line;
        f.root_cause_key = cause.to_string();
        f
    }

    /// TLS-106 (review, sol P1): one predicate filters and labels on each
    /// path. Cross-app (`in_primary_scope`): a finding located in dependency
    /// source with no workspace anchor is dropped by the primary tail (and
    /// remembered in `scoped_out`) even when no role map marks it, and
    /// labelled dependency when kept. Single-app (`has_primary_role`, review
    /// r2): the role rule alone, as before TLS-106, so it keeps that finding
    /// and labels it primary.
    ///
    /// Mutation (2026-11-20): `scope_and_sort`'s filter made
    /// `has_primary_role(&f, role_by_routine)` -> the `dep:` finding is kept
    /// at the cross-app primary tail (red); restored, green.
    #[test]
    fn scope_and_sort_filters_and_labels_by_one_predicate() {
        let empty = std::collections::HashMap::new();
        let input = || {
            vec![
                located("ws", "d1", "ws:src/A.al", 1, "k"),
                located("dep", "d1", "dep:abcd:src/B.al", 1, "k"),
            ]
        };
        let single = scope_and_sort(input(), &empty, true, has_primary_role);
        assert_eq!(single.primary, [true, true]);
        assert!(single.scoped_out.is_empty());
        let primary = scope_and_sort(input(), &empty, true, in_primary_scope);
        let ids: Vec<&str> = primary.findings.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, ["ws"]);
        assert_eq!(primary.primary, [true]);
        assert_eq!(primary.scoped_out.len(), 1);
        let all = scope_and_sort(input(), &empty, false, in_primary_scope);
        let labelled: Vec<(&str, bool)> = all
            .findings
            .iter()
            .map(|f| f.id.as_str())
            .zip(all.primary.iter().copied())
            .collect();
        assert_eq!(labelled, [("dep", false), ("ws", true)]);
        assert!(all.scoped_out.is_empty());
    }

    /// TLS-27, the production call site: `scope_and_sort` orders by the
    /// detector's natural order, then the location key AS TEXT, then the
    /// root-cause key, and keeps input order on a full tie. The expected
    /// order is stated by hand:
    /// - `d02` ties `d2`, so its finding at unit `c` sorts between the `d2`
    ///   findings at `a`/`b` and the one at `d`. Ranked apart, it would come
    ///   before or after all of them (which one depends on hash order, so
    ///   both sides are pinned). `d10` comes after both;
    /// - `"a1:1:1"` before `"a:1:1"`, because `'1' < ':'` (a field-wise
    ///   `(unit, line, column)` order would put `a` first);
    /// - `k1` before `k2` at the same location; `t1` before `t2` on a full
    ///   tie (input order; with six items this alone does not catch an
    ///   unstable sort, which `decorated_sort_equals_the_old_comparator`
    ///   does).
    ///
    /// DISCRIMINATION (2026-11-12, after review): (a) a field-wise location
    /// key (`"{unit}\0{line:010}\0{column:010}"`) fails (`a` before `a1`);
    /// (b) ranking `d02` apart from `d2` (no Equal sharing in
    /// `natural_ranks`) fails 3 runs of 3 (`c` first, or after `d`, as the
    /// hash order fell); restored, passes.
    #[test]
    fn findings_sort_by_natural_detector_then_location_text_then_cause() {
        let input = vec![
            located("late", "d10", "a", 1, "k"),
            located("a-k2", "d2", "a", 1, "k2"),
            located("t1", "d2", "b", 1, "k"),
            located("c", "d02", "c", 1, "k"),
            located("a1", "d2", "a1", 1, "k"),
            located("a-k1", "d2", "a", 1, "k1"),
            located("d", "d2", "d", 1, "k"),
            located("t2", "d2", "b", 1, "k"),
        ];
        let ids: Vec<String> = scope_and_sort(
            input,
            &std::collections::HashMap::new(),
            true,
            has_primary_role,
        )
        .findings
        .into_iter()
        .map(|f| f.id)
        .collect();
        assert_eq!(ids, ["a1", "a-k1", "a-k2", "t1", "t2", "c", "d", "late"]);
    }

    /// TLS-27: the decorated sort gives the order the old comparator gave
    /// (`compare_natural` on the detector, then the location key's string,
    /// then the root-cause key, a stable `sort_by` over those three), on
    /// 2,000 items with many full ties, so input order decides among them.
    ///
    /// DISCRIMINATION (2026-11-12, redone after review): an unstable sort
    /// (`sort_unstable_by_key` in place of `sort_by_cached_key`) in
    /// `sort_by_detector_location_cause` fails (tied items reordered);
    /// restored, passes.
    #[test]
    fn decorated_sort_equals_the_old_comparator() {
        // (detector, location key, root-cause key, input index)
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n) as usize
        };
        let units = ["a", "a1", "b", "u0", "u10", "u2"];
        let items: Vec<(&str, String, String, usize)> = (0..2000)
            .map(|i| {
                let d = RANK_NAMES[next(RANK_NAMES.len() as u64)];
                let loc = format!("{}:{}:{}", units[next(6)], next(12), next(3));
                (d, loc, format!("k{}", next(3)), i)
            })
            .collect();
        let mut old = items.clone();
        old.sort_by(|a, b| {
            compare_natural(a.0, b.0)
                .then_with(|| a.1.cmp(&b.1))
                .then_with(|| a.2.cmp(&b.2))
        });
        let mut new = items;
        sort_by_detector_location_cause(&mut new, |t| t.0, |t| t.1.clone(), |t| &t.2);
        let ties = old
            .windows(2)
            .filter(|w| (w[0].0, &w[0].1, &w[0].2) == (w[1].0, &w[1].1, &w[1].2))
            .count();
        assert!(ties > 50, "precondition: many full ties ({ties})");
        assert_eq!(new, old);
    }
}
