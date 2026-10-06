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

use crate::engine::l3::l3_workspace::L3Resolved;
use crate::engine::l5::detector_context::{
    DetectorContext, build_detector_context, build_detector_context_cross_app,
};
use crate::engine::l5::finding::{D1CohortIndex, Finding};
use crate::engine::perf_trace as pt;
use rayon::prelude::*;

/// Substrate demand bits (W1.0 demand-driven detector substrate).
///
/// `build_detector_context` ALWAYS builds the cheap, many-consumer CORE surface —
/// symbol table, `resolve_calls`, event graph, combined graph, reverse graph, entry
/// points, reachable roots, all borrowed indexes (routine/object/table/call-site
/// maps), `resolved_call_edge_by_callsite`, `uncertainty_edges_by_from`,
/// `upgraded_bindings_by_callsite`, `event_flow_indexes`, `cross_extension_subscribers`
/// (T3), `fingerprint_index` (T1), and `root_classifications_by_routine`. The four
/// EXPENSIVE substrates below are built only when some selected detector demands them;
/// `run_detectors` folds every detector's `requires` into the union it passes here.
/// A fifth bit, [`substrate::RAW_INHERITED_FACTS`], is NOT a substrate a detector may
/// demand — it is the policy-only raw-cone escape hatch (see its own doc).
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
    /// The second Tarjan SCC + the closed-form v2 CORE summaries →
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
    /// ⟨C1 Task 3⟩ Materialize the RAW per-routine `capability_facts_inherited`
    /// `Vec<CapabilityFact>` alongside the compact derived cone substrate — i.e.
    /// compose the cone under `ConeOutput::Both` instead of `DerivedOnly`.
    ///
    /// **POLICY-ONLY, and deliberately NOT part of [`ALL`].** This is the ~10.9 GB
    /// (8020 corpus, ~100k routines × their full reachable cone) allocation the C1
    /// arc exists to stop paying: every analyze-path consumer reads a derived
    /// predicate off `ctx.cone_derived` instead. The ONE consumer that genuinely
    /// needs the fact objects themselves is `gate::policy`'s `select_facts`
    /// (rule `facts: inherited | any` iterates real `CapabilityFact`s and matches
    /// their fields), so `gate/policy/pipeline.rs` passes
    /// `substrate::ALL | substrate::RAW_INHERITED_FACTS` explicitly.
    ///
    /// A summary built WITHOUT this bit carries `capability_facts_inherited:
    /// None`; `FullRoutineSummary::inherited_raw` then panics rather than
    /// silently serving a direct-only view (R6).
    pub const RAW_INHERITED_FACTS: u32 = 1 << 4;
    /// ⟨Task 6⟩ The [`crate::engine::l4::reverse_index::ReverseEffectIndex`]
    /// transpose over `db_effect_bundle`, exposed as `ctx.reverse_effect_index`.
    ///
    /// **QUERY-ONLY, deliberately NOT part of [`ALL`], and — unlike every other
    /// bit here — NOT a substrate a detector may declare in `requires`.** Same
    /// contract as [`RAW_INHERITED_FACTS`], for a DIFFERENT and less obvious
    /// reason, so read this before copying the pattern:
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
    /// FIRST; do not add the bit to `ALL` (that would charge the four
    /// `ALL`-passing non-registry callers — `gate/events.rs` ×2,
    /// `l5/digest_cli.rs`, `l5/prove.rs`, plus `gate/policy/pipeline.rs` — for a
    /// transpose none of them reads).
    ///
    /// Today's consumer is `alsem query`, which owns its own pipeline and never
    /// builds a `DetectorContext` at all — so this bit costs the analyze path
    /// nothing by construction. It exists for the eventual detector / LSP-hover
    /// path. Implies [`CORE_SUMMARIES`] (there is no bundle to transpose
    /// without it) — set both.
    pub const DB_EFFECT_REVERSE_INDEX: u32 = 1 << 5;
    /// ⟨2026-10-04⟩ The L4.5 ordering facts (`ctx.get_ordering_facts()`, read by
    /// d47/d49/d51). Unlike every other bit, `build_detector_context` ignores it:
    /// the facts stay lazy there, so the non-registry callers keep paying nothing.
    /// `run_each` reads it instead and computes the facts ONCE, BEFORE its parallel
    /// detector loop, when any selected detector declares it.
    ///
    /// Computing them lazily INSIDE that loop deadlocked (`r4_differential` hung
    /// for 10 minutes at 0 % CPU; a dump confirmed it). The thread filling the
    /// `OnceLock` runs a rayon `par_iter`; while it waits for its halves, rayon
    /// makes it steal queued jobs — including a half of the OUTER detector
    /// loop. That half's other half sits on another thread running d49, which
    /// blocks on the same `OnceLock`. Neither can finish. Declaring the bit is
    /// what keeps a detector out of that cycle; `get_ordering_facts` refuses
    /// (in debug builds) to fill the slot from inside the loop. That guard sees a
    /// missing declaration only when no OTHER selected detector declares the bit,
    /// so the registry test `every_detector_alone_declares_what_it_reads` runs
    /// each registered detector by itself.
    ///
    /// Not in [`ALL`]: it builds nothing in the context, and the substrate-parity
    /// test calls detectors directly, where the lazy path is safe.
    pub const ORDERING_FACTS: u32 = 1 << 6;
    /// Every substrate — the eager, pre-W1.0 behavior. Full/preset/all-detector runs
    /// and every non-registry `build_detector_context` caller pass this.
    ///
    /// ⟨C1 Task 3 — R2⟩ This is an EXPLICIT OR list, not "all bits set", and
    /// [`RAW_INHERITED_FACTS`] is deliberately **excluded** from it. `ALL` is
    /// passed verbatim by every non-registry caller (`gate/events.rs`,
    /// `l5/digest_cli.rs`, `l5/prove.rs`, `gate/policy/pipeline.rs`); folding the
    /// raw-cone bit in here would re-materialize the whole 10.9 GB on all of
    /// them. Anything that needs the raw facts ORs the bit in at its own call
    /// site.
    ///
    /// ⟨Task 6⟩ [`DB_EFFECT_REVERSE_INDEX`] is excluded for the same reason plus
    /// one more (the detector-parity trap) — see its own doc.
    pub const ALL: u32 = SUMMARIES | CORE_SUMMARIES | TRANSACTION_SPANS | CLOSED_WORLD_TEMP;
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
        }
    }

    /// Add `n` to a skip counter, inserting it if absent. Only inserts when `n > 0`.
    pub fn add_skip(&mut self, key: &str, n: u64) {
        if n > 0 {
            *self.skipped.entry(key.to_string()).or_insert(0) += n;
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
pub struct Detector {
    pub name: String,
    pub run: fn(&L3Resolved, &DetectorContext) -> Result<DetectorOutput, DetectorError>,
    /// The substrate bits (see `substrate`) this detector reads from the context.
    /// `run_detectors` folds every selected detector's `requires` into the union it
    /// hands to `build_detector_context`, so a substrate is built iff some selected
    /// detector demands it. Over-inclusive is SAFE (just less skipping);
    /// under-inclusive is caught by the full-vs-minimal parity test.
    pub requires: u32,
}

/// The combined output of `run_detectors`.
pub struct RunOutput {
    pub findings: Vec<Finding>,
    pub diagnostics: Vec<Diagnostic>,
    pub detector_stats: Vec<DetectorStats>,
    /// The L4 "summarizeDiagnostics" source (TS-order slot 3 — see
    /// `gate/run.rs`'s `compute_analyzer_diagnostics` doc) — presently just the
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
pub fn run_detectors(resolved: &L3Resolved, detectors: &[Detector]) -> RunOutput {
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
    let ctx = {
        let _s = pt::span("context", "context.build_total");
        build_detector_context(resolved, demanded)
    };
    let summarize_diagnostics: Vec<Diagnostic> = ctx
        .summarize_diagnostics
        .iter()
        .map(from_summarize_diagnostic)
        .collect();
    let (findings, diagnostics, detector_stats, d1_cohort_index) =
        run_each(resolved, &ctx, detectors);

    // The context dies here either way — it is a local of this function and
    // borrows nothing that outlives it. Dropping it explicitly, inside a span,
    // prices the teardown of the cones/summaries/spans substrate instead of
    // leaving it in this function's unattributed self time. No `Drop` impl is
    // involved (the engine's only two are `perf_trace`'s guards), so the
    // reorder is pure deallocation and not observable.
    {
        let _s = pt::span("context", "context.ctx_drop");
        drop(ctx);
    }

    // Role-scoping filter (registry.ts:161-172). Source-only: every routine's role
    // is "primary" (no analysisRole), so the predicate keeps everything.
    let scoped = {
        let _s = pt::span("l4_l5", "l4_l5.role_scope_and_sort");
        let role_by_routine: std::collections::HashMap<&str, &str> = resolved
            .workspace
            .routines
            .iter()
            // analysisRole is not modeled on L3Routine (source-only ⇒ always primary).
            .map(|r| (r.id.as_str(), "primary"))
            .collect();
        role_scope_and_sort(findings, &role_by_routine)
    };

    RunOutput {
        findings: scoped,
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
) -> RunOutput {
    let ctx = build_detector_context_cross_app(base);
    let summarize_diagnostics: Vec<Diagnostic> = ctx
        .summarize_diagnostics
        .iter()
        .map(from_summarize_diagnostic)
        .collect();
    // The detectors close over `(resolved, ctx)`. Build a throwaway L3Resolved view
    // over the merged routines so the `resolved.workspace` arg is consistent with the
    // ctx (detectors read `resolved.workspace.routines`/`.objects` for the fingerprint
    // index + role map; for d13/d16/d17 those are the merged sets in `base`).
    let resolved = L3Resolved {
        workspace: merged_workspace_view(base),
        root_classifications: Vec::new(),
        primary_app: None,
        infra_diagnostics: Vec::new(),
        precomputed_calls: None,
        precomputed_events: None,
    };
    let (findings, diagnostics, detector_stats, d1_cohort_index) =
        run_each(&resolved, &ctx, detectors);

    // role_by_routine: dep routines → "dependency", else "primary".
    let role_by_routine: std::collections::HashMap<&str, &str> = base
        .ws_routines
        .iter()
        .map(|r| {
            let role = if base.dep_routine_ids.contains(&r.id) {
                "dependency"
            } else {
                "primary"
            };
            (r.id.as_str(), role)
        })
        .collect();
    let scoped = role_scope_and_sort(findings, &role_by_routine);

    RunOutput {
        findings: scoped,
        diagnostics,
        detector_stats,
        summarize_diagnostics,
        d1_cohort_index,
    }
}

/// Build an `L3Workspace` view over the merged base routines/objects/tables — the
/// `resolved.workspace` arg every detector receives. The cross-app detectors read
/// `routines` (role map + fingerprint index) and `objects` (fingerprint index);
/// the merged sets come straight from `base`.
fn merged_workspace_view(
    base: &crate::engine::l4::capability_cone::R3a5CrossAppBase,
) -> crate::engine::l3::l3_workspace::L3Workspace {
    crate::engine::l3::l3_workspace::L3Workspace {
        objects: base.objects.clone(),
        tables: base.tables.clone(),
        routines: base.ws_routines.clone(),
    }
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
    resolved: &L3Resolved,
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
    // `detector.run` is a plain `fn(&L3Resolved, &DetectorContext)` — both
    // arguments are immutable shared borrows and no detector writes through
    // them, so the run phase has no shared mutable state at all. What DOES have
    // to stay ordered is the accumulation below: `findings` order survives into
    // the output through `role_scope_and_sort`, which is a STABLE sort, so two
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

    // Fill every lazy substrate BEFORE the parallel loop — see
    // `substrate::ORDERING_FACTS` for the deadlock this prevents. On the same
    // big-stack pool the detectors use: the fill recurses over statement trees
    // (`compute_return_summaries`), and it ran on a big-stack worker before this
    // moved it out of the loop. No detector job exists yet, so a waiting thread
    // in here has nothing from the loop to steal.
    if detectors
        .iter()
        .any(|d| d.requires & substrate::ORDERING_FACTS != 0)
    {
        let _s = pt::span("context", "context.ordering_facts");
        pool.install(|| {
            ctx.get_ordering_facts();
        });
    }

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

/// Apply the role-scope filter (drop dep-anchored findings) then the stable sort.
fn role_scope_and_sort(
    findings: Vec<Finding>,
    role_by_routine: &std::collections::HashMap<&str, &str>,
) -> Vec<Finding> {
    let mut scoped: Vec<Finding> = findings
        .into_iter()
        .filter(|f| {
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
        })
        .collect();

    scoped.sort_by(|a, b| {
        compare_natural(&a.detector, &b.detector)
            .then_with(|| primary_location_key(a).cmp(&primary_location_key(b)))
            .then_with(|| a.root_cause_key.cmp(&b.root_cause_key))
    });
    scoped
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn ordering_slow(
        _r: &L3Resolved,
        _c: &DetectorContext,
    ) -> Result<DetectorOutput, DetectorError> {
        std::thread::sleep(std::time::Duration::from_millis(200));
        Ok(ordering_output("slow"))
    }

    fn ordering_fast(
        _r: &L3Resolved,
        _c: &DetectorContext,
    ) -> Result<DetectorOutput, DetectorError> {
        Ok(ordering_output("fast"))
    }

    /// Stands in for d47/d49/d51: reports, from INSIDE the parallel loop, whether
    /// the ordering facts were already filled when it ran.
    fn ordering_facts_probe(
        _r: &L3Resolved,
        c: &DetectorContext,
    ) -> Result<DetectorOutput, DetectorError> {
        let state = if c.ordering_facts.get().is_some() {
            "filled"
        } else {
            "empty"
        };
        Ok(ordering_output(state))
    }

    /// Reads the facts the way d47/d49/d51 do. Only the debug-only guard test
    /// uses it, so it is debug-only too (CI lints in release).
    #[cfg(debug_assertions)]
    fn ordering_facts_reader(
        _r: &L3Resolved,
        c: &DetectorContext,
    ) -> Result<DetectorOutput, DetectorError> {
        c.get_ordering_facts();
        Ok(ordering_output("read"))
    }

    /// A detector declaring `substrate::ORDERING_FACTS` must find the facts
    /// ALREADY filled when it runs: filling them inside the parallel loop is
    /// what deadlocked `r4_differential` (see the bit's doc).
    ///
    /// PRECONDITION, hand-stated: the context is built with the slot EMPTY, and
    /// the probe only LOOKS at the slot (it never fills it), so "filled" can only
    /// come from `run_each` itself.
    ///
    /// DISCRIMINATION PROOF (recorded 2026-10-04): deleting the
    /// `ctx.get_ordering_facts()` call before `pool.install` in `run_each` makes
    /// this FAIL with `["empty"]`; restoring it passes.
    #[test]
    fn declared_ordering_facts_are_filled_before_the_parallel_loop() {
        let resolved = empty_resolved();
        let ctx = build_detector_context(&resolved, 0);
        assert!(
            ctx.ordering_facts.get().is_none(),
            "precondition: slot empty"
        );
        let detectors = vec![Detector {
            name: "probe".to_string(),
            run: ordering_facts_probe,
            requires: substrate::ORDERING_FACTS,
        }];

        let (_findings, diagnostics, _stats, _idx) = run_each(&resolved, &ctx, &detectors);

        assert_eq!(
            diagnostics
                .iter()
                .map(|d| d.message.as_str())
                .collect::<Vec<_>>(),
            ["filled"]
        );
    }

    /// The other half of the contract: a detector that READS the facts without
    /// declaring the bit fails loudly (debug builds) instead of deadlocking
    /// sometimes. Without the guard this run would succeed here (one detector
    /// cannot deadlock alone) and the missing declaration would only show up
    /// as a rare hang in a full parallel run.
    ///
    /// DISCRIMINATION PROOF (recorded 2026-10-04): deleting the `debug_assert!`
    /// in `DetectorContext::get_ordering_facts` makes this FAIL (the reader
    /// returns "read"); restoring it passes.
    #[cfg(debug_assertions)]
    #[test]
    fn undeclared_ordering_facts_read_inside_the_loop_fails_loudly() {
        let resolved = empty_resolved();
        let ctx = build_detector_context(&resolved, 0);
        let detectors = vec![Detector {
            name: "reader".to_string(),
            run: ordering_facts_reader,
            requires: 0,
        }];

        let (_findings, diagnostics, _stats, _idx) = run_each(&resolved, &ctx, &detectors);

        assert!(
            diagnostics
                .iter()
                .any(|d| d.message.contains("substrate::ORDERING_FACTS")),
            "{diagnostics:?}"
        );
    }

    /// Pins the REAL declarations, not just the mechanism: every registered
    /// detector runs ALONE through `run_each`, so no other detector's `requires`
    /// can trigger the pre-fill on its behalf. A detector that reads the ordering
    /// facts without declaring `substrate::ORDERING_FACTS` trips the
    /// `get_ordering_facts` guard and shows up as a "threw" diagnostic. d47/d49/d51
    /// read the facts on their first line, so an empty workspace is enough to
    /// reach the read.
    ///
    /// DISCRIMINATION PROOF (recorded 2026-10-04): setting d47's `requires` back
    /// to `0` in `detectors/mod.rs` makes this FAIL naming d47; restoring it
    /// passes.
    #[cfg(debug_assertions)]
    #[test]
    fn every_detector_alone_declares_what_it_reads() {
        let resolved = empty_resolved();
        let mut offenders = Vec::new();
        for detector in crate::engine::l5::detectors::registered_detectors() {
            let ctx = build_detector_context(&resolved, detector.requires);
            let name = detector.name.clone();
            let (_f, diagnostics, _s, _i) = run_each(&resolved, &ctx, &[detector]);
            if diagnostics
                .iter()
                .any(|d| d.message.contains("substrate::ORDERING_FACTS"))
            {
                offenders.push(name);
            }
        }
        assert!(
            offenders.is_empty(),
            "undeclared ordering-facts readers: {offenders:?}"
        );
    }

    /// `run_each` runs detectors in PARALLEL, so completion order and iteration
    /// order are different things. Everything it accumulates is order-bearing:
    /// `detector_stats` and `diagnostics` are serialized into the JSON envelope
    /// in this order, and `findings` order survives into the output through
    /// `role_scope_and_sort`, which is a STABLE sort (findings tying on
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
        let ctx = build_detector_context(&resolved, 0);
        let detectors = vec![
            Detector {
                name: "slow".to_string(),
                run: ordering_slow,
                requires: 0,
            },
            Detector {
                name: "fast".to_string(),
                run: ordering_fast,
                requires: 0,
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

    use crate::engine::l3::l3_workspace::L3Workspace;
    use crate::engine::l5::finding::{Finding, FindingConfidence, SourceAnchor};
    use std::cmp::Ordering;

    fn empty_resolved() -> L3Resolved {
        L3Resolved {
            workspace: L3Workspace {
                objects: vec![],
                tables: vec![],
                routines: vec![],
            },
            root_classifications: vec![],
            primary_app: None,
            infra_diagnostics: vec![],
            precomputed_calls: None,
            precomputed_events: None,
        }
    }

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
        }
    }

    fn ok_detector(
        _resolved: &L3Resolved,
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
        _resolved: &L3Resolved,
        _ctx: &DetectorContext,
    ) -> Result<DetectorOutput, DetectorError> {
        Err(DetectorError::new("boom"))
    }

    /// A detector that panics despite the `Result` contract. Only reachable via the
    /// debug-build-only `catch_unwind` backstop (see `run_each`) — this test runs
    /// under `cargo test`, which unwinds (unlike the shipped `panic = "abort"`
    /// release profile), so it exercises that backstop specifically.
    fn panic_detector(
        _resolved: &L3Resolved,
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
            },
            Detector {
                name: "d-err".to_string(),
                run: err_detector,
                requires: substrate::ALL,
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
            },
            Detector {
                name: "d-panic".to_string(),
                run: panic_detector,
                requires: substrate::ALL,
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
}
