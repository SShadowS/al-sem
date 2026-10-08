//! Transaction spans — faithful port of al-sem
//! `src/engine/transaction-spans.ts`.
//!
//! `compute_transaction_spans` produces a `TransactionSpan` per Commit (and per
//! checked Codeunit.Run implicit commit). For each seed it walks callers BACKWARD
//! over the reverse call graph to find every routine that participates in the
//! transaction; the walk stops at any routine that itself commits (a prior span's
//! domain) or at `MAX_DEPTH`. The span aggregates writes/events (union over the
//! span via the capability-query helpers) and a `coverage_complete` flag.
//!
//! ## Role threading
//!
//! al-sem reads `roleOf(r)` to restrict seeding + aggregation to PRIMARY
//! routines. The Rust model has no `roleOf`; role is `is_dep =
//! dep_routine_ids.contains(&r.id)` (empty set ⇒ all primary). We thread the role
//! oracle as `&BTreeSet<String>` (see `entry_points` for the same convention).
//!
//! ## Summaries
//!
//! al-sem reads `routine.summary` (each routine carries its own). The Rust model
//! keeps facts/coverage SEPARATE, so the caller passes a
//! `&HashMap<RoutineId, FullRoutineSummary>` (internal id → summary). A routine
//! with NO entry behaves like al-sem's `r.summary === undefined`:
//! `coverage_complete ← false` and it contributes no tables/events. This mirrors
//! `transaction-spans.ts` lines 100-108 / 155-163 exactly.
//!
//! Determinism: `visited` is a `BTreeSet` (so the per-seed walk's collected set
//! is order-independent); `writes`/`publishes` are `BTreeSet`s; every output Vec
//! is sorted. The seed iteration order over routines follows the input slice
//! order (al-sem iterates `model.routines` for the §B pass and a `Map` keyed by
//! insertion order for the explicit-commit pass — both yield spans in a stable
//! order which downstream Task 2b sorts by the span's own key).

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use crate::engine::l4::cone_derived::{ConeDerivedStore, ResBitset};
use crate::engine::l4::param_guard::{FrameGuards, Req, across_edge, conjoin, frame_guards_over};
use crate::engine::l5::capability_query::reachable_coverage;
use crate::engine::l5::full_summary::FullRoutineSummary;
use crate::engine::l5::pending_writes::{
    ForwardSites, Pending, forward_sites, is_checked_run, pending_before, toward_sites,
};
use crate::engine::l5::reverse_call_graph::ReverseCallGraph;
use crate::program::body::features::PCallee;
use crate::program::model::workspace::ModelRoutine;

const MAX_DEPTH: usize = 50;

/// Distinguishes an explicit `Commit()` span from a synthetic checked-Run
/// implicit commit. Mirrors al-sem `seedKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedKind {
    ExplicitCommit,
    CheckedRunImplicit,
}

/// A transaction span (al-sem `TransactionSpan`). All id lists SORTED.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionSpan {
    pub seed_kind: SeedKind,
    /// The Commit operation that bounds the span (for checked-run-implicit, the
    /// checked callsite id — al-sem stores the callsite id here too).
    pub commit_operation_id: String,
    /// For checked-run-implicit seeds only: the callsite id of the checked
    /// Codeunit.Run forming the implicit-commit boundary. `None` for explicit.
    pub seed_callsite_id: Option<String>,
    /// The routine containing the bounding Commit.
    pub commit_routine_id: String,
    /// All routines reachable backward from `commit_routine_id` up to another
    /// commit or root. SORTED.
    pub routines_in_span: Vec<String>,
    /// Union of tables written by any routine in the span. SORTED + deduped.
    pub writes_tables: Vec<String>,
    /// Number of distinct PHYSICAL (non-known-temp) tables written by any routine
    /// in the span. ⟨issue 23 rule⟩ GATES read this; `writes_tables` (temp-
    /// inclusive) is the WITNESS set.
    pub writes_physical_tables_count: usize,
    /// Union of events published by any routine in the span. SORTED + deduped.
    pub publishes_events: Vec<String>,
    /// Span entry roots — routines in the span with no upstream caller. SORTED.
    pub span_roots: Vec<String>,
    /// True iff EVERY routine in `routines_in_span` has a defined summary AND
    /// `reachable_coverage(summary) == "complete"`.
    pub coverage_complete: bool,
    /// The physical tables written BEFORE the Commit (engine-switch S8 gap 3,
    /// [`super::pending_writes`]): what this transaction leaves half-written.
    /// SORTED. `writes_tables` above is everything the span can reach.
    pub pending_physical_tables: Vec<String>,
    /// The events published before the Commit. SORTED.
    pub pending_events: Vec<String>,
    /// Each span member other than the Commit routine -> how many physical
    /// tables it writes before its call toward the Commit (d8's "manager" test).
    pub pending_physical_by_routine: BTreeMap<String, usize>,
}

/// `roleOf(r) === "primary"` — true when NOT in the dependency universe.
fn is_primary(routine: &ModelRoutine, dep_routine_ids: &BTreeSet<String>) -> bool {
    !dep_routine_ids.contains(&routine.id)
}

/// Backward BFS over the reverse graph from `seed`, stopping at another
/// committing routine (other than the seed) and at `MAX_DEPTH`. Returns the
/// visited set (a `BTreeSet` for order-independence). Mirrors the inner while-loop
/// in both al-sem seed passes verbatim.
///
/// A caller that reaches `id` only through a CHECKED run (`if Codeunit.Run(...)`)
/// is not in its transaction: the run is its own (engine-switch S8 gap 3).
fn backward_cone(
    seed: &str,
    seed_reqs: &[Req],
    inputs: &SpanInputs<'_>,
    frames: &mut HashMap<String, Option<FrameGuards>>,
) -> BTreeSet<String> {
    // routine -> what its parameters must be for the Commit to run when it is
    // entered (engine-switch S8 gap 3, [`crate::engine::l4::param_guard`]). A
    // caller whose literal argument contradicts that cannot reach the Commit
    // through this call, so it is not in the span. A routine reached again with
    // different requirements keeps none (it may reach the Commit either way).
    let mut reached: HashMap<String, Vec<Req>> = HashMap::new();
    reached.insert(seed.to_string(), seed_reqs.to_vec());
    let mut queue: VecDeque<(String, usize)> = VecDeque::new();
    queue.push_back((seed.to_string(), 0));
    while let Some((id, depth)) = queue.pop_front() {
        if depth >= MAX_DEPTH {
            continue;
        }
        // Don't walk past another committing routine (prior span bounds the trace).
        if id != seed && inputs.commits_by_routine.contains_key(&id) {
            continue;
        }
        let reqs = reached[&id].clone();
        let Some(callers) = inputs.reverse.get(&id) else {
            continue;
        };
        for caller in callers {
            let caller_routine = inputs.routine_by_id.get(caller.from.as_str()).copied();
            let cs = caller.callsite_id.as_deref().and_then(|cs_id| {
                caller_routine.and_then(|r| r.call_sites.iter().find(|c| c.id == cs_id))
            });
            // A checked run is its own transaction: its caller is not in this one.
            if cs.is_some_and(|cs| {
                is_checked_run(cs, inputs.routine_by_id.get(id.as_str()).copied())
            }) {
                continue;
            }
            let frame = caller_routine.and_then(|r| {
                frames
                    .entry(r.id.clone())
                    .or_insert_with(|| {
                        let callees_at = |cs_id: &str| -> Vec<&str> {
                            inputs
                                .fwd
                                .get(r.id.as_str())
                                .map_or(&[][..], Vec::as_slice)
                                .iter()
                                .filter(|(site, _)| *site == Some(cs_id))
                                .map(|(_, to)| *to)
                                .collect()
                        };
                        frame_guards_over(r, callees_at, inputs.routine_by_id)
                    })
                    .as_ref()
            });
            let across = match cs {
                Some(cs) => across_edge(&reqs, cs, frame),
                None => Some(Vec::new()),
            };
            let edge_reqs: &[Req] = match (frame, caller.callsite_id.as_deref()) {
                (Some(f), Some(cs_id)) => f.by_site.get(cs_id).map_or(&[], Vec::as_slice),
                _ => &[],
            };
            let Some(new) = across.and_then(|a| conjoin(&a, edge_reqs)) else {
                continue; // a literal argument makes the Commit unreachable here
            };
            match reached.get_mut(&caller.from) {
                None => {
                    reached.insert(caller.from.clone(), new);
                    queue.push_back((caller.from.clone(), depth + 1));
                }
                Some(old) if old.is_empty() || *old == new => {}
                Some(old) => {
                    old.clear();
                    queue.push_back((caller.from.clone(), depth + 1));
                }
            }
        }
    }
    reached.into_keys().collect()
}

/// Aggregate the writes/events/coverage over a visited span. Mirrors al-sem
/// lines 97-109 / 152-164: a routine with no summary → `coverage_complete` false
/// and contributes nothing; otherwise union its written tables / published events
/// and AND-in its `reachable_coverage == "complete"`.
/// ⟨C1 Task 2⟩ The write / event id-sets come from the folded cone rows
/// (`cone_derived`); the `summaries` lookup stays because BOTH remaining
/// behaviours depend on it — the "no summary ⇒ not complete AND contributes
/// nothing" arm, and `reachable_coverage`, which reads `coverage` (untouched by
/// this arc).
///
/// **The union rides INTERNED IDS, and that is the whole point of this function's
/// shape.** It used to call `ConeDerivedStore::writes_tables_of` /
/// `publishes_events_of` per visited routine, each of which resolves that
/// routine's whole folded-cone window into a fresh `Vec<String>` whose every
/// element is then inserted into a `BTreeSet<String>` and dropped. Censused on BC
/// Base App 8020: **261,772,789 `String`s allocated and discarded per run**, 2,023
/// per visited routine — the measured cost of `context.transaction_spans`. Ids go
/// into a caller-owned [`ResBitset`] instead (one word-OR each, no allocation, and
/// dedupe for free because the interner is injective, so id-dedupe IS
/// string-dedupe), and the set is resolved to `String` ONCE per span template.
///
/// The output is unchanged: `resolve_sorted_ids` sorts by the resolved STRING,
/// which is the order the old `BTreeSet<String>` produced. Bitset order is intern
/// order and is NOT lexicographic — dropping that sort would silently reorder
/// every span's `writes_tables`.
fn aggregate_span(
    visited: &BTreeSet<String>,
    summaries: &HashMap<String, FullRoutineSummary>,
    cone_derived: &ConeDerivedStore,
    writes_bs: &mut ResBitset,
    events_bs: &mut ResBitset,
    phys_bs: &mut ResBitset,
    census: &mut TxSpanCensus,
) -> (Vec<String>, usize, Vec<String>, bool) {
    writes_bs.clear();
    events_bs.clear();
    phys_bs.clear();
    let mut coverage_complete = true;
    for rid in visited {
        let Some(summary) = summaries.get(rid) else {
            coverage_complete = false;
            continue;
        };
        writes_bs.insert_all(cone_derived.writes_table_ids_of(&summary.routine_id));
        for id in cone_derived.physical_write_ids_of(&summary.routine_id) {
            phys_bs.insert(id);
        }
        events_bs.insert_all(cone_derived.event_ids_of(&summary.routine_id));
        if reachable_coverage(summary, None) != "complete" {
            coverage_complete = false;
        }
    }
    let writes = resolve_sorted_ids(writes_bs, cone_derived, census);
    let events = resolve_sorted_ids(events_bs, cone_derived, census);
    (writes, phys_bs.len(), events, coverage_complete)
}

/// Resolve a `ResId` set into the sorted-unique `Vec<String>` the old
/// `BTreeSet<String>` produced. The set is already unique (a bitset cannot hold a
/// duplicate); the sort is by string because intern order is not lexicographic.
fn resolve_sorted_ids(
    bs: &ResBitset,
    cone_derived: &ConeDerivedStore,
    census: &mut TxSpanCensus,
) -> Vec<String> {
    let mut out: Vec<String> = bs
        .iter_ids()
        .map(|id| cone_derived.resolve_res(id).to_string())
        .collect();
    census.materialized_strings += out.len();
    out.sort();
    out
}

/// span roots = visited routines with no reverse callers. Mirrors al-sem
/// `[...visited].filter((rid) => (reverse.get(rid) ?? []).length === 0)`.
fn span_roots_of(visited: &BTreeSet<String>, reverse: &ReverseCallGraph) -> Vec<String> {
    let mut roots: Vec<String> = visited
        .iter()
        .filter(|rid| reverse.get(*rid).map(|v| v.is_empty()).unwrap_or(true))
        .cloned()
        .collect();
    roots.sort();
    roots
}

/// Everything about a span that depends only on the seed ROUTINE.
struct SpanTemplate {
    /// Pending writes of every member except the seed routine (whose part
    /// depends on WHICH Commit), and each member's own count.
    pending_others: Pending,
    pending_by_routine: BTreeMap<String, usize>,
    routines_in_span: Vec<String>,
    writes_tables: Vec<String>,
    writes_physical_tables_count: usize,
    publishes_events: Vec<String>,
    span_roots: Vec<String>,
    coverage_complete: bool,
}

/// `ALSEM_TXSPAN_CENSUS=1` — the population this module actually processes,
/// printed to stderr at the end of [`compute_transaction_spans`]. Diagnostic
/// only: no production path reads it, and with the env var unset it costs a
/// handful of counter increments and allocates nothing. Mirrors the
/// `C1_CONE_CENSUS` convention in [`crate::engine::l4::cone_census`].
///
/// These are the figures that PRICE this module. `template_calls` is how many
/// seed occurrences ask for a span; `templates` is how many of those actually
/// walk (the cache collapses every commit op of one seed routine onto one
/// walk), so `templates` — not the seed count — is the BFS multiplier.
/// `visited_total` is the number of per-routine aggregate steps summed over
/// those walks: the population every per-visited-routine cost is multiplied by.
/// `payload_strings` is how many `String`s the emitted spans retain in their
/// four id lists.
#[derive(Default)]
struct TxSpanCensus {
    template_calls: usize,
    templates: usize,
    visited_total: usize,
    spans_emitted: usize,
    payload_strings: usize,
    /// Strings ALLOCATED and thrown away inside the per-visited-routine union:
    /// `writes_tables_of` / `publishes_events_of` each resolve their whole
    /// interned window into a fresh `Vec<String>` per visited routine, which is
    /// then inserted into a `BTreeSet<String>` and dropped.
    materialized_strings: usize,
}

impl TxSpanCensus {
    fn enabled() -> bool {
        std::env::var("ALSEM_TXSPAN_CENSUS").as_deref() == Ok("1")
    }

    fn report(&self) {
        eprintln!(
            "[txspan-census] template_calls={} templates={} visited_total={} \
             spans_emitted={} payload_strings={} materialized_strings={} mean_cone={:.1}",
            self.template_calls,
            self.templates,
            self.visited_total,
            self.spans_emitted,
            self.payload_strings,
            self.materialized_strings,
            if self.templates == 0 {
                0.0
            } else {
                self.visited_total as f64 / self.templates as f64
            },
        );
    }
}

/// Compute-or-lookup the per-seed-routine template: `backward_cone` +
/// `aggregate_span` + `span_roots_of` depend only on `(seed,
/// commits_by_routine, reverse, summaries)` — identical for every commit op on
/// the same routine AND for §B seeds of the same routine (see the CRITICAL
/// semantics note at both call sites), so compute it at most once per distinct
/// seed routine id and cache it.
/// The read-only inputs every span template derives from — grouped so
/// `span_template` takes four parameters instead of nine.
struct SpanInputs<'a> {
    commits_by_routine: &'a BTreeMap<String, Vec<String>>,
    reverse: &'a ReverseCallGraph,
    summaries: &'a HashMap<String, FullRoutineSummary>,
    cone_derived: &'a ConeDerivedStore,
    routine_by_id: &'a HashMap<&'a str, &'a ModelRoutine>,
    fwd: &'a ForwardSites<'a>,
}

/// Run-lifetime scratch: the two id bitsets the union fills and clears per
/// template (allocated ONCE for the whole run — see `aggregate_span`) plus the
/// census counters.
struct SpanScratch {
    /// Guard frames, computed on first use (`backward_cone`).
    frames: HashMap<String, Option<FrameGuards>>,
    writes: ResBitset,
    events: ResBitset,
    phys_writes: ResBitset,
    census: TxSpanCensus,
}

/// What the seed routine's parameters must be for `site` (a Commit, or a
/// checked run) to run.
fn seed_reqs(
    r: &ModelRoutine,
    site: &str,
    inputs: &SpanInputs<'_>,
    scratch: &mut SpanScratch,
) -> Vec<Req> {
    let frame = scratch.frames.entry(r.id.clone()).or_insert_with(|| {
        let callees_at = |cs_id: &str| -> Vec<&str> {
            inputs
                .fwd
                .get(r.id.as_str())
                .map_or(&[][..], Vec::as_slice)
                .iter()
                .filter(|(s, _)| *s == Some(cs_id))
                .map(|(_, to)| *to)
                .collect()
        };
        frame_guards_over(r, callees_at, inputs.routine_by_id)
    });
    frame
        .as_ref()
        .and_then(|f| f.by_site.get(site).cloned())
        .unwrap_or_default()
}

fn span_template<'c>(
    seed: &str,
    seed_reqs: &[Req],
    inputs: &SpanInputs<'_>,
    cache: &'c mut HashMap<String, SpanTemplate>,
    scratch: &mut SpanScratch,
) -> &'c SpanTemplate {
    scratch.census.template_calls += 1;
    // One template per seed routine AND the requirements its Commit needs.
    let key = format!("{seed}|{seed_reqs:?}");
    if !cache.contains_key(&key) {
        let visited = backward_cone(seed, seed_reqs, inputs, &mut scratch.frames);
        scratch.census.templates += 1;
        scratch.census.visited_total += visited.len();
        let (writes_tables, writes_physical_tables_count, publishes_events, coverage_complete) =
            aggregate_span(
                &visited,
                inputs.summaries,
                inputs.cone_derived,
                &mut scratch.writes,
                &mut scratch.events,
                &mut scratch.phys_writes,
                &mut scratch.census,
            );
        let span_roots = span_roots_of(&visited, inputs.reverse);
        let mut pending_others = Pending::default();
        let mut pending_by_routine: BTreeMap<String, usize> = BTreeMap::new();
        for rid in visited.iter().filter(|r| r.as_str() != seed) {
            let Some(r) = inputs.routine_by_id.get(rid.as_str()) else {
                continue;
            };
            let (targets, unsited) = toward_sites(rid, |to| visited.contains(to), inputs.fwd);
            let p = pending_before(
                r,
                &targets,
                unsited,
                inputs.fwd,
                inputs.routine_by_id,
                inputs.summaries,
                inputs.cone_derived,
            );
            pending_by_routine.insert(rid.clone(), p.tables.len());
            pending_others.tables.extend(p.tables);
            pending_others.events.extend(p.events);
        }
        cache.insert(
            key.clone(),
            SpanTemplate {
                pending_others,
                pending_by_routine,
                routines_in_span: visited.iter().cloned().collect(),
                writes_tables,
                writes_physical_tables_count,
                publishes_events,
                span_roots,
                coverage_complete,
            },
        );
    }
    &cache[&key]
}

/// Compute transaction spans. For each primary-app routine that contains a Commit
/// (and each checked Codeunit.Run implicit commit), walk callers backward to find
/// every routine that participates in the transaction. Each Commit operation
/// yields one `TransactionSpan`. Mirrors al-sem `computeTransactionSpans`.
///
/// `routines` — the model routines (al-sem `model.routines`).
/// `dep_routine_ids` — the role oracle (empty ⇒ all primary).
/// `reverse` — the reverse call graph (`build_reverse_call_graph`).
/// `summaries` — internal RoutineId → its `FullRoutineSummary`; a routine with no
/// entry behaves like al-sem's `summary === undefined`.
/// `cone_derived` — ⟨C1 Task 2⟩ the folded cone substrate the span's
/// `writes_tables` / `publishes_events` unions read; must be the store built
/// from the SAME cone walk that produced `summaries`.
pub fn compute_transaction_spans(
    routines: &[ModelRoutine],
    dep_routine_ids: &BTreeSet<String>,
    reverse: &ReverseCallGraph,
    summaries: &HashMap<String, FullRoutineSummary>,
    cone_derived: &ConeDerivedStore,
) -> Vec<TransactionSpan> {
    let mut spans: Vec<TransactionSpan> = Vec::new();

    // routineId → its Commit operationIds (operationSites with kind == "commit"),
    // PRIMARY routines only. A BTreeMap so the explicit-commit seed iteration is
    // deterministic (al-sem's Map is insertion-ordered over model.routines; we
    // key-sort, which Task 2b re-sorts by span key anyway).
    let mut commits_by_routine: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for r in routines {
        if !is_primary(r, dep_routine_ids) {
            continue;
        }
        let commit_ops: Vec<String> = r
            .operation_sites
            .iter()
            .filter(|os| os.kind == "commit")
            .map(|os| os.id.clone())
            .collect();
        if !commit_ops.is_empty() {
            commits_by_routine.insert(r.id.clone(), commit_ops);
        }
    }

    // Everything about a span that depends only on the seed ROUTINE — cached
    // per distinct seed routine id (see `span_template` above `compute_transaction_spans`).
    let mut template_cache: HashMap<String, SpanTemplate> = HashMap::new();
    let routine_by_id: HashMap<&str, &ModelRoutine> =
        routines.iter().map(|r| (r.id.as_str(), r)).collect();
    let fwd = forward_sites(reverse);
    let inputs = SpanInputs {
        commits_by_routine: &commits_by_routine,
        reverse,
        summaries,
        cone_derived,
        routine_by_id: &routine_by_id,
        fwd: &fwd,
    };
    // The seed routine's own pending part, up to ONE Commit (or checked run),
    // joined with the template's.
    let pending_at =
        |t: &SpanTemplate, seed: &ModelRoutine, at: &str| -> (Vec<String>, Vec<String>) {
            let own = pending_before(
                seed,
                &[at],
                false,
                &fwd,
                &routine_by_id,
                summaries,
                cone_derived,
            );
            let tables: BTreeSet<String> = t
                .pending_others
                .tables
                .iter()
                .cloned()
                .chain(own.tables)
                .collect();
            let events: BTreeSet<String> = t
                .pending_others
                .events
                .iter()
                .cloned()
                .chain(own.events)
                .collect();
            (tables.into_iter().collect(), events.into_iter().collect())
        };
    // ONE bitset pair for the whole run: `aggregate_span` clears and refills them
    // per template, so the union never allocates per routine or per element.
    let mut scratch = SpanScratch {
        frames: HashMap::new(),
        writes: ResBitset::new(cone_derived.res_universe_len()),
        events: ResBitset::new(cone_derived.res_universe_len()),
        phys_writes: ResBitset::new(cone_derived.res_universe_len()),
        census: TxSpanCensus::default(),
    };

    // --- explicit-commit seeds ---
    for (commit_routine_id, commit_ops) in &commits_by_routine {
        let seed = routine_by_id[commit_routine_id.as_str()];
        // clone the template fields once per OP (one walk per distinct
        // requirement list — usually one per routine)
        for commit_operation_id in commit_ops {
            let reqs = seed_reqs(seed, commit_operation_id, &inputs, &mut scratch);
            let t = span_template(
                commit_routine_id,
                &reqs,
                &inputs,
                &mut template_cache,
                &mut scratch,
            );
            let (pending_physical_tables, pending_events) =
                pending_at(t, seed, commit_operation_id);
            scratch.census.spans_emitted += 1;
            scratch.census.payload_strings += t.routines_in_span.len()
                + t.writes_tables.len()
                + t.publishes_events.len()
                + t.span_roots.len();
            spans.push(TransactionSpan {
                seed_kind: SeedKind::ExplicitCommit,
                commit_operation_id: commit_operation_id.clone(),
                seed_callsite_id: None,
                commit_routine_id: commit_routine_id.clone(),
                routines_in_span: t.routines_in_span.clone(),
                writes_tables: t.writes_tables.clone(),
                writes_physical_tables_count: t.writes_physical_tables_count,
                publishes_events: t.publishes_events.clone(),
                span_roots: t.span_roots.clone(),
                coverage_complete: t.coverage_complete,
                pending_physical_tables,
                pending_events,
                pending_physical_by_routine: t.pending_by_routine.clone(),
            });
        }
    }

    // --- §B: synthetic seeds for CHECKED codeunit-run implicit commits ---
    // Codeunit.Run only; objectRunReturnUsed === true only (the STRICT affirmative
    // predicate). NOT Page.Run / Report.Run.
    for r in routines {
        if !is_primary(r, dep_routine_ids) {
            continue;
        }
        for cs in &r.call_sites {
            let PCallee::ObjectRun { object_kind, .. } = &cs.callee else {
                continue;
            };
            if object_kind != "Codeunit" {
                continue;
            }
            if cs.object_run_return_used != Some(true) {
                continue;
            }
            let reqs = seed_reqs(r, &cs.id, &inputs, &mut scratch);
            let t = span_template(&r.id, &reqs, &inputs, &mut template_cache, &mut scratch);
            let (pending_physical_tables, pending_events) = pending_at(t, r, &cs.id);
            scratch.census.spans_emitted += 1;
            scratch.census.payload_strings += t.routines_in_span.len()
                + t.writes_tables.len()
                + t.publishes_events.len()
                + t.span_roots.len();
            // commitOperationId uses the callsite id (same opaque-string type at
            // runtime); seed_callsite_id provides the typed accessor.
            spans.push(TransactionSpan {
                seed_kind: SeedKind::CheckedRunImplicit,
                commit_operation_id: cs.id.clone(),
                seed_callsite_id: Some(cs.id.clone()),
                commit_routine_id: r.id.clone(),
                routines_in_span: t.routines_in_span.clone(),
                writes_tables: t.writes_tables.clone(),
                writes_physical_tables_count: t.writes_physical_tables_count,
                publishes_events: t.publishes_events.clone(),
                span_roots: t.span_roots.clone(),
                coverage_complete: t.coverage_complete,
                pending_physical_tables,
                pending_events,
                pending_physical_by_routine: t.pending_by_routine.clone(),
            });
        }
    }

    if TxSpanCensus::enabled() {
        scratch.census.report();
    }
    spans
}

// ===========================================================================
// Native oracles — ground-truth-free invariants on synthetic inputs.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::l5::reverse_call_graph::build_reverse_call_graph;
    use crate::engine::l5::test_support::{
        cone_store_of, coverage, edge, fact, graph_from_edges, object_run_call_site,
        op_commit_routine, routine, summary,
    };

    #[test]
    fn explicit_commit_span_is_backward_cone_with_union_and_roots() {
        // Call chain: root → mid → committer.  committer holds Commit (op id "c/op").
        // Backward cone from committer = {committer, mid, root}.
        let routines = vec![
            routine("root", "trigger"),
            routine("mid", "procedure"),
            op_commit_routine("committer", "procedure", &["c/op"]),
        ];
        let graph = graph_from_edges(
            &["root", "mid", "committer"],
            &[edge("root", "mid", "cs1"), edge("mid", "committer", "cs2")],
        );
        let reverse = build_reverse_call_graph(&graph);

        let mut summaries: HashMap<String, FullRoutineSummary> = HashMap::new();
        summaries.insert(
            "committer".to_string(),
            summary(
                "committer",
                vec![fact("insert", "table", Some("t/A"))],
                vec![],
                Some(coverage("complete")),
            ),
        );
        summaries.insert(
            "mid".to_string(),
            summary(
                "mid",
                vec![fact("publish", "event", Some("e/E"))],
                vec![],
                Some(coverage("complete")),
            ),
        );
        summaries.insert(
            "root".to_string(),
            summary("root", vec![], vec![], Some(coverage("complete"))),
        );

        let no_deps = BTreeSet::new();
        let spans = compute_transaction_spans(
            &routines,
            &no_deps,
            &reverse,
            &summaries,
            &cone_store_of(&summaries),
        );
        assert_eq!(spans.len(), 1);
        let span = &spans[0];
        assert_eq!(span.seed_kind, SeedKind::ExplicitCommit);
        assert_eq!(span.commit_operation_id, "c/op");
        assert_eq!(span.commit_routine_id, "committer");
        assert_eq!(span.routines_in_span, vec!["committer", "mid", "root"]);
        assert_eq!(span.writes_tables, vec!["t/A"]);
        assert_eq!(span.publishes_events, vec!["e/E"]);
        // Only `root` has no reverse callers.
        assert_eq!(span.span_roots, vec!["root"]);
        assert!(span.coverage_complete);
    }

    #[test]
    fn walk_stops_at_another_committing_routine() {
        // outer (commits) → inner (commits). The span seeded at `inner` must NOT
        // include `outer` (the walk stops when it reaches another committer).
        let routines = vec![
            op_commit_routine("outer", "procedure", &["outer/op"]),
            op_commit_routine("inner", "procedure", &["inner/op"]),
        ];
        let graph = graph_from_edges(&["outer", "inner"], &[edge("outer", "inner", "cs1")]);
        let reverse = build_reverse_call_graph(&graph);
        let summaries: HashMap<String, FullRoutineSummary> = HashMap::new();
        let no_deps = BTreeSet::new();
        let spans = compute_transaction_spans(
            &routines,
            &no_deps,
            &reverse,
            &summaries,
            &cone_store_of(&summaries),
        );

        let inner_span = spans
            .iter()
            .find(|s| s.commit_routine_id == "inner")
            .unwrap();
        // backward_cone from inner: visits inner, then its caller outer — but outer
        // is enqueued and on dequeue it's a committer != seed, so it is visited
        // (added) yet its callers are not walked. al-sem adds outer to `visited`.
        // The stop-at-committer rule prevents walking PAST outer, but outer itself
        // is in the cone. Assert that the prior committer bounds the trace: nothing
        // upstream of outer is pulled in.
        assert!(inner_span.routines_in_span.contains(&"inner".to_string()));
        assert!(inner_span.routines_in_span.contains(&"outer".to_string()));

        let outer_span = spans
            .iter()
            .find(|s| s.commit_routine_id == "outer")
            .unwrap();
        // outer's cone: outer only (no callers). inner is DOWNSTREAM (forward), not
        // reached by a backward walk.
        assert_eq!(outer_span.routines_in_span, vec!["outer"]);
    }

    #[test]
    fn walk_does_not_pull_in_callers_past_the_boundary_committer() {
        // grandparent (NO commit) → outer (commits) → inner (commits).
        // Seeded at `inner`, the backward walk reaches `outer`, includes it (the
        // boundary committer is in the cone), but MUST NOT walk past it to
        // `grandparent`. This is the test that actually exercises the
        // `id != seed && commits_by_routine.contains_key(id)` stop guard — the
        // outer→inner-only fixture above cannot, since `outer` has no callers.
        let routines = vec![
            op_commit_routine("grandparent", "procedure", &[]), // no commit ⇒ not a committer
            op_commit_routine("outer", "procedure", &["outer/op"]),
            op_commit_routine("inner", "procedure", &["inner/op"]),
        ];
        let graph = graph_from_edges(
            &["grandparent", "inner", "outer"],
            &[
                edge("grandparent", "outer", "cs1"),
                edge("outer", "inner", "cs2"),
            ],
        );
        let reverse = build_reverse_call_graph(&graph);
        let summaries: HashMap<String, FullRoutineSummary> = HashMap::new();
        let no_deps = BTreeSet::new();
        let spans = compute_transaction_spans(
            &routines,
            &no_deps,
            &reverse,
            &summaries,
            &cone_store_of(&summaries),
        );

        let inner_span = spans
            .iter()
            .find(|s| s.commit_routine_id == "inner")
            .unwrap();
        // Exactly {inner, outer} — `grandparent` (upstream of the boundary
        // committer `outer`) is excluded. Deleting the stop guard would pull
        // `grandparent` in and fail this assertion.
        assert_eq!(inner_span.routines_in_span, vec!["inner", "outer"]);
    }

    #[test]
    fn missing_summary_makes_coverage_incomplete_and_contributes_nothing() {
        let routines = vec![op_commit_routine("c", "procedure", &["c/op"])];
        let graph = graph_from_edges(&["c"], &[]);
        let reverse = build_reverse_call_graph(&graph);
        // No summary entry for "c" → coverage_complete false, no writes/events.
        let summaries: HashMap<String, FullRoutineSummary> = HashMap::new();
        let no_deps = BTreeSet::new();
        let spans = compute_transaction_spans(
            &routines,
            &no_deps,
            &reverse,
            &summaries,
            &cone_store_of(&summaries),
        );
        assert_eq!(spans.len(), 1);
        assert!(!spans[0].coverage_complete);
        assert!(spans[0].writes_tables.is_empty());
        // c has no callers → it is its own span root.
        assert_eq!(spans[0].span_roots, vec!["c"]);
    }

    #[test]
    fn partial_coverage_makes_span_incomplete() {
        let routines = vec![op_commit_routine("c", "procedure", &["c/op"])];
        let graph = graph_from_edges(&["c"], &[]);
        let reverse = build_reverse_call_graph(&graph);
        let mut summaries: HashMap<String, FullRoutineSummary> = HashMap::new();
        summaries.insert(
            "c".to_string(),
            summary("c", vec![], vec![], Some(coverage("partial"))),
        );
        let no_deps = BTreeSet::new();
        let spans = compute_transaction_spans(
            &routines,
            &no_deps,
            &reverse,
            &summaries,
            &cone_store_of(&summaries),
        );
        assert!(!spans[0].coverage_complete);
    }

    #[test]
    fn checked_codeunit_run_yields_implicit_seed() {
        // A routine with a checked Codeunit.Run (objectRunReturnUsed = Some(true)).
        let mut caller = routine("caller", "procedure");
        caller
            .call_sites
            .push(object_run_call_site("caller/cs0", "Codeunit", Some(true)));
        let routines = vec![caller];
        let graph = graph_from_edges(&["caller"], &[]);
        let reverse = build_reverse_call_graph(&graph);
        let summaries: HashMap<String, FullRoutineSummary> = HashMap::new();
        let no_deps = BTreeSet::new();
        let spans = compute_transaction_spans(
            &routines,
            &no_deps,
            &reverse,
            &summaries,
            &cone_store_of(&summaries),
        );
        assert_eq!(spans.len(), 1);
        let span = &spans[0];
        assert_eq!(span.seed_kind, SeedKind::CheckedRunImplicit);
        assert_eq!(span.seed_callsite_id.as_deref(), Some("caller/cs0"));
        assert_eq!(span.commit_operation_id, "caller/cs0");
        assert_eq!(span.commit_routine_id, "caller");
    }

    #[test]
    fn unchecked_run_and_non_codeunit_run_do_not_seed() {
        let mut r = routine("r", "procedure");
        // unchecked Codeunit.Run (return not used) → no seed
        r.call_sites
            .push(object_run_call_site("r/cs0", "Codeunit", Some(false)));
        // checked Page.Run → no seed (only Codeunit has the implicit-commit rule)
        r.call_sites
            .push(object_run_call_site("r/cs1", "Page", Some(true)));
        let routines = vec![r];
        let graph = graph_from_edges(&["r"], &[]);
        let reverse = build_reverse_call_graph(&graph);
        let summaries: HashMap<String, FullRoutineSummary> = HashMap::new();
        let no_deps = BTreeSet::new();
        let spans = compute_transaction_spans(
            &routines,
            &no_deps,
            &reverse,
            &summaries,
            &cone_store_of(&summaries),
        );
        assert!(spans.is_empty());
    }

    #[test]
    fn dep_routines_do_not_seed() {
        let routines = vec![op_commit_routine("c", "procedure", &["c/op"])];
        let graph = graph_from_edges(&["c"], &[]);
        let reverse = build_reverse_call_graph(&graph);
        let summaries: HashMap<String, FullRoutineSummary> = HashMap::new();
        let deps: BTreeSet<String> = ["c".to_string()].into_iter().collect();
        let spans = compute_transaction_spans(
            &routines,
            &deps,
            &reverse,
            &summaries,
            &cone_store_of(&summaries),
        );
        assert!(spans.is_empty());
    }

    #[test]
    fn multi_commit_routine_ops_share_identical_span_shape() {
        // Routine with TWO commit ops: both spans must have identical
        // routines_in_span/writes/events/roots (the template), differing only
        // in commit_operation_id.
        let routines = vec![
            routine("root", "trigger"),
            op_commit_routine("committer", "procedure", &["c/op1", "c/op2"]),
        ];
        let graph = graph_from_edges(&["root", "committer"], &[edge("root", "committer", "cs1")]);
        let reverse = build_reverse_call_graph(&graph);
        let summaries: HashMap<String, FullRoutineSummary> = HashMap::new();
        let no_deps = BTreeSet::new();
        let spans = compute_transaction_spans(
            &routines,
            &no_deps,
            &reverse,
            &summaries,
            &cone_store_of(&summaries),
        );
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].routines_in_span, spans[1].routines_in_span);
        assert_eq!(spans[0].span_roots, spans[1].span_roots);
        assert_ne!(spans[0].commit_operation_id, spans[1].commit_operation_id);
    }

    /// The span's write/event unions are deduped ACROSS routines and sorted by the
    /// resolved string, and one routine without a summary still poisons
    /// `coverage_complete`. This is the regression pin for the interned-id union:
    /// the fixture states both preconditions by hand — `t/A` is written by TWO
    /// routines (so a lost dedupe shows), the tables are declared in an order whose
    /// intern order is NOT their string order (so a lost sort shows), and `root`
    /// has no summary entry at all.
    #[test]
    fn span_union_is_deduped_string_sorted_and_coverage_follows_a_missing_summary() {
        let routines = vec![
            routine("root", "trigger"),
            routine("mid", "procedure"),
            op_commit_routine("committer", "procedure", &["c/op"]),
        ];
        let graph = graph_from_edges(
            &["root", "mid", "committer"],
            &[edge("root", "mid", "cs1"), edge("mid", "committer", "cs2")],
        );
        let reverse = build_reverse_call_graph(&graph);

        let mut summaries: HashMap<String, FullRoutineSummary> = HashMap::new();
        // `committer` declares t/Z BEFORE t/A: interning follows declaration order,
        // so id order is (t/Z, t/A) while string order is (t/A, t/Z).
        summaries.insert(
            "committer".to_string(),
            summary(
                "committer",
                vec![
                    fact("insert", "table", Some("t/Z")),
                    fact("insert", "table", Some("t/A")),
                    fact("publish", "event", Some("e/Z")),
                ],
                vec![],
                Some(coverage("complete")),
            ),
        );
        // `mid` writes t/A too — the cross-routine duplicate.
        summaries.insert(
            "mid".to_string(),
            summary(
                "mid",
                vec![
                    fact("insert", "table", Some("t/A")),
                    fact("publish", "event", Some("e/A")),
                ],
                vec![],
                Some(coverage("complete")),
            ),
        );
        // `root` deliberately has NO summary.

        let no_deps = BTreeSet::new();
        let spans = compute_transaction_spans(
            &routines,
            &no_deps,
            &reverse,
            &summaries,
            &cone_store_of(&summaries),
        );
        assert_eq!(spans.len(), 1);
        assert_eq!(
            spans[0].writes_tables,
            vec!["t/A", "t/Z"],
            "union is deduped across routines and sorted by the resolved string"
        );
        assert_eq!(spans[0].publishes_events, vec!["e/A", "e/Z"]);
        assert!(
            !spans[0].coverage_complete,
            "`root` has no summary, so the span is not coverage-complete"
        );
    }
}
