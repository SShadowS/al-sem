//! cli-b/b2 — PROVE ENGINE + CLI pipeline.
//!
//! Ports:
//!   - `src/prove/prove-questions.ts`   → ProveQuestion / parse_question / question_ids
//!   - `src/prove/prove-engine.ts`       → prove (tristate logic, blockers, obligations)
//!   - `src/contracts/prove.ts`          → project_prove_document (envelope kind "prove" 1.2.0)
//!   - `src/cli/prove.ts`                → format_prove_human / run_prove_pipeline
//!
//! ## Tristate semantics
//!
//! MAY-questions (may-commit, writes-table, publishes-event, reaches-ui, throws-error):
//!   yes     = ≥1 matching effect (blockers irrelevant for yes).
//!   (`reaches-ui` is interactive UI only: Message, Confirm, StrMenu, a page or
//!   report run, Hyperlink. An `Error` is `throws-error`'s, never UI's.)
//!   no      = zero matches AND ALL obligations clear.
//!   unknown = zero matches + ≥1 obligation failure.
//!
//! MUST-question (commits-on-success-path):
//!   yes     = ≥1 COMMIT with conditionality "unconditional-on-success" AND obligations clear.
//!   no      = zero COMMIT effects of ANY conditionality AND obligations clear.
//!   unknown = everything else (conditional COMMIT → non-unconditional-effect-exists blocker).
//!
//! ## Blocker sort
//!   Deterministic: (kind, anchor file, anchor line, detail) ascending NUL-delimited.

use crate::engine::batch::job::{
    Doc, Job, JobFailure, PlanPart, ResolveCx, SideDoc, SideIx, ViewCx, ViewDemand, ViewFailure,
    ViewNeeds, WorkspaceRead,
};
use crate::engine::batch::output::JobOutput;
use crate::engine::gate::format_json::serialize_document_value;
#[cfg(doc)]
use crate::engine::l4::effect_query_cli::RECEIVER_TEMP_STATE;
use crate::engine::l4::effect_query_cli::{OwnerIndex, owner_qualified};
use crate::engine::l5::conditionality::UNCONDITIONAL;
use crate::engine::l5::digest_cli::{
    ChangedInput, DbTempStates, DigestEffectFull, DigestEntryFull, FullAnchor, finish_default_run,
    primary_guid_of, resolve_changed_roots, run_digest_query_full_from_entries,
};
use crate::engine::l5::unresolved_cone::UnresolvedConeItem;
use crate::engine::line_base::{shown, shown_opt};
use crate::engine::session::{Cell, prove_plan};
use std::collections::{BTreeMap, HashMap, HashSet};

// ---------------------------------------------------------------------------
// ProveQuestion
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProveQuestion {
    MayCommit,
    CommitsOnSuccessPath,
    WritesTable { table: String },
    PublishesEvent { event: String },
    ReachesUi,
    ThrowsError,
}

/// Parse a question text string into a ProveQuestion. Returns None for invalid input.
pub fn parse_question(text: &str) -> Option<ProveQuestion> {
    match text {
        "may-commit" => Some(ProveQuestion::MayCommit),
        "commits-on-success-path" => Some(ProveQuestion::CommitsOnSuccessPath),
        "reaches-ui" => Some(ProveQuestion::ReachesUi),
        "throws-error" => Some(ProveQuestion::ThrowsError),
        _ => {
            if let Some(rest) = text.strip_prefix("writes-table:") {
                if rest.is_empty() {
                    None
                } else {
                    Some(ProveQuestion::WritesTable {
                        table: rest.to_string(),
                    })
                }
            } else if let Some(rest) = text.strip_prefix("publishes-event:") {
                if rest.is_empty() {
                    None
                } else {
                    Some(ProveQuestion::PublishesEvent {
                        event: rest.to_string(),
                    })
                }
            } else {
                None
            }
        }
    }
}

/// Return all valid question id strings (for error messages).
pub fn question_ids() -> &'static [&'static str] {
    &[
        "may-commit",
        "commits-on-success-path",
        "writes-table:<table>",
        "publishes-event:<event>",
        "reaches-ui",
        "throws-error",
        "callers-have-pending-writes",
        "callers-intercept-errors",
        "callers-may-run-noninteractive",
    ]
}

/// Whether `text` is a valid prove question: an existing-effects one
/// ([`parse_question`]) or a callers' context one
/// ([`crate::engine::l5::callers_context::CallersQuestion`]).
#[must_use]
pub fn is_question(text: &str) -> bool {
    parse_question(text).is_some()
        || crate::engine::l5::callers_context::CallersQuestion::parse(text).is_some()
}

// ---------------------------------------------------------------------------
// ProveResult types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProveAnswer {
    Yes,
    No,
    Unknown,
}

impl ProveAnswer {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProveAnswer::Yes => "yes",
            ProveAnswer::No => "no",
            ProveAnswer::Unknown => "unknown",
        }
    }
}

/// Anchor for a blocker. `source_kind` is "source" for callsite-derived blockers, but
/// can be "source" OR "symbol" for the non-unconditional-effect-exists blocker (it
/// carries the effect's evidence anchor verbatim — kept whenever sourceKind !=
/// "unavailable", mirroring TS prove-engine.ts:386).
#[derive(Debug, Clone)]
pub struct BlockerAnchor {
    pub source_kind: String,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub excerpt: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProveBlocker {
    pub kind: &'static str,
    pub detail: String,
    pub anchor: Option<BlockerAnchor>,
}

#[derive(Debug, Clone)]
pub struct ProveObligations {
    pub unresolved_callsites: usize,
    pub open_world_rows: usize,
    pub analysis_gaps: usize,
    pub coverage_complete: bool,
    pub cone_truncated: bool,
    /// The world the obligations were counted in, the open-world interface
    /// rows it waived because the cone already follows every loaded
    /// implementation ([`world_waives`]), and whether coverage counted as
    /// complete only by that assumption. 0/false in the strict world.
    pub world: World,
    pub waived_rows: usize,
    pub coverage_closed_by_assumption: bool,
    /// The coverage is partial only through interface open world (its one
    /// reason): a closed world would close it too (output validation V2,
    /// the `re-analysis under an assumption:` line names it among what
    /// `closedWorld` removes).
    pub coverage_open_world_only: bool,
    /// The answer holds only in `world`: the strict world answers differently
    /// ([`prove_with_callers`]). The [`World::label`] label.
    pub answer_relies_on_world: bool,
    /// The answer is a `no` whose cone raises an event an app outside the
    /// program may subscribe to ([`relies_on_loaded_subscribers`]): it holds
    /// only if no subscriber beyond the loaded ones exists. The
    /// `loaded-subscribers` label, when no stronger world's label applies.
    pub relies_on_loaded_subscribers: bool,
}

impl ProveObligations {
    /// The answer's `assumptions` label: the weakest world it holds in, one
    /// tier (a closed-world answer assumes loaded subscribers too).
    #[must_use]
    pub fn assumption(&self) -> Option<&'static str> {
        if self.answer_relies_on_world {
            self.world.label()
        } else if self.relies_on_loaded_subscribers {
            World::LoadedSubscribers.label()
        } else {
            None
        }
    }
}

/// Effects that raise an event any app may subscribe to: an explicit event
/// publication, and opening a page (the platform page events). Table events
/// are read off the cone's record operations
/// (`UnresolvedCone::fires_table_events`): a `Validate` is no effect.
const SUBSCRIBABLE_EFFECTS: &[&str] = &["EVENT_PUBLISH", "UI_WINDOW_OPEN"];

/// Q6: the cone follows the LOADED subscribers of every event it raises (a
/// table event through its record operation's table-event edges since F1,
/// spec `2026-11-06-db-event-subscribers.md`; before that it did not, and a
/// `no` here could hide a loaded subscriber's write), so
/// an event is closed over them, unlike an interface, which is open. A `yes`
/// stands on its witness whatever else subscribes; a `no` (MAY: no matching
/// effect; MUST: not on every success path) could be overturned by a
/// subscriber in an app that is not loaded, as soon as the cone raises an
/// event: a table write or `Validate` (the platform table events, raised for
/// temporary records and with `RunTrigger = false` too), an explicit
/// publisher, or a page opened. Over-approximates: an internal publisher only
/// the declaring app's friends can subscribe to counts as well.
fn relies_on_loaded_subscribers(
    result: &ProveResult,
    entry: &DigestEntryFull,
    fires_table_events: bool,
) -> bool {
    result.answer == ProveAnswer::No
        && (fires_table_events
            || entry.effects.iter().any(|e| {
                !e.literal_unreachable && SUBSCRIBABLE_EFFECTS.contains(&e.effect_type.as_str())
            }))
}

/// What a prove run assumes about code outside the loaded program, weakest
/// first: strict < loaded-subscribers < closed-world. Each world waives the
/// open-world rows of the one before it and more.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum World {
    /// Nothing: an interface site may reach an implementer not loaded, an
    /// event a subscriber not loaded.
    Strict,
    /// The default (Q6, G29): no subscriber exists beyond the loaded ones. The
    /// cone already follows only loaded subscribers. A publisher-scoped
    /// interface site (`Telemetry Logger`) falls under it too: a logger exists
    /// only through an `OnRegisterTelemetryLogger` subscriber that registers
    /// it, so an unloaded app's logger is an unloaded subscriber.
    LoadedSubscribers,
    /// `--closed-world` (G24): no interface implementer exists beyond the
    /// loaded ones either.
    Closed,
}

impl World {
    /// The `assumptions` label of an answer that holds only in this world.
    #[must_use]
    pub fn label(self) -> Option<&'static str> {
        match self {
            World::Strict => None,
            World::LoadedSubscribers => Some("loaded-subscribers"),
            World::Closed => Some("closed-world"),
        }
    }

    /// The world a prove run's flags ask for.
    #[must_use]
    pub fn of_flags(closed_world: bool) -> World {
        if closed_world {
            World::Closed
        } else {
            World::LoadedSubscribers
        }
    }
}

/// G24/G29: in a world beyond the strict one, an open-world interface site
/// whose every implementer is in the model is no obligation when the world
/// assumes no other implementer exists: the cone follows each loaded
/// implementation. The closed world assumes it of every site, the
/// loaded-subscribers world of a publisher-scoped site only. A site with an
/// implementer outside the model (a body not loaded, an enum value mapped to a
/// codeunit that is not loaded, an implementer no route reached) is still
/// open, in every world.
fn world_waives(item: &UnresolvedConeItem, world: World) -> bool {
    let assumed = match world {
        World::Strict => false,
        World::LoadedSubscribers => item.publisher_scoped,
        World::Closed => true,
    };
    assumed && item.open_world == Some(true) && item.closed_world_followable
}

/// The coverage reason an open-world interface site gives the routine.
const INTERFACE_OPEN_WORLD: &str = "interface-open-world";

#[derive(Debug, Clone)]
pub struct ProveResult {
    pub answer: ProveAnswer,
    /// Indices into the effect slice for matching effects.
    pub evidence: Option<Vec<usize>>,
    pub blocked_by: Option<Vec<ProveBlocker>>,
    pub obligations: ProveObligations,
}

// ---------------------------------------------------------------------------
// Internal: Obligation computation
// (the gaps-in-cone count is `digest_cli::count_gaps_in_cone`, #160 step 9)
// ---------------------------------------------------------------------------

fn compute_obligations(
    entry: &DigestEntryFull,
    gaps_in_cone_count: usize,
    world: World,
) -> ProveObligations {
    let mut unresolved_callsites = 0usize;
    let mut open_world_rows = 0usize;
    let mut waived_rows = 0usize;

    for item in &entry.unresolved {
        if world_waives(item, world) {
            waived_rows += 1;
        } else if item.open_world == Some(true) {
            open_world_rows += 1;
        } else {
            // Both "polymorphic" (without openWorld) and non-polymorphic non-openWorld
            // count as unresolved-callsite (mirrors TS prove-engine.ts lines 127-136).
            unresolved_callsites += 1;
        }
    }

    // Coverage that is partial ONLY through interface open world is complete
    // in a world that waived every open-world site (at least one: a reason
    // with no site here to waive stays partial).
    let complete = entry.coverage_status == "complete";
    let coverage_closed_by_assumption = !complete
        && world != World::Strict
        && waived_rows > 0
        && open_world_rows == 0
        && entry.coverage_status == "partial"
        && !entry.coverage_reasons.is_empty()
        && entry
            .coverage_reasons
            .iter()
            .all(|r| r == INTERFACE_OPEN_WORLD);

    ProveObligations {
        unresolved_callsites,
        open_world_rows,
        analysis_gaps: gaps_in_cone_count,
        coverage_complete: complete || coverage_closed_by_assumption,
        cone_truncated: entry.unresolved_traversal.truncated,
        world,
        waived_rows,
        coverage_closed_by_assumption,
        coverage_open_world_only: !complete
            && entry.coverage_status == "partial"
            && !entry.coverage_reasons.is_empty()
            && entry
                .coverage_reasons
                .iter()
                .all(|r| r == INTERFACE_OPEN_WORLD),
        answer_relies_on_world: false,
        relies_on_loaded_subscribers: false,
    }
}

/// W2-8 (2h, design §4.7 P4): where the obligations point. The unresolved
/// call sites the answer's world still counts (not waived), split by where
/// the call site is and by kind, and the most frequent callees.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObligationPointers {
    /// In workspace code: calls not resolved (dynamic targets and the like).
    pub workspace_unresolved: usize,
    /// In workspace code: interface calls counted open world.
    pub workspace_open_world: usize,
    /// The three most frequent callees, `(display, count)`.
    pub top_callees: Vec<(String, usize)>,
    /// Round-2 G31: the three most frequent callees by kind, for the human
    /// lines: calls not resolved, and interface calls counted open world
    /// (`interface dispatch (open world)`, never "not resolved").
    pub top_not_resolved: Vec<(String, usize)>,
    pub top_open_world: Vec<(String, usize)>,
}

impl ObligationPointers {
    /// Over `entry`'s unresolved cone, in `world`.
    #[must_use]
    pub fn of(entry: &DigestEntryFull, world: World) -> ObligationPointers {
        let mut p = ObligationPointers::default();
        let mut callees: BTreeMap<&str, usize> = BTreeMap::new();
        let mut not_resolved: BTreeMap<&str, usize> = BTreeMap::new();
        let mut open_world: BTreeMap<&str, usize> = BTreeMap::new();
        for item in entry.unresolved.iter().filter(|i| !world_waives(i, world)) {
            if item
                .callsite_file
                .as_deref()
                .is_some_and(|f| f.starts_with("ws:"))
            {
                if item.open_world == Some(true) {
                    p.workspace_open_world += 1;
                } else {
                    p.workspace_unresolved += 1;
                }
            }
            *callees.entry(item.callee_display.as_str()).or_default() += 1;
            let by_kind = if item.open_world == Some(true) {
                &mut open_world
            } else {
                &mut not_resolved
            };
            *by_kind.entry(item.callee_display.as_str()).or_default() += 1;
        }
        let top = |m: BTreeMap<&str, usize>| {
            let mut top: Vec<(String, usize)> =
                m.into_iter().map(|(c, n)| (c.to_string(), n)).collect();
            top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            top.truncate(3);
            top
        };
        p.top_callees = top(callees);
        p.top_not_resolved = top(not_resolved);
        p.top_open_world = top(open_world);
        p
    }

    fn to_value(&self) -> serde_json::Value {
        serde_json::json!({
            "unresolvedInWorkspace": {
                "notResolved": self.workspace_unresolved,
                "openWorld": self.workspace_open_world,
            },
            "topUnresolvedCallees": self.top_callees.iter()
                .map(|(c, n)| serde_json::json!({"callee": c, "count": n}))
                .collect::<Vec<_>>(),
        })
    }
}

fn obligations_clear(oblg: &ProveObligations, witness_truncated: bool) -> bool {
    oblg.unresolved_callsites == 0
        && oblg.open_world_rows == 0
        && oblg.analysis_gaps == 0
        && oblg.coverage_complete
        && !oblg.cone_truncated
        && !witness_truncated
}

fn build_obligation_blockers(
    entry: &DigestEntryFull,
    oblg: &ProveObligations,
    witness_truncated: bool,
) -> Vec<ProveBlocker> {
    let mut blockers: Vec<ProveBlocker> = Vec::new();

    // Unresolved callsites (non-openWorld first, then open-world)
    for item in &entry.unresolved {
        if item.open_world == Some(true) {
            continue;
        }
        let anchor = item.callsite_file.as_ref().map(|f| BlockerAnchor {
            source_kind: "source".to_string(),
            file: Some(f.clone()),
            line: item.callsite_line,
            column: item.callsite_column,
            excerpt: None, // callsite anchors never carry excerpt
        });
        blockers.push(ProveBlocker {
            kind: "unresolved-callsite",
            detail: format!("{} in {}", item.callee_display, item.owning_routine_display),
            anchor,
        });
    }

    // Open-world dispatch rows (less the ones a closed world waived)
    for item in &entry.unresolved {
        if item.open_world != Some(true) || world_waives(item, oblg.world) {
            continue;
        }
        let anchor = item.callsite_file.as_ref().map(|f| BlockerAnchor {
            source_kind: "source".to_string(),
            file: Some(f.clone()),
            line: item.callsite_line,
            column: item.callsite_column,
            excerpt: None, // callsite anchors never carry excerpt
        });
        blockers.push(ProveBlocker {
            kind: "open-world-dispatch",
            detail: format!("{} in {}", item.callee_display, item.owning_routine_display),
            anchor,
        });
    }

    if oblg.analysis_gaps > 0 {
        blockers.push(ProveBlocker {
            kind: "analysis-gap",
            detail: format!("{} analysis gap(s) in cone", oblg.analysis_gaps),
            anchor: None,
        });
    }

    // Round 2 G13: `coverage incomplete` is a blocker of its own only when
    // a reason of it is not already a listed blocker; when every reason is
    // (interface open world as open-world rows, a call not resolved as an
    // unresolved call site), it shows as the analysis line's reason and is
    // not counted twice.
    if !oblg.coverage_complete && !coverage_reasons_listed(&entry.coverage_reasons, &blockers) {
        blockers.push(ProveBlocker {
            kind: "coverage-incomplete",
            detail: format!("coverage status: {}", entry.coverage_status),
            anchor: None,
        });
    }

    if oblg.cone_truncated {
        blockers.push(ProveBlocker {
            kind: "cone-truncated",
            detail: "transitive cone BFS was truncated".to_string(),
            anchor: None,
        });
    }

    if witness_truncated {
        blockers.push(ProveBlocker {
            kind: "witness-truncated",
            detail: "witness path reconstruction was truncated or evidence unavailable".to_string(),
            anchor: None,
        });
    }

    blockers
}

/// The blocker kind that lists a coverage reason's sites, or `None` for a
/// reason no blocker lists (a cap, an opaque body, a parse gap, …).
fn blocker_kind_of_coverage_reason(reason: &str) -> Option<&'static str> {
    match reason {
        INTERFACE_OPEN_WORLD => Some("open-world-dispatch"),
        "unresolved-call" | "member-not-found" | "ambiguous-overload" | "dynamic-dispatch" => {
            Some("unresolved-callsite")
        }
        _ => None,
    }
}

/// Round 2 G13: whether every coverage reason is a blocker already listed
/// (so `coverage-incomplete` would count it twice). No reason at all (an
/// unknown coverage record) is not listed.
fn coverage_reasons_listed(reasons: &[String], blockers: &[ProveBlocker]) -> bool {
    !reasons.is_empty()
        && reasons.iter().all(|r| {
            blocker_kind_of_coverage_reason(r).is_some_and(|k| blockers.iter().any(|b| b.kind == k))
        })
}

/// Deterministic blocker sort key: NUL-delimited (kind, anchor file, 10-digit line, detail).
fn blocker_sort_key(b: &ProveBlocker) -> String {
    let file = b
        .anchor
        .as_ref()
        .and_then(|a| a.file.as_deref())
        .unwrap_or("");
    let line = b.anchor.as_ref().and_then(|a| a.line).unwrap_or(0);
    let line_str = format!("{:010}", line);
    format!("{}\x00{}\x00{}\x00{}", b.kind, file, line_str, b.detail)
}

fn sort_blockers(mut blockers: Vec<ProveBlocker>) -> Vec<ProveBlocker> {
    blockers.sort_by(|a, b| {
        let ka = blocker_sort_key(a);
        let kb = blocker_sort_key(b);
        // Ordinal `.cmp()` is correct here: every key component is ASCII (kind literals,
        // normalized ws:/app: paths, zero-padded digits, ASCII details), so Rust's byte
        // ordering == TS's UTF-16 code-unit ordering. This is the engine-wide tracked
        // UTF-16-comparator item — consistent with the rest of the engine.
        ka.cmp(&kb)
    });
    blockers
}

// ---------------------------------------------------------------------------
// Effect predicate matching
// ---------------------------------------------------------------------------

const DB_WRITE_TYPES: &[&str] = &["DB_INSERT", "DB_MODIFY", "DB_DELETE"];
/// What `reaches-ui` counts as UI: INTERACTIVE UI, a dialog or window a user
/// must see or answer (`Message`, `Confirm`, `StrMenu`, `Page`/`Report`
/// `Run`/`RunModal`, `Hyperlink`, a client file dialog, a progress window).
/// `UI_WINDOW_OPEN` was missing (G15), so a routine opening a page answered
/// `no` with full coverage claimed; file dialogs and progress windows had no
/// effect at all (G26), and routines reaching one answered `yes` only through
/// an `Error` on the same path until G18.
///
/// An `Error` is not counted (G18): it is `throws-error`'s question, and as UI
/// it was wrong where it never surfaces, e.g. an `Error('')` caught by a
/// `[TryFunction]`. The two questions no longer overlap.
pub(crate) const UI_TYPES: &[&str] = &[
    "UI_MESSAGE",
    "UI_CONFIRM",
    "UI_WINDOW_OPEN",
    "UI_HYPERLINK",
    "UI_FILE_DIALOG",
    "UI_PROGRESS",
];

/// G40: whether `query` names a real table of the model, by name (case-
/// insensitive) or by number, the two ways [`name_or_id_matches`] matches a
/// write. An extension stub is not a table.
fn names_a_known_table(model: &crate::program::model::workspace::Model, query: &str) -> bool {
    let q = query.trim();
    model.workspace.tables.iter().any(|t| {
        !t.is_extension_stub
            && (t.name.eq_ignore_ascii_case(q) || t.id.rsplit('/').next().is_some_and(|n| n == q))
    })
}

/// Case-insensitive match of `query` against the resourceDisplay or the
/// trailing segment of the resourceId (after the last '/').
fn resource_name_matches(detail: &[(String, String)], query: &str) -> bool {
    let mut resource_display: Option<&str> = None;
    let mut resource_id: Option<&str> = None;

    for (k, v) in detail {
        if k == "resourceDisplay" {
            resource_display = Some(v.as_str());
        }
        if k == "resourceId" {
            resource_id = Some(v.as_str());
        }
    }
    name_or_id_matches(resource_display, resource_id, query)
}

/// [`resource_name_matches`]'s rule over one resource: its display, or the
/// trailing segment of its id, equals `query` case-insensitively.
pub(crate) fn name_or_id_matches(
    resource_display: Option<&str>,
    resource_id: Option<&str>,
    query: &str,
) -> bool {
    let target = query.to_lowercase();
    if let Some(disp) = resource_display
        && disp.to_lowercase() == target
    {
        return true;
    }

    if let Some(rid) = resource_id
        && !rid.is_empty()
    {
        let trailing = match rid.rfind('/') {
            Some(sep) => &rid[sep + 1..],
            None => rid,
        };
        if trailing.to_lowercase() == target {
            return true;
        }
    }

    false
}

/// An effect's resource matches `query`: its own detail, or, for a boundary
/// group, any member of its full resource set (#160 step 9 D2 (6)), whose
/// `detail` lists only the first 10.
fn effect_resource_matches(eff: &DigestEffectFull, query: &str) -> bool {
    resource_name_matches(&eff.detail, query)
        || eff
            .group_resources
            .iter()
            .any(|(id, display)| name_or_id_matches(Some(display), Some(id), query))
}

/// The record temp states a DB effect's `tempState` detail lists (several when
/// its originating operations disagree), e.g. `["known(true)"]`; empty when the
/// detail is absent.
fn temp_states(eff: &DigestEffectFull) -> Vec<&str> {
    crate::engine::l5::rollup::temp_states(&eff.detail)
}

/// G17: the write is to a temporary record on every path: no table is
/// written. W2-1: the shared classifier's `Temporary` class.
fn writes_only_temporary(eff: &DigestEffectFull) -> bool {
    crate::engine::l5::rollup::classify(&temp_states(eff))
        == crate::engine::l5::rollup::WriteClass::Temporary
}

/// G17b: the write is to a record parameter whose temporariness is the
/// caller's (`parameter-dependent(i)`), and to nothing known to be persistent.
/// Asked of the routine itself the answer depends on its callers; a caller's
/// own frame resolves it (known(true) for a temporary argument). W2-1: the
/// shared classifier's `CallerDependent` class.
fn writes_caller_dependent(eff: &DigestEffectFull) -> bool {
    matches!(
        crate::engine::l5::rollup::classify(&temp_states(eff)),
        crate::engine::l5::rollup::WriteClass::CallerDependent(_)
    )
}

/// The `temp-state-depends-on-caller` blocker's text. A write whose every
/// caller-dependent state is the routine's own implicit `Rec`
/// ([`RECEIVER_TEMP_STATE`], G49) names that record: the routine has no
/// parameter a reader could look up.
fn caller_dependent_detail(eff: &DigestEffectFull) -> String {
    let receiver_only = crate::engine::l5::rollup::classify(&temp_states(eff))
        == crate::engine::l5::rollup::WriteClass::CallerDependent(
            crate::engine::l5::rollup::CallerReason::ImplicitRec,
        );
    if receiver_only {
        format!(
            "{} on the implicit Rec: a table write only if the record the routine runs \
             on is not temporary",
            eff.effect_type
        )
    } else {
        format!(
            "{} on a record parameter: a table write only if the caller passes \
             a record that is not temporary",
            eff.effect_type
        )
    }
}

/// A reachable table write that names no table (no resource, not a boundary
/// group, which names its resources) and is not known to write a temporary
/// record. A group whose every resource cannot run (G33) names none any more,
/// and is no unknown write. A group that also holds writes whose table is not
/// known (`unresolvedResources`) is one: its resource list is not all it
/// writes.
pub(crate) fn writes_unknown_table(eff: &DigestEffectFull) -> bool {
    let has = |key: &str| eff.detail.iter().any(|(k, _)| k == key);
    DB_WRITE_TYPES.contains(&eff.effect_type.as_str())
        && !eff.literal_unreachable
        && (!(has("resourceId") || has("boundary")) || has("unresolvedResources"))
        && !writes_only_temporary(eff)
}

/// `writes-table`'s table and write kind, temp state aside.
fn is_table_write(eff: &DigestEffectFull, table: &str) -> bool {
    DB_WRITE_TYPES.contains(&eff.effect_type.as_str()) && effect_resource_matches(eff, table)
}

fn effect_matches(eff: &DigestEffectFull, question: &ProveQuestion) -> bool {
    match question {
        ProveQuestion::MayCommit => eff.effect_type == "COMMIT",
        ProveQuestion::CommitsOnSuccessPath => eff.effect_type == "COMMIT",
        ProveQuestion::WritesTable { table } => {
            // A temporary record is no table write (G17); a caller-dependent
            // one is decided below, as neither a yes nor a no (G17b).
            is_table_write(eff, table)
                && !writes_only_temporary(eff)
                && !writes_caller_dependent(eff)
        }
        ProveQuestion::PublishesEvent { event } => {
            if eff.effect_type != "EVENT_PUBLISH" {
                return false;
            }
            effect_resource_matches(eff, event)
        }
        ProveQuestion::ReachesUi => UI_TYPES.contains(&eff.effect_type.as_str()),
        // An explicit Error, or an implicit runtime error (G20: an unguarded
        // record op in statement position, TestField, FieldError; the rule is
        // `ui_events_error::implicit_error_record_op` / `implicit_error_call`).
        ProveQuestion::ThrowsError => {
            eff.effect_type == "ERROR_THROW" || eff.effect_type == "ERROR_IMPLICIT"
        }
    }
}

// ---------------------------------------------------------------------------
// MAY-question evaluation
// ---------------------------------------------------------------------------

fn evaluate_may_question(
    entry: &DigestEntryFull,
    oblg: ProveObligations,
    question: &ProveQuestion,
    witness_truncated: bool,
    via_callers: &HashMap<usize, CallerRef>,
    field_guarded: &HashMap<usize, bool>,
) -> ProveResult {
    // A caller-dependent write a caller resolves to a real table counts (G25),
    // as does a field-guarded write the asked table has the field of (G36).
    let matching: Vec<usize> = entry
        .effects
        .iter()
        .enumerate()
        // G23: an effect the root cannot reach under literal arguments does not.
        .filter(|(i, e)| {
            (effect_matches(e, question)
                || via_callers.contains_key(i)
                || field_guarded.get(i) == Some(&true))
                && !e.literal_unreachable
        })
        .map(|(i, _)| i)
        .collect();

    if !matching.is_empty() {
        return ProveResult {
            answer: ProveAnswer::Yes,
            evidence: Some(matching),
            blocked_by: None,
            obligations: oblg,
        };
    }

    // G17b: writes whose table is real only if the caller passes a persistent
    // record. They rule out `no`, but they are no `yes` either.
    let caller_dependent: Vec<ProveBlocker> = match question {
        ProveQuestion::WritesTable { table } => entry
            .effects
            .iter()
            .filter(|e| is_table_write(e, table) && writes_caller_dependent(e))
            .map(|e| ProveBlocker {
                kind: "temp-state-depends-on-caller",
                detail: caller_dependent_detail(e),
                anchor: Some(BlockerAnchor {
                    source_kind: e.evidence.source_kind.to_string(),
                    file: e.evidence.file.clone(),
                    line: e.evidence.line,
                    column: e.evidence.column,
                    excerpt: e.evidence.excerpt.clone(),
                }),
            })
            .collect(),
        _ => Vec::new(),
    };
    // A write whose table is not known (a RecordRef opened on a computed
    // number, a record type that did not resolve) may write the asked table:
    // it rules out `no`. It was no match, so the answer was `no`. One whose
    // field guard the asked table cannot pass does not (G36).
    let unknown_table: Vec<ProveBlocker> = match question {
        ProveQuestion::WritesTable { .. } => entry
            .effects
            .iter()
            .enumerate()
            .filter(|(i, e)| writes_unknown_table(e) && field_guarded.get(i) != Some(&false))
            .map(|(_, e)| e)
            .map(|e| ProveBlocker {
                kind: "unknown-table-write",
                detail: format!("{} on a table that is not known", e.effect_type),
                anchor: Some(BlockerAnchor {
                    source_kind: e.evidence.source_kind.to_string(),
                    file: e.evidence.file.clone(),
                    line: e.evidence.line,
                    column: e.evidence.column,
                    excerpt: e.evidence.excerpt.clone(),
                }),
            })
            .collect(),
        _ => Vec::new(),
    };

    if caller_dependent.is_empty()
        && unknown_table.is_empty()
        && obligations_clear(&oblg, witness_truncated)
    {
        return ProveResult {
            answer: ProveAnswer::No,
            evidence: None,
            blocked_by: None,
            obligations: oblg,
        };
    }

    let mut blockers = build_obligation_blockers(entry, &oblg, witness_truncated);
    blockers.extend(caller_dependent);
    blockers.extend(unknown_table);
    ProveResult {
        answer: ProveAnswer::Unknown,
        evidence: None,
        blocked_by: Some(sort_blockers(blockers)),
        obligations: oblg,
    }
}

// ---------------------------------------------------------------------------
// MUST-question evaluation (commits-on-success-path)
// ---------------------------------------------------------------------------

fn evaluate_must_question(
    entry: &DigestEntryFull,
    oblg: ProveObligations,
    witness_truncated: bool,
) -> ProveResult {
    // G23, as for the MAY questions: a COMMIT the root cannot reach under
    // literal arguments is no commit on any path, successful or not.
    let commit_indices: Vec<usize> = entry
        .effects
        .iter()
        .enumerate()
        .filter(|(_, e)| e.effect_type == "COMMIT" && !e.literal_unreachable)
        .map(|(i, _)| i)
        .collect();

    let unconditional_indices: Vec<usize> = commit_indices
        .iter()
        .copied()
        .filter(|&i| entry.effects[i].conditionality == UNCONDITIONAL)
        .collect();

    let non_unconditional_indices: Vec<usize> = commit_indices
        .iter()
        .copied()
        .filter(|&i| entry.effects[i].conditionality != UNCONDITIONAL)
        .collect();

    // YES: ≥1 unconditional COMMIT AND all obligations clear
    if !unconditional_indices.is_empty() && obligations_clear(&oblg, witness_truncated) {
        return ProveResult {
            answer: ProveAnswer::Yes,
            evidence: Some(unconditional_indices),
            blocked_by: None,
            obligations: oblg,
        };
    }

    // NO: zero COMMIT effects of ANY conditionality AND obligations clear
    if commit_indices.is_empty() && obligations_clear(&oblg, witness_truncated) {
        return ProveResult {
            answer: ProveAnswer::No,
            evidence: None,
            blocked_by: None,
            obligations: oblg,
        };
    }

    // UNKNOWN: everything else — obligation blockers + non-unconditional-effect-exists
    let mut blockers: Vec<ProveBlocker> = Vec::new();

    // Obligation blockers first
    let ob = build_obligation_blockers(entry, &oblg, witness_truncated);
    blockers.extend(ob);

    // non-unconditional-effect-exists blockers for each non-unconditional COMMIT
    for idx in &non_unconditional_indices {
        let eff = &entry.effects[*idx];
        // Anchor from effect evidence — full evidence anchor (includes excerpt),
        // kept when sourceKind !== "unavailable" (i.e. BOTH "source" AND "symbol"),
        // mirroring TS prove-engine.ts:386. A COMMIT from a `.app` symbol routine
        // (sourceKind "symbol") still carries its anchor + excerpt.
        let anchor = if eff.evidence.source_kind != "unavailable" {
            Some(BlockerAnchor {
                source_kind: eff.evidence.source_kind.to_string(),
                file: eff.evidence.file.clone(),
                line: eff.evidence.line,
                column: eff.evidence.column,
                excerpt: eff.evidence.excerpt.clone(),
            })
        } else {
            None
        };
        blockers.push(ProveBlocker {
            kind: "non-unconditional-effect-exists",
            detail: format!("COMMIT with conditionality \"{}\"", eff.conditionality),
            anchor,
        });
    }

    let evidence = if !commit_indices.is_empty() {
        Some(commit_indices)
    } else {
        None
    };

    ProveResult {
        answer: ProveAnswer::Unknown,
        evidence,
        blocked_by: Some(sort_blockers(blockers)),
        obligations: oblg,
    }
}

// ---------------------------------------------------------------------------
// prove — main tristate evaluation
// ---------------------------------------------------------------------------

/// The caller through which a caller-dependent write resolves (G25).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerRef {
    pub stable_id: String,
    pub display: String,
}

/// How many caller levels G25 climbs while a write stays caller-dependent.
const CALLER_DEPTH: usize = 6;

/// G25: `writes-table`'s caller-dependent writes (G17b), asked in the frames of
/// the routine's callers in the loaded world. A write that a caller's frame
/// resolves to anything but a temporary record (`known(false)`, `unknown`)
/// is a table write the routine MAY make, through that caller; one that is
/// still `parameter-dependent` there climbs to that caller's callers, up to
/// [`CALLER_DEPTH`] levels. Returns, per effect index of `entry`, the first
/// such caller (callers in model order, breadth first). A write every caller
/// passes a temporary record to, or no caller at all, is not resolved: the
/// routine can still be called from outside the world.
pub(crate) fn resolve_caller_dependent_writes(
    model: &crate::program::model::workspace::Model,
    temp_states: &crate::engine::l5::digest_cli::DbTempStates,
    root_stable: &str,
    entry: &DigestEntryFull,
    question: &ProveQuestion,
) -> HashMap<usize, CallerRef> {
    use crate::engine::l4::effect_lattice::TempStateKind;
    let ProveQuestion::WritesTable { table } = question else {
        return HashMap::new();
    };
    let routines = &model.workspace.routines;
    let stable_of: HashMap<&str, (&str, &str)> = routines
        .iter()
        .map(|r| {
            (
                r.id.as_str(),
                (r.stable_routine_id.as_str(), r.name.as_str()),
            )
        })
        .collect();
    let Some(root_id) = routines
        .iter()
        .find(|r| r.stable_routine_id == root_stable)
        .map(|r| r.id.as_str())
    else {
        return HashMap::new();
    };
    let mut callers_of: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in &model.calls.edges {
        if let Some(to) = e.to.as_deref() {
            callers_of.entry(to).or_default().push(e.from.as_str());
        }
    }
    let mut out = HashMap::new();
    for (i, eff) in entry.effects.iter().enumerate() {
        if !(is_table_write(eff, table) && writes_caller_dependent(eff)) {
            continue;
        }
        let mut visited: HashSet<&str> = HashSet::from([root_id]);
        let mut frontier = vec![root_id];
        'climb: for _ in 0..CALLER_DEPTH {
            let mut next = Vec::new();
            for callee in frontier {
                for &caller in callers_of.get(callee).into_iter().flatten() {
                    if !visited.insert(caller) {
                        continue;
                    }
                    let Some(&(stable, name)) = stable_of.get(caller) else {
                        continue;
                    };
                    let join =
                        temp_states.for_effect(stable, &eff.origin_operation_ids, eff.effect_is_kt);
                    let persistent = join
                        .states
                        .iter()
                        .any(|s| matches!(s, TempStateKind::Known(false) | TempStateKind::Unknown));
                    if persistent {
                        out.insert(
                            i,
                            CallerRef {
                                stable_id: stable.to_string(),
                                display: name.to_string(),
                            },
                        );
                        break 'climb;
                    }
                    if join
                        .states
                        .iter()
                        .any(|s| matches!(s, TempStateKind::ParameterDependent(_)))
                    {
                        next.push(caller);
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
    }
    out
}

pub fn prove(
    entry: &DigestEntryFull,
    gaps_in_cone_count: usize,
    question: &ProveQuestion,
) -> ProveResult {
    prove_with_callers(
        entry,
        gaps_in_cone_count,
        question,
        &HashMap::new(),
        &HashMap::new(),
        World::Strict,
    )
}

/// [`prove`] with the caller-dependent writes resolved through callers
/// ([`resolve_caller_dependent_writes`]): those effects count as matches (G25);
/// `field_guarded` says which field-guarded writes of unknown table may write
/// the asked table and which cannot
/// ([`crate::engine::l5::field_guard::resolve_field_guarded_writes`], G36);
/// `world` waives the open-world rows [`world_waives`] (G24, G29).
///
/// An answer is labelled with the WEAKEST world, up to `world`, whose answer
/// DIFFERS from the strict one: it holds only under that world's assumption
/// ([`World::label`]). A decided answer the strict world gives too (a `yes`
/// with its witness) rests on no world's assumption: the strict result,
/// unlabelled here. When every world stays `unknown`, the result of `world`,
/// unlabelled: its blockers are what is left once the assumption is made, so
/// a verifier sees what to fix next.
pub fn prove_with_callers(
    entry: &DigestEntryFull,
    gaps_in_cone_count: usize,
    question: &ProveQuestion,
    via_callers: &HashMap<usize, CallerRef>,
    field_guarded: &HashMap<usize, bool>,
    world: World,
) -> ProveResult {
    let run = |w| {
        prove_in_world(
            entry,
            gaps_in_cone_count,
            question,
            via_callers,
            field_guarded,
            w,
        )
    };
    let strict = run(World::Strict);
    let mut last = None;
    for w in [World::LoadedSubscribers, World::Closed] {
        if w > world {
            break;
        }
        let mut assumed = run(w);
        if assumed.answer != strict.answer {
            assumed.obligations.answer_relies_on_world = true;
            return assumed;
        }
        last = Some(assumed);
    }
    match last {
        Some(assumed) if assumed.answer == ProveAnswer::Unknown => assumed,
        _ => strict,
    }
}

fn prove_in_world(
    entry: &DigestEntryFull,
    gaps_in_cone_count: usize,
    question: &ProveQuestion,
    via_callers: &HashMap<usize, CallerRef>,
    field_guarded: &HashMap<usize, bool>,
    world: World,
) -> ProveResult {
    let oblg = compute_obligations(entry, gaps_in_cone_count, world);

    // G22: a truncated witness caps how many via paths an effect lists, and an
    // unavailable evidence anchor loses where it sits; the effect itself stays
    // in `entry.effects` either way. A MAY question decides on which effects
    // exist, so neither can hide a match: a `no` stands. The MUST question
    // judges "unconditional" over a COMMIT's paths, so a COMMIT whose paths
    // were cut short still blocks it. (It blocked every question on any
    // effect's truncation: 49 of Continia DO's undecided claims.)
    match question {
        ProveQuestion::CommitsOnSuccessPath => {
            let commit_paths_truncated = entry
                .effects
                .iter()
                .any(|e| e.effect_type == "COMMIT" && e.via_paths_truncated);
            evaluate_must_question(entry, oblg, commit_paths_truncated)
        }
        ProveQuestion::WritesTable { table } => {
            evaluate_writes_table(entry, oblg, table, via_callers, field_guarded)
        }
        _ => evaluate_may_question(entry, oblg, question, false, via_callers, field_guarded),
    }
}

/// MCP output wave 2, W2-1 (design §3.2.1): THE answer to "does the root
/// write table `table`", shared by every reader that states a table-specific
/// answer: the write classification (`rollup::classify`) plus field-guarded
/// matches (G36), caller resolution (`via_callers`, G25) and the complete
/// obligation rules. A field-guarded write on a table not resolved can make it
/// `yes`; an absent match can be `unknown` on ordinary obligations alone.
pub(crate) fn evaluate_writes_table(
    entry: &DigestEntryFull,
    oblg: ProveObligations,
    table: &str,
    via_callers: &HashMap<usize, CallerRef>,
    field_guarded: &HashMap<usize, bool>,
) -> ProveResult {
    let question = ProveQuestion::WritesTable {
        table: table.to_string(),
    };
    evaluate_may_question(entry, oblg, &question, false, via_callers, field_guarded)
}

/// Everything "does the root write table T" reads (design §3.2.1, R3a; the
/// W2-2 gap (a)): the root's passed and joined entry, the L4 temp states for
/// the caller-frame question, the cone's gap count and table-event bit, the
/// world. Prove's `writes-table`, explain's and digest's `--writes-table` all
/// answer through [`TableQuestions::answer`], so the three tools give ONE
/// answer by construction (T-W2-16 leg 2).
pub(crate) struct TableQuestions<'a> {
    pub model: &'a crate::program::model::workspace::Model,
    pub temp_states: &'a crate::engine::l5::digest_cli::DbTempStates<'a>,
    pub root: &'a str,
    pub entry: &'a DigestEntryFull,
    pub gaps_in_cone_count: usize,
    pub fires_table_events: bool,
    pub world: World,
}

/// One table's answer and the caller-resolved writes its evidence cites.
pub(crate) struct TableAnswer {
    pub result: ProveResult,
    pub via_callers: HashMap<usize, CallerRef>,
}

impl TableQuestions<'_> {
    /// `writes-table:<table>`: `Err` for a table the program does not know
    /// (G40: matching no write would read as a proven `no`).
    pub(crate) fn answer(&self, table: &str) -> Result<TableAnswer, String> {
        if !names_a_known_table(self.model, table) {
            return Err(format!(
                "unknown table '{table}': no table of the program has that name or number"
            ));
        }
        let question = ProveQuestion::WritesTable {
            table: table.to_string(),
        };
        // G25: caller-dependent writes asked in the callers' frames.
        let via_callers = resolve_caller_dependent_writes(
            self.model,
            self.temp_states,
            self.root,
            self.entry,
            &question,
        );
        // G36: field-guarded RecordRef writes, judged against the table.
        let field_guarded = crate::engine::l5::field_guard::resolve_field_guarded_writes(
            self.model,
            crate::engine::l5::digest_cli::primary_guid_of(self.model).unwrap_or(""),
            self.entry,
            table,
        );
        let mut result = prove_with_callers(
            self.entry,
            self.gaps_in_cone_count,
            &question,
            &via_callers,
            &field_guarded,
            self.world,
        );
        result.obligations.relies_on_loaded_subscribers =
            relies_on_loaded_subscribers(&result, self.entry, self.fires_table_events);
        Ok(TableAnswer {
            result,
            via_callers,
        })
    }
}

/// A table answer as explain and digest print it in JSON: the table, prove's
/// `answer`, `blockedBy` and `assumptions` in prove's own serialization, and
/// how many evidence rows it cites.
pub(crate) fn table_answer_value(table: &str, a: &TableAnswer) -> serde_json::Value {
    let r = &a.result;
    let mut v = serde_json::json!({
        "table": table,
        "answer": r.answer.as_str(),
        "evidence": r.evidence.as_ref().map_or(0, Vec::len),
        "blockedBy": r.blocked_by.as_ref().map_or_else(Vec::new, |b| b.iter().map(blocker_to_value).collect()),
    });
    if let Some(label) = r.obligations.assumption() {
        v["assumptions"] = serde_json::json!([label]);
    }
    v
}

/// Per root stable id: the JSON values and human lines of its
/// `--writes-table` answers (explain and digest, W2-2 gap (a)).
pub(crate) type TableAnswers = HashMap<String, (Vec<serde_json::Value>, Vec<String>)>;

/// Every `tables` answer for every root of `entries`, in the default world,
/// through [`TableQuestions::answer`]. `cones`: per root, its cone's gap
/// count and table-event bit ([`cone_facts`], read before the entries are
/// joined, which takes the cones).
pub(crate) fn cone_facts(
    cones: &BTreeMap<String, crate::engine::l5::digest_cli::UnresolvedCone>,
) -> HashMap<String, (usize, bool)> {
    cones
        .iter()
        .map(|(r, c)| {
            (
                r.clone(),
                (
                    c.gaps_in_cone
                        .expect("a --writes-table plan counts the gaps in the cone"),
                    c.fires_table_events
                        .expect("a --writes-table plan reads the cone's table events"),
                ),
            )
        })
        .collect()
}

/// [`table_answers`] needs [`cone_facts`] of a plan through
/// `session::counting_cone_gaps`.
pub(crate) fn table_answers(
    model: &crate::program::model::workspace::Model,
    temp_states: &crate::engine::l5::digest_cli::DbTempStates<'_>,
    entries: &[DigestEntryFull],
    cones: &HashMap<String, (usize, bool)>,
    tables: &[String],
) -> TableAnswers {
    let mut out = TableAnswers::new();
    if tables.is_empty() {
        return out;
    }
    for entry in entries {
        let &(gaps_in_cone_count, fires_table_events) = cones
            .get(&entry.routine_id)
            .expect("a --writes-table plan counts each root's cone");
        let q = TableQuestions {
            model,
            temp_states,
            root: &entry.routine_id,
            entry,
            gaps_in_cone_count,
            fires_table_events,
            world: World::of_flags(false),
        };
        let (mut values, mut lines) = (Vec::new(), Vec::new());
        for t in tables {
            match q.answer(t) {
                Ok(a) => {
                    values.push(table_answer_value(t, &a));
                    lines.push(table_answer_human(t, &a));
                }
                Err(e) => {
                    values.push(serde_json::json!({"table": t, "error": e}));
                    lines.push(format!("writes-table:{t}: {e}"));
                }
            }
        }
        out.insert(entry.routine_id.clone(), (values, lines));
    }
    out
}

/// The human line of a table answer.
pub(crate) fn table_answer_human(table: &str, a: &TableAnswer) -> String {
    let r = &a.result;
    let mut s = format!("writes-table:{table}: {}", r.answer.as_str());
    let n = r.evidence.as_ref().map_or(0, Vec::len);
    if n > 0 {
        s.push_str(&format!(" ({n} evidence)"));
    }
    if let Some(b) = r.blocked_by.as_ref().filter(|b| !b.is_empty()) {
        s.push_str(&format!(" (blocked by {})", b.len()));
    }
    if let Some(label) = r.obligations.assumption() {
        s.push_str(&format!("; assumes {label}"));
    }
    s.push_str(" — the answer alsem prove gives");
    s
}

// ---------------------------------------------------------------------------
// JSON anchor helper
// ---------------------------------------------------------------------------

fn anchor_to_json(
    source_kind: &str,
    file: Option<&str>,
    line: Option<u32>,
    column: Option<u32>,
    excerpt: Option<&str>,
) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    m.insert("sourceKind".into(), source_kind.into());
    if let Some(f) = file {
        m.insert("file".into(), f.into());
    }
    // The model's 0-based row, written 1-based (`lineBase: 1`).
    if let Some(l) = shown_opt(line) {
        m.insert("line".into(), l.into());
    }
    if let Some(c) = column {
        m.insert("column".into(), c.into());
    }
    if let Some(x) = excerpt {
        m.insert("excerpt".into(), x.into());
    }
    serde_json::Value::Object(m)
}

pub(crate) fn blocker_to_value(b: &ProveBlocker) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    if let Some(ref a) = b.anchor {
        let anch = anchor_to_json(
            &a.source_kind,
            a.file.as_deref(),
            a.line,
            a.column,
            a.excerpt.as_deref(),
        );
        m.insert("anchor".into(), anch);
    }
    m.insert("detail".into(), b.detail.clone().into());
    m.insert("kind".into(), b.kind.into());
    serde_json::Value::Object(m)
}

fn obligations_to_value(o: &ProveObligations) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    m.insert("analysisGaps".into(), (o.analysis_gaps as u64).into());
    m.insert("coneTruncated".into(), o.cone_truncated.into());
    m.insert("coverageComplete".into(), o.coverage_complete.into());
    m.insert("openWorldRows".into(), (o.open_world_rows as u64).into());
    m.insert(
        "unresolvedCallsites".into(),
        (o.unresolved_callsites as u64).into(),
    );
    // G24: under `--closed-world` only. G29: the default world names its
    // waived rows only when it waived one, so a document no publisher-scoped
    // site touched is unchanged.
    let waived_key = match o.world {
        World::Strict => None,
        World::LoadedSubscribers => (o.waived_rows > 0).then_some("loadedSubscribersRows"),
        World::Closed => Some("closedWorldRows"),
    };
    if let Some(key) = waived_key {
        m.insert(key.into(), (o.waived_rows as u64).into());
    }
    serde_json::Value::Object(m)
}

// ---------------------------------------------------------------------------
// project_prove_document
// ---------------------------------------------------------------------------

pub struct ProveDocumentArgs<'a> {
    pub workspace_fp: &'a str,
    pub routine_stable_id: &'a str,
    pub routine_display: &'a str,
    pub object_display: &'a str,
    pub routine_anchor: Option<&'a FullAnchor>,
    pub question: &'a str,
    pub prove_result: &'a ProveResult,
    pub effects: &'a [DigestEffectFull],
    /// Evidence effects resolved through a caller (G25): their evidence entry
    /// carries `viaCaller` ({stableId, display}).
    pub via_callers: &'a HashMap<usize, CallerRef>,
    pub diagnostics_json: serde_json::Value,
    pub alsem_ver: &'a str,
    pub deterministic: bool,
    /// W2-8: where the obligations point (`None` for the no-root document).
    pub pointers: Option<&'a ObligationPointers>,
    /// Agent-tasks E1: a filtered question's slice. Its matching sites are
    /// the evidence; `payload.filter` and `payload.slice` are added and the
    /// document is [`PROVE_SLICE_SCHEMA_VERSION`]. `None`: the document is
    /// byte-identical to an unfiltered one.
    pub slice: Option<&'a crate::engine::l5::prove_slice::SliceOut>,
    /// V3 §2.1 (2.6.0): the root's one display and identity fields, added to
    /// `payload.routine` (`qualifiedDisplay`, `signature`, `member`,
    /// `selector`, `handle`). Empty for the no-root document.
    pub routine_identity: serde_json::Map<String, serde_json::Value>,
    /// V3 §2.3 (2.6.0): the question's target as resolved,
    /// `payload.result.table` / `payload.result.event` (and `tables` when a
    /// name or number names several).
    pub resolved_target: serde_json::Map<String, serde_json::Value>,
}

/// W2-0b: a group path's structured witness (`groupWitness`): per hop the
/// edge kind, its site id, the routine entered (stable id and qualified
/// display) and where the edge leaves its calling routine (1-based line);
/// then the terminal operation.
pub(crate) fn group_witness_value(
    path: &crate::engine::l5::group_path::GroupPath,
) -> serde_json::Value {
    let line = |l: Option<u32>| l.map(crate::engine::line_base::shown);
    let hops: Vec<serde_json::Value> = path
        .steps
        .iter()
        .map(|s| {
            serde_json::json!({
                "edge": s.edge,
                "edgeId": s.edge_id,
                "to": s.to,
                "display": s.display,
                "file": s.file,
                "line": line(s.line),
            })
        })
        .collect();
    serde_json::json!({
        "hops": hops,
        "terminal": {
            "operationId": path.terminal.operation_id,
            "file": path.terminal.file,
            "line": line(path.terminal.line),
        },
    })
}

/// The target of a `writes-table` or `publishes-event` question as resolved
/// (output validation V3, §2.3): the JSON fields for `payload.result`
/// (`table: {id, name, number, app, appId}`, plus `tables` when the name or
/// number names more than one; `event: {name, kind, publishers}`) and the
/// The tables a `writes-table` name or number names (every app's, by id,
/// once): prove's `payload.result.table` is the first.
fn table_hits<'m>(
    model: &'m crate::program::model::workspace::Model,
    q: &str,
) -> Vec<&'m crate::program::model::workspace::ModelTable> {
    let mut hits: Vec<&crate::program::model::workspace::ModelTable> = model
        .workspace
        .tables
        .iter()
        .filter(|t| {
            !t.is_extension_stub
                && (t.name.eq_ignore_ascii_case(q)
                    || t.id.rsplit('/').next().is_some_and(|n| n == q))
        })
        .collect();
    hits.sort_by(|a, b| a.id.cmp(&b.id));
    hits.dedup_by(|a, b| a.id == b.id);
    hits
}

/// A resolved table as prove's `payload.result.table` writes it: `{id,
/// name, number, app, appId}`.
fn table_row_json(
    cx: &crate::engine::l5::routine_rollup::RollupCx<'_>,
    t: &crate::program::model::workspace::ModelTable,
) -> serde_json::Value {
    serde_json::json!({
        "id": t.id,
        "name": t.name,
        "number": t.table_number,
        "app": cx.app_label(&t.app_guid),
        "appId": t.app_guid,
    })
}

/// A `writes-table` input's resolved table in prove's own shape (round-2
/// G42: digest's `tableAnswers[].tableRef`), or `null` when it names no
/// table.
pub(crate) fn table_ref(
    model: &crate::program::model::workspace::Model,
    cx: &crate::engine::l5::routine_rollup::RollupCx<'_>,
    table: &str,
) -> serde_json::Value {
    table_hits(model, table.trim())
        .first()
        .map_or(serde_json::Value::Null, |t| table_row_json(cx, t))
}

/// question as the human answer line prints it
/// (`writes-table:CDO E-Mail Job (table 6175283, this app)`). Any other
/// question: no field, the question as typed.
fn resolved_target(
    model: &crate::program::model::workspace::Model,
    cx: &crate::engine::l5::routine_rollup::RollupCx<'_>,
    question: &ProveQuestion,
    question_text: &str,
) -> (serde_json::Map<String, serde_json::Value>, String) {
    use serde_json::json;
    let mut out = serde_json::Map::new();
    let primary = model.primary_app.as_ref().map(|p| p.app_guid.as_str());
    let app_words = |guid: &str| {
        if primary.is_some_and(|p| p.eq_ignore_ascii_case(guid)) {
            "this app".to_string()
        } else {
            cx.app_label(guid)
        }
    };
    match question {
        ProveQuestion::WritesTable { table } => {
            let q = table.trim();
            let hits = table_hits(model, q);
            let row = |t: &crate::program::model::workspace::ModelTable| table_row_json(cx, t);
            let Some(first) = hits.first() else {
                return (out, question_text.to_string());
            };
            out.insert("table".into(), row(first));
            let human = if hits.len() == 1 {
                format!(
                    "writes-table:{} (table {}, {})",
                    first.name,
                    first.table_number,
                    app_words(&first.app_guid)
                )
            } else {
                out.insert("tables".into(), hits.iter().map(|t| row(t)).collect());
                let each: Vec<String> = hits
                    .iter()
                    .map(|t| {
                        format!(
                            "{} (table {}, {})",
                            t.name,
                            t.table_number,
                            app_words(&t.app_guid)
                        )
                    })
                    .collect();
                format!(
                    "writes-table:{q} ({} tables: {})",
                    hits.len(),
                    each.join("; ")
                )
            };
            (out, human)
        }
        ProveQuestion::PublishesEvent { event } => {
            let q = event.trim();
            let mut name: Option<&str> = None;
            let mut kind: Option<&str> = None;
            let mut publishers: Vec<(String, String)> = Vec::new();
            for e in &model.events.graph.events {
                // A symbol synthesized from a subscription whose publisher
                // the program does not hold (`unknown/…`) names no publisher.
                if !e.event_name.eq_ignore_ascii_case(q)
                    || e.publisher_object_id.starts_with("unknown")
                {
                    continue;
                }
                name.get_or_insert(e.event_name.as_str());
                kind.get_or_insert(e.event_kind.as_str());
                let (id, display) = match &e.publisher_stable_routine_id {
                    Some(s) => (s.clone(), cx.qualified(s)),
                    None => (
                        e.publisher_object_id.clone(),
                        cx.object_display(&e.publisher_object_id),
                    ),
                };
                if !publishers.iter().any(|(i, _)| *i == id) {
                    publishers.push((id, display));
                }
            }
            let Some(name) = name else {
                out.insert(
                    "event".into(),
                    json!({"name": q, "kind": null, "publishers": []}),
                );
                return (
                    out,
                    format!(
                        "publishes-event:{q} (no loaded publisher declares it; only subscriptions \
                         name it)"
                    ),
                );
            };
            publishers.sort();
            out.insert(
                "event".into(),
                json!({
                    "name": name,
                    "kind": kind,
                    "publishers": publishers
                        .iter()
                        .map(|(id, d)| json!({"stableId": id, "display": d}))
                        .collect::<Vec<_>>(),
                }),
            );
            const SHOWN: usize = 3;
            let mut by: Vec<String> = publishers
                .iter()
                .take(SHOWN)
                .map(|(_, d)| d.clone())
                .collect();
            if publishers.len() > SHOWN {
                by.push(format!("+{} more", publishers.len() - SHOWN));
            }
            (
                out,
                format!("publishes-event:{name} (published by {})", by.join(", ")),
            )
        }
        _ => (out, question_text.to_string()),
    }
}

/// prove's document `schemaVersion`. 2.10.0 (output validation V4,
/// additive): `payload.result.scope` (F76) and `payload.result.followUps`
/// (§6.1). 2.11.0 (round-2 output validation W4, additive): `volatile`
/// (G44).
pub const PROVE_SCHEMA_VERSION: &str = "2.11.0";

/// A filtered prove document's `schemaVersion` (agent-tasks E1, additive:
/// `payload.filter`, `payload.slice`, slice-shaped evidence rows). An
/// unfiltered document keeps [`PROVE_SCHEMA_VERSION`], byte for byte (E-5).
/// 2.11.0: V4's `result.scope` and `result.followUps`, as unfiltered.
/// 2.12.0: `volatile` (round-2 G44), as unfiltered.
pub const PROVE_SLICE_SCHEMA_VERSION: &str = "2.12.0";

/// Build the DocumentEnvelope<"prove", ProvePayload> as a serde_json::Value,
/// then serialize with serialize_document_value (sorted keys, null-drop, trailing \n).
pub fn project_prove_document(args: ProveDocumentArgs<'_>) -> String {
    let ProveDocumentArgs {
        workspace_fp,
        routine_stable_id,
        routine_display,
        object_display,
        routine_anchor,
        question,
        prove_result,
        effects,
        via_callers,
        diagnostics_json,
        alsem_ver,
        deterministic,
        pointers,
        slice,
        routine_identity,
        resolved_target,
    } = args;

    // result sub-object
    let mut result_obj = serde_json::Map::new();
    result_obj.insert("answer".into(), prove_result.answer.as_str().into());
    result_obj.extend(resolved_target);

    // evidence — only when present (and non-empty)
    if let Some(ref indices) = prove_result.evidence
        && !indices.is_empty()
    {
        // Prove's own serializer (W2-2, design §4.7): the 2.x effect shape
        // ConTinAI's verify.py reads; a write resolved through a caller says
        // which (G25).
        let ev: Vec<serde_json::Value> = indices
            .iter()
            .map(|&i| {
                let mut v = evidence_value(&effects[i]);
                if let (Some(c), Some(obj)) = (via_callers.get(&i), v.as_object_mut()) {
                    obj.insert(
                        "viaCaller".into(),
                        serde_json::json!({"stableId": c.stable_id, "display": c.display}),
                    );
                }
                // G35: a group matched on the asked table shows how its code
                // reaches that table.
                if let (Some(ProveQuestion::WritesTable { table }), Some(obj)) =
                    (parse_question(question), v.as_object_mut())
                {
                    let eff = &effects[i];
                    let hit = eff.group_resource_paths.iter().find_map(|(id, path)| {
                        let display = eff
                            .group_resources
                            .iter()
                            .find(|(r, _)| r == id)
                            .map(|(_, d)| d.as_str());
                        name_or_id_matches(display, Some(id), &table).then_some((display, path))
                    });
                    match hit {
                        Some((display, Ok(path))) => {
                            obj.insert(
                                "groupPath".into(),
                                serde_json::json!({"resource": display.unwrap_or_default(), "hops": path.hops}),
                            );
                            // W2-0b (prove 2.2.0, additive): the crossing,
                            // whether the path traverses it, and the
                            // structured witness beside the string hops.
                            obj.insert("crossing".into(), path.crossing.clone().into());
                            obj.insert("throughEntryPoint".into(), path.through_entry_point.into());
                            if let Some(reason) = path.reason {
                                obj.insert("throughEntryPointReason".into(), reason.into());
                            }
                            obj.insert("groupWitness".into(), group_witness_value(path));
                        }
                        // W2-0b: no path is invented; the reason is stated.
                        Some((_, Err(none))) => {
                            obj.insert("crossing".into(), none.crossing.clone().into());
                            obj.insert("groupPathUnavailable".into(), none.reason.into());
                        }
                        None => {}
                    }
                }
                // W2-6 (2f, prove 2.3.0): a resource-less group's terminal
                // witness, for any question that cites it.
                if let (Some(w), Some(obj)) = (&effects[i].group_terminal_path, v.as_object_mut())
                {
                    match w {
                        Ok(path) => {
                            obj.insert(
                                "groupPath".into(),
                                serde_json::json!({"hops": path.hops}),
                            );
                            obj.insert("crossing".into(), path.crossing.clone().into());
                            obj.insert("throughEntryPoint".into(), path.through_entry_point.into());
                            if let Some(reason) = path.reason {
                                obj.insert("throughEntryPointReason".into(), reason.into());
                            }
                            obj.insert("groupWitness".into(), group_witness_value(path));
                        }
                        Err(none) => {
                            obj.insert("crossing".into(), none.crossing.clone().into());
                            obj.insert("groupPathUnavailable".into(), none.reason.into());
                        }
                    }
                }
                v
            })
            .collect();
        result_obj.insert("evidence".into(), serde_json::Value::Array(ev));
    }
    // Agent-tasks E1: a filtered answer's evidence is its matching sites.
    if let Some(s) = slice {
        let ev: Vec<serde_json::Value> = s
            .matching()
            .map(crate::engine::l5::prove_slice::site_value)
            .collect();
        if !ev.is_empty() {
            result_obj.insert("evidence".into(), serde_json::Value::Array(ev));
        }
        // Output validation V2 (§4.3, additive): the filter is part of the
        // verdict, and the other reachable sites are listed beside it.
        result_obj.insert("filter".into(), s.request.to_value());
        result_obj.insert(
            "otherSites".into(),
            crate::engine::l5::prove_slice::other_sites_value(s),
        );
    }

    // blockedBy — only when present (and non-empty)
    if let Some(ref blockers) = prove_result.blocked_by
        && !blockers.is_empty()
    {
        let bv: Vec<serde_json::Value> = blockers.iter().map(blocker_to_value).collect();
        result_obj.insert("blockedBy".into(), serde_json::Value::Array(bv));
    }

    let mut obligations = obligations_to_value(&prove_result.obligations);
    // W2-8 (2h, prove 2.3.0, additive): where the obligations point.
    if let (Some(m), Some(serde_json::Value::Object(o))) = (
        obligations.as_object_mut(),
        pointers.map(ObligationPointers::to_value),
    ) {
        m.extend(o);
    }
    result_obj.insert("obligations".into(), obligations);
    // G24/G29: the label a KB needs for an answer that holds only in a world
    // beyond the open one.
    let o = &prove_result.obligations;
    if let Some(label) = o.assumption() {
        result_obj.insert("assumptions".into(), serde_json::json!([label]));
    }

    // routine ref
    let mut routine_ref = serde_json::Map::new();
    if let Some(anch) = routine_anchor {
        let anch_val = anchor_to_json(
            anch.source_kind,
            anch.file.as_deref(),
            anch.line,
            anch.column,
            anch.excerpt.as_deref(),
        );
        routine_ref.insert("anchor".into(), anch_val);
    }
    routine_ref.insert("display".into(), routine_display.into());
    routine_ref.insert("objectDisplay".into(), object_display.into());
    routine_ref.insert("stableId".into(), routine_stable_id.into());
    routine_ref.extend(routine_identity);

    // payload
    let mut payload = serde_json::Map::new();
    payload.insert("question".into(), question.into());
    // G13 / MCP output plan 1a: every `line` in this document (anchors,
    // evidence, via-path hops) is 1-based, as `alsem graph`'s are.
    payload.insert(
        "lineBase".into(),
        crate::engine::line_base::LINE_BASE.into(),
    );
    payload.insert("result".into(), serde_json::Value::Object(result_obj));
    payload.insert("routine".into(), serde_json::Value::Object(routine_ref));
    payload.insert("workspaceFingerprint".into(), workspace_fp.into());
    if let Some(s) = slice {
        payload.insert("filter".into(), s.request.to_value());
        payload.insert(
            "slice".into(),
            crate::engine::l5::prove_slice::slice_value(s),
        );
    }

    // envelope
    let generated_at = crate::engine::gate::format_json::pinned_or_now_iso8601(deterministic);

    let mut env = serde_json::Map::new();
    env.insert("alsemVersion".into(), alsem_ver.into());
    env.insert("deterministic".into(), deterministic.into());
    env.insert("diagnostics".into(), diagnostics_json);
    env.insert("generatedAt".into(), generated_at.into());
    env.insert("kind".into(), "prove".into());
    env.insert("payload".into(), serde_json::Value::Object(payload));
    env.insert(
        "volatile".into(),
        crate::engine::result_header::volatile(deterministic),
    );
    // 1.4.0: effects' `detail.tempState` is digest's per-operation, class-
    // filtered join (#69 D4).
    // 2.0.0 (MCP output plan 1a): every `line` is 1-based (`lineBase: 1`).
    // 2.1.0 (plan 1d): evidence effects carry digest 2.1.0's group fields.
    // 2.2.0 (wave 2, W2-0b, additive): a writes-table group match's
    // `groupPath` traverses its crossing, and carries `crossing`,
    // `throughEntryPoint` and the structured `groupWitness` beside it.
    // 2.3.0 (W2-6, additive): a resource-less group's evidence (COMMIT,
    // HTTP, UI, …) carries the same fields for its own terminal witness.
    // 2.4.0 (agent tasks A, additive): the callers' context questions
    // (`callers-have-pending-writes`, `callers-intercept-errors`,
    // `callers-may-run-noninteractive`, `payload.questionScope:
    // "callers-context"`); the existing-effects questions' JSON is unchanged.
    // 2.5.0 (agent-tasks E1, additive, filtered documents only): `payload.
    // filter`, `payload.slice`, and evidence rows that are the slice's sites
    // (`evidence.{file, line}` kept); an unfiltered document stays 2.4.0.
    // 2.6.0 unfiltered / 2.7.0 filtered (output validation V3, additive; the
    // spec's "2.5.0" was already the filtered number): `payload.routine`
    // gains `qualifiedDisplay` (the one display), `signature`, `member`,
    // `selector` and `handle`; `payload.result.table` (and `tables`) for
    // `writes-table`, `payload.result.event` for `publishes-event`.
    // 2.8.0 unfiltered / 2.9.0 filtered (output validation V2, additive;
    // the spec's "2.5.0" and "2.6.0" were taken by E1 and V3): the callers'
    // context result gains `establishedWitnesses`, `candidateWitnesses`
    // (`{count, byTier}`) and each evidence row `pathTier` (the §8.1 word
    // beside the legacy `tier`); a filtered result gains `result.filter`
    // and `result.otherSites[]`.
    // 2.11.0 unfiltered / 2.12.0 filtered (round-2 output validation W4,
    // additive): `volatile` beside `deterministic` (G44); a routine's
    // `objectDisplay` and the resolved table's display names go through the
    // one formatter (G41).
    let version = if slice.is_some() {
        PROVE_SLICE_SCHEMA_VERSION
    } else {
        PROVE_SCHEMA_VERSION
    };
    env.insert("schemaVersion".into(), version.into());

    // §9.8 (F78): every location says what it is.
    let mut env = serde_json::Value::Object(env);
    crate::engine::result_header::tag_location_roles(&mut env);
    serialize_document_value(env)
}

// ---------------------------------------------------------------------------
// Human formatter (formatProveHuman)
// ---------------------------------------------------------------------------

/// What the human answer needs to print paths (W2-6): the model joins and
/// the root's stable id. `None` for the no-root document.
pub struct HumanPaths<'c, 'm> {
    pub cx: &'c crate::engine::l5::routine_rollup::RollupCx<'m>,
    pub root: &'c str,
    /// W2-8: where the obligations point.
    pub pointers: &'c ObligationPointers,
    /// `--detail`: every blocker, not the first few per kind.
    pub detail: bool,
    /// `--surface` and `--single-app` (output validation §6.1): the words and
    /// scope of every follow-up.
    pub fcx: crate::engine::follow_up::Cx,
}

impl HumanPaths<'_, '_> {
    /// The root's selector (the follow-ups' `routine`).
    fn selector(&self) -> String {
        self.cx.names.selector(self.root)
    }
}

/// `detail` in `surface`'s words, for prove's fold notes.
fn detail_words(fcx: Option<crate::engine::follow_up::Cx>) -> &'static str {
    crate::engine::follow_up::input(
        fcx.map(|c| c.surface).unwrap_or_default(),
        crate::engine::follow_up::Input::Detail,
    )
}

/// What prove's four header lines need beyond the answer's own lines
/// (output validation §5.2).
pub(crate) struct ProveHeaderCx {
    pub fcx: crate::engine::follow_up::Cx,
    /// The loaded dependencies (`dependencies: N apps`).
    pub deps: usize,
    pub detail: bool,
    /// The root's selector, for the follow-ups (`None`: no root resolved).
    pub selector: Option<String>,
    pub question: String,
    /// No routine resolved: `answer: no answer — <why>`, `not analyzed`.
    pub no_answer: Option<String>,
}

/// A prove answer as the header's four lines and its body (output
/// validation §5.2): the renderers (existing effects, filtered, callers'
/// context) print their routine line, `question:`, `answer:`, analysis,
/// `assumptions:` and `limits:` lines; those become the header, every other
/// line stays in the body, in order. The routine line is `--- <display> ---`.
/// Returns the text and the header it printed (`None`: the text had no routine
/// line), for `result.scope`.
pub(crate) fn header_and_body(
    text: &str,
    hc: &ProveHeaderCx,
) -> (String, Option<crate::engine::result_header::Header>) {
    use crate::engine::follow_up::{FollowUp, Input, Tool};
    use crate::engine::result_header::{Analysis, Header, dependencies_clause};
    let mut routine = String::new();
    let mut question = hc.question.clone();
    let mut scope_words = String::new();
    let mut answer = String::new();
    let mut analysis = Analysis::Complete;
    let mut assumptions: Vec<String> = Vec::new();
    let mut limits: Vec<String> = Vec::new();
    let mut body: Vec<String> = Vec::new();
    let mut gaps_follow_up = false;
    let mut folded = false;
    let mut tops: Vec<String> = Vec::new();
    for line in text.lines() {
        if let Some(r) = line
            .strip_prefix("--- ")
            .and_then(|l| l.strip_suffix(" ---"))
            .filter(|_| routine.is_empty())
        {
            routine = r.to_string();
        } else if let Some(q) = line.strip_prefix("  question: ") {
            match q.split_once(" — ") {
                Some((q, s)) => {
                    question = q.to_string();
                    scope_words = s.to_string();
                }
                None => question = q.to_string(),
            }
        } else if let Some(a) = line.strip_prefix("  answer:   ") {
            answer = a.to_string();
        } else if let Some(rest) = line
            .strip_prefix("  analysis complete")
            .map(|r| (true, r))
            .or_else(|| line.strip_prefix("  analysis partial").map(|r| (false, r)))
        {
            let (complete, rest) = rest;
            let mut parts = rest.split("; ");
            let first = parts.next().unwrap_or("");
            let reasons: Vec<String> = first
                .strip_prefix(" — ")
                .map(|p| vec![p.to_string()])
                .unwrap_or_default();
            analysis = Analysis::of(complete, reasons);
            for p in parts {
                if let Some(top) = p.strip_prefix("top: ") {
                    tops.push(format!("  top callees not resolved: {top}"));
                } else if let Some(top) = p.strip_prefix("top open world: ") {
                    tops.push(format!("  top interface dispatch (open world): {top}"));
                } else if p.starts_with("alsem explain lists") {
                    gaps_follow_up = true;
                }
            }
        } else if let Some(a) = line.strip_prefix("  assumptions: ") {
            assumptions.push(a.to_string());
        } else if let Some(l) = line.strip_prefix("  limits: ") {
            if l != "none" {
                limits.extend(l.split("; ").map(str::to_string));
            }
        } else {
            // Round-2 G25: the header counts the stopped paths; the
            // `stopped:` line below says where (one copy of the line).
            if let Some(s) = line.strip_prefix("  stopped: ") {
                let n: usize = s
                    .split("; ")
                    .filter_map(|p| p.split(' ').next()?.replace(',', "").parse::<usize>().ok())
                    .sum();
                let paths = if n == 1 { "path" } else { "paths" };
                analysis = analysis.and(Analysis::Partial(vec![format!(
                    "{} {paths} stopped; stopped: says where",
                    crate::engine::l5::routine_rollup::thousands(n)
                )]));
            }
            if line.contains(" lists every one") || line.contains(" lists them") {
                folded = true;
            }
            body.push(line.to_string());
        }
    }
    if routine.is_empty() {
        return (text.to_string(), None);
    }
    // Round-2 G31: a top-callees line prints only when it adds to the
    // blocker list (some callee it names is not on a body line already).
    for t in tops {
        let names = t.split_once(": ").map_or("", |(_, r)| r);
        let adds = names.split(", ").any(|c| {
            let callee = c.rsplit_once(" ×").map_or(c, |(n, _)| n);
            !body.iter().any(|l| l.contains(callee))
        });
        if adds {
            body.push(t);
        }
    }
    let mut h = Header::new("prove", format!("{routine} {question}"), hc.fcx.single_app);
    h.answer = answer;
    h.analysis = analysis;
    if let Some(why) = &hc.no_answer {
        h.answer = format!("no answer — {why}");
        h.analysis = Analysis::NotAnalyzed("no routine resolved".to_string());
    }
    h.assumptions = assumptions;
    h.limits = limits;
    let mut scope = Vec::new();
    if !scope_words.is_empty() {
        scope.push(scope_words);
    }
    scope.push(dependencies_clause(hc.deps));
    h.scope = scope.join("; ");
    let s = hc.fcx.surface;
    if folded && !hc.detail {
        h.more.push(format!(
            "{} lists every blocker and row",
            Input::Detail.on(s)
        ));
    }
    h.more.push(format!(
        "{} for every evidence row and its hops",
        Input::FormatJson.on(s)
    ));
    // Trailing blank lines of the body are the answer's own end.
    while body.last().is_some_and(String::is_empty) {
        body.pop();
    }
    if gaps_follow_up && let Some(sel) = &hc.selector {
        let f = FollowUp::new(Tool::Explain, hc.fcx, "lists the workspace ones under gaps")
            .arg("routines", vec![sel.clone()]);
        body.push(format!("  next: {} — {}", f.text(s), f.why));
    }
    let mut out = h.human();
    for l in body {
        out.push_str(&l);
        out.push('\n');
    }
    (out, Some(h))
}

/// The follow-ups a prove answer names (§6.1), for its JSON `followUps`:
/// the `re-analysis under an assumption:` call of an `unknown` (conditional),
/// and explain for the workspace gaps.
pub(crate) fn effect_follow_ups(
    hc: &ProveHeaderCx,
    result: &ProveResult,
    pointers: Option<&ObligationPointers>,
) -> Vec<crate::engine::follow_up::FollowUp> {
    use crate::engine::follow_up::{FollowUp, Tool};
    let mut out = Vec::new();
    let Some(sel) = &hc.selector else {
        return out;
    };
    let blockers = result.blocked_by.as_deref().unwrap_or(&[]);
    if result.answer == ProveAnswer::Unknown
        && blockers.iter().any(|b| b.kind == "open-world-dispatch")
        && result.obligations.world != World::Closed
    {
        out.push(closed_world_follow_up(hc.fcx, sel, &hc.question));
    }
    if pointers.is_some_and(|p| {
        !p.top_callees.is_empty() && p.workspace_unresolved + p.workspace_open_world > 0
    }) {
        out.push(
            FollowUp::new(Tool::Explain, hc.fcx, "lists the workspace ones under gaps")
                .arg("routines", vec![sel.clone()]),
        );
    }
    out
}

/// The `closedWorld: true` re-analysis of an existing-effects question.
fn closed_world_follow_up(
    fcx: crate::engine::follow_up::Cx,
    selector: &str,
    question: &str,
) -> crate::engine::follow_up::FollowUp {
    crate::engine::follow_up::FollowUp::prove(
        fcx,
        selector,
        question,
        "removes the open-world interface blockers, only if the deployment has no other \
         implementations of these interfaces",
    )
    .arg("closedWorld", true)
    .conditional()
}

pub fn format_prove_human(
    routine_display: &str,
    object_display: &str,
    question: &str,
    prove_result: &ProveResult,
    effects: &[DigestEffectFull],
    paths: Option<&HumanPaths<'_, '_>>,
) -> String {
    use crate::engine::l5::routine_rollup::{is_group, no_path_reason, plural};
    let mut lines: Vec<String> = Vec::new();

    // Routine header: the one display (V3 §2.1) when the root is known.
    let routine_header = match paths {
        Some(p) => {
            p.cx.names
                .display_with(p.root, true)
                .unwrap_or_else(|| owner_qualified(object_display, routine_display))
        }
        None => owner_qualified(object_display, routine_display),
    };
    lines.push(format!("--- {} ---", routine_header));
    // Agent-tasks §5: the six older questions are about the routine's
    // EXISTING effects; the callers' context ones say so on their own line.
    if crate::engine::l5::callers_context::CallersQuestion::parse(question).is_some() {
        lines.push(format!(
            "  question: {question} — callers' context (not about the routine's own effects)"
        ));
    } else {
        lines.push(format!(
            "  question: {question} — existing effects (what the routine and its callees \
             already do)"
        ));
    }
    // W2-8 (R3e): prove's own answer is authoritative. An `unknown` with no
    // evidence is an absence the obligations block: say so; `yes` and `no`
    // print bare, `no` followed by its coverage and assumptions below.
    let no_evidence = prove_result.evidence.as_ref().is_none_or(Vec::is_empty);
    let blockers: &[ProveBlocker] = prove_result.blocked_by.as_deref().unwrap_or(&[]);
    let kinds = blocker_kind_counts(blockers);
    let answer_line = lines.len();
    lines.push(match &prove_result.answer {
        // (`paths` is `None` only for the no-root document: nothing was
        // looked at there.) Output validation V2 (§8.1): an `unknown` names
        // its blockers in the negative-certainty form, never softened.
        ProveAnswer::Unknown if paths.is_some() && !kinds.is_empty() => format!(
            "  answer:   unknown (blocked by: {}) — {}",
            named_kinds(&kinds, 3),
            if no_evidence {
                "no matching effect was found in the analyzed cone"
            } else {
                "matching effects exist, none established on every success path"
            }
        ),
        ProveAnswer::Unknown if no_evidence && paths.is_some() => {
            "  answer:   unknown — no matching effect was found in the analyzed cone".to_string()
        }
        a => format!("  answer:   {}", a.as_str()),
    });
    // §4.4 (F19, F21): on `unknown`, the plain reading, what the answer does
    // not establish, and every input that removes a blocker kind.
    if prove_result.answer == ProveAnswer::Unknown
        && let Some(p) = paths
    {
        lines.extend(unknown_lines(
            question,
            &kinds,
            no_evidence,
            &prove_result.obligations,
            p,
        ));
    }

    let asked_table = match parse_question(question) {
        Some(ProveQuestion::WritesTable { table }) => Some(table),
        _ => None,
    };
    // Evidence (W2-6, design §4.7): where each effect sits, and the path to it
    // (prefix and terminal of one witness, never invented).
    let evidence_start = lines.len();
    if let Some(ref indices) = prove_result.evidence
        && !indices.is_empty()
    {
        lines.push(format!(
            "  evidence: {}",
            plural(indices.len(), "effect", "effects")
        ));
        // MCP output step 2: by default one witnessed row per effect type
        // (its shortest path), then at most `FOLDED_ROWS` more as locations,
        // then a count; `--detail` prints every row with its path.
        let detail = paths.is_none_or(|p| p.detail);
        let path_len = |i: usize| -> usize {
            let e = &effects[i];
            e.via_paths
                .iter()
                .filter(|v| !v.is_empty())
                .map(Vec::len)
                .min()
                .or_else(|| {
                    e.group_terminal_path
                        .as_ref()?
                        .as_ref()
                        .ok()
                        .map(|w| w.steps.len())
                })
                .unwrap_or(usize::MAX)
        };
        // Round 2 G16: workspace sites before dependency ones, for the
        // witnessed row (then the shortest path) and the folded rows alike.
        // Rank: 0 a workspace site of the effect itself, 1 an effect beyond
        // a dependency call entered from workspace code (the effect is in
        // dependency code), 2 a dependency site.
        let in_dependency = |i: usize| -> u8 {
            let ws = effects[i]
                .evidence
                .file
                .as_deref()
                .is_some_and(|f| f.starts_with("ws:"));
            match (ws, is_group(&effects[i])) {
                (true, false) => 0,
                (true, true) => 1,
                (false, _) => 2,
            }
        };
        let mut ordered: Vec<usize> = indices.clone();
        ordered.sort_by_key(|&i| in_dependency(i));
        let mut witnessed: BTreeMap<&str, usize> = BTreeMap::new();
        for &i in &ordered {
            let t = effects[i].effect_type.as_str();
            match witnessed.get(t) {
                Some(&j) if (in_dependency(j), path_len(j)) <= (in_dependency(i), path_len(i)) => {}
                _ => {
                    witnessed.insert(t, i);
                }
            }
        }
        // Round 2 G16: a `yes` names its first witness's site and condition
        // on the answer line.
        if prove_result.answer == ProveAnswer::Yes
            && let Some(&first) = witnessed
                .values()
                .min_by_key(|&&i| (in_dependency(i), path_len(i)))
        {
            let eff = &effects[first];
            let at = match (&eff.evidence.file, eff.evidence.line) {
                (Some(f), Some(l)) => match paths {
                    Some(p) => p.cx.location(f, l),
                    None => format!("{f}:{}", shown(l)),
                },
                (Some(f), None) => f.clone(),
                _ => "no position kept".to_string(),
            };
            let place = match eff.detail.iter().find(|(k, _)| k == "boundary") {
                Some((_, b)) if is_group(eff) => format!("beyond {b}, entered at {at}"),
                _ => format!("at {at}"),
            };
            let more = if indices.len() > 1 {
                format!(
                    ", and {} more",
                    crate::engine::l5::routine_rollup::thousands(indices.len() - 1)
                )
            } else {
                String::new()
            };
            // An implicit error names its operation (G17).
            let what = match (eff.effect_type.as_str(), paths) {
                ("ERROR_IMPLICIT", Some(p)) => eff
                    .origin_operation_ids
                    .iter()
                    .find_map(|o| p.cx.op_kind(o))
                    .map_or_else(
                        || eff.effect_type.clone(),
                        |k| format!("{k} (implicit error)"),
                    ),
                _ => eff.effect_type.clone(),
            };
            lines[answer_line] = format!(
                "  answer:   yes — {what} {place} ({}){more}",
                crate::engine::vocab::condition(eff.conditionality)
            );
        }
        // Per type: the folded rows' places, and how many more.
        let mut folded: BTreeMap<&str, (Vec<String>, usize)> = BTreeMap::new();
        for &i in &ordered {
            let eff = &effects[i];
            let t = eff.effect_type.as_str();
            if !detail && witnessed.get(t) != Some(&i) {
                let (listed, hidden) = folded.entry(t).or_default();
                if listed.len() >= FOLDED_ROWS {
                    *hidden += 1;
                    continue;
                }
            }
            let full = detail || witnessed.get(t) == Some(&i);
            let at = match (&eff.evidence.file, eff.evidence.line) {
                (Some(f), Some(l)) => match paths {
                    Some(p) => p.cx.location(f, l),
                    None => format!("{f}:{}", shown(l)),
                },
                (Some(f), None) => f.clone(),
                _ => "no position kept".to_string(),
            };
            let group = is_group(eff);
            let boundary = eff
                .detail
                .iter()
                .find(|(k, _)| k == "boundary")
                .map(|(_, v)| v.as_str());
            let row = match (group, boundary) {
                (true, Some(b)) => format!("    [{}] beyond {b}, entered at {at}", eff.effect_type),
                _ => format!("    [{}] at {at}", eff.effect_type),
            };
            if !full {
                // A folded row: its place and conditionality, no path.
                let place = row
                    .trim_start()
                    .trim_start_matches(&format!("[{t}] "))
                    .to_string();
                folded
                    .entry(t)
                    .or_default()
                    .0
                    .push(if eff.conditionality != "unknown" {
                        format!(
                            "{place} ({})",
                            crate::engine::vocab::condition(eff.conditionality)
                        )
                    } else {
                        place
                    });
                continue;
            }
            // Round 2 G17: an implicit error names its operation, c27's form
            // (`Get (implicit error) at …:50 (in a loop)`).
            let implicit_op = (eff.effect_type == "ERROR_IMPLICIT" && !group)
                .then(|| {
                    paths.and_then(|p| {
                        eff.origin_operation_ids
                            .iter()
                            .find_map(|o| p.cx.op_kind(o))
                    })
                })
                .flatten();
            match implicit_op {
                Some(op) => lines.push(format!(
                    "    {op} (implicit error) at {at} ({})",
                    crate::engine::vocab::condition(eff.conditionality)
                )),
                None => {
                    lines.push(row);
                    if eff.conditionality != "unknown" {
                        lines.push(format!(
                            "      condition: {}",
                            crate::engine::vocab::condition(eff.conditionality)
                        ));
                    }
                }
            }
            let Some(p) = paths else {
                continue;
            };
            let witness = if group {
                match &asked_table {
                    Some(table) => eff.group_resource_paths.iter().find_map(|(id, w)| {
                        let display = eff
                            .group_resources
                            .iter()
                            .find(|(r, _)| r == id)
                            .map(|(_, d)| d.as_str());
                        name_or_id_matches(display, Some(id), table).then_some(w)
                    }),
                    None => eff.group_terminal_path.as_ref(),
                }
            } else {
                None
            };
            match witness {
                Some(Ok(w)) => lines.push(format!(
                    "      path: {}",
                    p.cx.witness_line(
                        p.root,
                        w,
                        None,
                        p.cx.terminal_kind(&w.terminal.operation_id, &eff.effect_type)
                    )
                )),
                Some(Err(n)) => {
                    lines.push(format!("      no path kept: {}", no_path_reason(n.reason)))
                }
                None if !group => {
                    if let Some(v) = eff
                        .via_paths
                        .iter()
                        .filter(|v| !v.is_empty())
                        .min_by_key(|v| v.len())
                    {
                        lines.push(format!("      path: {}", p.cx.path_line(p.root, v)));
                    } else {
                        // Final CDO review, item 2: the searched witness of
                        // a row whose listed paths cannot run, or why none.
                        match &eff.own_path {
                            Some(Ok(w)) => {
                                let mut s = p.cx.own_path_line(p.root, w);
                                // Path and anchor of one witness: say where
                                // it ends when that is another of the row's
                                // operations.
                                if (&w.terminal.file, w.terminal.line)
                                    != (&eff.evidence.file, eff.evidence.line)
                                    && let (Some(f), Some(l)) = (&w.terminal.file, w.terminal.line)
                                {
                                    s.push_str(&format!(" (ends at {})", p.cx.location(f, l)));
                                }
                                lines.push(format!("      path: {s}"));
                            }
                            Some(Err(n)) => lines
                                .push(format!("      no path kept: {}", no_path_reason(n.reason))),
                            None => {}
                        }
                    }
                }
                None => {}
            }
        }
        for (t, (listed, hidden)) in &folded {
            // The type's full count beside its fold (T-W2-3).
            let total = indices
                .iter()
                .filter(|&&i| effects[i].effect_type == *t)
                .count();
            let more = if *hidden > 0 {
                format!("; … +{hidden} rows; {total} rows of this type in all")
            } else {
                String::new()
            };
            lines.push(format!(
                "    also [{t}]: {}{more} ({} lists every row with its path)",
                listed.join("; "),
                detail_words(paths.map(|p| p.fcx))
            ));
        }
    }
    // Output validation §7.5 (F45): a run of hops several evidence paths
    // share prints once, under a segment id.
    let evidence = lines.split_off(evidence_start);
    lines.extend(crate::engine::path_segments::segment(evidence));

    // Round 2 G17: on `throws-error`, the root's own TryFunction
    // declaration (printed only when it is one) and the follow-up that asks
    // whether a caller traps the error.
    if matches!(parse_question(question), Some(ProveQuestion::ThrowsError))
        && prove_result.answer != ProveAnswer::No
        && let Some(p) = paths
    {
        let try_function =
            p.cx.model
                .workspace
                .routines
                .iter()
                .filter(|r| r.stable_routine_id == p.root)
                .any(|r| {
                    crate::program::attributes::find_attribute(&r.attributes_parsed, "TryFunction")
                        .is_some()
                });
        if try_function {
            lines.push(
                "  declared TryFunction; interception depends on how the caller invokes it (an \
                 error still propagates when the caller ignores the Boolean result)"
                    .to_string(),
            );
        }
        let f = crate::engine::follow_up::FollowUp::prove(
            p.fcx,
            &p.selector(),
            "callers-intercept-errors",
            "whether a caller traps it",
        );
        lines.push(format!(
            "  whether a caller traps it: {}",
            f.text(p.fcx.surface)
        ));
    }

    // Blockers
    if let Some(ref blockers) = prove_result.blocked_by
        && !blockers.is_empty()
    {
        lines.extend(blocker_lines(
            blockers,
            paths.is_some_and(|p| p.detail),
            paths.map(|p| p.fcx),
        ));
    }

    // W2-8 (2h, design §3.7, §4.7): coverage, assumptions and limits, always
    // visible; the answer above is prove's own and is never qualified here.
    lines.extend(coverage_lines(
        &prove_result.obligations,
        paths.map(|p| p.pointers),
    ));

    lines.push(String::new());
    lines.join("\n")
}

/// The blocker kinds of an answer in the §8.1 words with their counts,
/// largest first (ties by word): `[(key, words, count)]`.
fn blocker_kind_counts(blockers: &[ProveBlocker]) -> Vec<(&'static str, String, usize)> {
    let mut by: BTreeMap<&'static str, usize> = BTreeMap::new();
    for b in blockers {
        *by.entry(b.kind).or_default() += 1;
    }
    let mut v: Vec<(&'static str, String, usize)> = by
        .into_iter()
        .map(|(k, n)| (k, crate::engine::vocab::blocker(k), n))
        .collect();
    v.sort_by(|a, b| b.2.cmp(&a.2).then(a.1.cmp(&b.1)));
    v
}

/// At most `cap` blocker kinds named with their counts, the rest counted.
fn named_kinds(kinds: &[(&'static str, String, usize)], cap: usize) -> String {
    let mut s = kinds
        .iter()
        .take(cap)
        .map(|(_, w, n)| format!("{w} {n}"))
        .collect::<Vec<_>>()
        .join(", ");
    if kinds.len() > cap {
        let more = kinds.len() - cap;
        s.push_str(&format!(
            ", and {more} more {}",
            if more == 1 { "kind" } else { "kinds" }
        ));
    }
    s
}

/// What an existing-effects `unknown` does not establish (§4.4), in the
/// question's terms.
fn not_established_effect(question: &str) -> String {
    match parse_question(question) {
        Some(ProveQuestion::MayCommit) => "that it never commits".to_string(),
        Some(ProveQuestion::CommitsOnSuccessPath) => {
            "that it commits on every success path, or that it never does".to_string()
        }
        Some(ProveQuestion::ReachesUi) => "that it never shows UI".to_string(),
        Some(ProveQuestion::ThrowsError) => "that it never raises an error".to_string(),
        Some(ProveQuestion::WritesTable { table }) => format!("that it never writes {table}"),
        Some(ProveQuestion::PublishesEvent { event }) => format!("that it never raises {event}"),
        None => "the asked conclusion".to_string(),
    }
}

/// The lines after an existing-effects `unknown` (§4.4): the plain reading,
/// `not established:`, and `re-analysis under an assumption:` for every
/// input that removes a blocker kind (`closedWorld: true` removes the
/// open-world interface dispatches), with what it removes and what stays.
fn unknown_lines(
    question: &str,
    kinds: &[(&'static str, String, usize)],
    no_evidence: bool,
    o: &ProveObligations,
    p: &HumanPaths<'_, '_>,
) -> Vec<String> {
    let detail = p.detail;
    let mut out = Vec::new();
    if !kinds.is_empty() {
        out.push(format!(
            "  plain reading: {}; unknown because of {}",
            if no_evidence {
                "no matching effect was found in the analyzed cone"
            } else {
                "the matching effects found do not hold on every success path"
            },
            named_kinds(kinds, 3)
        ));
    }
    out.push(format!(
        "  not established: {}",
        not_established_effect(question)
    ));
    if kinds.iter().any(|(k, _, _)| *k == "open-world-dispatch") && o.world != World::Closed {
        // A closed world also closes the coverage when interface open world
        // is its only reason (`compute_obligations`'
        // `coverage_closed_by_assumption`).
        let coverage_too = o.coverage_open_world_only;
        let stay: Vec<(&'static str, String, usize)> = kinds
            .iter()
            .filter(|(k, _, _)| {
                *k != "open-world-dispatch" && !(coverage_too && *k == "coverage-incomplete")
            })
            .cloned()
            .collect();
        let removed: Vec<String> = kinds
            .iter()
            .filter(|k| !stay.contains(k))
            .map(|(_, w, n)| format!("{w} {n}"))
            .collect();
        let removes = if stay.is_empty() {
            format!(
                "removes every blocker of this answer ({})",
                removed.join(", ")
            )
        } else {
            format!(
                "removes {}; stay: {}",
                removed.join(", "),
                named_kinds(&stay, if detail { stay.len() } else { 3 })
            )
        };
        // §6.1: at `--surface=mcp` the complete call; on the CLI the flag.
        let how = match p.fcx.surface {
            crate::engine::follow_up::Surface::Mcp => {
                closed_world_follow_up(p.fcx, &p.selector(), question).text(p.fcx.surface)
            }
            crate::engine::follow_up::Surface::Cli => crate::engine::follow_up::Input::ClosedWorld
                .cli()
                .to_string(),
        };
        out.push(format!(
            "  re-analysis under an assumption: {how} — only if the deployment has no other \
             implementations of these interfaces; it changes the assumption, not what runs; \
             {removes}"
        ));
    }
    out
}

/// How many evidence rows of one effect type the default prints as places
/// after the witnessed one (the rest are counted).
pub const FOLDED_ROWS: usize = 3;

/// How many blockers of one kind the default prints (the rest are counted).
pub const BLOCKERS_PER_KIND: usize = 3;

/// Where a blocker's call site is: workspace code, dependency code, or not
/// placed (no anchor). The order the human answer lists them in: workspace
/// ones first, since those are the ones the user can act on.
fn blocker_place(b: &ProveBlocker) -> u8 {
    match b.anchor.as_ref().and_then(|a| a.file.as_deref()) {
        Some(f) if f.starts_with("ws:") => 0,
        Some(_) => 1,
        None => 2,
    }
}

/// One blocker as a line item: its detail and, when it has one, its anchor.
fn blocker_item(b: &ProveBlocker) -> String {
    // TS (prove.ts:60-63) gates on `anchor?.file !== undefined` — present
    // (even if "", which never occurs in practice), NOT on non-empty.
    match b.anchor.as_ref().and_then(|a| a.file.as_ref()) {
        Some(f) => {
            let line_str = b
                .anchor
                .as_ref()
                .and_then(|a| a.line)
                .map(|l| format!(":{}", shown(l)))
                .unwrap_or_default();
            format!("{} @ {}{}", b.detail, f, line_str)
        }
        None => b.detail.clone(),
    }
}

/// The human `blockedBy:` block (final CDO review, item 1): the blockers
/// grouped by kind, in the order the kinds first appear, each kind with its
/// count split by where the call site is (this workspace, dependency code,
/// not placed) and its first [`BLOCKERS_PER_KIND`] items, workspace ones
/// first. `detail` (`--detail`) lists every item. IssueReminder's
/// `writes-table:Data Exch.` on CDO has 287 blockers: one line each was 294
/// lines and 47,875 bytes, over the 40 KiB inline cap. The JSON
/// (`blockedBy`) is unchanged and always complete.
fn blocker_lines(
    blockers: &[ProveBlocker],
    detail: bool,
    fcx: Option<crate::engine::follow_up::Cx>,
) -> Vec<String> {
    use crate::engine::l5::routine_rollup::{fold, plural};
    let detail_input = detail_words(fcx);
    let mut kinds: Vec<(&str, Vec<&ProveBlocker>)> = Vec::new();
    for b in blockers {
        match kinds.iter_mut().find(|(k, _)| *k == b.kind) {
            Some((_, v)) => v.push(b),
            None => kinds.push((b.kind, vec![b])),
        }
    }
    // The header says how the items are cut only when a kind is cut.
    let folds = !detail
        && kinds.iter().any(|(_, v)| {
            crate::engine::l5::routine_rollup::fold_len(v.len(), BLOCKERS_PER_KIND, false) < v.len()
        });
    let mut out = vec![format!(
        "  blockedBy: {} in {}{}",
        plural(blockers.len(), "blocker", "blockers"),
        plural(kinds.len(), "kind", "kinds"),
        if folds {
            format!(
                " (at most {BLOCKERS_PER_KIND} each, workspace ones first; {detail_input} lists \
                 every one)"
            )
        } else {
            String::new()
        }
    )];
    for (kind, mut items) in kinds {
        // A stable sort: within a place, the engine's own order.
        items.sort_by_key(|b| blocker_place(b));
        let mut at = [0usize; 3];
        for b in &items {
            at[usize::from(blocker_place(b))] += 1;
        }
        let split: Vec<String> = [
            (at[0], "in this workspace"),
            (at[1], "in dependency code"),
            (at[2], "not placed"),
        ]
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, w)| format!("{} {w}", crate::engine::l5::routine_rollup::thousands(*n)))
        .collect();
        let total = items.len();
        // Where the call sites are, when any blocker of the kind has one (a
        // coverage or routine-not-found blocker names no place).
        let kind = crate::engine::vocab::blocker(kind);
        out.push(if at[0] + at[1] > 0 {
            format!(
                "    {kind}: {} ({})",
                crate::engine::l5::routine_rollup::thousands(total),
                split.join(", ")
            )
        } else {
            format!(
                "    {kind}: {}",
                crate::engine::l5::routine_rollup::thousands(total)
            )
        });
        let f = fold(
            items.into_iter().map(blocker_item).collect(),
            BLOCKERS_PER_KIND,
            "blockers",
            detail,
        );
        for item in &f.shown {
            out.push(format!("      {item}"));
        }
        if f.hidden > 0 {
            out.push(format!(
                "      … +{} (of {}; {detail_input} lists them)",
                plural(f.hidden, "blocker", "blockers"),
                plural(total, "blocker", "blockers")
            ));
        }
    }
    out
}

/// The always-visible `coverage:`, `assumptions:` and `limits:` lines of a
/// prove answer (W2-8, design §3.7): the obligations with counts and where
/// they point, the answer's world, the analysis incompleteness.
fn coverage_lines(o: &ProveObligations, pointers: Option<&ObligationPointers>) -> Vec<String> {
    use crate::engine::l5::routine_rollup::plural;
    let ws = |n: usize| match pointers {
        Some(_) => format!(" ({n} in this workspace)"),
        None => String::new(),
    };
    let mut parts = Vec::new();
    if o.unresolved_callsites > 0 {
        parts.push(format!(
            "{} not resolved{}",
            plural(o.unresolved_callsites, "call", "calls"),
            ws(pointers.map_or(0, |p| p.workspace_unresolved))
        ));
    }
    if o.open_world_rows > 0 {
        parts.push(format!(
            "{} open world{}",
            plural(o.open_world_rows, "interface call", "interface calls"),
            ws(pointers.map_or(0, |p| p.workspace_open_world))
        ));
    }
    // §8.1 completeness: always with its subject.
    let mut coverage = if o.coverage_complete {
        "analysis complete".to_string()
    } else {
        "analysis partial".to_string()
    };
    if !parts.is_empty() {
        coverage.push_str(&format!(" — {}", parts.join(", ")));
    }
    if let Some(p) = pointers.filter(|p| !p.top_callees.is_empty()) {
        // Round-2 G31: by kind; an interface call is open world, not "not
        // resolved".
        for (label, top) in [
            ("top", &p.top_not_resolved),
            ("top open world", &p.top_open_world),
        ] {
            if !top.is_empty() {
                coverage.push_str(&format!(
                    "; {label}: {}",
                    top.iter()
                        .map(|(c, n)| format!("{c} ×{n}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
        if p.workspace_unresolved + p.workspace_open_world > 0 {
            coverage.push_str("; alsem explain lists the workspace ones under gaps");
        }
    }
    let assumptions = match o.assumption() {
        Some(label) => format!("assumptions: {label}"),
        None if o.world != World::Closed => "assumptions: none (default world: an interface \
             call may reach an implementation that is not loaded)"
            .to_string(),
        None => "assumptions: none (closedWorld asked; the answer does not rely on it)".to_string(),
    };
    let mut limits = Vec::new();
    if o.analysis_gaps > 0 {
        limits.push(plural(o.analysis_gaps, "analysis gap", "analysis gaps"));
    }
    if o.cone_truncated {
        limits.push("cone truncated".to_string());
    }
    let limits = if limits.is_empty() {
        "limits: none".to_string()
    } else {
        format!("limits: {}", limits.join("; "))
    };
    vec![
        format!("  {coverage}"),
        format!("  {assumptions}"),
        format!("  {limits}"),
    ]
}

// ---------------------------------------------------------------------------
// ProveRunResult
// ---------------------------------------------------------------------------

pub struct ProveRunResult {
    pub json_text: String,
    pub human_text: String,
    /// Exit code: 0 = answered, 2 = routine not resolved (dummy-doc)
    pub exit_code: u8,
    /// The answer line and obligations, for a file answer (MCP output plan
    /// 1e); `null` for the dummy document.
    pub summary: serde_json::Value,
}

/// Run the full prove pipeline for a workspace + routine selector + question text.
///
/// `maxPaths` is hardcoded to 3 (prove.ts:124) inside the digest substrate — there is
/// no caller-tunable knob, so no parameter is exposed.
///
/// Returns:
///   Ok(result) with exit_code 0 (answered) or 2 (routine not resolved → dummy-doc).
///   Err(message) for workspace/pipeline errors → emit to stderr, exit 1.
pub fn run_prove_pipeline(
    inputs: &std::sync::Arc<crate::program::inputs::WorkspaceInputs>,
    routine_selector: &str,
    question_text: &str,
    alsem_ver: &str,
    deterministic: bool,
    single_app: bool,
) -> Result<ProveRunResult, String> {
    prove_pipeline_with(
        crate::engine::l5::detectors::default_detectors(),
        inputs,
        routine_selector,
        question_text,
        alsem_ver,
        deterministic,
        single_app,
    )
}

/// [`run_prove_pipeline`] over a given detector set: the plan is digest's,
/// built from `detectors` (#160 §4.4.4, A2.10). Prove is digest with one root
/// (#160 step 9 D2): the cross-app view by default (`single_app` the
/// workspace alone), the root resolved from the model before `prepare`, its
/// effects, unresolved cone and gap count read off the view's tree inside it.
pub(crate) fn prove_pipeline_with(
    detectors: Vec<crate::engine::l5::registry::Detector>,
    inputs: &std::sync::Arc<crate::program::inputs::WorkspaceInputs>,
    routine_selector: &str,
    question_text: &str,
    alsem_ver: &str,
    deterministic: bool,
    single_app: bool,
) -> Result<ProveRunResult, String> {
    let job = ProveJob {
        detectors,
        inputs,
        routine_selector,
        question_text,
        alsem_ver,
        deterministic,
        single_app,
        closed_world: false,
        assume_no_dependents: false,
        human: false,
        detail: false,
        out: None,
        slice: None,
        surface: Default::default(),
    };
    crate::engine::batch::run_doc(&job)
        .one()
        .unwrap_or_else(|f| unreachable!("prove fails no side: {f:?}"))
}

/// One evidence row of prove's JSON (W2-2, design §4.7, A9/F8): prove's own
/// 2.x contract, started as a copy of the digest 2.x effect serializer so a
/// change to the digest document never moves prove's. ConTinAI's `verify.py`
/// reads `evidence.{file, line}`; prove's additive fields go on top.
pub(crate) fn evidence_value(eff: &DigestEffectFull) -> serde_json::Value {
    use crate::engine::l5::digest_cli::{anchor_to_value, scoped_guarantee_to_value};
    let mut m = serde_json::Map::new();
    m.insert("type".into(), eff.effect_type.clone().into());
    let mut detail_obj = serde_json::Map::new();
    for (k, v) in &eff.detail {
        detail_obj.insert(k.clone(), v.clone().into());
    }
    m.insert("detail".into(), serde_json::Value::Object(detail_obj));
    if !eff.group_resources.is_empty() {
        let all: Vec<serde_json::Value> = eff
            .group_resources
            .iter()
            .map(|(id, display)| serde_json::json!({"id": id, "display": display}))
            .collect();
        m.insert("resources".into(), serde_json::Value::Array(all));
    }
    m.insert("provenance".into(), eff.provenance.into());
    m.insert("evidence".into(), anchor_to_value(&eff.evidence));
    let via: Vec<serde_json::Value> = eff
        .via_paths
        .iter()
        .map(|path| {
            serde_json::Value::Array(
                path.iter()
                    .map(|h| crate::engine::l5::digest::hop_to_json_value(&h.inner))
                    .collect(),
            )
        })
        .collect();
    m.insert("viaPaths".into(), serde_json::Value::Array(via));
    m.insert("viaPathsTruncated".into(), eff.via_paths_truncated.into());
    if eff.literal_unreachable {
        m.insert("literalUnreachable".into(), true.into());
    }
    m.insert("confidence".into(), "static".into());
    m.insert("conditionality".into(), eff.conditionality.into());
    m.insert("transactionContext".into(), eff.transaction_context.into());
    let guarantees: Vec<serde_json::Value> =
        eff.guarantees.iter().map(|g| g.clone().into()).collect();
    m.insert("guarantees".into(), serde_json::Value::Array(guarantees));
    m.insert("factId".into(), eff.fact_id.clone().into());
    let sg: Vec<serde_json::Value> = eff
        .scoped_guarantees
        .iter()
        .map(scoped_guarantee_to_value)
        .collect();
    m.insert("scopedGuarantees".into(), serde_json::Value::Array(sg));
    serde_json::Value::Object(m)
}

/// [`prove_pipeline_with`] on a target built already: a test hand-states the
/// model's precondition between the build and the job (CLAUDE.md).
#[cfg(test)]
pub(crate) fn prove_on_target(
    detectors: Vec<crate::engine::l5::registry::Detector>,
    target: crate::engine::gate::run::AnalysisTarget,
    inputs: &std::sync::Arc<crate::program::inputs::WorkspaceInputs>,
    routine_selector: &str,
    question_text: &str,
    alsem_ver: &str,
    deterministic: bool,
) -> Result<ProveRunResult, String> {
    let job = ProveJob {
        detectors,
        inputs,
        routine_selector,
        question_text,
        alsem_ver,
        deterministic,
        single_app: matches!(
            target,
            crate::engine::gate::run::AnalysisTarget::SingleApp(_)
        ),
        closed_world: false,
        assume_no_dependents: false,
        human: false,
        detail: false,
        out: None,
        slice: None,
        surface: Default::default(),
    };
    crate::engine::batch::run_on_target(&job, target)
        .unwrap_or_else(|f| unreachable!("prove fails no side: {f:?}"))
        .take()
}

/// `alsem prove` as a batch job (#277 D1; case B, #160 step 9 D2): digest with
/// one root, on the cross-app view by default (`single_app` the workspace
/// alone). `resolve` resolves the root from the model; the view's tree over it
/// gives its effects, unresolved cone and gap count. Its document is
/// `Result<ProveRunResult, String>`; its writes are the `--format` text (to
/// `--out` or stdout), or an error line and exit 1.
pub struct ProveJob<'a> {
    pub detectors: Vec<crate::engine::l5::registry::Detector>,
    /// Handed to the driver; compute reads `ViewCx::inputs` (#277 R7-1).
    pub inputs: &'a dyn WorkspaceRead,
    pub routine_selector: &'a str,
    pub question_text: &'a str,
    pub alsem_ver: &'a str,
    pub deterministic: bool,
    /// `--single-app`: the workspace alone.
    pub single_app: bool,
    /// `--closed-world` (G24): an interface call dispatches only to the loaded
    /// implementations.
    pub closed_world: bool,
    /// `--no-dependents` (agent-tasks §3.5): no app outside the program
    /// calls the workspace, so external callers do not block a callers'
    /// context `no`. Its own flag, not `closed_world`.
    pub assume_no_dependents: bool,
    /// `--format human`: the human text, else the JSON.
    pub human: bool,
    /// `--detail`: the human answer lists every blocker (the JSON always
    /// does).
    pub detail: bool,
    pub out: Option<&'a str>,
    /// Agent-tasks E1: `--match`, `--site`, `--op`, `--table` (validated by
    /// [`crate::engine::l5::prove_slice::SliceRequest::of`]). `None`: the
    /// unfiltered answer, byte-identical to before.
    pub slice: Option<crate::engine::l5::prove_slice::SliceRequest>,
    /// `--surface` (output validation §6.1): the words of every follow-up.
    pub surface: crate::engine::follow_up::Surface,
}

impl ProveJob<'_> {
    /// The callers' context question asked, if it is one.
    fn callers_question(&self) -> Option<crate::engine::l5::callers_context::CallersQuestion> {
        crate::engine::l5::callers_context::CallersQuestion::parse(self.question_text)
    }

    /// The follow-ups' surface and scope inputs.
    fn fcx(&self) -> crate::engine::follow_up::Cx {
        crate::engine::follow_up::Cx::new(self.surface, self.single_app)
    }
}

impl Job for ProveJob<'_> {
    fn name(&self) -> &'static str {
        "prove"
    }
    fn demands(&self) -> Vec<ViewDemand> {
        // A callers' context question walks the sited graph beside the model.
        // A filtered question reads the message texts (rules) and the guard
        // arms (agent-tasks E1); no other prove builds them.
        let needs = if self.callers_question().is_some() {
            ViewNeeds::MODEL_SITED
        } else if self.slice.is_some() {
            ViewNeeds::PROVE_SLICE
        } else {
            ViewNeeds::MODEL
        };
        ViewDemand::one(self.inputs, self.single_app, false, needs)
    }

    fn resolve(&self, _: SideIx, cx: &ResolveCx<'_>) -> Result<PlanPart, JobFailure> {
        // The root is resolved before the composition, from the model alone
        // (#160 step 9 D2); the owner index dies here, the identity table
        // with the resolve step.
        let model = cx.model();
        let changed_input = ChangedInput {
            files: None,
            routines: Some(vec![self.routine_selector.to_string()]),
            diff_text: None,
            diff_root: Default::default(),
            // No file inputs here; any would need the workspace files (#339).
            workspace_files: &[],
        };
        let changed_roots = resolve_changed_roots(
            cx.ids(),
            model,
            &changed_input,
            primary_guid_of(model),
            &OwnerIndex::build(model),
        );
        // Digest's plan over the one root prove reads (the first resolved);
        // none resolved: no tree, the run's diagnostics only. A callers'
        // context question reads no tree (its walk goes up, over the sited
        // graph): the run's detector artifacts only.
        let tree_root = if self.callers_question().is_some() {
            None
        } else {
            changed_roots.roots.first().cloned()
        };
        let plan = match &self.slice {
            Some(s) => crate::engine::session::prove_slice_plan(
                self.detectors.clone(),
                tree_root,
                s.clone(),
            ),
            None => prove_plan(self.detectors.clone(), tree_root),
        };
        Ok(PlanPart {
            plan,
            reads: vec![Cell::Det, Cell::L4, Cell::Effects],
            carry: Some(Box::new(changed_roots)),
        })
    }

    fn compute(&self, _: SideIx, cx: &mut ViewCx<'_>) -> Result<SideDoc, JobFailure> {
        Ok(SideDoc::new(prove_view(cx, self)))
    }

    fn view_failed(&self, _: SideIx, failure: &ViewFailure) -> Result<SideDoc, JobFailure> {
        Ok(SideDoc::new(Err::<ProveRunResult, _>(
            failure.target.message("prove"),
        )))
    }

    fn serialize(&self, doc: Doc) -> JobOutput {
        let r = doc
            .one::<Result<ProveRunResult, String>>()
            .unwrap_or_else(|f| unreachable!("prove fails no side: {f:?}"));
        JobOutput::text_with_summary(
            self.out,
            r.map(|r| {
                let text = if self.human {
                    r.human_text
                } else {
                    r.json_text
                };
                (text, r.exit_code, r.summary)
            }),
        )
    }
}

/// Prove off its prepared view.
fn prove_view(cx: &mut ViewCx<'_>, job: &ProveJob<'_>) -> Result<ProveRunResult, String> {
    let ProveJob {
        routine_selector,
        question_text,
        alsem_ver,
        deterministic,
        closed_world,
        ..
    } = *job;
    // The roots `resolve` resolved off the model, before the composition.
    let changed_roots: crate::engine::l5::digest_cli::ChangedRootsResult = cx.take_carry();
    let key = cx.keys()[0].clone();
    let workspace_fp =
        crate::engine::l5::snapshot_full::workspace_fingerprint_of(cx.inputs(), alsem_ver);
    // A callers' context question walks the reverse cone while the detector
    // artifacts (summaries, cone writes) are alive (agent-tasks §6).
    let callers = match (job.callers_question(), changed_roots.roots.first()) {
        (Some(q), Some(root)) => Some(callers_answer(cx, job, q, root)?),
        _ => None,
    };
    let (diagnostics_json, tx_ctx_map) = finish_default_run(cx, &key);
    if let Some((answer, header)) = callers {
        cx.release(Cell::L4);
        cx.release(Cell::Effects);
        let deps = cx.view().target().dependencies().apps.len();
        let mut doc =
            callers_document(job, &answer, &header, &workspace_fp, diagnostics_json, deps);
        // F5 (2g): readable dependency locations, as the effects path prints
        // them (the JSON keeps the guid).
        doc.human_text = cx
            .view()
            .target()
            .dependencies()
            .readable_locations(doc.human_text);
        return Ok(doc);
    }

    // EXIT CODE 2: routine not resolved (zero roots → dummy-doc)
    if changed_roots.roots.is_empty() {
        // Dummy ProveResult: answer=unknown, zeroed obligations (coverageComplete=false)
        let dummy_oblg = ProveObligations {
            unresolved_callsites: 0,
            open_world_rows: 0,
            analysis_gaps: 0,
            coverage_complete: false,
            cone_truncated: false,
            world: World::Strict,
            waived_rows: 0,
            coverage_closed_by_assumption: false,
            coverage_open_world_only: false,
            answer_relies_on_world: false,
            relies_on_loaded_subscribers: false,
        };
        // G42: say WHY there is no answer (an `unknown` with no blocker read
        // like an answer and hid a mistyped id or a missing app-guid prefix):
        // the selector names no routine, or several. Exit 2 stays the
        // no-root code digest and fingerprint share.
        use crate::engine::l5::digest_cli::ChangedRootsDiagnostic;
        let apps = cx.view().target().dependencies().apps;
        let blocker = match changed_roots.diagnostics.iter().find_map(|d| match d {
            ChangedRootsDiagnostic::SelectorAmbiguous {
                candidates,
                routines,
                listed,
                ..
            } => Some((candidates, routines, listed)),
            _ => None,
        }) {
            // V3 §2.4: each candidate with its complete parameter list and
            // the ready `routines` value.
            Some((candidates, routines, listed)) => ProveBlocker {
                kind: "routine-ambiguous",
                detail: if listed.is_empty() {
                    crate::engine::l5::explain::ambiguity_text(
                        "",
                        routine_selector,
                        candidates,
                        routines,
                    )
                } else {
                    crate::engine::l5::selector_index::ambiguity_lines("", routine_selector, listed)
                        .join("\n    ")
                },
                anchor: None,
            },
            // V3 §2.4: the nearest names (or where the name is found only in
            // dependency code) and the selector forms.
            None => {
                let hints = changed_roots.diagnostics.iter().find_map(|d| match d {
                    ChangedRootsDiagnostic::SelectorUnmatched { hints, .. } => Some(hints),
                    _ => None,
                });
                let mut lines = vec![crate::engine::l5::explain::unmatched_line(
                    "",
                    routine_selector,
                )];
                if let Some(h) = hints {
                    lines.extend(
                        crate::engine::l5::selector_index::hint_lines(h, &apps)
                            .into_iter()
                            .map(|l| format!("      {l}")),
                    );
                }
                ProveBlocker {
                    kind: "routine-not-found",
                    detail: lines.join("\n"),
                    anchor: None,
                }
            }
        };
        let dummy_result = ProveResult {
            answer: ProveAnswer::Unknown,
            evidence: None,
            blocked_by: Some(vec![blocker]),
            obligations: dummy_oblg,
        };

        // As digest's no-root path: `l4` and the (unfilled) effects die
        // before the projection, so the release sequence is digest's.
        cx.release(Cell::L4);
        cx.release(Cell::Effects);
        cx.view().census_output();
        let json_text = project_prove_document(ProveDocumentArgs {
            workspace_fp: &workspace_fp,
            routine_stable_id: routine_selector,
            routine_display: routine_selector,
            object_display: "",
            routine_anchor: None,
            question: question_text,
            prove_result: &dummy_result,
            effects: &[],
            via_callers: &HashMap::new(),
            diagnostics_json,
            alsem_ver,
            deterministic,
            pointers: None,
            slice: None,
            routine_identity: serde_json::Map::new(),
            resolved_target: serde_json::Map::new(),
        });

        // Round-2 G40: a lookup miss prints its failure sentence once (the
        // `answer:` line), then the candidates or nearest names and the
        // complete calls; no analysis, assumptions or scope clause.
        let detail = dummy_result
            .blocked_by
            .as_deref()
            .and_then(|b| b.first())
            .map_or(String::new(), |b| b.detail.clone());
        let mut detail_lines = detail.lines().map(str::trim_start);
        let first = detail_lines
            .next()
            .unwrap_or("the selector names no routine");
        let mut h = crate::engine::result_header::Header::new(
            "prove",
            format!("{routine_selector} {question_text}"),
            job.fcx().single_app,
        );
        h.answer = format!("no answer — {first}");
        let mut human_text = h.miss_human();
        for l in detail_lines {
            // Candidate lines keep their numbering under the answer.
            human_text.push_str(&format!("  {l}\n"));
        }
        if let Some(listed) = changed_roots.diagnostics.iter().find_map(|d| match d {
            ChangedRootsDiagnostic::SelectorAmbiguous { listed, .. } if !listed.is_empty() => {
                Some(listed)
            }
            _ => None,
        }) {
            let sels: Vec<String> = listed.iter().map(|c| c.selector.clone()).collect();
            for l in crate::engine::l5::selector_index::complete_call_lines(
                crate::engine::follow_up::Tool::Prove,
                job.fcx(),
                &sels,
                &[("question", serde_json::Value::from(question_text))],
            ) {
                human_text.push_str(&l);
                human_text.push('\n');
            }
        }

        // §6.3 (round-2 n08): the outcome from the blockers' kinds
        // (`routine-ambiguous` / `routine-not-found`), not from the words.
        let summary = crate::engine::result_header::outcome_of_kinds(
            dummy_result.blocked_by.iter().flatten().map(|b| b.kind),
        )
        .map_or(
            serde_json::Value::Null,
            |o| serde_json::json!({"kind": "prove", "outcome": o}),
        );
        return Ok(ProveRunResult {
            json_text,
            human_text,
            exit_code: 2,
            summary,
        });
    }

    // Use first root (prove always passes a single routine selector)
    let root_id = &changed_roots.roots[0];

    // (5) The root's S4 entry, unresolved cone and gap count, read off the
    // view's tree inside `prepare` (the tree is gone).
    let mut effects = cx.take_effects();
    // Agent-tasks E1: the slice the plan read off the tree (a filtered
    // question only).
    let slice = effects.slice.take();
    let cone = effects
        .unresolved
        .get(root_id)
        .ok_or_else(|| "prove: internal error — no unresolved cone for the root".to_string())?;
    let gaps_in_cone_count = cone
        .gaps_in_cone
        .expect("prove's plan counts the gaps in the cone");
    let fires_table_events = cone
        .fires_table_events
        .expect("prove's plan reads the cone's table events");
    let resolved = cx.view().model();

    // The L4 db-effect store, for DB_* `detail.tempState` (#65): the view's `l4`.
    let temp_states = DbTempStates::new(resolved, &cx.view().l4().bundle);
    let owners = OwnerIndex::build(resolved);

    // Run full digest query for this single root
    let query_full = run_digest_query_full_from_entries(
        &mut effects,
        std::slice::from_ref(root_id),
        &tx_ctx_map,
        &temp_states,
        &changed_roots,
        &owners,
    );
    // Parse question (already validated at CLI layer, but re-check cleanly)
    let parsed_question = parse_question(question_text)
        .ok_or_else(|| format!("prove: unknown question '{}'", question_text))?;
    // G40: a table the model does not know answers nothing. Matching no write
    // would read as a proven `no`.
    if let ProveQuestion::WritesTable { table } = &parsed_question
        && !names_a_known_table(resolved, table)
    {
        return Err(format!(
            "prove: unknown table '{table}' in '{question_text}': no table of the program has \
             that name or number"
        ));
    }
    // G47: the same for an event no publisher of the program declares.
    if let ProveQuestion::PublishesEvent { event } = &parsed_question
        && !resolved
            .events
            .graph
            .events
            .iter()
            .any(|e| e.event_name.eq_ignore_ascii_case(event.trim()))
    {
        return Err(format!(
            "prove: unknown event '{event}' in '{question_text}': no publisher of the program \
             declares an event of that name"
        ));
    }
    // `writes-table`: THE shared table answer (design §3.2.1), while the
    // bundle is alive; explain and digest `--writes-table` call the same.
    let table_answer = match (&parsed_question, query_full.entries.first()) {
        (ProveQuestion::WritesTable { table }, Some(entry)) => Some(
            TableQuestions {
                model: resolved,
                temp_states: &temp_states,
                root: root_id,
                entry,
                gaps_in_cone_count,
                fires_table_events,
                world: World::of_flags(closed_world),
            }
            .answer(table)
            .map_err(|e| format!("prove: internal error — {e}"))?,
        ),
        _ => None,
    };
    // G25: caller-dependent writes asked in the callers' frames (a question
    // other than writes-table resolves none).
    let other_via_callers = match (&table_answer, query_full.entries.first()) {
        (None, Some(entry)) => resolve_caller_dependent_writes(
            resolved,
            &temp_states,
            root_id,
            entry,
            &parsed_question,
        ),
        _ => HashMap::new(),
    };
    // The bundle's only borrower is dead: release `l4`.
    drop(temp_states);
    drop(owners);
    drop(effects);
    cx.release(Cell::L4);
    cx.release(Cell::Effects);

    if query_full.entries.is_empty() {
        return Err("prove: internal error — entry missing after resolution".to_string());
    }

    let entry = &query_full.entries[0];

    // Run tristate evaluation
    let (prove_result, via_callers) = match table_answer {
        Some(a) => (a.result, a.via_callers),
        None => {
            let mut r = prove_with_callers(
                entry,
                gaps_in_cone_count,
                &parsed_question,
                &other_via_callers,
                &HashMap::new(),
                World::of_flags(closed_world),
            );
            r.obligations.relies_on_loaded_subscribers =
                relies_on_loaded_subscribers(&r, entry, fires_table_events);
            (r, other_via_callers)
        }
    };

    // W2-8 (2h): where the obligations point, in the answer's world.
    let pointers = ObligationPointers::of(entry, prove_result.obligations.world);
    // Agent-tasks E1: a filtered question answers over the slice's sites,
    // under the unfiltered answer's obligations.
    let filtered = slice
        .as_ref()
        .map(|s| filtered_result(entry, &prove_result, s));
    let prove_result = filtered.unwrap_or(prove_result);

    // W2-6: the human answer's paths read the model's names (and, W2-7, the
    // loaded apps' names); V3 §2.1: so do the root's identity fields.
    let dependencies = cx.view().target().dependencies();
    let rollup_cx =
        crate::engine::l5::routine_rollup::RollupCx::new(cx.view().model(), &dependencies.apps);
    let mut routine_identity = rollup_cx.names.identity_json(root_id);
    if let Some(d) = routine_identity.remove("display") {
        routine_identity.insert("qualifiedDisplay".into(), d);
    }
    let handle = rollup_cx.handle(root_id);
    if !handle.is_empty() {
        routine_identity.insert("handle".into(), handle.into());
    }
    // V3 §2.3: the question's target as resolved.
    let (resolved_target, question_human) = resolved_target(
        cx.view().model(),
        &rollup_cx,
        &parsed_question,
        question_text,
    );

    let paths = HumanPaths {
        cx: &rollup_cx,
        root: root_id,
        pointers: &pointers,
        detail: job.detail,
        fcx: job.fcx(),
    };
    let human_text = match &slice {
        Some(s) => format_prove_slice_human(
            &entry.routine_display,
            &entry.object_display,
            question_text,
            &prove_result,
            s,
            &paths,
        ),
        None => format_prove_human(
            &entry.routine_display,
            &entry.object_display,
            question_text,
            &prove_result,
            &entry.effects,
            Some(&paths),
        ),
    };
    // V3 §2.3: the question line names the target as resolved.
    let human_text = human_text.replacen(
        &format!("  question: {question_text} "),
        &format!("  question: {question_human} "),
        1,
    );
    // Output validation §5.2: the four header lines; JSON `result.scope`
    // and `result.followUps` (§6.1), additive (2.10.0 / 2.11.0).
    let hc = ProveHeaderCx {
        fcx: job.fcx(),
        deps: dependencies.apps.len(),
        detail: job.detail,
        selector: Some(rollup_cx.names.selector(root_id)),
        question: question_text.to_string(),
        no_answer: None,
    };
    let (human_text, header) = header_and_body(&human_text, &hc);
    let mut resolved_target = resolved_target;
    if let Some(h) = &header {
        resolved_target.insert("scope".into(), h.scope_json(serde_json::Map::new()));
    }
    resolved_target.insert(
        "followUps".into(),
        crate::engine::follow_up::json_list(&effect_follow_ups(
            &hc,
            &prove_result,
            Some(&pointers),
        )),
    );
    // W2-7 (2g): readable dependency locations (the JSON keeps the guid).
    let human_text = dependencies.readable_locations(human_text);

    // Project document
    cx.view().census_output();
    let json_text = project_prove_document(ProveDocumentArgs {
        workspace_fp: &workspace_fp,
        routine_stable_id: root_id,
        routine_display: &entry.routine_display,
        object_display: &entry.object_display,
        routine_anchor: entry.routine_anchor.as_ref(),
        question: question_text,
        prove_result: &prove_result,
        effects: &entry.effects,
        via_callers: &via_callers,
        diagnostics_json,
        alsem_ver,
        deterministic,
        pointers: Some(&pointers),
        slice: slice.as_ref(),
        routine_identity,
        resolved_target,
    });

    let mut summary = serde_json::json!({
        "kind": "prove",
        "routine": rollup_cx.qualified(root_id),
        "question": question_text,
        "answer": prove_result.answer.as_str(),
        "evidence": match &slice {
            Some(s) => s.matching().count(),
            None => prove_result.evidence.as_ref().map_or(0, Vec::len),
        },
        "blockedBy": prove_result.blocked_by.as_ref().map_or(0, Vec::len),
        "obligations": obligations_to_value(&prove_result.obligations),
    });
    // §4.3: a filtered answer's summary carries its filter, so a reader of
    // `answer: "no"` reads what it is "no" about.
    if let Some(s) = &slice {
        summary["filter"] = s.request.to_value();
        summary["otherSites"] = serde_json::json!(
            s.sites
                .iter()
                .filter(|x| !matches!(
                    x.outcome,
                    crate::engine::l5::prove_slice::Outcome::Match { .. }
                ))
                .count()
        );
    }
    // Output validation §5.2: the answer line, the analysis, and the
    // assumption inputs with what each removes (the conditional follow-ups);
    // the routine and its verdict as the result index row.
    if let Some(h) = &header {
        summary["answerLine"] = h.answer.clone().into();
        summary["analysis"] = h.analysis.human().into();
        summary["assumptions"] = serde_json::json!(h.assumptions);
    }
    summary["followUps"] = crate::engine::follow_up::json_list(&effect_follow_ups(
        &hc,
        &prove_result,
        Some(&pointers),
    ));
    let row = serde_json::json!({
        "routine": summary["routine"],
        "selector": hc.selector,
        "stableId": root_id,
        "question": question_text,
        "answer": prove_result.answer.as_str(),
        "filter": summary.get("filter").cloned(),
    });
    let summary = crate::engine::result_index::with_index(summary, "", "", vec![row]);
    Ok(ProveRunResult {
        json_text,
        human_text,
        exit_code: 0,
        summary,
    })
}

/// Agent-tasks E1 (design §9.3): the answer of a filtered question, from its
/// slice and the unfiltered answer `unfiltered` (whose obligations it keeps).
///
/// - `yes`: a matching site is reachable (its row and route are the slice's).
/// - `no`: no site matches, every reachable site is decided against the
///   filter (an unknown message, a placeholder text, a site with no
///   position, … is not), the root's search completed, and the cone's
///   obligations are clear.
/// - `unknown` otherwise, blocked by each undecided site (its reason is the
///   blocker kind) and the obligations that are not clear.
///
/// An unfiltered `yes` whose slice finds no site at all is `unknown`
/// (`slice-found-no-site`): the two reads disagree, so nothing is concluded.
pub(crate) fn filtered_result(
    entry: &DigestEntryFull,
    unfiltered: &ProveResult,
    slice: &crate::engine::l5::prove_slice::SliceOut,
) -> ProveResult {
    let oblg = unfiltered.obligations.clone();
    if slice.matching().next().is_some() {
        return ProveResult {
            answer: ProveAnswer::Yes,
            evidence: None,
            blocked_by: None,
            obligations: oblg,
        };
    }
    let mut blockers: Vec<ProveBlocker> = Vec::new();
    if !slice.complete {
        blockers.push(ProveBlocker {
            kind: "slice-search-capped",
            detail: "the root's search over literal arguments stopped at its state cap; no \
                     site was judged"
                .to_string(),
            anchor: None,
        });
    }
    // One blocker per undecided kind, with its count and its first site
    // (`payload.slice.undecided` lists the first few of each).
    for (kind, sites) in slice.undecided() {
        let s = sites[0];
        blockers.push(ProveBlocker {
            kind,
            detail: crate::engine::l5::prove_slice::undecided_detail(kind, sites.len()),
            anchor: s.file.as_ref().map(|f| BlockerAnchor {
                source_kind: "source".to_string(),
                file: Some(f.clone()),
                line: s.line,
                column: s.column,
                excerpt: None,
            }),
        });
    }
    if unfiltered.answer == ProveAnswer::Yes && slice.complete && slice.sites.is_empty() {
        blockers.push(ProveBlocker {
            kind: "slice-found-no-site",
            detail: "the unfiltered answer has evidence, but the slice found no reachable site \
                     of the question's kinds"
                .to_string(),
            anchor: None,
        });
    }
    if !obligations_clear(&oblg, false) {
        blockers.extend(build_obligation_blockers(entry, &oblg, false));
    }
    ProveResult {
        answer: if blockers.is_empty() {
            ProveAnswer::No
        } else {
            ProveAnswer::Unknown
        },
        evidence: None,
        blocked_by: (!blockers.is_empty()).then(|| sort_blockers(blockers)),
        obligations: oblg,
    }
}

/// The human answer of a filtered question (agent-tasks E1, design §9.4):
/// the answer over the slice, each matching site with its message, route and
/// selected guards, the other sites, then blockers and coverage as for any
/// question.
fn format_prove_slice_human(
    routine_display: &str,
    object_display: &str,
    question: &str,
    result: &ProveResult,
    slice: &crate::engine::l5::prove_slice::SliceOut,
    paths: &HumanPaths<'_, '_>,
) -> String {
    use crate::engine::l5::prove_slice::{STATIC_LABEL, human_lines};
    use crate::engine::l5::routine_rollup::plural;
    let mut lines: Vec<String> = Vec::new();
    let header = owner_qualified(object_display, routine_display);
    lines.push(format!("--- {header} ---"));
    lines.push(format!(
        "  question: {question} {} — existing effects (what the routine and its callees \
         already do), filtered",
        slice.request.label()
    ));
    let matching = slice.matching().count();
    // Output validation V2 (§4.3): the filter is part of the verdict.
    let filtered = match &slice.request.op {
        Some(op) => format!("{op} site"),
        None => format!("site {}", slice.request.label()),
    };
    let kinds = blocker_kind_counts(result.blocked_by.as_deref().unwrap_or(&[]));
    lines.push(match result.answer {
        ProveAnswer::Yes => format!(
            "  answer:   yes — {} reachable from {header} ({STATIC_LABEL})",
            plural(matching, "matching site", "matching sites")
        ),
        ProveAnswer::No => format!(
            "  answer:   no — no {filtered} is reachable (every candidate decided; \
             {STATIC_LABEL})"
        ),
        ProveAnswer::Unknown => format!(
            "  answer:   unknown (blocked by: {}) — no {filtered} was found reachable, and some \
             sites or the analysis cannot rule one out ({STATIC_LABEL})",
            named_kinds(&kinds, 3)
        ),
    });
    if result.answer == ProveAnswer::Unknown {
        lines.extend(unknown_lines(
            question,
            &kinds,
            true,
            &result.obligations,
            paths,
        ));
    }
    lines.extend(human_lines(
        slice,
        paths.cx,
        paths.root,
        paths.detail,
        paths.fcx.surface,
    ));
    if let Some(blockers) = &result.blocked_by
        && !blockers.is_empty()
    {
        lines.extend(blocker_lines(blockers, paths.detail, Some(paths.fcx)));
    }
    lines.extend(coverage_lines(&result.obligations, Some(paths.pointers)));
    lines.push(String::new());
    lines.join("\n")
}

/// The root of a callers' context answer, as prove's document names it.
pub(crate) struct RootHeader {
    stable_id: String,
    routine_display: String,
    object_display: String,
    /// The declaration's unit and 0-based line.
    anchor: Option<(String, u32)>,
    /// The one routine display (V3 §2.1, with the member's caption).
    qualified: String,
    /// The JSON identity fields (`signature`, `member`, `selector`).
    identity: serde_json::Map<String, serde_json::Value>,
}

/// The root's header off the model (the routine row with that stable id).
pub(crate) fn root_header(
    model: &crate::program::model::workspace::Model,
    stable_id: &str,
) -> RootHeader {
    let row = model
        .workspace
        .routines
        .iter()
        .find(|r| r.stable_routine_id == stable_id);
    let object = row.and_then(|r| model.workspace.objects.iter().find(|o| o.id == r.object_id));
    let names = crate::engine::l5::routine_rollup::RoutineNames::new(model, &[]);
    let object_display =
        object.map_or_else(String::new, |o| format!("{} {}", o.object_type, o.name));
    let routine_display = row.map_or_else(|| stable_id.to_string(), |r| r.name.clone());
    let mut identity = names.identity_json(stable_id);
    identity.remove("display");
    RootHeader {
        stable_id: stable_id.to_string(),
        qualified: names
            .display_with(stable_id, true)
            .unwrap_or_else(|| owner_qualified(&object_display, &routine_display)),
        identity,
        routine_display,
        object_display,
        anchor: row.map(|r| {
            (
                r.source_anchor.source_unit_id.clone(),
                r.source_anchor.start_line,
            )
        }),
    }
}

/// A callers' context question's answer for `root` (agent-tasks §6), off the
/// view's model, detector artifacts and sited graph.
pub(crate) fn callers_answer(
    cx: &ViewCx<'_>,
    job: &ProveJob<'_>,
    q: crate::engine::l5::callers_context::CallersQuestion,
    root: &str,
) -> Result<
    (
        crate::engine::l5::callers_context::CallersAnswer,
        RootHeader,
    ),
    String,
> {
    use crate::engine::l5::callers_context::{CallersCx, answer};
    let g = cx
        .sited()
        .ok_or_else(|| "prove: the sited graph's build did not finish (cancelled)".to_string())?;
    let nodes = g.nodes_with_stable_id(root);
    if nodes.is_empty() {
        return Err(format!(
            "prove: the routine '{root}' is not in the call graph (a symbol-only routine has no \
             callers the analysis can see)"
        ));
    }
    let model = cx.view().model();
    let det = cx.view().det();
    let ccx = CallersCx {
        model,
        g,
        summaries: &det.summaries,
        cone_derived: &det.cone_derived,
        inputs: Some(cx.inputs()),
        assume_no_dependents: job.assume_no_dependents,
        cancel: None,
    };
    Ok((answer(q, &nodes, &ccx), root_header(model, root)))
}

/// prove's document, human text and summary for a callers' context answer.
fn callers_document(
    job: &ProveJob<'_>,
    a: &crate::engine::l5::callers_context::CallersAnswer,
    header: &RootHeader,
    workspace_fp: &str,
    diagnostics_json: serde_json::Value,
    deps: usize,
) -> ProveRunResult {
    use crate::engine::l5::callers_context::{
        HumanCx, format_human, reanalysis_follow_up, result_value,
    };
    let selector = header
        .identity
        .get("selector")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(&header.stable_id)
        .to_string();
    let hcx = HumanCx {
        fcx: job.fcx(),
        selector: &selector,
    };
    let hc = ProveHeaderCx {
        fcx: job.fcx(),
        deps,
        detail: job.detail,
        selector: Some(selector.clone()),
        question: job.question_text.to_string(),
        no_answer: None,
    };
    let (human_text, h) =
        header_and_body(&format_human(&header.qualified, a, job.detail, &hcx), &hc);
    let mut result = result_value(a);
    if let Some(h) = &h {
        result["scope"] = h.scope_json(serde_json::Map::new());
    }
    result["followUps"] = crate::engine::follow_up::json_list(
        &reanalysis_follow_up(a, &hcx)
            .into_iter()
            .collect::<Vec<_>>(),
    );
    let mut routine_ref = serde_json::Map::new();
    if let Some((file, line)) = &header.anchor {
        routine_ref.insert(
            "anchor".into(),
            anchor_to_json("source", Some(file), Some(*line), None, None),
        );
    }
    routine_ref.insert("display".into(), header.routine_display.clone().into());
    routine_ref.insert("objectDisplay".into(), header.object_display.clone().into());
    routine_ref.insert("stableId".into(), header.stable_id.clone().into());
    // V3 §2.1 (2.5.0, additive): the one display and its parts.
    routine_ref.insert("qualifiedDisplay".into(), header.qualified.clone().into());
    routine_ref.extend(header.identity.clone());
    let mut payload = serde_json::Map::new();
    payload.insert("question".into(), job.question_text.into());
    payload.insert("questionScope".into(), "callers-context".into());
    payload.insert(
        "lineBase".into(),
        crate::engine::line_base::LINE_BASE.into(),
    );
    payload.insert("result".into(), result.clone());
    payload.insert("routine".into(), serde_json::Value::Object(routine_ref));
    payload.insert("workspaceFingerprint".into(), workspace_fp.into());
    let mut env = serde_json::Map::new();
    env.insert("alsemVersion".into(), job.alsem_ver.into());
    env.insert("deterministic".into(), job.deterministic.into());
    env.insert("diagnostics".into(), diagnostics_json);
    env.insert(
        "generatedAt".into(),
        crate::engine::gate::format_json::pinned_or_now_iso8601(job.deterministic).into(),
    );
    env.insert("kind".into(), "prove".into());
    env.insert("payload".into(), serde_json::Value::Object(payload));
    env.insert("schemaVersion".into(), PROVE_SCHEMA_VERSION.into());
    env.insert(
        "volatile".into(),
        crate::engine::result_header::volatile(job.deterministic),
    );
    let mut env = serde_json::Value::Object(env);
    crate::engine::result_header::tag_location_roles(&mut env);
    let json_text = serialize_document_value(env);
    let routine = header.qualified.clone();
    let summary = serde_json::json!({
        "kind": "prove",
        "routine": routine,
        "question": job.question_text,
        "questionScope": "callers-context",
        "answer": a.answer.as_str(),
        "evidence": a.witness_count,
        // §3.1: established witnesses and candidates over a hub, apart.
        "establishedWitnesses": a.witness_count - a.open_witness_count,
        "candidateWitnesses": a.open_witness_count,
        "blockedBy": a.blocked_counts.values().sum::<usize>(),
        // Output validation §5.2: the answer line, the analysis, the
        // assumption input and what it removes.
        "answerLine": h.as_ref().map(|h| h.answer.clone()),
        "analysis": h.as_ref().map(|h| h.analysis.human()),
        "followUps": result["followUps"],
    });
    let summary = crate::engine::result_index::with_index(
        summary,
        "",
        "",
        vec![serde_json::json!({
            "routine": routine,
            "selector": selector,
            "stableId": header.stable_id,
            "question": job.question_text,
            "answer": a.answer.as_str(),
        })],
    );
    ProveRunResult {
        json_text,
        human_text,
        exit_code: 0,
        summary,
    }
}

// ---------------------------------------------------------------------------
// Native oracles for corpus-invisible blocker types
//
// The prove corpus (18 entries) exercises: unresolved-callsite,
// non-unconditional-effect-exists, coverage-incomplete. The following types
// are NOT in any golden but ARE in the TS source — we verify them unit-wise:
//   - open-world-dispatch  (item.openWorld == true → separate blocker kind)
//   - analysis-gap         (gapsInCone.length > 0)
//   - cone-truncated       (entry.unresolvedTraversal.truncated == true)
//   - witness-truncated    (any effect.viaPathsTruncated OR evidence-unavailable diag)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::gate::run::AnalysisTarget;
    use crate::engine::l5::digest_cli::{
        DigestEffectFull, DigestEntryFull, EntryDiagnostic, FullAnchor,
    };
    use crate::engine::l5::unresolved_cone::{UnresolvedConeItem, UnresolvedTraversal};

    /// The cross-app target of `write_fixture_rule_b` (`CallWrite` → the
    /// source dependency's `Dep Rec Lib.Write`) with `Write` hand-stated
    /// body-less: a body-less demanded row of an app WITH source. No AL source
    /// makes one (a routine with no body there is an interface or
    /// declaration-only member, which the resolver never targets), so the
    /// precondition is stated by assignment (CLAUDE.md).
    fn bodyless_source_row_target(ws: &std::path::Path) -> AnalysisTarget {
        let inputs = crate::program::inputs::WorkspaceInputs::read(ws, &Default::default());
        let mut target = AnalysisTarget::build(&inputs, false, "prove").expect("cross-app base");
        let AnalysisTarget::CrossApp(b) = &mut target else {
            panic!("precondition: a cross-app target")
        };
        let primary = b
            .resolved
            .primary_app
            .as_ref()
            .expect("primary app")
            .app_guid
            .clone();
        let mut stated = 0;
        for r in b.resolved.workspace.routines.iter_mut() {
            if r.name == "Write" && !r.app_guid.eq_ignore_ascii_case(&primary) {
                assert!(r.body_available, "precondition: Write has a body before");
                r.body_available = false;
                stated += 1;
            }
        }
        assert_eq!(stated, 1, "precondition: one dependency Write row");
        target
    }

    /// #160 step 9 T2 (a'), spec §5: `prove CallWrite may-commit` cross-app,
    /// where `CallWrite` calls a body-less row of a source dependency app.
    /// Preconditions (asserted on the same hand-stated model's tree): the
    /// `CallWrite → Write` ledger row is `unfetched-dependency`, and the tree's
    /// one gap is `body-unavailable` on `Write` (D9: its app has source, so no
    /// app-level `symbol-only-boundary`; Task 2 asserted that, Task 5 switched it).
    /// Assertion: the blockers are exactly the callsite, the gap and
    /// coverage. DISCRIMINATION: B1, `into_unfetched` forced false
    /// (`snapshot.rs`) drops the callsite blocker; B2, `gaps_in_cone` forced 0
    /// (`digest_effects`) drops the gap blocker.
    #[test]
    fn prove_one_hop_ledger_and_gap() {
        use crate::engine::l4::cone_derived::{ConeScope, WitnessScope};
        use crate::engine::l5::detector_context::{compose_capabilities, det_cone_inputs};
        let tmp = tempfile::tempdir().unwrap();
        crate::engine::l5::test_support::write_fixture_rule_b(tmp.path());

        // Preconditions, off a tree of the same hand-stated model.
        let target = bodyless_source_row_target(tmp.path());
        let AnalysisTarget::CrossApp(b) = &target else {
            unreachable!()
        };
        let model = &b.resolved;
        let (mut caps, raw) = compose_capabilities(
            model,
            b.rel().cone(),
            det_cone_inputs(model, b.rel()),
            ConeScope {
                derived: true,
                raw: WitnessScope::None,
            },
        );
        let pd = std::borrow::Cow::Owned(caps.pd.take().unwrap_or_default());
        let tree = crate::engine::l5::snapshot::compose_tree(
            model,
            b.rel().cone(),
            b.dependency_calls_or_empty(),
            &caps,
            crate::engine::l5::snapshot::TreeRaw::Drain(raw),
            pd,
            &crate::engine::l5::snapshot::TreeApps {
                names: Default::default(),
                source_less: crate::engine::l5::snapshot::source_less_apps(&b.dependency_apps),
            },
        );
        let call_write = model
            .workspace
            .routines
            .iter()
            .find(|r| r.name == "CallWrite")
            .expect("CallWrite");
        let rows: Vec<&str> = tree
            .callsite_resolutions
            .iter()
            .filter(|r| r.from == call_write.stable_routine_id)
            .map(|r| r.status.as_str())
            .collect();
        assert_eq!(
            rows,
            ["unfetched-dependency"],
            "precondition: CallWrite → Write is unfetched-dependency"
        );
        // D9 (#160 step 9 Task 5): the gap is the routine's `body-unavailable`,
        // never an app-level `symbol-only-boundary` (XRec has source).
        let write = model
            .workspace
            .routines
            .iter()
            .find(|r| r.name == "Write" && !r.body_available)
            .expect("the hand-stated Write");
        let kinds: Vec<(&str, &str)> = tree
            .analysis_gaps
            .iter()
            .map(|g| (g.kind.as_str(), g.subject.as_str()))
            .collect();
        assert_eq!(
            kinds,
            [("body-unavailable", write.stable_routine_id.as_str())],
            "precondition: the tree's gap for Write"
        );
        drop(tree);
        drop(target);

        // The claim, through prove's pipeline on a fresh hand-stated target.
        let inputs = crate::program::inputs::WorkspaceInputs::read(
            tmp.path(),
            &crate::program::inputs::WorkspaceNeeds {
                fingerprint_rows: true,
                ..Default::default()
            },
        );
        let r = prove_on_target(
            crate::engine::l5::detectors::default_detectors(),
            bodyless_source_row_target(tmp.path()),
            &inputs,
            "CallWrite",
            "may-commit",
            "t",
            true,
        )
        .expect("prove runs");
        let doc: serde_json::Value = serde_json::from_str(&r.json_text).unwrap();
        let result = &doc["payload"]["result"];
        assert_eq!(result["answer"], "unknown", "{}", r.json_text);
        let mut got: Vec<(String, String)> = result["blockedBy"]
            .as_array()
            .expect("blockedBy")
            .iter()
            .map(|b| {
                (
                    b["kind"].as_str().unwrap().to_string(),
                    b["detail"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        got.sort();
        assert_eq!(
            got,
            [
                (
                    "analysis-gap".to_string(),
                    "1 analysis gap(s) in cone".to_string()
                ),
                (
                    "coverage-incomplete".to_string(),
                    "coverage status: partial".to_string()
                ),
                (
                    "unresolved-callsite".to_string(),
                    "Lib.Write in CallWrite".to_string()
                ),
            ],
            "{}",
            r.json_text
        );
    }

    /// #160 step 9 T27 (D9, spec §4.10): a source dependency app (`XRec`,
    /// precondition: `has_source`) with one body-less demanded row (`Write`,
    /// hand-stated). Assertions: the tree's gaps hold `body-unavailable` for
    /// `Write` and no `symbol-only-boundary` for the app; the cross-app
    /// inventory assembly over that tree does not panic. DISCRIMINATION: every body-less row's app back into
    /// `opaque_apps` (D9's filter removed) fails the gap assertion.
    #[test]
    fn source_app_bodyless_row_gets_body_unavailable_gap() {
        use crate::engine::l4::cone_derived::{ConeScope, WitnessScope};
        use crate::engine::l5::detector_context::{compose_capabilities, det_cone_inputs};
        use crate::engine::l5::snapshot_full::{
            FullSnapshotOptions, SnapshotKeys, SnapshotUniverse, assemble_full_snapshot,
        };
        let tmp = tempfile::tempdir().unwrap();
        crate::engine::l5::test_support::write_fixture_rule_b(tmp.path());
        let target = bodyless_source_row_target(tmp.path());
        let AnalysisTarget::CrossApp(b) = &target else {
            unreachable!()
        };
        let dep_guid = "bbbb1604-0000-0000-0000-000000000160"; // the fixture's XRec
        assert!(
            b.dependency_apps
                .iter()
                .any(|a| a.guid.eq_ignore_ascii_case(dep_guid) && a.has_source),
            "precondition: XRec ships source"
        );
        let model = &b.resolved;
        let (mut caps, raw) = compose_capabilities(
            model,
            b.rel().cone(),
            det_cone_inputs(model, b.rel()),
            ConeScope {
                derived: true,
                raw: WitnessScope::None,
            },
        );
        let pd = std::borrow::Cow::Owned(caps.pd.take().unwrap_or_default());
        let tree = crate::engine::l5::snapshot::compose_tree(
            model,
            b.rel().cone(),
            b.dependency_calls_or_empty(),
            &caps,
            crate::engine::l5::snapshot::TreeRaw::Drain(raw),
            pd,
            &crate::engine::l5::snapshot::TreeApps {
                names: Default::default(),
                source_less: crate::engine::l5::snapshot::source_less_apps(&b.dependency_apps),
            },
        );
        let write = model
            .workspace
            .routines
            .iter()
            .find(|r| r.name == "Write" && !r.body_available)
            .expect("the hand-stated Write");
        assert!(
            tree.analysis_gaps
                .iter()
                .any(|g| g.kind == "body-unavailable" && g.subject == write.stable_routine_id),
            "body-unavailable for Write: {:?}",
            tree.analysis_gaps
        );
        assert!(
            tree.analysis_gaps
                .iter()
                .all(|g| g.kind != "symbol-only-boundary"),
            "no app-level gap for an app with source: {:?}",
            tree.analysis_gaps
        );
        let inputs = crate::program::inputs::WorkspaceInputs::read(tmp.path(), &Default::default());
        let keys = assemble_full_snapshot(
            model,
            tree,
            crate::engine::gate::cbor::CborValue::Array(vec![]),
            &FullSnapshotOptions {
                inputs: &inputs,
                driver_version: "t",
                deterministic: true,
                roots_config_ignored: false,
                universe: SnapshotUniverse::of(&target),
            },
            SnapshotKeys::Inventory,
        );
        assert_eq!(keys.len(), 5, "the inventory assembled without a panic");
    }

    /// A2.10: prove builds digest's plan from the set it is given.
    #[test]
    fn tx_context_without_d9() {
        crate::engine::l5::digest_cli::assert_prepare_refuses_a_reduced_set(
            crate::engine::l5::registry::substrate::TRANSACTION_SPANS,
            "TRANSACTION_SPANS",
            "ws-d8-commit-in-tx",
            |set, inputs| {
                let _ = prove_pipeline_with(
                    set,
                    inputs,
                    "PostSalesDoc",
                    "may-commit",
                    "test",
                    true,
                    false,
                );
            },
        );
    }

    /// Build a minimal DigestEntryFull for oracle tests.
    fn minimal_entry(
        coverage_status: &str,
        unresolved: Vec<UnresolvedConeItem>,
        effects: Vec<DigestEffectFull>,
        entry_diagnostics: Vec<EntryDiagnostic>,
        cone_truncated: bool,
    ) -> DigestEntryFull {
        DigestEntryFull {
            routine_id: "test-routine".to_string(),
            routine_display: "TestRoutine".to_string(),
            object_display: "".to_string(),
            routine_anchor: None,
            routine_anchors: Vec::new(),
            coverage_status: coverage_status.to_string(),
            coverage_reasons: Vec::new(),
            effects,
            unresolved,
            unresolved_traversal: UnresolvedTraversal {
                truncated: cone_truncated,
                max_depth: 64,
                visited_routines: 1,
                dependency_routines: 0,
            },
            entry_diagnostics,
            declaration_only: false,
        }
    }

    fn unresolved_item(open_world: Option<bool>) -> UnresolvedConeItem {
        UnresolvedConeItem {
            callsite_id: "cs".to_string(),
            callsite_file: Some("ws:src/Test.al".to_string()),
            callsite_line: Some(10),
            callsite_column: Some(8),
            callee_display: "SomeCallee".to_string(),
            status: if open_world == Some(true) {
                "open-world".to_string()
            } else {
                "unresolved".to_string()
            },
            candidates: None,
            open_world,
            closed_world_followable: false,
            publisher_scoped: false,
            owning_routine: "test-routine".to_string(),
            owning_routine_display: "TestRoutine".to_string(),
        }
    }

    // -- open-world-dispatch oracle ------------------------------------------

    /// An unresolved item with openWorld=true produces an open-world-dispatch
    /// blocker (not an unresolved-callsite blocker).
    #[test]
    fn open_world_dispatch_blocker_kind() {
        let entry = minimal_entry(
            "complete",
            vec![unresolved_item(Some(true))],
            vec![],
            vec![],
            false,
        );
        let result = prove(&entry, 0, &ProveQuestion::MayCommit);
        assert_eq!(result.answer, ProveAnswer::Unknown);
        let blockers = result.blocked_by.as_deref().unwrap_or(&[]);
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].kind, "open-world-dispatch");
        // obligations: openWorldRows=1, unresolvedCallsites=0
        assert_eq!(result.obligations.open_world_rows, 1);
        assert_eq!(result.obligations.unresolved_callsites, 0);
    }

    /// Round 2 G16: a `yes` names its first witness on the answer line, and a
    /// workspace site is that witness before a dependency one (n01: the
    /// pathed witness was a dependency activation-wizard commit). Hand-stated:
    /// a dependency COMMIT listed first, then a workspace COMMIT in a loop.
    #[test]
    fn a_yes_leads_with_a_workspace_witness() {
        let commit = |file: &str, line: u32, cond: &'static str| DigestEffectFull {
            effect_type: "COMMIT".to_string(),
            detail: vec![],
            provenance: "transitive",
            evidence: crate::engine::l5::digest_cli::FullAnchor {
                source_kind: "source",
                file: Some(file.to_string()),
                line: Some(line),
                column: None,
                excerpt: None,
            },
            via_paths: vec![],
            via_paths_truncated: false,
            conditionality: cond,
            transaction_context: "unknown",
            guarantees: vec![],
            fact_id: format!("f{line}"),
            scoped_guarantees: vec![],
            group_resources: vec![],
            group_resource_paths: vec![],
            group_terminal_path: None,
            own_path: None,
            origin_operation_ids: vec![],
            effect_is_kt: false,
            literal_unreachable: false,
            crossing: None,
        };
        let entry = minimal_entry(
            "complete",
            vec![],
            vec![
                commit("dep:Core:src/Wizard.al", 117, "unconditional-on-success"),
                commit("ws:src/Dispatcher.al", 228, "loop-body"),
            ],
            vec![],
            false,
        );
        let result = prove(&entry, 0, &ProveQuestion::MayCommit);
        assert_eq!(result.answer, ProveAnswer::Yes);
        let text = format_prove_human(
            "TestRoutine",
            "",
            "may-commit",
            &result,
            &entry.effects,
            None,
        );
        assert!(
            text.contains(
                "  answer:   yes — COMMIT at ws:src/Dispatcher.al:229 (in a loop), and 1 more"
            ),
            "{text}"
        );
        let ws = text
            .find("[COMMIT] at ws:src/Dispatcher.al:229")
            .expect("ws row");
        let dep = text.find("dep:Core:src/Wizard.al:118").expect("dep row");
        assert!(ws < dep, "workspace site first: {text}");
    }

    /// Round 2 G13: partial coverage whose only reason is interface open
    /// world, with that open-world row listed, is one blocker (the row), not
    /// two; a coverage reason no blocker lists (an opaque body) keeps
    /// `coverage-incomplete` as a blocker. Hand-stated reasons.
    #[test]
    fn coverage_incomplete_is_not_counted_twice() {
        let mut entry = minimal_entry(
            "partial",
            vec![unresolved_item(Some(true))],
            vec![],
            vec![],
            false,
        );
        entry.coverage_reasons = vec![INTERFACE_OPEN_WORLD.to_string()];
        let result = prove(&entry, 0, &ProveQuestion::MayCommit);
        assert_eq!(result.answer, ProveAnswer::Unknown);
        let kinds: Vec<&str> = result
            .blocked_by
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .map(|b| b.kind)
            .collect();
        assert_eq!(kinds, ["open-world-dispatch"], "{kinds:?}");
        entry.coverage_reasons = vec![INTERFACE_OPEN_WORLD.to_string(), "opaque-body".to_string()];
        let result = prove(&entry, 0, &ProveQuestion::MayCommit);
        let kinds: Vec<&str> = result
            .blocked_by
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .map(|b| b.kind)
            .collect();
        assert_eq!(
            kinds,
            ["coverage-incomplete", "open-world-dispatch"],
            "an unlisted reason stays a blocker: {kinds:?}"
        );
    }

    /// Verify that open-world rows do NOT contribute to unresolvedCallsites.
    #[test]
    fn open_world_counted_separately_from_unresolved() {
        let entry = minimal_entry(
            "complete",
            vec![
                unresolved_item(None),       // → unresolvedCallsites
                unresolved_item(Some(true)), // → openWorldRows
            ],
            vec![],
            vec![],
            false,
        );
        let result = prove(&entry, 0, &ProveQuestion::MayCommit);
        assert_eq!(result.obligations.unresolved_callsites, 1);
        assert_eq!(result.obligations.open_world_rows, 1);
        let blockers = result.blocked_by.as_deref().unwrap_or(&[]);
        // Two blockers: one unresolved-callsite, one open-world-dispatch
        assert_eq!(blockers.len(), 2);
        let kinds: Vec<&str> = blockers.iter().map(|b| b.kind).collect();
        assert!(kinds.contains(&"unresolved-callsite"));
        assert!(kinds.contains(&"open-world-dispatch"));
    }

    // -- analysis-gap oracle ------------------------------------------------

    /// Gaps in cone produce an analysis-gap blocker with count in the detail.
    #[test]
    fn analysis_gap_blocker_kind() {
        let entry = minimal_entry("complete", vec![], vec![], vec![], false);
        let result = prove(&entry, 3, &ProveQuestion::MayCommit);
        assert_eq!(result.answer, ProveAnswer::Unknown);
        let blockers = result.blocked_by.as_deref().unwrap_or(&[]);
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].kind, "analysis-gap");
        assert!(
            blockers[0].detail.contains("3"),
            "detail should mention gap count: {}",
            blockers[0].detail
        );
        assert_eq!(result.obligations.analysis_gaps, 3);
    }

    // -- cone-truncated oracle ----------------------------------------------

    /// cone_truncated=true produces a cone-truncated blocker.
    #[test]
    fn cone_truncated_blocker_kind() {
        let entry = minimal_entry("complete", vec![], vec![], vec![], true);
        let result = prove(&entry, 0, &ProveQuestion::MayCommit);
        assert_eq!(result.answer, ProveAnswer::Unknown);
        let blockers = result.blocked_by.as_deref().unwrap_or(&[]);
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].kind, "cone-truncated");
        assert!(result.obligations.cone_truncated);
    }

    // -- witness-truncated oracle -------------------------------------------

    /// G22: another effect's truncated via paths do not block a MAY `no`: the
    /// effect is still listed, only its paths are capped, so it cannot hide a
    /// match. The MUST question still blocks on a COMMIT whose paths were cut
    /// short ([`a_truncated_commit_still_blocks_the_must_question`]).
    #[test]
    fn a_truncated_witness_does_not_block_a_may_no() {
        use crate::engine::l5::conditionality::UNKNOWN;
        let effect = DigestEffectFull {
            effect_type: "EVENT_PUBLISH".to_string(),
            detail: vec![],
            provenance: "direct",
            evidence: FullAnchor {
                source_kind: "source",
                file: Some("ws:src/Test.al".to_string()),
                line: Some(5),
                column: Some(4),
                excerpt: None,
            },
            via_paths: vec![],
            via_paths_truncated: true, // ← the key flag
            conditionality: UNKNOWN,
            transaction_context: "unknown",
            guarantees: vec![],
            fact_id: "abcdef1234567890".to_string(),
            scoped_guarantees: vec![],
            group_resources: vec![],
            group_resource_paths: vec![],
            group_terminal_path: None,
            own_path: None,
            origin_operation_ids: vec![],
            effect_is_kt: false,
            literal_unreachable: false,
            crossing: None,
        };
        let entry = minimal_entry("complete", vec![], vec![effect], vec![], false);
        let result = prove(&entry, 0, &ProveQuestion::MayCommit);
        assert_eq!(result.answer, ProveAnswer::No, "{:?}", result.blocked_by);
        assert!(result.blocked_by.is_none());
    }

    /// The MUST question judges "unconditional" over a COMMIT's paths, so a
    /// COMMIT with truncated paths blocks it with `witness-truncated`.
    #[test]
    fn a_truncated_commit_still_blocks_the_must_question() {
        let commit = DigestEffectFull {
            effect_type: "COMMIT".to_string(),
            detail: vec![],
            provenance: "direct",
            evidence: FullAnchor {
                source_kind: "source",
                file: Some("ws:src/Test.al".to_string()),
                line: Some(5),
                column: Some(4),
                excerpt: None,
            },
            via_paths: vec![],
            via_paths_truncated: true,
            conditionality: crate::engine::l5::conditionality::UNCONDITIONAL,
            transaction_context: "unknown",
            guarantees: vec![],
            fact_id: "abcdef1234567890".to_string(),
            scoped_guarantees: vec![],
            group_resources: vec![],
            group_resource_paths: vec![],
            group_terminal_path: None,
            own_path: None,
            origin_operation_ids: vec![],
            effect_is_kt: false,
            literal_unreachable: false,
            crossing: None,
        };
        let entry = minimal_entry("complete", vec![], vec![commit], vec![], false);
        let result = prove(&entry, 0, &ProveQuestion::CommitsOnSuccessPath);
        assert_eq!(result.answer, ProveAnswer::Unknown);
        let blockers = result.blocked_by.as_deref().unwrap_or(&[]);
        assert!(
            blockers.iter().any(|b| b.kind == "witness-truncated"),
            "expected witness-truncated blocker; got: {:?}",
            blockers.iter().map(|b| b.kind).collect::<Vec<_>>()
        );
    }

    /// G22: an unavailable evidence anchor does not block a MAY `no` either: the
    /// effect it belongs to is still listed.
    #[test]
    fn evidence_unavailable_does_not_block_a_may_no() {
        let entry = minimal_entry(
            "complete",
            vec![],
            vec![],
            vec![EntryDiagnostic {
                kind: "evidence-unavailable",
                effect_type: Some("COMMIT".to_string()),
                fact_subject: Some("some-fact".to_string()),
                fact_id: None,
                operation_id: None,
                reason: None,
            }],
            false,
        );
        let result = prove(&entry, 0, &ProveQuestion::MayCommit);
        assert_eq!(result.answer, ProveAnswer::No, "{:?}", result.blocked_by);
        assert!(result.blocked_by.is_none());
    }

    // -- Blocker sort oracle ------------------------------------------------

    /// Blockers are sorted deterministically by the FULL key tier:
    /// (kind, anchor file, anchor line, detail). This drives all four tiers:
    ///   - kind:   "analysis-gap" < "open-world-dispatch" < "unresolved-callsite"
    ///   - file:   same file for the two unresolved-callsite rows
    ///   - line:   5 before 20
    ///   - detail: "A in TestRoutine" < "Z in TestRoutine"
    #[test]
    fn blockers_are_deterministically_sorted() {
        // Two unresolved-callsite items in reverse callee/line order, one open-world
        // item, plus an analysis gap (count > 0). The expected sorted output exercises
        // every tier of the key.
        let entry = minimal_entry(
            "complete",
            vec![
                {
                    let mut item = unresolved_item(None);
                    item.callee_display = "Z".to_string();
                    item.callsite_line = Some(20);
                    item
                },
                {
                    let mut item = unresolved_item(None);
                    item.callee_display = "A".to_string();
                    item.callsite_line = Some(5);
                    item
                },
                {
                    let mut item = unresolved_item(Some(true));
                    item.callee_display = "OW".to_string();
                    item.callsite_line = Some(9);
                    item
                },
            ],
            vec![],
            vec![],
            false,
        );
        // gaps_in_cone_count = 2 → one analysis-gap blocker
        let result = prove(&entry, 2, &ProveQuestion::MayCommit);
        let blockers = result.blocked_by.as_deref().unwrap_or(&[]);

        // Expected canonical order: analysis-gap < open-world-dispatch < unresolved-callsite,
        // and within unresolved-callsite (same file) by line then detail.
        let seq: Vec<(&str, Option<u32>, &str)> = blockers
            .iter()
            .map(|b| {
                (
                    b.kind,
                    b.anchor.as_ref().and_then(|a| a.line),
                    b.detail.as_str(),
                )
            })
            .collect();
        assert_eq!(
            seq,
            vec![
                ("analysis-gap", None, "2 analysis gap(s) in cone"),
                ("open-world-dispatch", Some(9), "OW in TestRoutine"),
                ("unresolved-callsite", Some(5), "A in TestRoutine"),
                ("unresolved-callsite", Some(20), "Z in TestRoutine"),
            ],
            "blockers must be sorted by (kind, file, line, detail) tiers"
        );
    }

    // -- #1 symbol-evidence anchor survives oracle --------------------------

    /// A non-unconditional COMMIT whose evidence has sourceKind "symbol" (a `.app`
    /// dependency routine) STILL produces a blocker with the FULL anchor (incl. excerpt),
    /// because the TS gate is `sourceKind !== "unavailable"` (BOTH source AND symbol),
    /// not `== "source"`. Corpus-invisible — the prove corpus only has "source" commits.
    #[test]
    fn non_unconditional_symbol_evidence_keeps_anchor() {
        use crate::engine::l5::conditionality::LOOP_BODY;
        let effect = DigestEffectFull {
            effect_type: "COMMIT".to_string(),
            detail: vec![],
            provenance: "transitive",
            evidence: FullAnchor {
                source_kind: "symbol", // ← from a .app symbol routine
                file: Some("app:dep/src/Sym.al".to_string()),
                line: Some(42),
                column: Some(8),
                excerpt: Some("Commit()".to_string()),
            },
            via_paths: vec![],
            via_paths_truncated: false,
            conditionality: LOOP_BODY, // non-unconditional
            transaction_context: "unknown",
            guarantees: vec![],
            fact_id: "1234567890abcdef".to_string(),
            scoped_guarantees: vec![],
            group_resources: vec![],
            group_resource_paths: vec![],
            group_terminal_path: None,
            own_path: None,
            origin_operation_ids: vec![],
            effect_is_kt: false,
            literal_unreachable: false,
            crossing: None,
        };
        let entry = minimal_entry("complete", vec![], vec![effect], vec![], false);
        let result = prove(&entry, 0, &ProveQuestion::CommitsOnSuccessPath);
        assert_eq!(result.answer, ProveAnswer::Unknown);
        let blockers = result.blocked_by.as_deref().unwrap_or(&[]);
        let nu = blockers
            .iter()
            .find(|b| b.kind == "non-unconditional-effect-exists")
            .expect("expected non-unconditional-effect-exists blocker");
        let anchor = nu
            .anchor
            .as_ref()
            .expect("symbol-evidence anchor must survive (sourceKind != unavailable)");
        assert_eq!(anchor.source_kind, "symbol");
        assert_eq!(anchor.file.as_deref(), Some("app:dep/src/Sym.al"));
        assert_eq!(anchor.line, Some(42));
        assert_eq!(
            anchor.excerpt.as_deref(),
            Some("Commit()"),
            "the full evidence anchor (incl. excerpt) must be kept"
        );
    }

    /// Counter-case: an "unavailable" evidence anchor on a non-unconditional COMMIT
    /// drops the blocker anchor (the only sourceKind that drops it).
    #[test]
    fn non_unconditional_unavailable_evidence_drops_anchor() {
        use crate::engine::l5::conditionality::LOOP_BODY;
        let effect = DigestEffectFull {
            effect_type: "COMMIT".to_string(),
            detail: vec![],
            provenance: "transitive",
            evidence: FullAnchor {
                source_kind: "unavailable",
                file: None,
                line: None,
                column: None,
                excerpt: None,
            },
            via_paths: vec![],
            via_paths_truncated: false,
            conditionality: LOOP_BODY,
            transaction_context: "unknown",
            guarantees: vec![],
            fact_id: "fedcba0987654321".to_string(),
            scoped_guarantees: vec![],
            group_resources: vec![],
            group_resource_paths: vec![],
            group_terminal_path: None,
            own_path: None,
            origin_operation_ids: vec![],
            effect_is_kt: false,
            literal_unreachable: false,
            crossing: None,
        };
        let entry = minimal_entry("complete", vec![], vec![effect], vec![], false);
        let result = prove(&entry, 0, &ProveQuestion::CommitsOnSuccessPath);
        let blockers = result.blocked_by.as_deref().unwrap_or(&[]);
        let nu = blockers
            .iter()
            .find(|b| b.kind == "non-unconditional-effect-exists")
            .expect("expected non-unconditional-effect-exists blocker");
        assert!(
            nu.anchor.is_none(),
            "unavailable evidence must drop the blocker anchor"
        );
    }

    /// Final CDO output review, item 1: the human `blockedBy:` block groups
    /// the blockers by kind (first-seen order), states each kind's count
    /// split by where the call site is, and lists at most
    /// [`BLOCKERS_PER_KIND`] items per kind, workspace ones first; the rest
    /// are counted on one line that also states the kind's total.
    /// `--detail` lists every item. Hand-stated blockers: one coverage row
    /// (no anchor) and five open-world rows whose two workspace ones come
    /// LAST in the engine's order, so only a workspace-first sort lists them.
    ///
    /// Discrimination (2026-11-05): dropping `items.sort_by_key(...)` in
    /// `blocker_lines` fails at "workspace ones first" (the first dependency
    /// row is listed); restored, passes.
    #[test]
    fn human_blockers_fold_by_kind_workspace_first() {
        let at = |file: Option<&str>, line: u32| {
            file.map(|f| BlockerAnchor {
                source_kind: "source".to_string(),
                file: Some(f.to_string()),
                line: Some(line),
                column: None,
                excerpt: None,
            })
        };
        let b = |kind, detail: &str, anchor| ProveBlocker {
            kind,
            detail: detail.to_string(),
            anchor,
        };
        let blockers = vec![
            b("coverage-incomplete", "coverage status: partial", None),
            b(
                "open-world-dispatch",
                "D1 in A",
                at(Some("dep:Base:a.al"), 0),
            ),
            b(
                "open-world-dispatch",
                "D2 in A",
                at(Some("dep:Base:a.al"), 1),
            ),
            b("open-world-dispatch", "N1 in B", None),
            b("open-world-dispatch", "W1 in C", at(Some("ws:src/c.al"), 2)),
            b("open-world-dispatch", "W2 in C", at(Some("ws:src/c.al"), 3)),
        ];
        let lines = blocker_lines(&blockers, false, None);
        let text = lines.join("\n");
        assert_eq!(
            lines[0],
            "  blockedBy: 6 blockers in 2 kinds (at most 3 each, workspace ones first; \
             --detail lists every one)",
            "{text}"
        );
        assert_eq!(lines[1], "    coverage incomplete: 1", "no place: {text}");
        assert_eq!(lines[2], "      coverage status: partial", "{text}");
        assert_eq!(
            lines[3],
            "    interface dispatch (open world): 5 (2 in this workspace, 2 in dependency code, 1 not placed)",
            "{text}"
        );
        assert_eq!(
            &lines[4..6],
            [
                "      W1 in C @ ws:src/c.al:3",
                "      W2 in C @ ws:src/c.al:4"
            ],
            "workspace ones first: {text}"
        );
        assert_eq!(lines[6], "      D1 in A @ dep:Base:a.al:1", "{text}");
        assert_eq!(
            lines[7], "      … +2 blockers (of 5 blockers; --detail lists them)",
            "{text}"
        );
        assert_eq!(lines.len(), 8, "{text}");
        // --detail: every item, the same grouping.
        let all = blocker_lines(&blockers, true, None);
        assert_eq!(all[0], "  blockedBy: 6 blockers in 2 kinds", "{all:?}");
        assert_eq!(all.len(), 1 + 2 + 1 + 5, "{all:?}");
        assert!(all.iter().all(|l| !l.contains("… +")), "{all:?}");
    }
}
