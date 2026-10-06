//! D1 — database operation inside a loop (direct or through an in-loop call
//! chain). Originally a port of al-sem `src/detectors/d1-db-op-in-loop.ts`; the
//! call-chain analysis is now the Rust-owned REACHABILITY pipeline
//! (`.superpowers/sdd/task-5-brief.md` — the d1-reachability redesign).
//!
//! ## Production pipeline (terminal-centric reachability)
//!
//! `detect_d1` no longer walks every simple path (the old `walk_evidence`
//! exhaustive enumeration, silently truncated at a 500-node budget). Instead:
//!   1. [`enumerate_direct_ops`] — the production direct-op (old branch (a))
//!      enumeration + the `candidatesConsidered`/`skipped_*`/`downgradedToInfo`
//!      stat counting (the branch-(b) callsite skips are counted alongside).
//!   2. [`build_d1_graph`] (`d1_graph`) — the compact filtered call graph + the
//!      in-loop-call seed set.
//!   3. [`search_loops`] (`d1_reach`) — one unbounded multi-source label search
//!      per loop group over Task 2's forward param-temp vector, aggregated per
//!      `(loop, terminal-op)` into a [`LoopTerminalAgg`] with a selected winner
//!      witness. Cycle safety is label dedup alone — NO node/depth budget.
//!   4. [`assemble_findings`] — group aggregates by `(terminal routine, op)` into
//!      ONE terminal-centric [`Finding`] per group, carrying one [`LoopContext`]
//!      per reaching loop. `contexts[0]` (the winner: severity desc, verdict
//!      quality desc, loop routine id, loop id) drives the finding's severity,
//!      confidence, `evidence_path`, temp/setup notes and wording — all from the
//!      SAME context (fixing the old best-confidence-across-loops mismatch). The
//!      non-winner witnesses become `additional_paths`.
//!
//! `id = root_cause_key = "d1/{terminal_routine_id}/{op_id}"` — TERMINAL-based
//! identity (the schema change: the old per-loop `d1/{loop}/{routine}/{op}` ids
//! are gone). Two defect classes the redesign closes: **D-A** (the 500-node
//! budget silently under-found deep terminals) and **D-B** (DFS-order-accidental
//! verdicts + canonical-loop merge accidents + best-confidence-across-loops
//! mismatch).
//!
//! ## Dependency-role path is DEAD (source-only)
//! al-sem's `terminalsAt` and the finding-build op-recovery both fall back to
//! `summary.dbEffects` for `roleOf(r) === "dependency"` routines. In the
//! SOURCE-ONLY Rust pipeline every routine is primary, so that fallback never
//! engages; it is documented inline but not implemented (mirrors `run_detectors`).

// `BTreeMap` is test-only (Task C9 replaced the production `by_rv` grouping's
// `BTreeMap<Vec<i32>, GroupBitmap>` with a bitmap partition — see the
// finest-cohort assembly below); `assemble_findings` (the `#[cfg(test)]`
// shadow-oracle path) still groups aggregates with one.
#[cfg(test)]
use std::collections::BTreeMap;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use crate::engine::l3::l3_workspace::L3Table;
use crate::engine::l3::l3_workspace::{L3RecordOperation, L3Resolved, L3Routine, L3Workspace};
use crate::engine::l4::combined_graph::CombinedEdge;
// Only the `search_loops`/`WalkResult` shadow oracle still handles OWNED
// uncertainties; the production cohort path carries `UncertaintyId`s and resolves
// them through the run-level `UncertaintyTable`.
#[cfg(test)]
use crate::engine::l4::summary::Uncertainty;
use crate::engine::l5::actionable_anchor::pick_actionable_anchor;
use crate::engine::l5::confidence::{UncertaintyLite, to_confidence};
use crate::engine::l5::d1_cohort::{
    GroupBitmap, LoopSetId, LoopSetRegistry, UncertaintyId, reachable_verdicts_of,
};
use crate::engine::l5::d1_graph::build_d1_graph;
use crate::engine::l5::d1_reach::{D1CohortRun, DirectOp, search_loops_cohorts};
#[cfg(test)]
use crate::engine::l5::d1_reach::{LoopTerminalAgg, search_loops};
use crate::engine::l5::d1_witness::StepInterner;
#[cfg(test)]
use crate::engine::l5::detector_context::OwnedUncertainties;
use crate::engine::l5::detector_context::{DetectorContext, UncertaintyIndex};
use crate::engine::l5::detectors::{
    anchor_of, is_known_temp, is_terminator_next, op_targets_virtual_system_table,
    unquoted_field_name,
};
use crate::engine::l5::finding::{
    D1CohortContext, D1CohortIndex, Evidence, EvidenceStep, Finding, FindingConfidence, FixOption,
    SourceAnchor,
};
#[cfg(test)]
use crate::engine::l5::finding::{LoopContext, id_list};
use crate::engine::l5::op_classification::{classify_op, is_db_touching_class};
use crate::engine::l5::path_merge::{annotate_root_cause, sev_rank};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::engine::l5::table_display::{DescribeOp, describe_table};
use crate::engine::perf_trace as pt;

// The OLD exhaustive-path-walker pipeline (`detect_d1_premerge` + `D1Policy` +
// `build_finding` + `apply_seed_transform`) survives ONLY as the Task 4 shadow
// oracle — the regression net the `shadow_tests` / `memo_tests` unit tests below
// run side-by-side with the production reachability pipeline. It is off the
// production path entirely, so it (and the imports it alone needs) is
// `#[cfg(test)]`-gated: it never enters a release build.
#[cfg(test)]
use crate::engine::l4::effect_lattice::TempStateKind;
#[cfg(test)]
use crate::engine::l5::capability_query::{EffectPresence, touches_db_derived};
#[cfg(test)]
use crate::engine::l5::closed_world_temp::ClosedWorldTempParams;
#[cfg(test)]
use crate::engine::l5::path_temp_resolve::resolve_temp_along_path_closed_world;
#[cfg(test)]
use crate::engine::l5::path_walker::{
    PathCtx, Terminal, WalkBounds, WalkOpts, WalkPolicy, WalkResult, WalkStop, walk_evidence,
};
#[cfg(test)]
use std::cell::RefCell;
#[cfg(test)]
use std::rc::Rc;

const DETECTOR: &str = "d1-db-op-in-loop";

/// The OLD path-walker's depth/node budget for the interprocedural call-chain
/// walk. Kept only for the `#[cfg(test)]` shadow oracle (`detect_d1_premerge`) —
/// the production reachability search has NO budget (cycle safety = label dedup).
#[cfg(test)]
const BOUNDS: WalkBounds = WalkBounds {
    max_depth: 20,
    max_nodes: 500,
};

const WRITE_OPS: [&str; 6] = [
    "Modify",
    "ModifyAll",
    "Insert",
    "Delete",
    "DeleteAll",
    "Rename",
];
const HEAVY_READ_OPS: [&str; 2] = ["CalcFields", "CalcSums"];
/// RV-1 (Task 11): ops whose temp-downgrade is GATED on the field arguments. A
/// FlowField calculation queries the (physical) flow-target tables even on a
/// temporary host record, so the temp ⇒ info downgrade only applies when EVERY
/// named field argument is a non-FlowField (Blob/Normal → in-memory).
pub(crate) const FLOWFIELD_GATED_OPS: [&str; 2] = ["CalcFields", "SetAutoCalcFields"];
const RETRIEVAL_OPS: [&str; 6] = ["FindSet", "FindFirst", "FindLast", "Find", "Get", "Next"];
/// Ops that open a recordset cursor BEFORE a `repeat..until` loop. An in-loop
/// `Next` on the same record-var IS the cursor advance, not an N+1 antipattern.
const CURSOR_OPENER_OPS: [&str; 4] = ["FindSet", "FindFirst", "FindLast", "Find"];

/// The terminal op's `temp_state` as a [`TempStateKind`] (the resolver's input).
/// A `None` temp_state → `Unknown` (al-sem always sets `{kind:"unknown"}` for
/// untracked ops, so the absence maps the same way). Test-only: the production
/// path resolves temp-ness via `d1_temp`'s forward vector, not this.
#[cfg(test)]
fn op_temp_state_kind(op: &L3RecordOperation) -> TempStateKind {
    match &op.temp_state {
        Some(ts) => TempStateKind::from_p_temp_state(ts),
        None => TempStateKind::Unknown,
    }
}

/// RV-1 (Task 11): the FlowField gate for a temp `CalcFields`/`SetAutoCalcFields`.
///
/// A temporary host record's FlowField is still computed by evaluating its
/// CalcFormula against the (physical) flow-target tables — a real SQL query,
/// host-tempness irrelevant. Blob/Normal field loads ARE in-memory. So the temp ⇒
/// info downgrade may only apply when EVERY named field argument resolves (via the
/// table model) to `field_class != "FlowField"`.
///
/// Returns `true` when the downgrade is BLOCKED (keep firing): ANY field arg is a
/// FlowField, OR any field arg is UNRESOLVABLE (name not in the table, table_id is
/// None, or the table is not in `table_by_id`), OR there are NO capturable field
/// arguments (conservative). Returns `false` only when every field arg is a
/// confirmed non-FlowField → safe to downgrade as in-memory.
///
/// SOUNDNESS: this only ever PREVENTS a downgrade (keeps firing) when uncertain; it
/// never suppresses a finding that would otherwise fire.
pub(crate) fn flowfield_gate_blocks_downgrade(
    op: &L3RecordOperation,
    table_by_id: &HashMap<&str, &L3Table>,
) -> bool {
    // Resolve the op's table; an unresolved table is conservative → block.
    let Some(table_id) = op.table_id.as_deref() else {
        return true;
    };
    let Some(table) = table_by_id.get(table_id).copied() else {
        return true;
    };

    // The named field arguments. `field_argument_infos` carries the structured,
    // unquoted-resolvable form (mirrors d22/d18); an empty/None list means we could
    // not capture any field name → conservative → block.
    let Some(infos) = &op.field_argument_infos else {
        return true;
    };
    if infos.is_empty() {
        return true;
    }

    for info in infos {
        let arg_lc = unquoted_field_name(info).to_lowercase();
        let field = table
            .fields
            .iter()
            .find(|f| f.name.to_lowercase() == arg_lc);
        match field {
            // Unresolvable field name (not in the table) → conservative → block.
            None => return true,
            // ANY FlowField field arg → the calculation queries the flow targets.
            Some(f) if f.field_class == "FlowField" => return true,
            Some(_) => {}
        }
    }
    // Every field arg is a confirmed non-FlowField → in-memory → allow the downgrade.
    false
}

/// The PATH-RESOLVED temp verdict for a single finding (Component 3 / RV-6).
/// Derived from `resolve_temp_along_path` over THIS finding's evidence path:
///   - `Temporary`  ← resolved `Known(true)`  → severity forced to `info`;
///   - `Physical`   ← resolved `Known(false)` → fires at normal severity, no temp note;
///   - `Uncertain`  ← resolved `Unknown`      → fires at normal severity, "(temp state uncertain)".
///   - `FlowFieldGated` ← RV-1 (Task 11): the path resolved `Temporary`, but the
///     terminal op is a `CalcFields`/`SetAutoCalcFields` whose FlowField gate BLOCKS
///     the info-downgrade (a FlowField — or unresolvable — field arg). It fires at
///     NORMAL severity (like `Physical` — no info downgrade) but carries its OWN note
///     (`NOTE_TEMP_FLOWFIELD`): the host record is in-memory yet the FlowField
///     CalcFormula still queries the physical flow targets. A DEDICATED variant (not a
///     faked `Physical`) so the merge-tie reconciliation preserves the FlowField fact
///     in the dual-verdict note instead of silently dropping it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum TempVerdict {
    Temporary,
    Physical,
    Uncertain,
    FlowFieldGated,
}

impl TempVerdict {
    /// Map a resolved [`TempStateKind`] to a verdict. Test-only: the OLD
    /// `build_finding` used it after the backward per-path resolver; the
    /// production path composes verdicts forward in `d1_reach::flowfield_verdict`.
    #[cfg(test)]
    fn from_resolved(state: &TempStateKind) -> Self {
        match state {
            TempStateKind::Known(true) => TempVerdict::Temporary,
            TempStateKind::Known(false) => TempVerdict::Physical,
            // PD should never survive resolution (the resolver always returns a
            // concrete Known/Unknown), but a residual PD is treated as uncertain.
            TempStateKind::Unknown | TempStateKind::ParameterDependent(_) => TempVerdict::Uncertain,
        }
    }

    /// The verdict label fragment (`temporary` / `physical` / `uncertain` /
    /// `flowfield-on-temp`) surfaced in a [`LoopContext`]'s `verdict` /
    /// `reachable_verdicts`.
    pub(crate) fn label(self) -> &'static str {
        match self {
            TempVerdict::Temporary => "temporary",
            TempVerdict::Physical => "physical",
            TempVerdict::Uncertain => "uncertain",
            TempVerdict::FlowFieldGated => "flowfield-on-temp",
        }
    }

    /// Verdict-quality rank for the winner-selection / context ordering:
    /// `Physical == FlowFieldGated > Uncertain > Temporary`. (Distinct from the
    /// derived `Ord`, which is declaration order and only used to sort a
    /// context's `reachable_verdicts` list.)
    pub(crate) fn quality(self) -> i32 {
        match self {
            TempVerdict::Physical | TempVerdict::FlowFieldGated => 3,
            TempVerdict::Uncertain => 2,
            TempVerdict::Temporary => 1,
        }
    }
}

/// A pre-merge finding from the OLD walker. Test-only: the shadow differential
/// (`shadow_tests` below) reads `.finding` off the raw pre-merge population
/// `detect_d1_premerge` returns. (The old `verdict`/`caller_label` fields died
/// with `reconcile_merge_tie` — the redesign carries per-loop verdicts in
/// `Finding.contexts`, not a reconciliation note.)
#[cfg(test)]
pub(crate) struct FindingRec {
    pub(crate) finding: Finding,
}

/// `describeTable(op, routine, tableById)`. Builds the `DescribeOp` view from an
/// `L3RecordOperation`.
fn describe_op_table(
    op: &L3RecordOperation,
    routine: Option<&L3Routine>,
    table_by_id: &HashMap<&str, &L3Table>,
) -> String {
    let describe = DescribeOp {
        table_id: op.table_id.as_deref(),
        record_variable_name: &op.record_variable_name,
    };
    describe_table(&describe, routine, table_by_id)
}

/// `tableNote(op, routine, tableById)` → `"<Op> on <table>"`.
fn table_note(
    op: &L3RecordOperation,
    routine: Option<&L3Routine>,
    table_by_id: &HashMap<&str, &L3Table>,
) -> String {
    format!(
        "{} on {}",
        op.op,
        describe_op_table(op, routine, table_by_id)
    )
}

/// `isSetupSingletonGet`: op is `Get` AND the rendered table name (minus the
/// `(type not loaded)` suffix) ends in `Setup` (case-insensitive) AND is not a
/// `var ` / `unknown table` / empty placeholder.
pub(crate) fn is_setup_singleton_get(
    op: &L3RecordOperation,
    routine: Option<&L3Routine>,
    table_by_id: &HashMap<&str, &L3Table>,
) -> bool {
    if op.op != "Get" {
        return false;
    }
    let display = describe_op_table(op, routine, table_by_id);
    // Strip the `(type not loaded)` suffix (case-insensitive) then trim.
    let name = strip_type_not_loaded(&display);
    let name = name.trim();
    if name.is_empty() || name.starts_with("var ") || name == "unknown table" {
        return false;
    }
    ends_with_setup_ci(name)
}

/// `display.replace(/\s*\(type not loaded\)$/i, "")`: strip a trailing
/// (case-insensitive) `(type not loaded)` plus any whitespace immediately before
/// it. Anchored at the end only.
fn strip_type_not_loaded(display: &str) -> String {
    // The suffix is pure ASCII, so match it case-insensitively over the trailing
    // BYTES of `display` directly (never via a lowercased copy — `to_lowercase` is
    // not length-preserving, so a byte offset from the lowercased string would slice
    // `display` mid-char for non-ASCII names). A trailing match guarantees the cut
    // byte is `(` (ASCII) → a valid char boundary.
    let suffix = b"(type not loaded)";
    let db = display.as_bytes();
    if db.len() >= suffix.len() {
        let start = db.len() - suffix.len();
        if db[start..].eq_ignore_ascii_case(suffix) {
            return display[..start].trim_end().to_string(); // `\s*` before the suffix
        }
    }
    display.to_string()
}

/// `/\bSetup$/i.test(name)`: the name ends in `Setup` (case-insensitive) on a word
/// boundary. JS `\b`/`\w` are ASCII-only, so the boundary char (from the ORIGINAL
/// `name`, never a lowercased copy) is tested with ASCII word-ness.
fn ends_with_setup_ci(name: &str) -> bool {
    let suf = b"setup";
    let nb = name.as_bytes();
    if nb.len() < suf.len() {
        return false;
    }
    let start = nb.len() - suf.len();
    if !nb[start..].eq_ignore_ascii_case(suf) {
        return false;
    }
    // `start` is a char boundary (nb[start] is the ASCII 's'/'S' of "setup").
    if start == 0 {
        return true; // "Setup" is the whole name — boundary at string start.
    }
    let prev = name[..start].chars().next_back().unwrap();
    !(prev.is_ascii_alphanumeric() || prev == '_')
}

/// `representativeLoopId(loopStack)` — the innermost (last) loop.
fn representative_loop_id(loop_stack: &[String]) -> Option<&str> {
    loop_stack.last().map(|s| s.as_str())
}

/// `severityFor(op, effectiveLoopDepth, isSetupSingleton)`.
///
/// Component 3 / RV-6 (Task 10): the temp-derived `info` downgrade now keys off the
/// PATH-RESOLVED verdict (`TempVerdict::Temporary`), not the raw `op.temp_state`. A
/// terminal op that is already `Known(true)` resolves `Temporary` immediately (no
/// stepping), so this is BEHAVIOUR-PRESERVING for non-PD ops; only PD-terminal
/// (by-var param) ops gain per-path precision.
pub(crate) fn severity_for(
    op: &L3RecordOperation,
    verdict: TempVerdict,
    effective_loop_depth: i64,
    is_setup_singleton: bool,
) -> &'static str {
    // Only `Temporary` forces the info downgrade. `FlowFieldGated` (RV-1 / Task 11)
    // deliberately does NOT — it fires at the op-based severity, like `Physical`.
    if verdict == TempVerdict::Temporary {
        return "info";
    }
    if is_setup_singleton {
        return "info";
    }
    // al-sem orders these as distinct branches (write → high, heavy-read → high,
    // retrieval → medium, db-lock → low, else medium). The write + heavy-read arms
    // both yield "high"; they are merged here (clippy `if_same_then_else`) with the
    // SAME precedence — `op` is in at most one of the disjoint op-sets, so the OR is
    // behaviourally identical to the two ordered branches.
    let mut base: &'static str =
        if WRITE_OPS.contains(&op.op.as_str()) || HEAVY_READ_OPS.contains(&op.op.as_str()) {
            "high" // write inside loop / FlowField materialisation = high
        } else if RETRIEVAL_OPS.contains(&op.op.as_str()) {
            "medium" // pure retrieval = medium
        } else if classify_op(&op.op).as_str() == "db-lock" {
            "low"
        } else {
            "medium"
        };
    if effective_loop_depth >= 2 {
        if base == "high" {
            base = "critical";
        } else if base == "medium" {
            base = "high";
        }
    }
    base
}

/// Convert a walk's accumulated OWNED `Uncertainty` set to `UncertaintyLite`s.
/// `#[cfg(test)]` — only the `search_loops`/`WalkResult` oracle still holds owned
/// uncertainties; the production cohort path uses
/// [`uncertainty_lites_of_ids`].
#[cfg(test)]
fn uncertainty_lites(uncertainties: &[Uncertainty]) -> Vec<UncertaintyLite> {
    uncertainties.iter().map(UncertaintyLite::of).collect()
}

/// Convert a cohort's INTERNED uncertainty union to `UncertaintyLite`s, resolving
/// each id through the run-level `table`. Order is the id sequence's order, which
/// is the de-duped, key-sorted order `UncertaintyTable::dedupe` produced — the
/// same sequence the owned-value form produced before the ids existed.
///
/// The clone is a pair of refcount bumps, not text: the table materialised each
/// distinct uncertainty's `kind` and evidence note once, and every
/// `Evidence` this produces shares those allocations (see
/// [`crate::engine::l5::finding::Evidence`]).
fn uncertainty_lites_of_ids(
    ids: &[UncertaintyId],
    index: &UncertaintyIndex,
) -> Vec<UncertaintyLite> {
    ids.iter().map(|&id| index.lite(id).clone()).collect()
}

/// `buildFinding(...)` — assemble the internal Finding for the OLD walker path.
/// Test-only (the shadow oracle): the production pipeline assembles findings from
/// `LoopTerminalAgg`s in [`assemble_findings`], not from a single `WalkResult`.
///
/// `terminal_routine_id` is al-sem's `terminalOp.routineId` (a separate field on
/// `RecordOperation`; the Rust `L3RecordOperation` carries no routine id, so the
/// caller threads the owning routine's internal id). `terminal_op_anchor` is the
/// op's INTERNAL `SourceAnchor` (built by the caller via `anchor_of`).
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn build_finding(
    loop_routine: &L3Routine,
    representative_loop: &str,
    result: &WalkResult,
    terminal_op: &L3RecordOperation,
    terminal_routine_id: &str,
    terminal_op_anchor: SourceAnchor,
    routine_by_id: &HashMap<&str, &L3Routine>,
    table_by_id: &HashMap<&str, &L3Table>,
    role_by_routine: &HashMap<&str, &str>,
    edge_kind_by_callsite: &HashMap<&str, &str>,
    closed_world_temp_params: &ClosedWorldTempParams,
) -> (Finding, TempVerdict) {
    let terminal_routine = routine_by_id.get(terminal_routine_id).copied();
    let setup_singleton = is_setup_singleton_get(terminal_op, terminal_routine, table_by_id);

    // Component 3 / RV-6 (Task 10): resolve the terminal op's temp_state EXACTLY
    // along THIS finding's evidence path. A non-PD op resolves immediately (no
    // stepping) so the verdict equals the raw state — behaviour-preserving. A
    // PD-terminal (by-var param) op resolves per-path: temp on a temp-caller path,
    // physical on a physical-caller path, uncertain at a path root. The edge-kind
    // allowlist guard inside the resolver keeps dynamic/interface/run hops sound.
    // G-19: the closed-world proven set lets a PD frame belonging to a `local`
    // all-temp-callers routine resolve Known(true) even at a path root (the
    // intra-callee shape) — see `closed_world_temp`.
    let resolved = resolve_temp_along_path_closed_world(
        &result.path,
        op_temp_state_kind(terminal_op),
        routine_by_id,
        edge_kind_by_callsite,
        closed_world_temp_params,
    );
    let resolved_verdict = TempVerdict::from_resolved(&resolved);

    // RV-1 (Task 11): the FlowField gate. A temp `CalcFields`/`SetAutoCalcFields`
    // only downgrades to info when EVERY named field arg is a confirmed
    // non-FlowField (Blob/Normal → in-memory). A FlowField — or any unresolvable
    // field arg — keeps the op FIRING because its CalcFormula queries the physical
    // flow targets. When the gate blocks, the verdict becomes the DEDICATED
    // `FlowFieldGated` (fires at normal severity like `Physical`, but carries its own
    // FlowField note) — NOT a faked `Physical`, so the merge-tie reconciliation can
    // preserve the FlowField fact when this path merges with a genuinely-physical one.
    let verdict = if resolved_verdict == TempVerdict::Temporary
        && FLOWFIELD_GATED_OPS.contains(&terminal_op.op.as_str())
        && flowfield_gate_blocks_downgrade(terminal_op, table_by_id)
    {
        TempVerdict::FlowFieldGated
    } else {
        resolved_verdict
    };

    let severity = severity_for(
        terminal_op,
        verdict,
        result.effective_loop_depth,
        setup_singleton,
    );

    let temp_note = match verdict {
        TempVerdict::Temporary => NOTE_TEMPORARY,
        TempVerdict::Uncertain => NOTE_UNCERTAIN,
        TempVerdict::FlowFieldGated => NOTE_TEMP_FLOWFIELD,
        // Physical: a concrete physical record reached along this path — honest
        // omission (no temp note), matching the prior Known(false) `""` branch.
        TempVerdict::Physical => "",
    };
    let setup_note = if setup_singleton {
        " (Setup singleton — BC caches Get() per session, so the round-trip happens at most once.)"
    } else {
        ""
    };

    let id = format!(
        "d1/{}/{}/{}",
        representative_loop, terminal_routine_id, terminal_op.id
    );
    let root_cause_key = format!("d1/{}/{}", terminal_routine_id, terminal_op.id);

    // G-4 (docs/engine-gaps.md): PURE-TRANSITIVE wording. When the terminal op's
    // OWN routine is not the loop routine AND the op sits in no loop of its own
    // (empty loop_stack), the original "A loop in X reaches <op on table>." reads
    // as if the terminal routine loops. The finding is GENUINELY REAL (the op runs
    // once per ancestor iteration — real SQL cost), so the fix is WORDING ONLY:
    // name the terminal routine and attribute the loop to the ancestor explicitly.
    // Severity / confidence / id / rootCauseKey / fingerprint are all unchanged.
    let pure_transitive = terminal_routine_id != loop_routine.id
        && terminal_op.loop_stack.is_empty()
        && terminal_routine.is_some();
    let root_cause = if pure_transitive {
        let tr = terminal_routine.expect("guarded by pure_transitive");
        format!(
            "A loop in {} reaches {} in {}, which has no loop of its own \u{2014} the \
             operation runs once per iteration of that loop{}{}.",
            loop_routine.name,
            table_note(terminal_op, terminal_routine, table_by_id),
            tr.name,
            temp_note,
            setup_note
        )
    } else {
        format!(
            "A loop in {} reaches {}{}{}.",
            loop_routine.name,
            table_note(terminal_op, terminal_routine, table_by_id),
            temp_note,
            setup_note
        )
    };

    // affectedObjects = sorted-dedup [loopRoutine.objectId, terminalRoutine?.objectId].
    let mut affected_set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    affected_set.insert(loop_routine.object_id.clone());
    if let Some(tr) = terminal_routine {
        affected_set.insert(tr.object_id.clone());
    }
    let affected_objects: Vec<String> = affected_set.into_iter().collect();

    let affected_tables: Vec<String> = match &terminal_op.table_id {
        Some(t) => vec![t.clone()],
        None => Vec::new(),
    };

    let confidence: FindingConfidence =
        to_confidence(&uncertainty_lites(&result.uncertainties), "likely");

    let fix_options = if setup_singleton {
        vec![FixOption {
            description: "Setup tables are session-cached by BC, so a Get() inside a loop is \
                          typically O(1) after the first hit. Hoist the Get() outside the loop \
                          only if the call site shows up in a CPU profile."
                .into(),
            safety: "high".into(),
        }]
    } else {
        vec![FixOption {
            description: "Move the database operation outside the loop, or batch it into a \
                          set-based operation."
                .into(),
            safety: "medium".into(),
        }]
    };

    let mut finding = Finding {
        id,
        root_cause_key,
        detector: DETECTOR.to_string(),
        title: "Database operation inside a loop".into(),
        root_cause,
        severity: severity.to_string(),
        confidence,
        primary_location: terminal_op_anchor,
        evidence_path: result.path.clone(),
        additional_paths: None,
        affected_objects: id_list(affected_objects),
        affected_tables: id_list(affected_tables),
        fix_options,
        provenance: vec![Evidence {
            source: "tree-sitter",
            note: None,
        }],
        actionable_anchor: None,
        fingerprint: None,
        event_kind: None,
        cross_extension_subscribers: None,
        cohort_contexts: None,
    };

    let actionable = pick_actionable_anchor(&finding, role_by_routine);
    if actionable.is_some() {
        finding.actionable_anchor = actionable;
    }
    (finding, verdict)
}

/// G-18 (docs/engine-gaps.md): does this resolved edge's TARGET routine actually
/// carry the call site's own callee name?
///
/// Why this is needed: the internal routine id has NO member discriminator, so
/// two same-name same-signature triggers in one object (two page actions'
/// `OnAction`, two fields' `OnValidate`, …) collide on the id — and with them
/// every derived id (`{rid}/cs{n}`). The combined graph then files BOTH bodies'
/// edges under the one shared `from` key, and a lookup by callsite id alone can
/// return the SIBLING body's edge — splicing an in-loop call site onto a call
/// chain the loop is not on (the G-18 false positive).
///
/// Why it can never reject a genuinely-own edge: the call resolver is NAME-keyed
/// — a `direct`/`method` edge's target routine always carries the call site's
/// callee name (case-insensitive, quotes stripped). Un-nameable callees
/// (object-run / unknown) and out-of-source targets (no routine entry) are
/// ACCEPTED — the pre-G-18 behavior — so the guard only ever filters cross-body
/// edges under a colliding id; it cannot suppress a genuine transitive finding.
/// (Implicit-trigger edges never reach this guard: their `callsite_id` is the
/// record-op id `{rid}/op{n}`, which can never equal a call site's `{rid}/cs{n}`.)
pub(crate) fn edge_target_matches_callsite_callee(
    edge: &CombinedEdge,
    cs: &crate::engine::l2::features::PCallSite,
    routine_by_id: &HashMap<&str, &L3Routine>,
) -> bool {
    use crate::engine::l2::features::PCallee;
    let callee_name = match &cs.callee {
        PCallee::Bare { name } => name,
        PCallee::Member { method, .. } => method,
        // No comparable method name — accept (cannot disambiguate; conservative
        // in the keep-firing direction).
        PCallee::ObjectRun { .. } | PCallee::Unknown => return true,
    };
    let Some(target) = routine_by_id.get(edge.to.as_str()) else {
        return true; // out-of-source target — accept (pre-G-18 behavior)
    };
    crate::engine::l2::node_util::strip_quotes(callee_name).to_lowercase()
        == target.name.to_lowercase()
}

/// The D1 WalkPolicy — holds references to the eager indexes the closures read.
/// Test-only: it drives the OLD `walk_evidence` exhaustive walk, kept solely as
/// the `#[cfg(test)]` shadow oracle.
#[cfg(test)]
struct D1Policy<'a> {
    routine_by_id: &'a HashMap<&'a str, &'a L3Routine>,
    table_by_id: &'a HashMap<&'a str, &'a L3Table>,
    summaries: &'a HashMap<String, crate::engine::l5::full_summary::FullRoutineSummary>,
    edges_by_from: &'a HashMap<String, Vec<CombinedEdge>>,
    call_site_by_id: &'a HashMap<&'a str, &'a crate::engine::l2::features::PCallSite>,
    /// ⟨C1 Task 2⟩ The derived cone substrate the `touches_db` probe reads. Held
    /// as a borrow off the same `DetectorContext` that owns `summaries`, so the
    /// two can never describe different cone walks.
    cone_derived: &'a crate::engine::l4::cone_derived::ConeDerivedStore,
    /// Per-run memo of `touches_db_of`, keyed by callee routine id. `expand`
    /// probes the same callee's cone once per INCOMING edge across the whole
    /// `walk_evidence` DFS — and cones reach thousands of facts at high graph
    /// density — so the first probe used to walk the chain iterator (early-exiting
    /// on the first `"table"` fact) and every later probe of that routine is an
    /// O(1) memo hit. ⟨C1 Task 2⟩ The first probe is now itself O(1) (a folded
    /// presence flag), but the memo is retained: it is part of this struct's
    /// shape and the walk's hot path is unchanged. ⟨fix M1⟩ Not free, though:
    /// it now trades what used to be an O(cone-size) scan for one hash-map
    /// insert per distinct routine — a net win, not a costless one. `expand`
    /// only has `&self`, hence `RefCell` for the lazy fill. Keys borrow from
    /// `summaries` (owned by `ctx`), which outlives the walk. The probe is a pure
    /// function of the (immutable) summary + store, so the answer is stable for
    /// the run.
    touches_db_memo: RefCell<HashMap<&'a str, EffectPresence>>,
    /// Per-run memo of the CANONICAL interprocedural walk from a callee entry
    /// (`initial_loop_depth: 0`, empty prefix), keyed by callee routine id.
    /// `detect_d1` re-seeds one `walk_evidence` per IN-LOOP CALLSITE into a
    /// db-touching callee; at Base-App density many callsites share the same hot
    /// callee, so each was re-walking the same ≤500-node dense subgraph. The walk
    /// from a callee is a PURE FUNCTION of the callee (`D1Policy` reads only
    /// run-global indexes; no method reads the calling routine — see the Wave-2c
    /// §3 proof), so the caller-specific result is recovered by the mechanical
    /// `apply_seed_transform` (prepend the `[loopStep, callStep]` prefix, ADD the
    /// callsite's loop depth) — byte-identical. `Rc` so a lookup hands back the
    /// shared canonical vec without re-cloning it. `detect_d1` is the sole
    /// accessor; `walk_evidence` fills `touches_db_memo` (a DISTINCT cell) while
    /// this one is borrowed, so the two never alias.
    walk_memo: RefCell<HashMap<String, Rc<Vec<WalkResult>>>>,
}

#[cfg(test)]
impl<'a> D1Policy<'a> {
    /// `touches_db_of(s)` — ⟨C1 Task 2⟩ served off the derived cone substrate —
    /// memoized once-per-run by the summary's routine id.
    fn touches_db_memoized(
        &self,
        s: &'a crate::engine::l5::full_summary::FullRoutineSummary,
    ) -> EffectPresence {
        *self
            .touches_db_memo
            .borrow_mut()
            .entry(s.routine_id.as_str())
            .or_insert_with(|| touches_db_derived(self.cone_derived, s))
    }
}

/// A HOP evidence step for a `from -> to` call edge. Extracted verbatim from the
/// old `D1Policy::build_hop_step` (`&self` -> explicit `routine_by_id` +
/// per-edge params) so the reachability search (`d1_reach`) can build the SAME
/// hop step off a compact [`crate::engine::l5::d1_graph::D1Edge`] without a live
/// `D1Policy`. `build_hop_step` now delegates here — behaviour-preserving (the
/// full suite proves the old walk path is byte-identical).
pub(crate) fn hop_step(
    routine_by_id: &HashMap<&str, &L3Routine>,
    from: &str,
    to: &str,
    kind: &str,
    callsite_id: Option<&str>,
) -> EvidenceStep {
    let from_routine = routine_by_id.get(from).copied();
    let cs = callsite_id.and_then(|cid| {
        from_routine.and_then(|fr| fr.call_sites.iter().find(|c| c.id.as_str() == cid))
    });
    let to_name = routine_by_id
        .get(to)
        .map(|r| r.name.clone())
        .unwrap_or_else(|| to.to_string());
    let trigger_note = if kind == "implicit-trigger" {
        format!(" (via implicit {to_name} trigger)")
    } else {
        String::new()
    };
    let source_anchor = if let Some(cs) = cs {
        anchor_of(&cs.source_anchor, from_routine.unwrap())
    } else if let Some(fr) = from_routine {
        anchor_of(&fr.source_anchor, fr)
    } else {
        SourceAnchor {
            source_unit_id: String::new(),
            start_line: 0,
            start_column: 0,
            end_line: 0,
            end_column: 0,
            enclosing_routine_id: from.to_string(),
            syntax_kind: "call".to_string(),
            normalized_text_hash: None,
            leading_context_hash: None,
            trailing_context_hash: None,
        }
    };
    EvidenceStep {
        routine_id: from.to_string(),
        operation_id: None,
        callsite_id: callsite_id.map(|s| s.to_string()),
        loop_id: None,
        source_anchor,
        note: format!("calls {to_name}{trigger_note}"),
    }
}

/// The TERMINAL evidence step for a db op owned by `terminal_routine_id`.
/// Extracted verbatim from the old `D1Policy::build_terminal_step` (`&self` ->
/// explicit `routine_by_id`/`table_by_id` + `(routine_id, op_id)` params) so the
/// reachability search (`d1_reach`) can build the SAME terminal step off a
/// compact [`crate::engine::l5::d1_graph::D1Terminal`]. `build_terminal_step` now
/// delegates here — behaviour-preserving.
pub(crate) fn terminal_step(
    routine_by_id: &HashMap<&str, &L3Routine>,
    table_by_id: &HashMap<&str, &L3Table>,
    terminal_routine_id: &str,
    op_id: Option<&str>,
) -> EvidenceStep {
    let routine = routine_by_id.get(terminal_routine_id).copied();
    let op = op_id.and_then(|oid| {
        routine.and_then(|r| r.record_operations.iter().find(|o| o.id.as_str() == oid))
    });
    // op is always Some on the primary path (the op_id was just emitted by
    // terminals_at over the SAME routine's record_operations).
    let (op_id_out, anchor, note) = match op {
        Some(op) => (
            Some(op.id.clone()),
            anchor_of(&op.source_anchor, routine.unwrap()),
            table_note(op, routine, table_by_id),
        ),
        None => (
            op_id.map(|s| s.to_string()),
            SourceAnchor {
                source_unit_id: String::new(),
                start_line: 0,
                start_column: 0,
                end_line: 0,
                end_column: 0,
                enclosing_routine_id: terminal_routine_id.to_string(),
                syntax_kind: String::new(),
                normalized_text_hash: None,
                leading_context_hash: None,
                trailing_context_hash: None,
            },
            String::new(),
        ),
    };
    EvidenceStep {
        routine_id: terminal_routine_id.to_string(),
        operation_id: op_id_out,
        callsite_id: None,
        loop_id: None,
        source_anchor: anchor,
        note,
    }
}

#[cfg(test)]
impl<'a> WalkPolicy for D1Policy<'a> {
    fn terminals_at(&self, node: &str, _ctx: &PathCtx) -> Vec<Terminal> {
        let Some(r) = self.routine_by_id.get(node).copied() else {
            return Vec::new();
        };
        // Source-only: every routine is primary (roleOf != "dependency"). The
        // dependency `summary.dbEffects` fallback is DEAD here.
        r.record_operations
            .iter()
            .filter(|op| is_db_touching_class(classify_op(&op.op)))
            // G-1: a callee's own `until <var>.Next() …` terminator is the callee
            // loop's advancement, never an actionable db op for ANY ancestor loop —
            // exclude it from the interprocedural terminals too.
            .filter(|op| !is_terminator_next(op))
            // G-6: ops on a BC virtual/system table (AllObjWithCaption, Field, …)
            // read the platform's in-memory metadata store — no SQL round-trip, so
            // they are never d1 terminals for ANY ancestor loop either.
            .filter(|op| !op_targets_virtual_system_table(op, r, self.table_by_id))
            .map(|op| Terminal {
                routine_id: node.to_string(),
                local_loop_depth: op.loop_stack.len() as i64,
                op_id: Some(op.id.clone()),
            })
            .collect()
    }

    fn expand(&self, node: &str, _ctx: &PathCtx) -> Vec<CombinedEdge> {
        let Some(edges) = self.edges_by_from.get(node) else {
            return Vec::new();
        };
        edges
            .iter()
            .filter(|e| {
                // event fan-out is D2's job
                if e.kind == "event-dispatch" {
                    return false;
                }
                match self.summaries.get(&e.to) {
                    Some(s) => self.touches_db_memoized(s) != EffectPresence::No,
                    None => false,
                }
            })
            .cloned()
            .collect()
    }

    fn build_hop_step(&self, edge: &CombinedEdge, _ctx: &PathCtx) -> EvidenceStep {
        hop_step(
            self.routine_by_id,
            &edge.from,
            &edge.to,
            &edge.kind,
            edge.callsite_id.as_deref(),
        )
    }

    fn build_terminal_step(&self, t: &Terminal, _ctx: &PathCtx) -> EvidenceStep {
        terminal_step(
            self.routine_by_id,
            self.table_by_id,
            &t.routine_id,
            t.op_id.as_deref(),
        )
    }

    fn loop_depth_of_edge(&self, edge: &CombinedEdge) -> i64 {
        // al-sem `loopDepthOfEdge`: ctx.callSiteById.get(edge.callsiteId).loopStack.length.
        edge.callsite_id
            .as_ref()
            .and_then(|cid| self.call_site_by_id.get(cid.as_str()))
            .map(|cs| cs.loop_stack.len() as i64)
            .unwrap_or(0)
    }
}

/// Derive a caller-specific `WalkResult` set from the CANONICAL callee walk
/// (`initial_loop_depth: 0`, empty `initial_steps`) by the mechanical seed
/// transform `walk_evidence` itself applies: prepend `prefix` to every result
/// path and ADD `initial_loop_depth` to every `effective_loop_depth`. Everything
/// else — the path SUFFIX, terminal ops, uncertainties, stop kind, and result
/// ORDER — is caller-independent, so this reproduces `walk_evidence(C, …,
/// WalkOpts { initial_loop_depth, initial_steps: prefix })` BYTE-FOR-BYTE.
///
/// Sound because in `path_walker::visit` `initial_steps` is only ever pushed
/// (never read for a branch/terminal/cut decision) and `initial_loop_depth`
/// seeds `inherited_loop_depth`, which flows ONLY into `effective_loop_depth`
/// additively — never into a cut (cycle/depth/budget use `routine_path` +
/// `nodes_visited` alone). See Wave-2c design §3. Test-only (the shadow oracle).
#[cfg(test)]
fn apply_seed_transform(
    canonical: &[WalkResult],
    initial_loop_depth: i64,
    prefix: &[EvidenceStep],
) -> Vec<WalkResult> {
    canonical
        .iter()
        .map(|r| {
            let mut path = Vec::with_capacity(prefix.len() + r.path.len());
            path.extend(prefix.iter().cloned());
            path.extend(r.path.iter().cloned());
            WalkResult {
                path,
                effective_loop_depth: r.effective_loop_depth + initial_loop_depth,
                uncertainties: r.uncertainties.clone(),
                stop: r.stop,
            }
        })
        .collect()
}

/// The OLD PW-0 path-walker phase — every direct in-loop db op (branch a) and
/// every in-loop call-chain walk into a db-touching callee (branch b), PRE-dedupe
/// / PRE-merge. Test-only: it is the Task 4 shadow oracle the `shadow_tests` /
/// `shadow_do_workspace` differential runs side-by-side with the production
/// `build_d1_graph` + `search_loops` reachability pipeline on identical
/// `DetectorContext` input. Only the FINDING population is produced — the stat
/// counters moved to the production [`enumerate_direct_ops`], and the Hot-tier
/// walk/memo trace instrumentation was removed with the cutover (production census
/// is `d1.reach`); the `walk_memo` canonical-walk optimization is kept (the
/// `memo_tests` prove it byte-identical).
#[cfg(test)]
pub(crate) fn detect_d1_premerge(resolved: &L3Resolved, ctx: &DetectorContext) -> Vec<FindingRec> {
    let ws = &resolved.workspace;

    // Source-only role map (every routine primary) — used by pick_actionable_anchor.
    let role_by_routine: HashMap<&str, &str> = ws
        .routines
        .iter()
        .map(|r| (r.id.as_str(), "primary"))
        .collect();

    // Component 3 / RV-6 (Task 10): callsite_id → resolved edge KIND, derived from
    // the combined graph d1 already holds. `resolve_temp_along_path` consults this to
    // enforce the edge-kind allowlist (only `direct | method | implicit-trigger` hops
    // carry usable binding semantics; everything else stops the PD chase → Unknown).
    // First edge per callsite wins (edges_by_from is edgeSortKey-sorted, matching the
    // resolver's deterministic per-callsite view).
    let mut edge_kind_by_callsite: HashMap<&str, &str> = HashMap::new();
    for edges in ctx.graph.edges_by_from.values() {
        for e in edges {
            if let Some(cs) = e.callsite_id.as_deref() {
                edge_kind_by_callsite.entry(cs).or_insert(e.kind.as_str());
            }
        }
    }

    let mut findings: Vec<FindingRec> = Vec::new();

    let policy = D1Policy {
        routine_by_id: &ctx.routine_by_id,
        table_by_id: &ctx.table_by_id,
        summaries: &ctx.summaries,
        edges_by_from: &ctx.graph.edges_by_from,
        call_site_by_id: &ctx.call_site_by_id,
        cone_derived: &ctx.cone_derived,
        touches_db_memo: RefCell::new(HashMap::new()),
        walk_memo: RefCell::new(HashMap::new()),
    };

    for routine in &ws.routines {
        if !routine.body_available {
            continue;
        }
        if routine.parse_incomplete {
            continue;
        }

        let loop_by_id: HashMap<&str, &crate::engine::l2::features::PLoop> =
            routine.loops.iter().map(|l| (l.id.as_str(), l)).collect();

        // Record-vars that had a cursor opened before any loop.
        let mut cursor_opened_record_vars: HashSet<String> = HashSet::new();
        for op in &routine.record_operations {
            if !op.loop_stack.is_empty() {
                continue;
            }
            if !CURSOR_OPENER_OPS.contains(&op.op.as_str()) {
                continue;
            }
            cursor_opened_record_vars.insert(op.record_variable_name.to_lowercase());
        }

        // (a) Direct in-loop DB ops.
        for op in &routine.record_operations {
            if op.loop_stack.is_empty() {
                continue;
            }
            if !is_db_touching_class(classify_op(&op.op)) {
                continue;
            }
            if op.op == "Next"
                && cursor_opened_record_vars.contains(&op.record_variable_name.to_lowercase())
            {
                continue;
            }
            // G-1: the `until <var>.Next() …` TERMINATOR of the enclosing repeat loop
            // is the loop's own cursor advancement — it cannot be hoisted or removed
            // without breaking the loop, so it is never an actionable finding.
            if is_terminator_next(op) {
                continue;
            }
            // G-6: an op on a BC virtual/system table reads the platform's in-memory
            // metadata store — no physical SQL backing, never a SQL round-trip, so
            // an in-loop read of one is never a d1 finding (docs/engine-gaps.md G-6).
            if op_targets_virtual_system_table(op, routine, &ctx.table_by_id) {
                continue;
            }
            let Some(representative_loop) = representative_loop_id(&op.loop_stack) else {
                continue;
            };
            let Some(loop_info) = loop_by_id.get(representative_loop).copied() else {
                continue;
            };

            let loop_step = EvidenceStep {
                routine_id: routine.id.clone(),
                operation_id: None,
                callsite_id: None,
                loop_id: Some(loop_info.id.clone()),
                source_anchor: anchor_of(&loop_info.source_anchor, routine),
                note: format!("{} loop", loop_info.loop_type),
            };
            let op_step = EvidenceStep {
                routine_id: routine.id.clone(),
                operation_id: Some(op.id.clone()),
                callsite_id: None,
                loop_id: None,
                source_anchor: anchor_of(&op.source_anchor, routine),
                note: table_note(op, Some(routine), &ctx.table_by_id),
            };
            let result = WalkResult {
                path: vec![loop_step, op_step],
                effective_loop_depth: op.loop_stack.len() as i64,
                uncertainties: Vec::new(),
                stop: WalkStop::Complete,
            };
            let (finding, _verdict) = build_finding_internal(
                routine,
                loop_info.id.as_str(),
                &result,
                op,
                routine,
                &ctx.routine_by_id,
                &ctx.table_by_id,
                &role_by_routine,
                &edge_kind_by_callsite,
                &ctx.closed_world_temp_params,
            );
            findings.push(FindingRec { finding });
        }

        // (b) In-loop calls to DB-touching callees — walk the call chain.
        for cs in &routine.call_sites {
            if cs.loop_stack.is_empty() {
                continue;
            }
            let Some(representative_loop) = representative_loop_id(&cs.loop_stack) else {
                continue;
            };
            let Some(loop_info) = loop_by_id.get(representative_loop).copied() else {
                continue;
            };

            // Resolve the edge from graph.edgesByFrom by callsiteId.
            //
            // G-18 (docs/engine-gaps.md): the callsite-id match alone is NOT
            // sufficient. Two same-name same-signature triggers in one object
            // (e.g. two page actions, each `trigger OnAction()`) used to COLLIDE
            // on the internal routine id, so their call-site ids (`{rid}/cs{n}`)
            // collided too and `edges_by_from[{rid}]` mixed BOTH bodies' edges
            // under one key. Task 3 gave `compute_routine_id` a conditional
            // enclosing-member discriminator, which closes that collision for
            // every member trigger (DO: 262 groups → 0). This guard is KEPT
            // deliberately: a small residual still collides by construction
            // (8020: 15 groups — XMLport same-name elements at different nesting
            // paths, and preproc `#if` alternatives), and it is fail-closed for
            // any future id-schema regression.
            // Picking the sibling body's edge for THIS body's in-loop call site
            // attributed the loop to a call chain it is not on (the CDO batch-7
            // `eDocumentsConfigExists` false positive). The edge's TARGET must
            // also match this call site's own callee name — see
            // `edge_target_matches_callsite_callee` for why this can never
            // reject a genuinely-own edge.
            let edge = ctx.graph.edges_by_from.get(&routine.id).and_then(|edges| {
                edges.iter().find(|e| {
                    e.callsite_id.as_deref() == Some(cs.id.as_str())
                        && edge_target_matches_callsite_callee(e, cs, &ctx.routine_by_id)
                })
            });
            let Some(edge) = edge else {
                // No resolved edge — opaque callee.
                continue;
            };
            if edge.kind == "interface" || edge.kind == "dynamic" {
                continue;
            }
            let Some(callee_summary) = ctx.summaries.get(&edge.to) else {
                continue;
            };
            if policy.touches_db_memoized(callee_summary) == EffectPresence::No {
                continue;
            }

            let loop_step = EvidenceStep {
                routine_id: routine.id.clone(),
                operation_id: None,
                callsite_id: None,
                loop_id: Some(loop_info.id.clone()),
                source_anchor: anchor_of(&loop_info.source_anchor, routine),
                note: format!("{} loop", loop_info.loop_type),
            };
            let to_name = ctx
                .routine_by_id
                .get(edge.to.as_str())
                .map(|r| r.name.clone())
                .unwrap_or_else(|| edge.to.clone());
            let call_step = EvidenceStep {
                routine_id: routine.id.clone(),
                operation_id: None,
                callsite_id: Some(cs.id.clone()),
                loop_id: None,
                source_anchor: anchor_of(&cs.source_anchor, routine),
                note: format!("calls {to_name}"),
            };

            // Wave-2c: the walk from a callee is caller-independent (design §3),
            // so compute the CANONICAL walk (empty prefix, zero initial depth)
            // ONCE per callee and derive THIS callsite's result by the mechanical
            // prefix+depth transform. At Base-App density many in-loop callsites
            // share the same hot callee, so this collapses O(in-loop callsites)
            // re-walks of the same dense subgraph into O(distinct callees).
            let canonical = {
                let mut memo = policy.walk_memo.borrow_mut();
                if memo.contains_key(&edge.to) {
                    Rc::clone(memo.get(&edge.to).expect("contains_key just succeeded"))
                } else {
                    // Memo MISS → one canonical walk (no trace instrumentation).
                    let results = walk_evidence(
                        &edge.to,
                        &policy,
                        BOUNDS,
                        WalkOpts {
                            initial_loop_depth: 0,
                            initial_steps: Vec::new(),
                        },
                        ctx.uncertainty_view(),
                        None,
                    );
                    let rc = Rc::new(results);
                    memo.insert(edge.to.clone(), Rc::clone(&rc));
                    rc
                }
            };

            let results = apply_seed_transform(
                canonical.as_slice(),
                cs.loop_stack.len() as i64,
                &[loop_step, call_step],
            );

            for result in &results {
                if result.stop != WalkStop::Complete {
                    continue;
                }
                let Some(last_step) = result.path.last() else {
                    continue;
                };
                let Some(op_id) = last_step.operation_id.as_ref() else {
                    continue;
                };
                let terminal_routine = ctx
                    .routine_by_id
                    .get(last_step.routine_id.as_str())
                    .copied();
                // Primary routines have real RecordOperations; the dep
                // summary.dbEffects fallback is DEAD (source-only).
                let Some(terminal_routine) = terminal_routine else {
                    continue;
                };
                let terminal_op = terminal_routine
                    .record_operations
                    .iter()
                    .find(|o| &o.id == op_id);
                let Some(terminal_op) = terminal_op else {
                    continue;
                };
                let (finding, _verdict) = build_finding_internal(
                    routine,
                    loop_info.id.as_str(),
                    result,
                    terminal_op,
                    terminal_routine,
                    &ctx.routine_by_id,
                    &ctx.table_by_id,
                    &role_by_routine,
                    &edge_kind_by_callsite,
                    &ctx.closed_world_temp_params,
                );
                findings.push(FindingRec { finding });
            }
        }
    }

    findings
}

/// The d1 `DetectorStats` counters the production enumeration accumulates. The
/// branch-(a) direct-op ladder yields `candidates_considered` /
/// `skipped_parse_incomplete` / `skipped_virtual_table` / `downgraded_to_info`;
/// the branch-(b) in-loop-call ladder yields `skipped_opaque_callee` /
/// `skipped_dynamic_dispatch` (the production seeds themselves are built by
/// `build_d1_graph`, which does not surface those skip reasons).
#[derive(Default)]
struct DirectOpStats {
    candidates_considered: usize,
    skipped_parse_incomplete: u64,
    skipped_opaque_callee: u64,
    skipped_dynamic_dispatch: u64,
    skipped_virtual_table: u64,
    downgraded_to_info: u64,
}

/// The PRODUCTION direct-op (old branch (a)) enumeration + the d1 stat counting.
///
/// Returns every DIRECT in-loop db op surviving branch (a)'s EXACT ladder
/// (`loop_stack` non-empty, db-touching class, the cursor-opener `Next` skip, G-1
/// terminator-`Next`, G-6 virtual-system-table) as a [`DirectOp`] the
/// reachability search folds into its per-`(loop, terminal-op)` aggregation, PLUS
/// the [`DirectOpStats`]. Every predicate mirrors the OLD `detect_d1_premerge`
/// ladder EXACTLY so the reported stats are byte-identical (pinned by
/// `tests/cli/d1_downgraded_to_info_oracle.rs` + the cli-a stats goldens).
fn enumerate_direct_ops<'a>(
    ws: &'a L3Workspace,
    ctx: &DetectorContext,
) -> (Vec<DirectOp<'a>>, DirectOpStats) {
    let mut out: Vec<DirectOp<'a>> = Vec::new();
    let mut s = DirectOpStats::default();

    for routine in &ws.routines {
        // detect_d1's routine gate (candidatesConsidered / parseIncomplete).
        if !routine.body_available {
            continue;
        }
        if routine.parse_incomplete {
            s.skipped_parse_incomplete += 1;
            continue;
        }
        s.candidates_considered += 1;

        let loop_by_id: HashMap<&str, &crate::engine::l2::features::PLoop> =
            routine.loops.iter().map(|l| (l.id.as_str(), l)).collect();

        // Record-vars with a cursor opened before any loop (the in-loop `Next`
        // cursor-advance exemption).
        let mut cursor_opened_record_vars: HashSet<String> = HashSet::new();
        for op in &routine.record_operations {
            if !op.loop_stack.is_empty() {
                continue;
            }
            if !CURSOR_OPENER_OPS.contains(&op.op.as_str()) {
                continue;
            }
            cursor_opened_record_vars.insert(op.record_variable_name.to_lowercase());
        }

        // (a) DIRECT in-loop db ops.
        for op in &routine.record_operations {
            if op.loop_stack.is_empty() {
                continue;
            }
            if !is_db_touching_class(classify_op(&op.op)) {
                continue;
            }
            if op.op == "Next"
                && cursor_opened_record_vars.contains(&op.record_variable_name.to_lowercase())
            {
                continue;
            }
            if is_terminator_next(op) {
                continue;
            }
            if op_targets_virtual_system_table(op, routine, &ctx.table_by_id) {
                s.skipped_virtual_table += 1;
                continue;
            }
            let Some(representative_loop) = representative_loop_id(&op.loop_stack) else {
                continue;
            };
            let Some(loop_info) = loop_by_id.get(representative_loop).copied() else {
                continue;
            };
            // downgradedToInfo: PER DIRECT IN-LOOP OP (mirrors d1.ts:320-322). A
            // known-temp FlowField-gated CalcFields/SetAutoCalcFields still FIRES
            // (RV-1), so it is excluded here so the stat tracks the ops that
            // genuinely downgrade.
            let flowfield_gated_direct = FLOWFIELD_GATED_OPS.contains(&op.op.as_str())
                && flowfield_gate_blocks_downgrade(op, &ctx.table_by_id);
            if is_known_temp(op) && !flowfield_gated_direct {
                s.downgraded_to_info += 1;
            }
            out.push(DirectOp {
                routine,
                loop_id: representative_loop,
                loop_info,
                op,
            });
        }

        // (b) In-loop CALL skip counting (opaqueCallee / dynamicDispatch). The
        // production seeds are built by `build_d1_graph` (which applies the SAME
        // ladder); here we reproduce only branch (b)'s skip ladder to preserve
        // those two stat counters. Mirrors `detect_d1_premerge`'s branch (b)
        // exactly: the loop guard, then G-18 edge resolution, then the two skips
        // (missing-summary / touches_db == No were never counted — not counted
        // here either).
        for cs in &routine.call_sites {
            if cs.loop_stack.is_empty() {
                continue;
            }
            let Some(representative_loop) = representative_loop_id(&cs.loop_stack) else {
                continue;
            };
            if !loop_by_id.contains_key(representative_loop) {
                continue;
            }
            let edge = ctx.graph.edges_by_from.get(&routine.id).and_then(|edges| {
                edges.iter().find(|e| {
                    e.callsite_id.as_deref() == Some(cs.id.as_str())
                        && edge_target_matches_callsite_callee(e, cs, &ctx.routine_by_id)
                })
            });
            let Some(edge) = edge else {
                s.skipped_opaque_callee += 1;
                continue;
            };
            if edge.kind == "interface" || edge.kind == "dynamic" {
                s.skipped_dynamic_dispatch += 1;
            }
        }
    }

    (out, s)
}

/// One per-loop context under assembly: the source [`LoopTerminalAgg`] + the
/// built [`LoopContext`] + the fields the group finding lifts from the WINNER.
///
/// TEST-ONLY as of the C6 cohort cutover: `detect_d1` now assembles compressed
/// cohort findings via [`assemble_cohort_findings`]; this per-loop `LoopContext`
/// assembly (`build_context`/`build_group_finding`/`assemble_findings`) survives
/// only as the differential ORACLE the d1 test modules run against
/// `search_loops` (the old `Vec<LoopTerminalAgg>` path).
#[cfg(test)]
struct CtxUnderAssembly<'agg, 'data> {
    agg: &'agg LoopTerminalAgg<'data>,
    context: LoopContext,
    confidence: FindingConfidence,
    setup_singleton: bool,
}

/// Build one [`CtxUnderAssembly`] from a reachability aggregate: the per-loop
/// confidence (from the aggregate's uncertainty union), the setup-singleton flag
/// (for the winner's note/fix), and the serialized-shape [`LoopContext`].
#[cfg(test)]
fn build_context<'agg, 'data>(
    agg: &'agg LoopTerminalAgg<'data>,
    ctx: &DetectorContext,
) -> CtxUnderAssembly<'agg, 'data> {
    let confidence = to_confidence(&uncertainty_lites(&agg.uncertainties), "likely");
    let setup_singleton =
        is_setup_singleton_get(agg.terminal.op, Some(agg.terminal.owner), &ctx.table_by_id);
    // depth_class: "nested-loop" iff the winner's scoring depth bucket >= 2.
    let depth_class = if agg.depth_bucket >= 2 {
        "nested-loop"
    } else {
        "single-loop"
    };
    let reachable_verdicts: Vec<String> = agg
        .reachable_verdicts
        .iter()
        .map(|v| v.label().to_string())
        .collect();
    let context = LoopContext {
        loop_id: agg.loop_id.to_string(),
        loop_routine_id: agg.loop_routine.id.clone(),
        entry_callsite_id: agg.entry_callsite_id.map(|s| s.to_string()),
        verdict: agg.verdict.label().to_string(),
        reachable_verdicts,
        depth_class: depth_class.to_string(),
        severity: agg.severity.to_string(),
        confidence: confidence.clone(),
        witness: agg.witness.clone(),
    };
    CtxUnderAssembly {
        agg,
        context,
        confidence,
        setup_singleton,
    }
}

/// Assemble ONE terminal-centric [`Finding`] from a group's per-loop contexts.
/// `ctxs` is already sorted so `ctxs[0]` is the WINNER; the finding lifts its
/// severity / confidence / evidence_path / temp+setup notes / G-4 wording from
/// that single context, and the non-winner witnesses become `additional_paths`.
///
/// Returns the per-loop [`LoopContext`]s ALONGSIDE the finding rather than inside
/// it: `Finding::contexts` is gone (the production path emits `cohort_contexts`),
/// so this oracle carries its own representation out-of-band. The tests that
/// assert per-loop context order read the second element.
#[cfg(test)]
fn build_group_finding(
    ctxs: &[CtxUnderAssembly],
    ctx: &DetectorContext,
    role_by_routine: &HashMap<&str, &str>,
) -> (Finding, Vec<LoopContext>) {
    let winner = &ctxs[0];
    let agg = winner.agg;
    let terminal_op = agg.terminal.op;
    let terminal_routine = agg.terminal.owner;
    let loop_routine = agg.loop_routine;
    let terminal_routine_id = terminal_routine.id.as_str();

    // Terminal-based identity — the schema change (old per-loop id is gone).
    let id = format!("d1/{}/{}", terminal_routine_id, terminal_op.id);
    let root_cause_key = id.clone();

    // Winner verdict → temp note (Physical carries none).
    let temp_note = match agg.verdict {
        TempVerdict::Temporary => NOTE_TEMPORARY,
        TempVerdict::Uncertain => NOTE_UNCERTAIN,
        TempVerdict::FlowFieldGated => NOTE_TEMP_FLOWFIELD,
        TempVerdict::Physical => "",
    };
    let setup_note = if winner.setup_singleton {
        " (Setup singleton — BC caches Get() per session, so the round-trip happens at most once.)"
    } else {
        ""
    };

    // G-4 pure-transitive wording (from the WINNER's loop/terminal).
    let pure_transitive =
        terminal_routine_id != loop_routine.id && terminal_op.loop_stack.is_empty();
    let base_root_cause = if pure_transitive {
        format!(
            "A loop in {} reaches {} in {}, which has no loop of its own \u{2014} the \
             operation runs once per iteration of that loop{}{}.",
            loop_routine.name,
            table_note(terminal_op, Some(terminal_routine), &ctx.table_by_id),
            terminal_routine.name,
            temp_note,
            setup_note
        )
    } else {
        format!(
            "A loop in {} reaches {}{}{}.",
            loop_routine.name,
            table_note(terminal_op, Some(terminal_routine), &ctx.table_by_id),
            temp_note,
            setup_note
        )
    };
    // Multi-context annotation (reuse path_merge's fn) — "(Also reached from N
    // other in-loop ancestors.)" when > 1 context.
    let root_cause = annotate_root_cause(&base_root_cause, ctxs.len());

    // affectedObjects = sorted union over contexts' loop-routine object ids + the
    // terminal object id.
    let mut affected_set: BTreeSet<String> = BTreeSet::new();
    for c in ctxs {
        affected_set.insert(c.agg.loop_routine.object_id.clone());
    }
    affected_set.insert(terminal_routine.object_id.clone());
    let affected_objects: Vec<String> = affected_set.into_iter().collect();

    // affectedTables = the terminal op's own table (unchanged rule).
    let affected_tables: Vec<String> = match &terminal_op.table_id {
        Some(t) => vec![t.clone()],
        None => Vec::new(),
    };

    // additional_paths = non-winner witnesses in CONTEXT order (None for a
    // single-context finding — matching the old singleton shape / pathCount 1).
    let additional_paths = if ctxs.len() > 1 {
        Some(
            ctxs[1..]
                .iter()
                .map(|c| c.context.witness.clone())
                .collect(),
        )
    } else {
        None
    };

    let fix_options = if winner.setup_singleton {
        vec![FixOption {
            description: "Setup tables are session-cached by BC, so a Get() inside a loop is \
                          typically O(1) after the first hit. Hoist the Get() outside the loop \
                          only if the call site shows up in a CPU profile."
                .into(),
            safety: "high".into(),
        }]
    } else {
        vec![FixOption {
            description: "Move the database operation outside the loop, or batch it into a \
                          set-based operation."
                .into(),
            safety: "medium".into(),
        }]
    };

    let contexts: Vec<LoopContext> = ctxs.iter().map(|c| c.context.clone()).collect();

    let mut finding = Finding {
        id,
        root_cause_key,
        detector: DETECTOR.to_string(),
        title: "Database operation inside a loop".into(),
        root_cause,
        // Severity + confidence from the SAME (winning) context.
        severity: agg.severity.to_string(),
        confidence: winner.confidence.clone(),
        primary_location: anchor_of(&terminal_op.source_anchor, terminal_routine),
        evidence_path: agg.witness.clone(),
        additional_paths,
        affected_objects: id_list(affected_objects),
        affected_tables: id_list(affected_tables),
        fix_options,
        provenance: vec![Evidence {
            source: "tree-sitter",
            note: None,
        }],
        actionable_anchor: None,
        fingerprint: None,
        event_kind: None,
        cross_extension_subscribers: None,
        cohort_contexts: None,
    };

    let actionable = pick_actionable_anchor(&finding, role_by_routine);
    if actionable.is_some() {
        finding.actionable_anchor = actionable;
    }
    (finding, contexts)
}

/// Group the reachability aggregates by `(terminal routine, terminal op)` and
/// assemble ONE terminal-centric [`Finding`] per group. Deterministic: the
/// grouping is a `BTreeMap` over already-sorted `aggs` (`search_loops` rule 8),
/// and the per-group context order (winner first) is a total order on
/// (severity rank desc, verdict quality desc, loop routine id asc, loop id asc).
#[cfg(test)]
fn assemble_findings(
    aggs: &[LoopTerminalAgg],
    ctx: &DetectorContext,
    role_by_routine: &HashMap<&str, &str>,
) -> Vec<(Finding, Vec<LoopContext>)> {
    let mut groups: BTreeMap<(&str, &str), Vec<&LoopTerminalAgg>> = BTreeMap::new();
    for agg in aggs {
        groups
            .entry((agg.terminal.owner.id.as_str(), agg.terminal.op.id.as_str()))
            .or_default()
            .push(agg);
    }

    let mut out: Vec<(Finding, Vec<LoopContext>)> = Vec::new();
    for (_key, members) in groups {
        let mut ctxs: Vec<CtxUnderAssembly> =
            members.iter().map(|&agg| build_context(agg, ctx)).collect();
        // Winner selection / context order (the brief's locked rule).
        ctxs.sort_by(|a, b| {
            sev_rank(b.agg.severity)
                .cmp(&sev_rank(a.agg.severity))
                .then_with(|| b.agg.verdict.quality().cmp(&a.agg.verdict.quality()))
                .then_with(|| a.agg.loop_routine.id.cmp(&b.agg.loop_routine.id))
                .then_with(|| a.agg.loop_id.cmp(b.agg.loop_id))
        });
        out.push(build_group_finding(&ctxs, ctx, role_by_routine));
    }
    out
}

/// The lowest GLOBAL group index a bitmap names (its "representative" loop —
/// which, because groups are visited in sorted `(loop_routine_id, loop_id)`
/// order and a cohort's first-seen loop is therefore its lowest group index,
/// equals the OLD winner-selection's `loop_routine_id`/`loop_id`-minimal reaching
/// loop). `u32::MAX` for an empty bitmap (never happens for a real cohort).
fn min_group(bm: &GroupBitmap) -> u32 {
    bm.iter().next().unwrap_or(u32::MAX)
}

/// The run's ONE `Arc<str>` for an internal object/table id, minted on first
/// sight. `cache` borrows its keys from the workspace (which outlives the
/// assembly), so a repeat lookup allocates nothing at all.
fn intern_id<'w>(cache: &mut HashMap<&'w str, Arc<str>>, id: &'w str) -> Arc<str> {
    match cache.get(id) {
        Some(a) => Arc::clone(a),
        None => {
            let a: Arc<str> = Arc::from(id);
            cache.insert(id, Arc::clone(&a));
            a
        }
    }
}

/// Assemble ONE compressed [`Finding`] per reached terminal from the cohort run
/// (the C6 cutover replacement for [`assemble_findings`]), and build the
/// run-level [`LoopSetRegistry`] as cohorts are interned.
///
/// Per terminal:
/// - The WINNER ContextKey cohort = max `(sev_rank, verdict quality)`, tie-broken
///   by SMALLEST representative group — which reproduces the OLD winner-context
///   selection EXACTLY (max severity → max verdict quality → min
///   `(loop_routine_id, loop_id)`), because a cohort's representative loop is its
///   lowest group index and the global-min top-`(sev, verdict)` loop is the min of
///   its OWN cohort. So the finding's `severity` (the max), `confidence` (from the
///   winner cohort's representative-path uncertainties = the old winner's), the
///   root-cause loop name, `primary_location`, realizing witness path and
///   fingerprint inputs are all PRESERVED across the cutover — only the witness
///   becomes bounded and the per-loop `contexts` become per-class
///   `cohort_contexts`.
/// - `cohort_contexts` = every ContextKey cohort partitioned FURTHER by
///   `reachable_verdicts` (a per-`(loop, terminal)` property that can vary within a
///   ContextKey class), each with an interned `loop_set` + `loop_count` + the ck
///   cohort's shared bounded representative `witness`; ordered `(sev desc, verdict
///   quality desc, min group asc)` so `cohort_contexts[0]` is the winner class.
/// - `evidence_path`/`additional_paths` are NOT built: they were byte-for-byte
///   copies of `cohort_contexts[0].witness` / `cohort_contexts[1..].witness`,
///   which `project_finding` has discarded since Task C8. Readers derive them
///   through [`crate::engine::l5::finding::evidence_path_of`] /
///   [`crate::engine::l5::finding::realizing_path_count`] instead (so a SARIF
///   code-flow index still maps to a `cohort_contexts` index).
fn assemble_cohort_findings(
    run: &D1CohortRun,
    ctx: &DetectorContext,
    role_by_routine: &HashMap<&str, &str>,
) -> (Vec<Finding>, LoopSetRegistry) {
    let mut registry = LoopSetRegistry::new();
    // Hash-cons for the witness steps the findings are about to RETAIN. The sink's
    // copies are transient (freed with the run); these are not, and they repeat
    // 4.29x across cohorts on Base App 8020. Local, so it is dropped with this
    // call and leaves only the sharing behind. See `d1_witness::StepInterner`.
    let mut steps = StepInterner::default();
    // The same idea for the object/table ids: 563,126 entries over 2,042
    // distinct values on 8020 (276x duplication). See `affected_objects` below.
    let mut object_ids: HashMap<&str, Arc<str>> = HashMap::new();
    // Catalog group -> its loop routine's OWNING OBJECT id, resolved ONCE per
    // distinct loop instead of once per (cohort, set bit). The `affected_objects`
    // loop below runs over every reaching loop of every cohort of every terminal
    // — the ~3.2M `(loop, terminal)` population the cohort redesign compressed
    // everywhere EXCEPT here — and each visit used to pay a `HashMap<&str, _>`
    // lookup keyed by the loop routine's id string. The catalog is `run`-lifetime
    // and immutable, so one pass over it answers every visit by index. `None`
    // reproduces the old miss arm (a catalog loop whose routine is absent from
    // `routine_by_id` contributes no object), so the `BTreeSet` this feeds — and
    // therefore `affectedObjects`' content and order — is unchanged.
    let loop_object_of: Vec<Option<&str>> = run
        .catalog
        .iter()
        .map(|l| {
            ctx.routine_by_id
                .get(l.loop_routine_id.as_str())
                .map(|r| r.object_id.as_str())
        })
        .collect();
    // Every d1 finding carries the SAME title and one of TWO fix options
    // (census: `title` 1 distinct over 22,383 findings, `fix_options`
    // description/safety 2 each). Hoisted out of the emit loop so the run holds
    // one allocation of each instead of one per finding.
    let title: Arc<str> = Arc::from("Database operation inside a loop");
    let setup_fix = FixOption {
        description: Arc::from(
            "Setup tables are session-cached by BC, so a Get() inside a loop is \
             typically O(1) after the first hit. Hoist the Get() outside the loop \
             only if the call site shows up in a CPU profile.",
        ),
        safety: Arc::from("high"),
    };
    let general_fix = FixOption {
        description: Arc::from(
            "Move the database operation outside the loop, or batch it into a \
             set-based operation.",
        ),
        safety: Arc::from("medium"),
    };
    let mut out: Vec<Finding> = Vec::new();

    // Iterate terminals in a DETERMINISTIC key order so the run-level loop-set
    // registry's intern order (and thus every `LoopSetId`) is reproducible: the
    // sink's terminal order is partly HashMap-derived (the direct-only emission
    // pass), and `finalize` collects each terminal's cohorts from a HashMap, so
    // neither is stable on its own. Sorting terminals here + the finest cohorts
    // below (before interning) makes the registry a pure function of the input.
    let mut term_order: Vec<usize> = (0..run.terminals.len()).collect();
    term_order.sort_by(|&a, &b| {
        let (ra, oa) = run.terminals[a].key;
        let (rb, ob) = run.terminals[b].key;
        (ra.id.as_str(), oa.id.as_str()).cmp(&(rb.id.as_str(), ob.id.as_str()))
    });

    for &ti in &term_order {
        let tc = &run.terminals[ti];
        if tc.cohorts.is_empty() {
            continue;
        }
        // The terminal's OWNING routine + db op are the sink's stored graph
        // references (the first-seen `(owner, op)` the sink interned) — NOT
        // re-derived from `routine_by_id`, which for a colliding-id trigger
        // (`OnAction`, …) would return a SIBLING body whose `record_operations`
        // lack this op and silently drop the finding (G-18).
        let (terminal_routine, terminal_op) = tc.key;

        // WINNER ContextKey cohort: max (sev_rank, verdict quality), tie → min
        // representative group (reproduces the old winner-context selection).
        let winner_ix = tc
            .cohorts
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| {
                sev_rank(a.0.severity)
                    .cmp(&sev_rank(b.0.severity))
                    .then_with(|| a.0.verdict.quality().cmp(&b.0.verdict.quality()))
                    .then_with(|| min_group(&b.1).cmp(&min_group(&a.1)))
            })
            .map(|(i, _)| i)
            .expect("tc.cohorts is non-empty");
        let (winner_ck, _winner_bm, winner_rep) = &tc.cohorts[winner_ix];

        // The winner's loop routine (from the representative witness's loop step).
        let loop_routine_id = winner_rep
            .witness
            .first_steps
            .first()
            .map(|s| s.routine_id.as_str())
            .unwrap_or(terminal_routine.id.as_str());
        let loop_routine = ctx.routine_by_id.get(loop_routine_id).copied();
        let loop_routine_name = loop_routine.map(|r| r.name.as_str()).unwrap_or("");

        // Build the FINEST cohorts (ContextKey × reachable_verdicts). Interning is
        // DEFERRED until after the sort below (with a placeholder `loop_set`) so
        // the registry order is deterministic regardless of `tc.cohorts`' HashMap
        // iteration order.
        //
        // Task C9: this used to re-expand every `(loop, terminal)` pair by
        // iterating `bm.iter()` per cohort (a per-loop `Vec<i32>` key alloc +
        // `BTreeMap` insert) — summed over all cohorts/terminals, ~3.2M loop
        // visits, ~208s on Base App 8020. A loop's reachable-verdict set is
        // entirely determined by which of `tc.verdict_sets`' ≤4 per-verdict
        // bitmaps contain it, so the possible partitions are exactly the ≤16
        // subsets (masks) of `TempVerdict`'s 4 variants. For each mask, the
        // sub-cohort is `bm` intersected with (reaches v) / excluding (doesn't
        // reach v) for every verdict bit — the SAME partition the old per-loop
        // `by_rv: BTreeMap<Vec<i32>, _>` built, via ≤16 bitmap AND/AND-NOT ops
        // per cohort instead of one step per loop.
        let mut finest: Vec<(i32, i32, u32, GroupBitmap, D1CohortContext)> = Vec::new();
        for (ck, bm, rep) in &tc.cohorts {
            for mask in 0u32..(1 << tc.verdict_sets.len()) {
                let mut sub_bm = bm.clone();
                for (i, vset) in tc.verdict_sets.iter().enumerate() {
                    if sub_bm.is_empty() {
                        break; // AND/AND-NOT can only shrink further from here
                    }
                    if mask & (1 << i) != 0 {
                        sub_bm.and_with(vset); // loops that DO reach this verdict
                    } else {
                        sub_bm.and_not(vset); // loops that do NOT reach it
                    }
                }
                if sub_bm.is_empty() {
                    continue;
                }
                let g0 = min_group(&sub_bm);
                // Re-derive the labels via the SAME function the old per-loop
                // path used (called once per non-empty mask, not per loop) so
                // the label set + order are byte-identical.
                let rv_labels: Vec<String> = reachable_verdicts_of(&tc.verdict_sets, g0)
                    .iter()
                    .map(|v| v.label().to_string())
                    .collect();
                let loop_count = sub_bm.count();
                finest.push((
                    sev_rank(ck.severity),
                    ck.verdict.quality(),
                    g0,
                    sub_bm,
                    D1CohortContext {
                        severity: ck.severity.to_string(),
                        verdict: ck.verdict.label().to_string(),
                        depth_bucket: ck.depth_bucket,
                        uncertain: ck.unc,
                        reachable_verdicts: rv_labels,
                        loop_set: LoopSetId(0), // placeholder — assigned after the sort
                        loop_count,
                        // Value-identical to `rep.witness`; every step is the
                        // run's ONE allocation for that step value.
                        witness: steps.intern_witness(&rep.witness),
                    },
                ));
            }
        }
        // Order: winner class first (sev desc, verdict quality desc, min group asc).
        // `min_group` is unique per finest cohort (a loop lands in exactly one), so
        // this is a TOTAL order — a stable, input-only interning sequence.
        finest.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.1.cmp(&a.1))
                .then_with(|| a.2.cmp(&b.2))
        });
        let total_loops: u64 = finest.iter().map(|(_, _, _, _, c)| c.loop_count).sum();
        let cohort_contexts: Vec<D1CohortContext> = finest
            .into_iter()
            .map(|(_, _, _, sub_bm, mut c)| {
                c.loop_set = registry.intern(&sub_bm);
                c
            })
            .collect();

        // `evidence_path`/`additional_paths` are deliberately NOT built here.
        // They were the winner's flattened representative witness and the
        // non-winner cohort witnesses in `cohort_contexts` order — i.e. a
        // byte-for-byte second copy of `cohort_contexts[..].witness`, which Task
        // C8 already stopped emitting (`project_finding` returns `Vec::new()`/
        // `None` for a cohort-bearing finding). Retaining them cost 95.9 MiB in
        // 1,085,149 allocations per Base App 8020 run for data no consumer ever
        // received. Every reader now derives the path on demand via
        // [`crate::engine::l5::finding::evidence_path_of`], which reconstructs
        // exactly what this code used to store.

        // Identity + wording (all derived from the winner — preserved vs. old).
        let terminal_routine_id = terminal_routine.id.as_str();
        let id = format!("d1/{}/{}", terminal_routine_id, terminal_op.id);
        let root_cause_key = id.clone();

        let temp_note = match winner_ck.verdict {
            TempVerdict::Temporary => NOTE_TEMPORARY,
            TempVerdict::Uncertain => NOTE_UNCERTAIN,
            TempVerdict::FlowFieldGated => NOTE_TEMP_FLOWFIELD,
            TempVerdict::Physical => "",
        };
        let setup_singleton =
            is_setup_singleton_get(terminal_op, Some(terminal_routine), &ctx.table_by_id);
        let setup_note = if setup_singleton {
            " (Setup singleton — BC caches Get() per session, so the round-trip happens at most once.)"
        } else {
            ""
        };

        let pure_transitive =
            terminal_routine_id != loop_routine_id && terminal_op.loop_stack.is_empty();
        let base_root_cause = if pure_transitive {
            format!(
                "A loop in {} reaches {} in {}, which has no loop of its own \u{2014} the \
                 operation runs once per iteration of that loop{}{}.",
                loop_routine_name,
                table_note(terminal_op, Some(terminal_routine), &ctx.table_by_id),
                terminal_routine.name,
                temp_note,
                setup_note
            )
        } else {
            format!(
                "A loop in {} reaches {}{}{}.",
                loop_routine_name,
                table_note(terminal_op, Some(terminal_routine), &ctx.table_by_id),
                temp_note,
                setup_note
            )
        };
        // "(Also reached from N other in-loop ancestors.)" over TOTAL reaching
        // loops — the same count the old per-loop `ctxs.len()` gave.
        let root_cause = annotate_root_cause(&base_root_cause, total_loops as usize);

        // affectedObjects = every reaching loop's routine object + the terminal's.
        // Borrowed into the `BTreeSet` (the ids live in the workspace, which
        // outlives this call) so the per-terminal set costs no allocation at all;
        // only the survivors are interned below.
        let mut affected_set: BTreeSet<&str> = BTreeSet::new();
        for (_ck, bm, _rep) in &tc.cohorts {
            for g in bm.iter() {
                // `loop_object_of` replaces what used to be a `routine_by_id`
                // HASH LOOKUP on a `String` key here — once per set bit, summed
                // over every cohort of every terminal.
                if let Some(obj) = loop_object_of[g as usize] {
                    affected_set.insert(obj);
                }
            }
        }
        affected_set.insert(terminal_routine.object_id.as_str());
        // Interned, NOT `id_list`: this is the site the whole `Arc<str>` id list
        // exists for. Across a run d1 emits 563,126 object ids drawn from 2,042
        // distinct values (27.6 MB of text, 276x duplication) — one per reaching
        // loop's owning object, per finding — so a per-entry allocation is the
        // cost. `object_ids` is a run-level cache, so each distinct id is
        // allocated once and every finding holds handles.
        let affected_objects: Vec<Arc<str>> = affected_set
            .into_iter()
            .map(|o| intern_id(&mut object_ids, o))
            .collect();

        let affected_tables: Vec<Arc<str>> = match &terminal_op.table_id {
            Some(t) => vec![intern_id(&mut object_ids, t.as_str())],
            None => Vec::new(),
        };

        let confidence: FindingConfidence = to_confidence(
            &uncertainty_lites_of_ids(&winner_rep.uncertainties, &ctx.uncertainties),
            "likely",
        );

        // A refcount bump on one of the two hoisted options, not a fresh pair of
        // allocations per finding.
        let fix_options = vec![if setup_singleton {
            setup_fix.clone()
        } else {
            general_fix.clone()
        }];

        let mut finding = Finding {
            id,
            root_cause_key,
            detector: DETECTOR.to_string(),
            title: Arc::clone(&title),
            root_cause,
            severity: winner_ck.severity.to_string(),
            confidence,
            primary_location: anchor_of(&terminal_op.source_anchor, terminal_routine),
            evidence_path: Vec::new(),
            additional_paths: None,
            affected_objects,
            affected_tables,
            fix_options,
            provenance: vec![Evidence {
                source: "tree-sitter",
                note: None,
            }],
            actionable_anchor: None,
            fingerprint: None,
            event_kind: None,
            cross_extension_subscribers: None,
            cohort_contexts: Some(cohort_contexts),
        };

        let actionable = pick_actionable_anchor(&finding, role_by_routine);
        if actionable.is_some() {
            finding.actionable_anchor = actionable;
        }
        out.push(finding);
    }

    (out, registry)
}

/// D1 — database operation inside a loop. The production cohort pipeline (see the
/// module doc): enumerate direct ops + stats, build the compact filtered graph +
/// seeds, run the reachability search emitting per-terminal bitmap cohorts, and
/// assemble one compressed terminal-centric finding per `(terminal routine, op)`.
pub fn detect_d1(
    resolved: &L3Resolved,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let fp_index = &ctx.fingerprint_index;
    let ws = &resolved.workspace;

    // Source-only role map (every routine primary) — used by pick_actionable_anchor.
    let role_by_routine: HashMap<&str, &str> = ws
        .routines
        .iter()
        .map(|r| (r.id.as_str(), "primary"))
        .collect();

    // (1) Direct-op enumeration + stat counting (old branch-(a) ladder + the
    // branch-(b) opaque/dynamic skip counts).
    let g_dir = pt::span("d1", "enumerate_direct");
    let (direct_ops, dstats) = enumerate_direct_ops(ws, ctx);
    drop(g_dir);

    // (2) Compact filtered graph + in-loop-call seeds (`d1_graph`).
    let g_bg = pt::span("d1", "build_graph");
    let mut touches_db_memo = HashMap::new();
    let (graph, seeds) = build_d1_graph(ctx, ws, &mut touches_db_memo);
    drop(g_bg);

    // (3) The reachability search, emitting per-terminal bitmap COHORTS (the C6
    // cutover): ONE bounded representative witness per (terminal, ContextKey)
    // class instead of one full witness per (loop, terminal). Winner selection is
    // byte-identical to the old aggregate path — see `search_loops_cohorts`.
    let g_scl = pt::span("d1", "search_loops_cohorts");
    let run = search_loops_cohorts(
        &graph,
        &seeds,
        &direct_ops,
        ctx,
        &ctx.closed_world_temp_params,
    );
    drop(g_scl);

    // `d1.cohort` census (Hot-tier, measurement-only — zero cost when disabled).
    if pt::enabled(pt::Detail::Hot) {
        let mut lc = pt::LocalCounters::new();
        lc.set("nodes", graph.node_ids.len() as u64);
        lc.set("edges", graph.edges.iter().map(|e| e.len() as u64).sum());
        lc.set("seeds", seeds.len() as u64);
        lc.set("direct_ops", direct_ops.len() as u64);
        lc.set("terminals", run.terminals.len() as u64);
        lc.set("loop_groups", run.catalog.len() as u64);
        lc.flush("d1.cohort");
    }

    // ⟨ALSEM_D1_SCORING_CENSUS=1⟩ Attribution inside the `d1.cohort / scoring`
    // span — see `d1_dataflow::scoring_census`. Reported here, once, after every
    // batch has scored.
    if crate::engine::l5::d1_dataflow::scoring_census::enabled() {
        crate::engine::l5::d1_dataflow::scoring_census::report();
    }

    // (4) Assemble ONE compressed terminal-centric finding per reached terminal;
    // the run-level loop-set registry is built as cohorts are interned.
    let g_asm = pt::span("d1", "assemble_cohort_findings");
    let (mut findings, registry) = assemble_cohort_findings(&run, ctx, &role_by_routine);
    drop(g_asm);

    // downgradedSetupSingleton: counted POST-assembly by rootCause text (mirrors
    // the old post-merge count, d1.ts:439) — unchanged.
    let mut downgraded_setup_singleton = 0u64;
    for f in &findings {
        if f.root_cause.contains("Setup singleton") {
            downgraded_setup_singleton += 1;
        }
    }

    // G-7 (docs/engine-gaps.md): DOWN-CONFIDENCE (never suppress) a finding whose
    // EVERY reaching loop's routine is provably dead per d14's EXACT criteria.
    // The old per-loop path collected each context witness's first-step (loop)
    // routine; the compressed report carries the SAME population as its loops'
    // catalog entries, so decompress every cohort's `loop_set` → loop routine id
    // via the run catalog (one loop routine per reaching loop — identical set).
    let g_g7 = pt::span("d1", "g7_down_confidence");
    let mut down_confidenced_dead_routine = 0u64;
    if !findings.is_empty() {
        let dead = crate::engine::l5::detectors::d14::provably_dead_routine_ids(resolved, ctx);
        if !dead.is_empty() {
            for f in &mut findings {
                let mut roots: Vec<&str> = Vec::new();
                for cc in f.cohort_contexts.iter().flatten() {
                    for g in registry.iter(cc.loop_set) {
                        roots.push(run.catalog[g as usize].loop_routine_id.as_str());
                    }
                }
                if roots.is_empty() || !roots.iter().all(|r| dead.contains(*r)) {
                    continue;
                }
                down_confidenced_dead_routine += 1;
                // One notch down; `possible` is already the floor, so it stays put.
                f.confidence.level = match f.confidence.level.as_str() {
                    "confirmed" => "likely".to_string(),
                    "likely" => "possible".to_string(),
                    other => other.to_string(),
                };
                f.root_cause = insert_temp_note(&f.root_cause, NOTE_DEAD_ROUTINE);
            }
        }
    }

    drop(g_g7);
    // Fingerprint: rootCauseKey + terminal primary location + affected tables —
    // all UNCHANGED by the cutover, so the edit-stable identity is preserved.
    let g_fp = pt::span("d1", "fingerprint_pass");
    for f in &mut findings {
        f.fingerprint = Some(fp_index.fingerprint_of(f));
    }
    drop(g_fp);
    // Deterministic output order by the terminal-based id.
    findings.sort_by(|a, b| a.id.cmp(&b.id));

    let emitted = findings.len();
    let mut stats = DetectorStats::new(DETECTOR, dstats.candidates_considered, emitted);
    stats.add_skip("opaqueCallee", dstats.skipped_opaque_callee);
    stats.add_skip("dynamicDispatch", dstats.skipped_dynamic_dispatch);
    stats.add_skip("parseIncomplete", dstats.skipped_parse_incomplete);
    stats.add_skip("virtualTable", dstats.skipped_virtual_table);
    stats.add_skip("downgradedToInfo", dstats.downgraded_to_info);
    stats.add_skip("downgradedSetupSingleton", downgraded_setup_singleton);
    stats.add_skip("downConfidencedDeadRoutine", down_confidenced_dead_routine);
    Ok(DetectorOutput {
        findings,
        stats,
        diagnostics: vec![],
        d1_cohort_index: Some(D1CohortIndex {
            catalog: run.catalog,
            registry,
        }),
    })
}

/// The fixed temp-note fragments (leading space included) the assembly appends to
/// a finding's rootCause, keyed off the WINNER context's verdict.
const NOTE_TEMPORARY: &str = " (temporary record — not a SQL round-trip)";
const NOTE_UNCERTAIN: &str = " (temp state uncertain)";
/// RV-1 (Task 11): the temp-record CalcFields/SetAutoCalcFields finding that the
/// FlowField gate KEEPS FIRING (a FlowField field arg, or an unresolvable one).
/// The host record is in-memory, but the FlowField CalcFormula is evaluated against
/// the physical flow targets — a real SQL round-trip.
const NOTE_TEMP_FLOWFIELD: &str =
    " (temporary record, but FlowField calculation queries the flow targets)";
/// G-7 (docs/engine-gaps.md): appended (with the one-notch confidence drop) when
/// EVERY path root routine of the finding is provably dead per d14's exact
/// criteria. The finding still fires — the loop cost is real IF the routine is
/// ever wired up — but a dead host makes it less actionable today.
const NOTE_DEAD_ROUTINE: &str =
    " (looping routine appears unreachable from any entry point; see d14-dead-routine)";

/// Insert `note` (which carries its own leading space) right before the trailing
/// setup-note/`.`. Both rootCause shapes — `"A loop in X reaches <tableNote>[tempNote]
/// [setupNote]."` and the G-4 pure-transitive `"… in Z, which has no loop of its own
/// — … of that loop[tempNote][setupNote]."` — keep `[tempNote][setupNote].` as the
/// tail, so re-inserting before the setup note if present (else before the final `.`)
/// lands the note exactly where the winner's temp note sat. Used for G-7's
/// `NOTE_DEAD_ROUTINE`.
fn insert_temp_note(root_cause: &str, note: &str) -> String {
    const SETUP_NOTE_PREFIX: &str = " (Setup singleton";
    if let Some(pos) = root_cause.find(SETUP_NOTE_PREFIX) {
        let mut s = root_cause.to_string();
        s.insert_str(pos, note);
        return s;
    }
    // Insert before the trailing period.
    if let Some(stripped) = root_cause.strip_suffix('.') {
        return format!("{stripped}{note}.");
    }
    format!("{root_cause}{note}")
}

/// Wrapper around `build_finding` that recovers the terminal op's owning-routine
/// id + internal source anchor before delegating. `terminal_routine` is the
/// op's owning routine (the DIRECT case passes `routine`; the call case passes
/// the routine resolved from `last_step.routine_id`). Test-only (the shadow
/// oracle's finding builder).
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn build_finding_internal(
    loop_routine: &L3Routine,
    representative_loop: &str,
    result: &WalkResult,
    terminal_op: &L3RecordOperation,
    terminal_routine: &L3Routine,
    routine_by_id: &HashMap<&str, &L3Routine>,
    table_by_id: &HashMap<&str, &L3Table>,
    role_by_routine: &HashMap<&str, &str>,
    edge_kind_by_callsite: &HashMap<&str, &str>,
    closed_world_temp_params: &ClosedWorldTempParams,
) -> (Finding, TempVerdict) {
    let terminal_op_anchor = anchor_of(&terminal_op.source_anchor, terminal_routine);
    build_finding(
        loop_routine,
        representative_loop,
        result,
        terminal_op,
        terminal_routine.id.as_str(),
        terminal_op_anchor,
        routine_by_id,
        table_by_id,
        role_by_routine,
        edge_kind_by_callsite,
        closed_world_temp_params,
    )
}

#[cfg(test)]
mod memo_tests {
    use super::*;
    use crate::engine::l5::capability_query::touches_db_of;
    use crate::engine::l5::full_summary::FullRoutineSummary;
    use crate::engine::l5::test_support::{coverage, edge, fact, routine, summary};

    /// `D1Policy::touches_db_memoized` must return exactly what a direct
    /// `touches_db_of` returns, for EVERY routine — across all three
    /// `EffectPresence` outcomes — and the cached (second) probe must equal the
    /// first. This is the soundness contract for the per-run memo that replaces
    /// the old per-edge `touches_db_of` call in the `walk_evidence` DFS.
    ///
    /// ⟨C1 Task 2⟩ The memo now fills from the DERIVED substrate, so this is
    /// additionally a raw-vs-derived parity assertion over the same three
    /// outcomes — the `touches_db_of` side is deliberately still the raw scan.
    #[test]
    fn touches_db_memo_matches_direct_for_every_routine() {
        // Spread covering every EffectPresence branch:
        //   Yes  — a `table` fact reachable (direct OR inherited);
        //   No   — no `table` fact AND inherited coverage "complete";
        //   Unk. — no `table` fact AND partial/absent coverage.
        let seed = vec![
            summary(
                "r_yes_direct",
                vec![fact("read", "table", Some("t/A"))],
                vec![],
                Some(coverage("complete")),
            ),
            summary(
                "r_yes_inherited",
                vec![fact("send", "http", None)],
                vec![fact("modify", "table", Some("t/B"))],
                Some(coverage("partial")),
            ),
            summary(
                "r_no",
                vec![fact("commit", "transaction", None)],
                vec![],
                Some(coverage("complete")),
            ),
            summary(
                "r_unknown_partial",
                vec![],
                vec![],
                Some(coverage("partial")),
            ),
            summary(
                "r_unknown_nocov",
                vec![fact("send", "http", None)],
                vec![],
                None,
            ),
        ];
        let summaries: HashMap<String, FullRoutineSummary> = seed
            .into_iter()
            .map(|s| (s.routine_id.clone(), s))
            .collect();

        // Otherwise-empty indexes — the memo path reads only `summaries` and the
        // derived cone folded from them.
        let routine_by_id: HashMap<&str, &L3Routine> = HashMap::new();
        let table_by_id: HashMap<&str, &L3Table> = HashMap::new();
        let edges_by_from: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        let call_site_by_id: HashMap<&str, &crate::engine::l2::features::PCallSite> =
            HashMap::new();
        let cone_derived = crate::engine::l5::test_support::cone_store_of(&summaries);

        let policy = D1Policy {
            routine_by_id: &routine_by_id,
            table_by_id: &table_by_id,
            summaries: &summaries,
            edges_by_from: &edges_by_from,
            call_site_by_id: &call_site_by_id,
            cone_derived: &cone_derived,
            touches_db_memo: RefCell::new(HashMap::new()),
            walk_memo: RefCell::new(HashMap::new()),
        };

        // At least one of each outcome is present (guards the fixture itself).
        let mut saw_yes = false;
        let mut saw_no = false;
        let mut saw_unknown = false;

        for (id, s) in &summaries {
            let direct = touches_db_of(s);
            let first = policy.touches_db_memoized(s);
            let second = policy.touches_db_memoized(s); // cached hit
            assert_eq!(
                direct, first,
                "first memo probe of {id} diverged from touches_db_of"
            );
            assert_eq!(
                first, second,
                "cached memo probe of {id} diverged from first"
            );
            match direct {
                EffectPresence::Yes => saw_yes = true,
                EffectPresence::No => saw_no = true,
                EffectPresence::Unknown => saw_unknown = true,
            }
        }
        assert!(
            saw_yes && saw_no && saw_unknown,
            "fixture must cover all three outcomes"
        );

        // Exactly one cached entry per distinct routine probed.
        assert_eq!(policy.touches_db_memo.borrow().len(), summaries.len());
    }

    /// An `EvidenceStep` with the given routine id + note (positions irrelevant).
    fn estep(rid: &str, note: &str) -> EvidenceStep {
        EvidenceStep {
            routine_id: rid.to_string(),
            operation_id: None,
            callsite_id: None,
            loop_id: None,
            source_anchor: SourceAnchor {
                source_unit_id: String::new(),
                start_line: 0,
                start_column: 0,
                end_line: 0,
                end_column: 0,
                enclosing_routine_id: rid.to_string(),
                syntax_kind: "call".to_string(),
                normalized_text_hash: None,
                leading_context_hash: None,
                trailing_context_hash: None,
            },
            note: note.to_string(),
        }
    }

    /// Soundness contract for the per-callee walk memo (Wave-2c): the CANONICAL
    /// walk from a callee (`initial_loop_depth: 0`, empty prefix) reused across
    /// callsites, then run through `apply_seed_transform`, must be BYTE-IDENTICAL
    /// to a fresh caller-specific `walk_evidence` for every callsite — same paths
    /// (incl. prepended prefixes), same `effective_loop_depth` (initial depth
    /// added), same uncertainties, same stop kind, same order. Callee `C → D`;
    /// `D` performs an in-loop `Modify` (a db-write terminal at local depth 1);
    /// two in-loop callsites reach `C` at DIFFERENT depths (1 and 2) with
    /// DIFFERENT evidence prefixes.
    #[test]
    fn memoized_walk_matches_fresh_walk_for_two_callsites() {
        // D's in-loop `Modify` op (db-write; not a terminator-Next; table_id None
        // and no matching record var ⇒ never a virtual-system-table op).
        let modify_op = L3RecordOperation {
            id: "D/op0".to_string(),
            op: "Modify".to_string(),
            record_variable_name: "Rec".to_string(),
            record_variable_id: None,
            table_id: None,
            temp_state: None,
            field_arguments: None,
            source_anchor: crate::engine::l2::features::PAnchor {
                source_unit_id: "ws:test".to_string(),
                start_line: 0,
                start_column: 0,
                end_line: 0,
                end_column: 0,
                syntax_kind: "test".to_string(),
            },
            loop_stack: vec!["D/loop0".to_string()], // local_loop_depth = 1
            field_argument_infos: None,
            in_until_condition: false,
            run_trigger: None,
        };

        let c = routine("C", "procedure");
        let mut d = routine("D", "procedure");
        d.record_operations = vec![modify_op];

        let routine_by_id: HashMap<&str, &L3Routine> = [("C", &c), ("D", &d)].into_iter().collect();
        let table_by_id: HashMap<&str, &L3Table> = HashMap::new();
        // Only D's summary is probed by `expand("C")`; a `table` fact makes
        // `touches_db_of(D) == Yes`, so the C→D edge is followed.
        let summaries: HashMap<String, FullRoutineSummary> = [(
            "D".to_string(),
            summary(
                "D",
                vec![fact("modify", "table", Some("t/X"))],
                vec![],
                Some(coverage("complete")),
            ),
        )]
        .into_iter()
        .collect();
        let mut edges_by_from: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        edges_by_from.insert("C".to_string(), vec![edge("C", "D", "C/cs0")]);
        // Empty ⇒ `loop_depth_of_edge` returns 0 for the C→D edge (deterministic).
        let call_site_by_id: HashMap<&str, &crate::engine::l2::features::PCallSite> =
            HashMap::new();

        // A per-node uncertainty on D so the transform's uncertainty passthrough
        // is exercised on a NON-empty set.
        let unc = Uncertainty {
            kind: "dynamic-call".to_string(),
            callsite_id: Some("D/cs1".to_string()),
            operation_id: None,
            routine_id: None,
            interface_name: None,
        };
        let mut uncertainties = OwnedUncertainties::default();
        uncertainties.insert("D", vec![unc]);

        let cone_derived = crate::engine::l5::test_support::cone_store_of(&summaries);
        let policy = D1Policy {
            routine_by_id: &routine_by_id,
            table_by_id: &table_by_id,
            summaries: &summaries,
            edges_by_from: &edges_by_from,
            call_site_by_id: &call_site_by_id,
            cone_derived: &cone_derived,
            touches_db_memo: RefCell::new(HashMap::new()),
            walk_memo: RefCell::new(HashMap::new()),
        };

        // Callsite 1: loop depth 1, a 2-step prefix. Callsite 2: loop depth 2, a
        // 1-step prefix — genuinely different seeds.
        let prefix1 = vec![estep("A", "OnRun loop"), estep("A", "calls C")];
        let prefix2 = vec![estep("B", "foreach loop")];

        let fresh_1 = walk_evidence(
            "C",
            &policy,
            BOUNDS,
            WalkOpts {
                initial_loop_depth: 1,
                initial_steps: prefix1.clone(),
            },
            uncertainties.view(),
            None,
        );
        let fresh_2 = walk_evidence(
            "C",
            &policy,
            BOUNDS,
            WalkOpts {
                initial_loop_depth: 2,
                initial_steps: prefix2.clone(),
            },
            uncertainties.view(),
            None,
        );

        // The CANONICAL walk (empty prefix, zero initial depth) — computed ONCE.
        let canonical = walk_evidence(
            "C",
            &policy,
            BOUNDS,
            WalkOpts {
                initial_loop_depth: 0,
                initial_steps: Vec::new(),
            },
            uncertainties.view(),
            None,
        );

        // Fixture guards: the walk actually reaches the db op, and the two
        // callsites genuinely differ (so the assertion has teeth).
        assert!(
            canonical.iter().any(|r| r.stop == WalkStop::Complete),
            "canonical walk must reach a Complete terminal"
        );
        assert_ne!(fresh_1, fresh_2, "the two callsite seeds must differ");

        // The load-bearing claim: memoized (canonical + transform) ≡ fresh.
        assert_eq!(
            apply_seed_transform(&canonical, 1, &prefix1),
            fresh_1,
            "memoized callsite-1 result diverged from a fresh walk"
        );
        assert_eq!(
            apply_seed_transform(&canonical, 2, &prefix2),
            fresh_2,
            "memoized callsite-2 result diverged from a fresh walk"
        );
    }
}

/// Task 4 (`.superpowers/sdd/task-4-brief.md`) — shadow differential: the OLD
/// `detect_d1_premerge` walk (`D1Policy` + `walk_evidence`, the still-live
/// exhaustive path walker) as a LOWER-BOUND oracle for the NEW
/// `build_d1_graph` and `search_loops` reachability pipeline (Tasks 1-3).
/// Nothing here changes `detect_d1`'s own output — this is a read-only
/// differential over the SAME `DetectorContext`/`L3Workspace` input, run
/// through both pipelines side by side.
///
/// Three oracles (every one a REAL assertion, not vacuous — each is proven to
/// fire on a non-empty population by the `assert!(... > 0, ...)` guards):
///   1. `shadow_old_premerge_keys_subset_of_new` — every OLD premerge
///      `(loop, terminal routine, terminal op)` key is present in the NEW
///      aggregate key set. The old walker's 500-node budget can UNDER-find (see
///      `budget_buster_star_fanout` below) — the new pipeline may find MORE,
///      never fewer.
///   2. `shadow_severity_non_decreasing` — for every key both pipelines share,
///      the NEW severity is never WORSE (lower-ranked) than the OLD one. Where
///      several old premerge records collapse onto the SAME `(loop, terminal
///      routine, terminal op)` key (multiple complete walk routes to the same
///      terminal, or multiple in-loop callsites into the same PD-terminal
///      callee — all sharing one exact `id`), the OLD side of the comparison
///      is the MAX severity among them. CORRECTNESS NOTE — this is a
///      DELIBERATELY STRICTER baseline than what `detect_d1` actually emits
///      for that exact `id` today: `detect_d1`'s own id-dedupe
///      (`seen.contains(&f.finding.id)`, first-wins) runs BEFORE
///      `reconcile_merge_tie`/`merge_by_terminal` and keeps whichever route the
///      DFS discovered FIRST, not the highest-severity one — `merge_by_terminal`
///      never even sees the dropped duplicates (it only reconciles ACROSS
///      DIFFERENT loops/ids that share a `root_cause_key`, picking the worst
///      there). So `max(severities sharing one id) >= detect_d1`'s actual
///      first-wins severity for that id always, which is why asserting
///      `new >= max` is still SOUND (it implies the weaker `new >=
///      detect_d1`'s-actual-output too) — but it is a stricter comparison than
///      "vs. what `detect_d1` emits today", not an exact restatement of it.
///      `shadow_do_workspace` below additionally compares against `detect_d1`'s
///      REAL post-merge output directly, since the MAX baseline undercounts
///      real severity upgrades.
///   3. `shadow_root_cause_keys_subset` — every OLD `rootCauseKey`
///      `(terminal routine, terminal op)` pair (the LOOP-independent identity
///      `merge_by_terminal` itself groups by) is present in the NEW
///      `(terminal owner, terminal op)` set.
///
/// Key extraction reads STRUCTURAL fields off `Finding`/`LoopTerminalAgg`
/// (`evidence_path[0].loop_id`, `evidence_path.last().routine_id`/
/// `.operation_id`, `agg.loop_id`/`agg.terminal.owner.id`/`agg.terminal.op.id`)
/// rather than parsing the `d1/{loop}/{routine}/{op}` id STRING: a routine or
/// op internal id can itself contain a `/` (every existing fixture already
/// shows this — e.g. an op id like `"T/op0"` or `"R/T"`), so a naive
/// slash-split is ambiguous. `build_finding`'s `loop_step`/`terminal_step`
/// evidence steps carry the SAME three identity fields the id string is built
/// from, unambiguously, for every finding (direct AND transitive) — see
/// `terminal_step`/`hop_step`'s callers above and `d1_reach::loop_step_ev`.
#[cfg(test)]
mod shadow_tests {
    use super::*;
    use crate::engine::l3::l3_workspace::L3Workspace;
    use crate::engine::l5::d1_graph::build_d1_graph;
    use crate::engine::l5::d1_reach::LoopTerminalAgg;
    use crate::engine::l5::full_summary::FullRoutineSummary;
    use crate::engine::l5::test_support::{
        arg_binding, call_site, coverage, edge_kind, fact, loop_def, minimal_ctx, record_op,
        routine, summary, ts_known, ts_pd,
    };

    /// A built fixture: the owned routines + the `edges_by_from` map + the
    /// per-routine summaries `minimal_ctx`/`build_d1_graph`/`detect_d1_premerge`
    /// all consume — the SAME shape `d1_graph`'s and `d1_reach`'s own test
    /// modules use.
    type Fixture = (
        Vec<L3Routine>,
        HashMap<String, Vec<CombinedEdge>>,
        HashMap<String, FullRoutineSummary>,
    );

    /// `severity_rank`, per the brief: `info=0, low=1, medium=2, high=3,
    /// critical=4`. Deliberately a FRESH, independent ranking (not a reuse of
    /// `path_merge::sev_rank`, which the merge/reconciliation logic under test
    /// itself already depends on) — an oracle should not share its measuring
    /// stick with the code it is checking.
    fn severity_rank(sev: &str) -> i32 {
        match sev {
            "info" => 0,
            "low" => 1,
            "medium" => 2,
            "high" => 3,
            "critical" => 4,
            other => panic!("shadow oracle: unrecognized severity {other:?}"),
        }
    }

    /// A summary with a single `read table` fact (`touches_db == Yes`).
    fn db_read_summary(id: &str, table: &str) -> FullRoutineSummary {
        summary(
            id,
            vec![fact("read", "table", Some(table))],
            vec![],
            Some(coverage("complete")),
        )
    }

    /// A summary with a single `modify table` fact (`touches_db == Yes`).
    fn db_write_summary(id: &str, table: &str) -> FullRoutineSummary {
        summary(
            id,
            vec![fact("modify", "table", Some(table))],
            vec![],
            Some(coverage("complete")),
        )
    }

    // =======================================================================
    // Fixtures — one scenario per named function, gathered by `fixtures()`.
    // Recreated locally (not re-exported from `d1_graph`'s/`d1_reach`'s own
    // `#[cfg(test)]` modules, which are private to those files) but covering
    // the SAME shapes those modules' own unit tests exercise: a direct in-loop
    // op, a single-hop transitive call, the budget-buster star fan-out (defect
    // D-A), the depth-2-beats-depth-1 severity race, the physical-beats-temp PD
    // race, an A->B->A cycle, a direct+transitive merge onto the same op, the
    // G-1/G-6 terminal filters, an event-dispatch-filtered edge, and two
    // distinct loops in one routine.
    // =======================================================================

    /// Branch (a) only: a direct in-loop `Modify`, no calls at all.
    fn fixture_direct_op_in_loop() -> Fixture {
        let mut r = routine("R", "procedure");
        r.loops = vec![loop_def("R/loop0")];
        r.record_operations = vec![record_op(
            "R/op0",
            "Modify",
            "Rec",
            Some("t/R"),
            vec!["R/loop0".to_string()],
            false,
        )];
        (vec![r], HashMap::new(), HashMap::new())
    }

    /// Branch (b): one loop, one in-loop call into a callee with its own
    /// (non-looping) db-touching terminal.
    fn fixture_transitive_single_hop() -> Fixture {
        let mut l = routine("L", "procedure");
        l.loops = vec![loop_def("L/loop0")];
        l.call_sites = vec![call_site("L/cs0", "B", vec!["L/loop0".to_string()])];
        let mut b = routine("B", "procedure");
        b.record_operations = vec![record_op(
            "B/op0",
            "Modify",
            "Rec",
            Some("t/B"),
            vec![],
            false,
        )];

        let routines = vec![l, b];
        let mut graph_edges: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        graph_edges.insert(
            "L".to_string(),
            vec![edge_kind("L", "B", "L/cs0", "direct")],
        );
        let summaries: HashMap<String, FullRoutineSummary> =
            [("B".to_string(), db_write_summary("B", "t/B"))]
                .into_iter()
                .collect();
        (routines, graph_edges, summaries)
    }

    /// Defect D-A: a star fan-out of 600 dead-end nodes plus one path to a
    /// terminal placed AFTER them in edge order — past the OLD walker's
    /// 500-node budget. OLD premerge finds NOTHING here; NEW finds the
    /// terminal (label-dedup cycle safety only, no node budget). Proves oracle
    /// 1 (subset, not equality) actually has teeth.
    fn fixture_budget_buster_star_fanout() -> Fixture {
        let mut r = routine("R", "procedure");
        r.loops = vec![loop_def("R/loop0")];
        r.call_sites = vec![call_site("R/cs0", "A", vec!["R/loop0".to_string()])];
        let a = routine("A", "procedure");

        let mut routines = vec![r, a];
        let mut a_edges: Vec<CombinedEdge> = Vec::new();
        let mut summaries: HashMap<String, FullRoutineSummary> = HashMap::new();
        summaries.insert("A".to_string(), db_read_summary("A", "t/A"));

        for i in 0..600 {
            let did = format!("D{i}");
            routines.push(routine(&did, "procedure"));
            a_edges.push(edge_kind("A", &did, &format!("A/cs{i}"), "direct"));
            summaries.insert(did.clone(), db_read_summary(&did, &format!("t/{did}")));
        }
        let mut t = routine("T", "procedure");
        t.record_operations = vec![record_op(
            "T/op0",
            "Modify",
            "Rec",
            Some("t/T"),
            vec![],
            false,
        )];
        routines.push(t);
        a_edges.push(edge_kind("A", "T", "A/csT", "direct"));
        summaries.insert("T".to_string(), db_read_summary("T", "t/T"));

        let mut graph_edges: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        graph_edges.insert(
            "R".to_string(),
            vec![edge_kind("R", "A", "R/cs0", "direct")],
        );
        graph_edges.insert("A".to_string(), a_edges);
        (routines, graph_edges, summaries)
    }

    /// Two routes to the same op: A->T (1 hop, bucket 1, medium) and
    /// A->X->Y->T (3 hops, bucket 2, high). Exercises oracle 2's "take the MAX
    /// old severity across records sharing a key" rule: the OLD walker's DFS
    /// pushes BOTH complete routes (same id, different severities) into the
    /// pre-merge population before dedup ever runs.
    fn fixture_severity_depth2_beats_depth1() -> Fixture {
        let mut r = routine("R", "procedure");
        r.loops = vec![loop_def("R/loop0")];
        r.call_sites = vec![call_site("R/cs0", "A", vec!["R/loop0".to_string()])];

        let mut a = routine("A", "procedure");
        a.call_sites = vec![
            call_site("A/csT", "T", vec![]),
            call_site("A/csX", "X", vec!["A/loop0".to_string()]),
        ];
        let x = routine("X", "procedure");
        let y = routine("Y", "procedure");
        let mut t = routine("T", "procedure");
        t.record_operations = vec![record_op(
            "T/op0",
            "FindSet",
            "Rec",
            Some("t/T"),
            vec![],
            false,
        )];

        let routines = vec![r, a, x, y, t];

        let mut graph_edges: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        graph_edges.insert(
            "R".to_string(),
            vec![edge_kind("R", "A", "R/cs0", "direct")],
        );
        graph_edges.insert(
            "A".to_string(),
            vec![
                edge_kind("A", "T", "A/csT", "direct"),
                edge_kind("A", "X", "A/csX", "direct"),
            ],
        );
        graph_edges.insert(
            "X".to_string(),
            vec![edge_kind("X", "Y", "X/csY", "direct")],
        );
        graph_edges.insert(
            "Y".to_string(),
            vec![edge_kind("Y", "T", "Y/csT", "direct")],
        );

        let summaries: HashMap<String, FullRoutineSummary> = ["A", "X", "Y", "T"]
            .iter()
            .map(|id| (id.to_string(), db_read_summary(id, &format!("t/{id}"))))
            .collect();
        (routines, graph_edges, summaries)
    }

    /// A PD-terminal op reached by two callsites in the SAME loop: one passing
    /// a temp record (verdict Temporary, info severity), one passing a
    /// physical record (verdict Physical, high severity). Exercises the SAME
    /// "max old severity" rule via a verdict/severity race rather than a
    /// depth race.
    fn fixture_physical_beats_temp() -> Fixture {
        let mut r = routine("R", "procedure");
        r.loops = vec![loop_def("R/loop0")];
        let mut cs0 = call_site("R/cs0", "H", vec!["R/loop0".to_string()]);
        cs0.argument_bindings = vec![arg_binding(0, Some(ts_known(true)))]; // temp
        let mut cs1 = call_site("R/cs1", "H", vec!["R/loop0".to_string()]);
        cs1.argument_bindings = vec![arg_binding(0, Some(ts_known(false)))]; // physical
        r.call_sites = vec![cs0, cs1];

        let mut h = routine("H", "procedure");
        let mut op0 = record_op("H/op0", "Modify", "Rec", Some("t/H"), vec![], false);
        op0.temp_state = Some(ts_pd(0));
        h.record_operations = vec![op0];

        let routines = vec![r, h];
        let mut graph_edges: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        graph_edges.insert(
            "R".to_string(),
            vec![
                edge_kind("R", "H", "R/cs0", "direct"),
                edge_kind("R", "H", "R/cs1", "direct"),
            ],
        );
        let summaries: HashMap<String, FullRoutineSummary> =
            [("H".to_string(), db_write_summary("H", "t/H"))]
                .into_iter()
                .collect();
        (routines, graph_edges, summaries)
    }

    /// An A->B->A cycle, terminal on B. Proves both pipelines terminate and
    /// agree on the single (loop, B/op0) key.
    fn fixture_cycle_terminates() -> Fixture {
        let mut r = routine("R", "procedure");
        r.loops = vec![loop_def("R/loop0")];
        r.call_sites = vec![call_site("R/cs0", "A", vec!["R/loop0".to_string()])];
        let a = routine("A", "procedure");
        let mut b = routine("B", "procedure");
        b.record_operations = vec![record_op(
            "B/op0",
            "Modify",
            "Rec",
            Some("t/B"),
            vec![],
            false,
        )];

        let routines = vec![r, a, b];
        let mut graph_edges: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        graph_edges.insert(
            "R".to_string(),
            vec![edge_kind("R", "A", "R/cs0", "direct")],
        );
        graph_edges.insert(
            "A".to_string(),
            vec![edge_kind("A", "B", "A/cs0", "direct")],
        );
        graph_edges.insert(
            "B".to_string(),
            vec![edge_kind("B", "A", "B/cs0", "direct")],
        );

        let summaries: HashMap<String, FullRoutineSummary> = [
            ("A".to_string(), db_read_summary("A", "t/A")),
            ("B".to_string(), db_write_summary("B", "t/B")),
        ]
        .into_iter()
        .collect();
        (routines, graph_edges, summaries)
    }

    /// R's in-loop op T is BOTH a direct op (branch a) AND a transitive
    /// terminal reached via a cycle back into R (branch b) — the shared
    /// per-(loop, terminal) aggregation must adjudicate to the same op either
    /// way.
    fn fixture_direct_and_transitive_same_op() -> Fixture {
        let mut r = routine("R", "procedure");
        r.loops = vec![loop_def("R/loop0")];
        r.call_sites = vec![call_site("R/cs0", "A", vec!["R/loop0".to_string()])];
        r.record_operations = vec![record_op(
            "R/T",
            "FindSet",
            "Rec",
            Some("t/R"),
            vec!["R/loop0".to_string()],
            false,
        )];

        let mut a = routine("A", "procedure");
        a.call_sites = vec![call_site("A/csR", "R", vec!["A/loop0".to_string()])];

        let routines = vec![r, a];
        let mut graph_edges: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        graph_edges.insert(
            "R".to_string(),
            vec![edge_kind("R", "A", "R/cs0", "direct")],
        );
        graph_edges.insert(
            "A".to_string(),
            vec![edge_kind("A", "R", "A/csR", "direct")],
        );

        let summaries: HashMap<String, FullRoutineSummary> = [
            ("A".to_string(), db_read_summary("A", "t/A")),
            ("R".to_string(), db_read_summary("R", "t/R")),
        ]
        .into_iter()
        .collect();
        (routines, graph_edges, summaries)
    }

    /// A terminator-`Next` (G-1) and a virtual/system-table `Get` (G-6) must be
    /// excluded from the terminal population by BOTH pipelines identically; a
    /// plain `Get` on the SAME routine survives.
    fn fixture_g1_g6_filters() -> Fixture {
        let mut l = routine("L", "procedure");
        l.loops = vec![loop_def("L/loop0")];
        l.call_sites = vec![call_site("L/cs0", "B", vec!["L/loop0".to_string()])];

        let mut b = routine("B", "procedure");
        b.record_variables = vec![crate::engine::l3::l3_workspace::L3RecordVariable {
            id: "B/rv0".to_string(),
            name: "F".to_string(),
            table_name: Some("Field".to_string()),
            table_id: None,
            is_parameter: false,
            parameter_index: None,
            temp_state: crate::engine::l2::features::PTempState {
                kind: "unknown".to_string(),
                value: None,
                parameter_index: None,
            },
            scope: None,
        }];
        let op_get = record_op("B/op0", "Get", "Rec", None, vec![], false);
        let op_next_terminator = record_op("B/op1", "Next", "Rec", None, vec![], true);
        let op_get_virtual = record_op("B/op2", "Get", "F", None, vec![], false);
        b.record_operations = vec![op_get, op_next_terminator, op_get_virtual];

        let routines = vec![l, b];
        let mut graph_edges: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        graph_edges.insert(
            "L".to_string(),
            vec![edge_kind("L", "B", "L/cs0", "direct")],
        );
        let summaries: HashMap<String, FullRoutineSummary> =
            [("B".to_string(), db_read_summary("B", "t/B"))]
                .into_iter()
                .collect();
        (routines, graph_edges, summaries)
    }

    /// An `event-dispatch` edge out of the in-loop callee must never be
    /// followed by either pipeline (D2's job, not D1's) — only the `direct`
    /// sibling edge's target contributes a terminal.
    fn fixture_event_dispatch_filtered() -> Fixture {
        let mut l = routine("L", "procedure");
        l.loops = vec![loop_def("L/loop0")];
        l.call_sites = vec![call_site("L/cs0", "A", vec!["L/loop0".to_string()])];

        let a = routine("A", "procedure");
        let mut b = routine("B", "procedure");
        b.record_operations = vec![record_op(
            "B/op0",
            "Modify",
            "Rec",
            Some("t/B"),
            vec![],
            false,
        )];
        let c = routine("C", "procedure");

        let routines = vec![l, a, b, c];
        let mut graph_edges: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        graph_edges.insert(
            "L".to_string(),
            vec![edge_kind("L", "A", "L/cs0", "direct")],
        );
        graph_edges.insert(
            "A".to_string(),
            vec![
                edge_kind("A", "B", "A/cs0", "direct"),
                edge_kind("A", "C", "A/cs1", "event-dispatch"),
            ],
        );
        let summaries: HashMap<String, FullRoutineSummary> = [
            ("A".to_string(), db_read_summary("A", "t/A")),
            ("B".to_string(), db_write_summary("B", "t/B")),
            ("C".to_string(), db_read_summary("C", "t/C")),
        ]
        .into_iter()
        .collect();
        (routines, graph_edges, summaries)
    }

    /// Two DISTINCT loops in one routine, each calling a different db-touching
    /// callee — checks the loop-keyed identity stays distinct rather than
    /// collapsing across loops.
    fn fixture_multi_loop_same_routine() -> Fixture {
        let mut r = routine("R", "procedure");
        r.loops = vec![loop_def("R/loop0"), loop_def("R/loop1")];
        r.call_sites = vec![
            call_site("R/cs0", "A", vec!["R/loop0".to_string()]),
            call_site("R/cs1", "B", vec!["R/loop1".to_string()]),
        ];
        let mut a = routine("A", "procedure");
        a.record_operations = vec![record_op(
            "A/op0",
            "Modify",
            "Rec",
            Some("t/A"),
            vec![],
            false,
        )];
        let mut b = routine("B", "procedure");
        b.record_operations = vec![record_op(
            "B/op0",
            "FindSet",
            "Rec",
            Some("t/B"),
            vec![],
            false,
        )];

        let routines = vec![r, a, b];
        let mut graph_edges: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        graph_edges.insert(
            "R".to_string(),
            vec![
                edge_kind("R", "A", "R/cs0", "direct"),
                edge_kind("R", "B", "R/cs1", "direct"),
            ],
        );
        let summaries: HashMap<String, FullRoutineSummary> = [
            ("A".to_string(), db_write_summary("A", "t/A")),
            ("B".to_string(), db_read_summary("B", "t/B")),
        ]
        .into_iter()
        .collect();
        (routines, graph_edges, summaries)
    }

    fn fixtures() -> Vec<(&'static str, Fixture)> {
        vec![
            ("direct_op_in_loop", fixture_direct_op_in_loop()),
            ("transitive_single_hop", fixture_transitive_single_hop()),
            (
                "budget_buster_star_fanout",
                fixture_budget_buster_star_fanout(),
            ),
            (
                "severity_depth2_beats_depth1",
                fixture_severity_depth2_beats_depth1(),
            ),
            ("physical_beats_temp", fixture_physical_beats_temp()),
            ("cycle_terminates", fixture_cycle_terminates()),
            (
                "direct_and_transitive_same_op",
                fixture_direct_and_transitive_same_op(),
            ),
            ("g1_g6_filters", fixture_g1_g6_filters()),
            ("event_dispatch_filtered", fixture_event_dispatch_filtered()),
            ("multi_loop_same_routine", fixture_multi_loop_same_routine()),
        ]
    }

    // The shadow differential reuses the PRODUCTION `enumerate_direct_ops`
    // (`super::enumerate_direct_ops`) — the same direct-op population + stat
    // counting `detect_d1` runs — so the oracle can never drift from production.

    /// The OWNED result of running both pipelines over one fixture: every
    /// field is a plain `String`/`i32` (no borrowed lifetimes), so the three
    /// oracle tests below can each call this once per fixture without any
    /// lifetime entanglement between the OLD (`L3Resolved`-based) and NEW
    /// (`L3Workspace`-based) call paths.
    struct ShadowCase {
        /// Every `(loop, terminal routine, terminal op)` key ANY pre-merge OLD
        /// finding carries.
        old_keys: HashSet<(String, String, String)>,
        /// The MAX severity rank across every pre-merge OLD finding sharing a
        /// key — a deliberately STRICTER baseline than `detect_d1`'s own
        /// id-first-wins dedupe actually emits for that exact id (see the
        /// module doc's oracle-2 correctness note for why `new >= max` is
        /// still sound, just not an exact restatement of production output).
        old_max_sev_by_key: HashMap<(String, String, String), i32>,
        /// The NEW aggregate's severity rank, by the SAME 3-part key (one
        /// `LoopTerminalAgg` per key, by construction — see `search_loops`).
        new_sev_by_key: HashMap<(String, String, String), i32>,
        /// Every OLD `rootCauseKey`, as its `(terminal routine, terminal op)`
        /// pair (loop-independent — the identity `merge_by_terminal` groups by).
        old_root_cause_keys: HashSet<(String, String)>,
        /// Every NEW `(terminal owner, terminal op)` pair.
        new_owner_op_keys: HashSet<(String, String)>,
    }

    /// The `(loop_id, terminal_routine_id, terminal_op_id)` identity
    /// `build_finding` bakes into every finding's OWN evidence path — read
    /// structurally (see the module doc's "Key extraction" paragraph for why:
    /// a naive slash-split of the `d1/{loop}/{routine}/{op}` id string is
    /// ambiguous whenever a routine/op internal id itself contains a `/`).
    /// Shared by every key-extraction helper below, including the vs-actual
    /// comparison (`extract_final_sev_by_key`), so the identity rule exists
    /// exactly once.
    fn finding_identity(f: &Finding) -> (String, String, String) {
        // Through `evidence_path_of`: `extract_final_sev_by_key` runs this over
        // `detect_d1`'s REAL post-merge output, whose findings are cohort-bearing
        // and therefore derive their path from `cohort_contexts[0].witness`.
        let path = crate::engine::l5::finding::evidence_path_of(f);
        let loop_id = path[0]
            .loop_id
            .clone()
            .expect("a d1 finding's first evidence step is always a loop step (build_finding)");
        let last = path
            .last()
            .expect("a d1 finding's evidence_path is never empty");
        let routine_id = last.routine_id.clone();
        let op_id = last.operation_id.clone().expect(
            "a d1 finding's terminal evidence step always carries operation_id \
             (terminal_step/direct op_step)",
        );
        (loop_id, routine_id, op_id)
    }

    /// The three OLD-side key sets, extracted from a `detect_d1_premerge` run
    /// — shared by `compute_shadow_case` (fixtures) and `shadow_do_workspace`
    /// (the real-workspace DO run) so the extraction logic exists exactly
    /// once.
    #[allow(clippy::type_complexity)]
    fn extract_old_keys(
        premerge: &[FindingRec],
    ) -> (
        HashSet<(String, String, String)>,
        HashMap<(String, String, String), i32>,
        HashSet<(String, String)>,
    ) {
        let mut old_keys: HashSet<(String, String, String)> = HashSet::new();
        let mut old_max_sev_by_key: HashMap<(String, String, String), i32> = HashMap::new();
        let mut old_root_cause_keys: HashSet<(String, String)> = HashSet::new();
        for rec in premerge {
            let f = &rec.finding;
            let (loop_id, routine_id, op_id) = finding_identity(f);
            let key = (loop_id, routine_id.clone(), op_id.clone());
            old_keys.insert(key.clone());
            let rank = severity_rank(&f.severity);
            old_max_sev_by_key
                .entry(key)
                .and_modify(|v| *v = (*v).max(rank))
                .or_insert(rank);
            old_root_cause_keys.insert((routine_id, op_id));
        }
        (old_keys, old_max_sev_by_key, old_root_cause_keys)
    }

    /// The two NEW-side key maps, extracted from a `search_loops` run —
    /// shared by `compute_shadow_case` and `shadow_do_workspace` (see
    /// `extract_old_keys`).
    #[allow(clippy::type_complexity)]
    fn extract_new_keys(
        aggs: &[LoopTerminalAgg],
    ) -> (
        HashMap<(String, String, String), i32>,
        HashSet<(String, String)>,
    ) {
        let mut new_sev_by_key: HashMap<(String, String, String), i32> = HashMap::new();
        let mut new_owner_op_keys: HashSet<(String, String)> = HashSet::new();
        for agg in aggs {
            let key = (
                agg.loop_id.to_string(),
                agg.terminal.owner.id.clone(),
                agg.terminal.op.id.clone(),
            );
            new_sev_by_key.insert(key, severity_rank(agg.severity));
            new_owner_op_keys.insert((agg.terminal.owner.id.clone(), agg.terminal.op.id.clone()));
        }
        (new_sev_by_key, new_owner_op_keys)
    }

    /// Key a `detect_d1`-shaped `&[Finding]` by `(terminal routine, terminal
    /// op)` ONLY — no loop, matching `root_cause_key`'s granularity — taking
    /// the MAX severity per key (defensive: `detect_d1`'s own
    /// `merge_by_terminal` already collapses to at most one `Finding` per
    /// `root_cause_key`, so in practice every key has exactly one severity,
    /// but `max` costs nothing and keeps this helper robust to that invariant
    /// ever loosening). Used by `shadow_do_workspace`'s vs-actual comparison
    /// (Task 4 review finding #1): the OLD-max-baseline oracles above compare
    /// against a STRICTER baseline than `detect_d1` actually emits (see the
    /// module doc), so this reads `detect_d1`'s REAL post-merge output
    /// directly for an apples-to-apples check.
    fn extract_final_sev_by_key(findings: &[Finding]) -> HashMap<(String, String), i32> {
        let mut out: HashMap<(String, String), i32> = HashMap::new();
        for f in findings {
            let (_loop_id, routine_id, op_id) = finding_identity(f);
            let rank = severity_rank(&f.severity);
            out.entry((routine_id, op_id))
                .and_modify(|v| *v = (*v).max(rank))
                .or_insert(rank);
        }
        out
    }

    /// Fold a loop-keyed severity map down to `(terminal routine, terminal
    /// op)` by taking the MAX severity across every loop that reaches it —
    /// the SAME reduction `merge_by_terminal` performs on the OLD side
    /// (worst-wins across different loops/ids sharing a `root_cause_key`),
    /// applied here to the NEW side so the vs-actual comparison is
    /// apples-to-apples at the SAME (post-merge) granularity as
    /// `extract_final_sev_by_key`'s output.
    fn fold_to_owner_op_max(
        sev_by_key: &HashMap<(String, String, String), i32>,
    ) -> HashMap<(String, String), i32> {
        let mut out: HashMap<(String, String), i32> = HashMap::new();
        for ((_loop_id, routine_id, op_id), &rank) in sev_by_key {
            out.entry((routine_id.clone(), op_id.clone()))
                .and_modify(|v| *v = (*v).max(rank))
                .or_insert(rank);
        }
        out
    }

    /// Run BOTH pipelines over one fixture and extract the oracle keys.
    fn compute_shadow_case(
        routines: &[L3Routine],
        graph_edges: HashMap<String, Vec<CombinedEdge>>,
        summaries: HashMap<String, FullRoutineSummary>,
    ) -> ShadowCase {
        let ctx = minimal_ctx(routines, graph_edges, summaries);

        // OLD: the still-live PW-0 premerge walk, over its OWN (separately
        // owned) `L3Resolved`/`L3Workspace` clone — `detect_d1_premerge`
        // returns fully OWNED data (no lifetime tie to `resolved` or `ctx`),
        // so this is safe even though `resolved.workspace.routines` and
        // `ctx`'s backing store are different (content-identical) allocations,
        // exactly like `ctx`/`workspace` already are in `d1_graph`'s and
        // `d1_reach`'s own test modules.
        let resolved = L3Resolved {
            workspace: L3Workspace {
                objects: vec![],
                tables: vec![],
                routines: routines.to_vec(),
            },
            root_classifications: vec![],
            primary_app: None,
            infra_diagnostics: vec![],
            precomputed_calls: None,
            precomputed_events: None,
        };
        let premerge = detect_d1_premerge(&resolved, &ctx);
        let (old_keys, old_max_sev_by_key, old_root_cause_keys) = extract_old_keys(&premerge);

        // NEW: build_d1_graph + direct-op enumeration + search_loops, over the
        // SAME `ctx` (borrowing the original `routines` slice) plus a second,
        // separately-owned `L3Workspace` clone (mirrors `d1_graph`'s/
        // `d1_reach`'s own `ws(&routines)` helper).
        let ws = L3Workspace {
            objects: vec![],
            tables: vec![],
            routines: routines.to_vec(),
        };
        let mut memo = HashMap::new();
        let (graph, seeds) = build_d1_graph(&ctx, &ws, &mut memo);
        let (direct_ops, _stats) = enumerate_direct_ops(&ws, &ctx);
        let aggs = search_loops(
            &graph,
            &seeds,
            &direct_ops,
            &ctx,
            &ctx.closed_world_temp_params,
        );
        let (new_sev_by_key, new_owner_op_keys) = extract_new_keys(&aggs);

        ShadowCase {
            old_keys,
            old_max_sev_by_key,
            new_sev_by_key,
            old_root_cause_keys,
            new_owner_op_keys,
        }
    }

    /// The ONE fixture with a legitimate, BY-DESIGN empty OLD-side population:
    /// `budget_buster_star_fanout`'s whole point is that the old walker's
    /// 500-node budget starves before it ever reaches the terminal, so its
    /// `detect_d1_premerge` output (and every OLD-keyed set derived from it)
    /// is EXPECTED to be empty. Every other fixture must contribute a
    /// non-empty OLD-side population — an aggregate-only `> 0` guard across
    /// all ten fixtures would let any ONE of them silently regress to zero
    /// findings without failing (Task 4 review finding #2); naming the one
    /// legitimate exception explicitly closes that gap.
    const EXPECTED_EMPTY_OLD_SIDE_FIXTURE: &str = "budget_buster_star_fanout";

    #[test]
    fn shadow_old_premerge_keys_subset_of_new() {
        let mut total_old = 0usize;
        let mut total_new = 0usize;
        for (name, (routines, graph_edges, summaries)) in fixtures() {
            let case = compute_shadow_case(&routines, graph_edges, summaries);
            total_old += case.old_keys.len();
            total_new += case.new_sev_by_key.len();
            for key in &case.old_keys {
                assert!(
                    case.new_sev_by_key.contains_key(key),
                    "fixture {name}: old premerge key {key:?} missing from the new \
                     search_loops aggregate key set"
                );
            }
            if name == EXPECTED_EMPTY_OLD_SIDE_FIXTURE {
                assert!(
                    case.old_keys.is_empty(),
                    "fixture {name}: expected ZERO old premerge keys (the old \
                     walker's 500-node budget should starve before reaching the \
                     terminal) — got {:?}",
                    case.old_keys
                );
                assert!(
                    !case.new_sev_by_key.is_empty(),
                    "fixture {name}: expected the NEW pipeline to still find the \
                     terminal past the old budget — got zero new aggregates \
                     (the fixture would be pointless if it did)"
                );
            } else {
                assert!(
                    !case.old_keys.is_empty(),
                    "fixture {name}: expected at least one old premerge key, got \
                     zero — a silent per-fixture regression an aggregate-only \
                     count would hide"
                );
                assert!(
                    !case.new_sev_by_key.is_empty(),
                    "fixture {name}: expected at least one new aggregate, got zero"
                );
            }
        }
        assert!(
            total_old > 0,
            "fixture population must exercise at least one OLD premerge finding"
        );
        assert!(
            total_new > 0,
            "fixture population must exercise at least one NEW aggregate"
        );
    }

    #[test]
    fn shadow_severity_non_decreasing() {
        let mut compared_total = 0usize;
        for (name, (routines, graph_edges, summaries)) in fixtures() {
            let case = compute_shadow_case(&routines, graph_edges, summaries);
            let mut compared_here = 0usize;
            for (key, &old_rank) in &case.old_max_sev_by_key {
                let Some(&new_rank) = case.new_sev_by_key.get(key) else {
                    // A key missing from the NEW side is oracle 1's violation to
                    // report, not this oracle's — skip (nothing to compare).
                    continue;
                };
                compared_here += 1;
                assert!(
                    new_rank >= old_rank,
                    "fixture {name}: severity regressed for key {key:?}: \
                     old(max)={old_rank} new={new_rank}"
                );
            }
            compared_total += compared_here;
            if name == EXPECTED_EMPTY_OLD_SIDE_FIXTURE {
                assert_eq!(
                    compared_here, 0,
                    "fixture {name}: expected ZERO shared (old, new) keys (the old \
                     side has no premerge findings here) — got {compared_here}"
                );
            } else {
                assert!(
                    compared_here > 0,
                    "fixture {name}: expected at least one shared (old, new) key \
                     to compare severities over, got zero — a silent per-fixture \
                     regression an aggregate-only count would hide"
                );
            }
        }
        assert!(
            compared_total > 0,
            "fixture population must exercise at least one shared (old, new) key"
        );
    }

    #[test]
    fn shadow_root_cause_keys_subset() {
        let mut total = 0usize;
        for (name, (routines, graph_edges, summaries)) in fixtures() {
            let case = compute_shadow_case(&routines, graph_edges, summaries);
            total += case.old_root_cause_keys.len();
            for key in &case.old_root_cause_keys {
                assert!(
                    case.new_owner_op_keys.contains(key),
                    "fixture {name}: old rootCauseKey (routine, op) {key:?} missing from \
                     the new (owner, op) set"
                );
            }
            if name == EXPECTED_EMPTY_OLD_SIDE_FIXTURE {
                assert!(
                    case.old_root_cause_keys.is_empty(),
                    "fixture {name}: expected ZERO old rootCauseKeys — got {:?}",
                    case.old_root_cause_keys
                );
            } else {
                assert!(
                    !case.old_root_cause_keys.is_empty(),
                    "fixture {name}: expected at least one old rootCauseKey, got \
                     zero — a silent per-fixture regression an aggregate-only \
                     count would hide"
                );
            }
        }
        assert!(
            total > 0,
            "fixture population must exercise at least one old rootCauseKey"
        );
    }

    /// Manual DO shadow run (Task 4, step 3): loads a REAL Business Central
    /// workspace via the `DO_WS` env var, runs the SAME three oracles above
    /// over it, and prints the partitioned diff — keys only in the NEW
    /// aggregate population ("new-coverage") vs. shared keys where the NEW
    /// severity is STRICTLY better ("severity-upgrade") vs. unchanged.
    ///
    /// ALSO prints a companion "vs-actual" comparison (Task 4 review finding
    /// #1): the partition above is measured against `old_max_sev_by_key` — a
    /// DELIBERATELY STRICTER baseline than what `detect_d1` actually emits
    /// today (see the module doc's oracle-2 correctness note), so it
    /// UNDERCOUNTS real severity upgrades. This block instead runs the ACTUAL
    /// production `detect_d1` and compares the new severity against its REAL
    /// post-merge output, keyed by `(terminal routine, terminal op)` (folding
    /// both sides' per-loop severities to their max first — the same
    /// reduction `merge_by_terminal` itself performs), which is the number
    /// Task 5/6 should actually use as the triage pre-read.
    /// `#[ignore]`d: reads a real workspace off disk, which does not exist in
    /// CI or on most dev machines. Run explicitly:
    /// ```text
    /// DO_WS='...' cargo test -p al-call-hierarchy --lib shadow_do_workspace -- --ignored --nocapture
    /// ```
    /// FAILS (not just warns) if any of the three oracles is violated — on a
    /// real workspace that is a genuine Tasks 1-3 divergence bug, never noise.
    #[test]
    #[ignore]
    fn shadow_do_workspace() {
        let Some(ws_path) = std::env::var_os("DO_WS").map(std::path::PathBuf::from) else {
            eprintln!(
                "shadow_do_workspace: DO_WS not set — skipping (see \
                 .superpowers/sdd/task-4-brief.md step 3)"
            );
            return;
        };
        assert!(
            ws_path.exists(),
            "DO_WS does not point at an existing path: {}",
            ws_path.display()
        );

        let resolved =
            crate::engine::l3::l3_workspace::assemble_and_resolve_workspace_default(&ws_path)
                .expect("assemble_and_resolve_workspace_default failed for DO_WS");
        let ctx = crate::engine::l5::detector_context::build_detector_context(
            &resolved,
            crate::engine::l5::registry::substrate::SUMMARIES
                | crate::engine::l5::registry::substrate::CORE_SUMMARIES
                | crate::engine::l5::registry::substrate::CLOSED_WORLD_TEMP,
        );
        let ws = &resolved.workspace;

        let premerge = detect_d1_premerge(&resolved, &ctx);
        let (old_keys, old_max_sev_by_key, old_root_cause_keys) = extract_old_keys(&premerge);

        let mut memo = HashMap::new();
        let (graph, seeds) = build_d1_graph(&ctx, ws, &mut memo);
        let (direct_ops, _stats) = enumerate_direct_ops(ws, &ctx);
        let aggs = search_loops(
            &graph,
            &seeds,
            &direct_ops,
            &ctx,
            &ctx.closed_world_temp_params,
        );
        let (new_sev_by_key, new_owner_op_keys) = extract_new_keys(&aggs);

        let mut new_coverage = 0usize;
        let mut severity_upgrade = 0usize;
        let mut unchanged = 0usize;
        for (key, &new_rank) in &new_sev_by_key {
            match old_max_sev_by_key.get(key) {
                None => new_coverage += 1,
                Some(&old_rank) => {
                    if new_rank > old_rank {
                        severity_upgrade += 1;
                    } else {
                        unchanged += 1;
                    }
                }
            }
        }

        println!(
            "SUBSET counts: oldKeys={} newKeys={}",
            old_keys.len(),
            new_sev_by_key.len()
        );
        println!(
            "partition: added(new-coverage)={new_coverage} upgraded(severity-upgrade)={severity_upgrade} unchanged={unchanged}"
        );
        println!(
            "rootCauseKey counts: old={} newOwnerOp={}",
            old_root_cause_keys.len(),
            new_owner_op_keys.len()
        );

        // vs-actual (Task 4 review finding #1): compare against detect_d1's
        // REAL post-merge output, not the stricter MAX-of-premerge baseline
        // above. `detect_d1` re-runs the SAME premerge walk internally, so
        // this is a second (more expensive but more accurate) pass over the
        // SAME `resolved`/`ctx` — acceptable here since this whole test is
        // manual/`#[ignore]`d.
        let actual_output = detect_d1(&resolved, &ctx).expect("detect_d1 must not error on DO_WS");
        let actual_final_sev_by_key = extract_final_sev_by_key(&actual_output.findings);
        let new_owner_op_max_sev = fold_to_owner_op_max(&new_sev_by_key);

        let mut vs_actual_upgraded = 0usize;
        let mut vs_actual_unchanged = 0usize;
        let mut vs_actual_regressed = 0usize;
        for (key, &actual_rank) in &actual_final_sev_by_key {
            let Some(&new_rank) = new_owner_op_max_sev.get(key) else {
                // Covered by oracle 3 (rootCauseKeys subset) — every actual
                // rootCauseKey is a premerge rootCauseKey, which is already
                // asserted a subset of the new owner/op set below.
                continue;
            };
            match new_rank.cmp(&actual_rank) {
                std::cmp::Ordering::Greater => vs_actual_upgraded += 1,
                std::cmp::Ordering::Equal => vs_actual_unchanged += 1,
                std::cmp::Ordering::Less => vs_actual_regressed += 1,
            }
        }
        println!(
            "vs-actual: upgraded={vs_actual_upgraded} unchanged={vs_actual_unchanged} regressed={vs_actual_regressed}"
        );

        for key in &old_keys {
            assert!(
                new_sev_by_key.contains_key(key),
                "DO SUBSET violation: old premerge key {key:?} missing from the new \
                 aggregate set"
            );
        }
        println!("SUBSET oracle 1 (premerge keys): PASS");

        for (key, &old_rank) in &old_max_sev_by_key {
            if let Some(&new_rank) = new_sev_by_key.get(key) {
                assert!(
                    new_rank >= old_rank,
                    "SEVERITY regression for key {key:?}: old(max)={old_rank} new={new_rank}"
                );
            }
        }
        println!("SEVERITY oracle 2 (non-decreasing): PASS");

        for key in &old_root_cause_keys {
            assert!(
                new_owner_op_keys.contains(key),
                "rootCauseKey SUBSET violation: {key:?} missing from the new (owner, op) set"
            );
        }
        println!("SUBSET oracle 3 (rootCauseKeys): PASS");

        // vs-actual is mathematically implied by oracle 2 (new >= old-max >=
        // detect_d1's actual first-wins severity per id, and taking max over
        // loops on both sides preserves that inequality) — assert it holds in
        // practice too rather than only printing the (expected-zero) count.
        assert_eq!(
            vs_actual_regressed, 0,
            "new severity regressed vs. detect_d1's ACTUAL post-merge output for \
             {vs_actual_regressed} key(s) — a genuine Tasks 1-3 divergence bug"
        );
        println!("vs-actual regression check: PASS");
    }
}
/// Task 5 assembly tests (`.superpowers/sdd/task-5-brief.md` Step 1): each test
/// asserts a locked-semantics bullet of the terminal-centric assembly. Tests 1-3
/// drive the production pipeline directly (`build_d1_graph` + `search_loops` +
/// `assemble_findings`) over hand-built fixtures; test 4 exercises the G-7
/// down-confidence over REAL AL source (it needs the d14 dead-routine substrate).
#[cfg(test)]
mod assembly_tests {
    use super::*;
    use crate::engine::l3::l3_workspace::{L3Resolved, L3Workspace};
    use crate::engine::l5::d1_graph::build_d1_graph;
    use crate::engine::l5::d1_reach::search_loops;
    use crate::engine::l5::full_summary::FullRoutineSummary;
    use crate::engine::l5::test_support::{
        call_site, coverage, edge_kind, fact, loop_def, minimal_ctx, record_op, routine, summary,
    };

    type Fixture = (
        Vec<L3Routine>,
        HashMap<String, Vec<CombinedEdge>>,
        HashMap<String, FullRoutineSummary>,
    );

    fn db_write_summary(id: &str, table: &str) -> FullRoutineSummary {
        summary(
            id,
            vec![fact("modify", "table", Some(table))],
            vec![],
            Some(coverage("complete")),
        )
    }

    fn ws(routines: &[L3Routine]) -> L3Workspace {
        L3Workspace {
            objects: vec![],
            tables: vec![],
            routines: routines.to_vec(),
        }
    }

    fn role_map(routines: &[L3Routine]) -> HashMap<&str, &str> {
        routines
            .iter()
            .map(|r| (r.id.as_str(), "primary"))
            .collect()
    }

    /// Two DISTINCT loop routines (L1, L2) each in-loop-call a shared helper H
    /// whose single (non-looping) `Modify` is the terminal — the canonical
    /// "one terminal reached by N loops" shape.
    fn two_loops_one_terminal() -> Fixture {
        let mut l1 = routine("L1", "procedure");
        l1.loops = vec![loop_def("L1/loop0")];
        l1.call_sites = vec![call_site("L1/cs0", "H", vec!["L1/loop0".to_string()])];
        let mut l2 = routine("L2", "procedure");
        l2.loops = vec![loop_def("L2/loop0")];
        l2.call_sites = vec![call_site("L2/cs0", "H", vec!["L2/loop0".to_string()])];
        let mut h = routine("H", "procedure");
        h.record_operations = vec![record_op(
            "H/op0",
            "Modify",
            "Rec",
            Some("t/H"),
            vec![],
            false,
        )];

        let routines = vec![l1, l2, h];
        let mut edges: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        edges.insert(
            "L1".to_string(),
            vec![edge_kind("L1", "H", "L1/cs0", "direct")],
        );
        edges.insert(
            "L2".to_string(),
            vec![edge_kind("L2", "H", "L2/cs0", "direct")],
        );
        let summaries: HashMap<String, FullRoutineSummary> =
            [("H".to_string(), db_write_summary("H", "t/H"))]
                .into_iter()
                .collect();
        (routines, edges, summaries)
    }

    /// Run the SHADOW ORACLE (the retired per-loop path) over a fixture and return
    /// each finding with its per-loop `LoopContext`s beside it — `Finding` no
    /// longer carries them.
    fn assemble(
        routines: &[L3Routine],
        edges: HashMap<String, Vec<CombinedEdge>>,
        summaries: HashMap<String, FullRoutineSummary>,
    ) -> Vec<(Finding, Vec<LoopContext>)> {
        let ctx = minimal_ctx(routines, edges, summaries);
        let workspace = ws(routines);
        let mut memo = HashMap::new();
        let (graph, seeds) = build_d1_graph(&ctx, &workspace, &mut memo);
        let (direct_ops, _stats) = enumerate_direct_ops(&workspace, &ctx);
        let aggs = search_loops(
            &graph,
            &seeds,
            &direct_ops,
            &ctx,
            &ctx.closed_world_temp_params,
        );
        assemble_findings(&aggs, &ctx, &role_map(routines))
    }

    /// Bullet: "Group aggregates by (terminal_routine_id, op_id). One Finding per
    /// group" + one `LoopContext` per reaching loop + terminal-based id.
    #[test]
    fn one_finding_per_terminal_with_contexts() {
        let (routines, edges, summaries) = two_loops_one_terminal();
        let findings = assemble(&routines, edges, summaries);

        assert_eq!(findings.len(), 1, "one Finding per (terminal routine, op)");
        let f = &findings[0].0;
        assert_eq!(f.id, "d1/H/H/op0", "terminal-based id");
        assert_eq!(f.root_cause_key, f.id, "id == root_cause_key");
        let ctxs = &findings[0].1;
        assert_eq!(ctxs.len(), 2, "one context per reaching loop");
        // Context order: same severity/verdict here, so loop routine id ascending.
        assert_eq!(ctxs[0].loop_routine_id, "L1");
        assert_eq!(ctxs[1].loop_routine_id, "L2");
        // additional_paths = the one non-winner witness.
        assert_eq!(
            f.additional_paths.as_ref().map(|p| p.len()),
            Some(1),
            "non-winner witness kept in additional_paths"
        );
    }

    /// A terminal reached by TWO loops at DIFFERENT severities: L1 directly
    /// (depth 1 -> high) and L2 via M whose call to H is in M's own loop
    /// (depth 2 -> critical). The winner (L2/critical) must drive severity,
    /// confidence, evidence_path and wording.
    fn severity_race_fixture() -> Fixture {
        let mut l1 = routine("L1", "procedure");
        l1.loops = vec![loop_def("L1/loop0")];
        l1.call_sites = vec![call_site("L1/cs0", "H", vec!["L1/loop0".to_string()])];

        let mut l2 = routine("L2", "procedure");
        l2.loops = vec![loop_def("L2/loop0")];
        l2.call_sites = vec![call_site("L2/cs0", "M", vec!["L2/loop0".to_string()])];

        // M's call to H sits in M/loop0, so the M->H edge carries loop_depth 1 (via
        // ctx.call_site_by_id). M has no loops of its own, so it never seeds.
        let mut m = routine("M", "procedure");
        m.call_sites = vec![call_site("M/cs0", "H", vec!["M/loop0".to_string()])];

        let mut h = routine("H", "procedure");
        h.record_operations = vec![record_op(
            "H/op0",
            "Modify",
            "Rec",
            Some("t/H"),
            vec![],
            false,
        )];

        let routines = vec![l1, l2, m, h];
        let mut edges: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        edges.insert(
            "L1".to_string(),
            vec![edge_kind("L1", "H", "L1/cs0", "direct")],
        );
        edges.insert(
            "L2".to_string(),
            vec![edge_kind("L2", "M", "L2/cs0", "direct")],
        );
        edges.insert(
            "M".to_string(),
            vec![edge_kind("M", "H", "M/cs0", "direct")],
        );
        let summaries: HashMap<String, FullRoutineSummary> = [
            ("M".to_string(), db_write_summary("M", "t/M")),
            ("H".to_string(), db_write_summary("H", "t/H")),
        ]
        .into_iter()
        .collect();
        (routines, edges, summaries)
    }

    /// Bullet: "Finding severity, confidence, evidence_path, notes, wording all
    /// from the winner (severity and confidence from the SAME context)."
    #[test]
    fn winner_drives_severity_confidence_and_wording() {
        let (routines, edges, summaries) = severity_race_fixture();
        let findings = assemble(&routines, edges, summaries);

        assert_eq!(findings.len(), 1);
        let f = &findings[0].0;
        let ctxs = &findings[0].1;
        assert_eq!(ctxs.len(), 2);
        // Winner is the depth-2 (critical) L2 route; the high L1 route is second.
        assert_eq!(ctxs[0].loop_routine_id, "L2");
        assert_eq!(ctxs[0].severity, "critical");
        assert_eq!(ctxs[1].loop_routine_id, "L1");
        assert_eq!(ctxs[1].severity, "high");
        // Finding fields all lift from ctxs[0].
        assert_eq!(f.severity, "critical", "severity from the winner");
        assert_eq!(
            f.confidence, ctxs[0].confidence,
            "confidence from the SAME (winning) context"
        );
        assert_eq!(
            f.evidence_path, ctxs[0].witness,
            "evidence_path is the winner witness"
        );
        assert_eq!(
            f.evidence_path.len(),
            4,
            "the winner is the L2->M->H route (loop, call, hop, terminal)"
        );
        assert!(
            f.root_cause.contains("A loop in L2"),
            "wording names the winner's loop routine: {}",
            f.root_cause
        );
    }

    /// Run the PRODUCTION cohort pipeline over a fixture and return the assembled
    /// findings — `search_loops_cohorts` (which interns each cohort's uncertainty
    /// union into the run's [`UncertaintyTable`]) + `assemble_cohort_findings`
    /// (which resolves the winner's ids back through it). Distinct from
    /// [`assemble`] above, which drives the `#[cfg(test)]` `search_loops` oracle.
    fn assemble_cohorts(
        routines: &[L3Routine],
        edges: HashMap<String, Vec<CombinedEdge>>,
        summaries: HashMap<String, FullRoutineSummary>,
    ) -> Vec<Finding> {
        let ctx = minimal_ctx_with_uncertainties(routines, edges, summaries, Vec::new());
        cohort_findings(&ctx, routines)
    }

    /// [`minimal_ctx`] plus an injected `uncertainties_by_node` map (the substrate
    /// `path_uncertainty_ids` unions along a cohort's representative path).
    fn minimal_ctx_with_uncertainties<'a>(
        routines: &'a [L3Routine],
        edges: HashMap<String, Vec<CombinedEdge>>,
        summaries: HashMap<String, FullRoutineSummary>,
        uncertainties: Vec<(String, Vec<Uncertainty>)>,
    ) -> DetectorContext<'a> {
        let mut ctx = minimal_ctx(routines, edges, summaries);
        for (node, set) in uncertainties {
            ctx.set_uncertainties_for_test(&node, set);
        }
        ctx
    }

    /// The production cohort path over an already-built context.
    fn cohort_findings(ctx: &DetectorContext, routines: &[L3Routine]) -> Vec<Finding> {
        let workspace = ws(routines);
        let mut memo = HashMap::new();
        let (graph, seeds) = build_d1_graph(ctx, &workspace, &mut memo);
        let (direct_ops, _stats) = enumerate_direct_ops(&workspace, ctx);
        let run = search_loops_cohorts(
            &graph,
            &seeds,
            &direct_ops,
            ctx,
            &ctx.closed_world_temp_params,
        );
        assemble_cohort_findings(&run, ctx, &role_map(routines)).0
    }

    /// R's loop calls A; A reaches the terminal T BOTH directly (`A/csT`, no loop)
    /// and via a deeper route `A/csX` (inside A's own loop) → X → Y → T. The deep
    /// route wins on severity, so the winning cohort's representative path is
    /// `A → X → Y → T` — which is where the uncertainties are injected. Shape
    /// lifted from `d1_dataflow`'s `agrees_on_uncertain_winner`, the fixture that
    /// established this route wins and carries `unc == true`.
    fn uncertain_winner_fixture() -> Fixture {
        let mut r = routine("R", "procedure");
        r.loops = vec![loop_def("R/loop0")];
        r.call_sites = vec![call_site("R/cs0", "A", vec!["R/loop0".to_string()])];
        let mut a = routine("A", "procedure");
        a.call_sites = vec![
            call_site("A/csT", "T", vec![]),
            call_site("A/csX", "X", vec!["A/loop0".to_string()]),
        ];
        let x = routine("X", "procedure");
        let y = routine("Y", "procedure");
        let mut t = routine("T", "procedure");
        t.record_operations = vec![record_op(
            "T/op0",
            "FindSet",
            "Rec",
            Some("t/T"),
            vec![],
            false,
        )];
        let routines = vec![r, a, x, y, t];

        let mut edges: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        edges.insert(
            "R".to_string(),
            vec![edge_kind("R", "A", "R/cs0", "direct")],
        );
        edges.insert(
            "A".to_string(),
            vec![
                edge_kind("A", "T", "A/csT", "direct"),
                edge_kind("A", "X", "A/csX", "direct"),
            ],
        );
        edges.insert(
            "X".to_string(),
            vec![edge_kind("X", "Y", "X/csY", "direct")],
        );
        edges.insert(
            "Y".to_string(),
            vec![edge_kind("Y", "T", "Y/csT", "direct")],
        );

        let summaries: HashMap<String, FullRoutineSummary> = ["A", "X", "Y", "T"]
            .iter()
            .map(|id| (id.to_string(), db_read_summary(id, &format!("t/{id}"))))
            .collect();
        (routines, edges, summaries)
    }

    fn db_read_summary(id: &str, table: &str) -> FullRoutineSummary {
        summary(
            id,
            vec![fact("read", "table", Some(table))],
            vec![],
            Some(coverage("complete")),
        )
    }

    fn unc(kind: &str, callsite: Option<&str>, routine: Option<&str>) -> Uncertainty {
        Uncertainty {
            kind: kind.to_string(),
            callsite_id: callsite.map(|s| s.to_string()),
            operation_id: None,
            routine_id: routine.map(|s| s.to_string()),
            interface_name: None,
        }
    }

    /// **Pins the interned-uncertainty USE, end to end.** The cohort's uncertainty
    /// union is stored in `CohortRep` as `UncertaintyId`s into the run-level
    /// `UncertaintyTable`; `assemble_cohort_findings` resolves them back at
    /// `to_confidence`. This asserts the resolved `Finding.confidence` EXACTLY —
    /// level, `cappedBy`, and every evidence note in order — so a wrong id, a
    /// wrong table, a lost dedupe or a lost sort all fail here.
    ///
    /// The fixture is built so each of those is separately load-bearing:
    /// - **Order** is not discovery order. The path is `A → X → Y → T`, so X's
    ///   `external-target` is *discovered* before Y's `dynamic-dispatch`, but the
    ///   emitted order must be key-sorted (`dynamic-dispatch|Y` <
    ///   `external-target|X/csY` < `unresolved-call|A/csX`).
    /// - **Dedup** is exercised: the same `unresolved-call at A/csX` sits on BOTH
    ///   X and Y and must appear exactly once.
    /// - **Resolution** is exercised: three DISTINCT table entries, so an
    ///   off-by-one or a table mix-up changes a note.
    ///
    /// A golden now ALSO covers this — `tests/r4-goldens/ws-d1-uncertain-path.r4.golden.json`
    /// (added at `fe8a018`, "test(d1): cover confidence.evidence with a golden") carries
    /// one `d1-db-op-in-loop` finding with six non-empty `confidence.evidence` entries;
    /// see `tests/r4/r4_differential.rs:693-704`, which documents it as the only golden
    /// anywhere in this repository that covers `Finding.confidence.evidence` (every other
    /// d1 golden's winner path is certain, so `to_confidence` takes its `is_empty()` fast
    /// path there). This unit test stays as a second, independent guard: it asserts
    /// against a hand-built fixture (`uncertain_winner_fixture`) rather than the golden's
    /// fixed shape, so a change that happens to preserve the golden's exact bytes cannot
    /// silently break ORDER, DEDUP or RESOLUTION in general.
    #[test]
    fn cohort_confidence_resolves_interned_uncertainties_in_key_order() {
        let (routines, edges, summaries) = uncertain_winner_fixture();
        let uncertainties: Vec<(String, Vec<Uncertainty>)> = [
            (
                "X".to_string(),
                vec![
                    unc("external-target", Some("X/csY"), None),
                    unc("unresolved-call", Some("A/csX"), None),
                ],
            ),
            (
                "Y".to_string(),
                vec![
                    unc("dynamic-dispatch", None, Some("Y")),
                    unc("unresolved-call", Some("A/csX"), None),
                ],
            ),
        ]
        .into_iter()
        .collect();

        let ctx = minimal_ctx_with_uncertainties(&routines, edges, summaries, uncertainties);
        let findings = cohort_findings(&ctx, &routines);

        assert_eq!(findings.len(), 1, "one terminal -> one finding");
        let c = &findings[0].confidence;

        assert_eq!(c.level, "possible", "any uncertainty caps the level");
        assert_eq!(
            c.capped_by,
            Some(vec![
                "dynamic-dispatch".to_string(),
                "opaque-callee".to_string(), // alias of external-target
                "unresolved-call".to_string(),
            ]),
        );
        let notes: Vec<&str> = c
            .evidence
            .iter()
            .filter_map(|e| e.note.as_deref())
            .collect();
        assert_eq!(
            notes,
            vec![
                "dynamic-dispatch at Y",
                "external-target at X/csY",
                "unresolved-call at A/csX",
            ],
            "the winner cohort's interned ids must resolve to these three \
             uncertainties, de-duped, in uncertainty_key order"
        );
    }

    /// [`uncertain_winner_fixture`] plus a SECOND terminal off the same uncertain
    /// node `Y`, so two independent findings' confidence evidence is built from
    /// the same interned uncertainty.
    fn shared_uncertainty_two_terminals_fixture() -> Fixture {
        let (mut routines, mut edges, mut summaries) = uncertain_winner_fixture();
        let y = routines
            .iter_mut()
            .find(|r| r.id == "Y")
            .expect("Y is in the base fixture");
        y.call_sites = vec![call_site("Y/csU", "U", vec![])];
        let mut u = routine("U", "procedure");
        u.record_operations = vec![record_op(
            "U/op0",
            "FindSet",
            "Rec",
            Some("t/U"),
            vec![],
            false,
        )];
        routines.push(u);
        edges
            .get_mut("Y")
            .expect("Y has edges in the base fixture")
            .push(edge_kind("Y", "U", "Y/csU", "direct"));
        summaries.insert("U".to_string(), db_read_summary("U", "t/U"));
        (routines, edges, summaries)
    }

    /// **Pins the SHARED-witness-step representation at the use site.** A run's
    /// retained cohort witnesses repeat their steps 4.29x on Base App 8020
    /// (172,915 steps, 40,325 distinct), and `assemble_cohort_findings`
    /// hash-conses them through a `StepInterner` so each distinct step is ONE
    /// allocation. Equality alone cannot see that: a version that cloned per
    /// cohort produces byte-identical output and costs 71 MiB more.
    ///
    /// Two independent shares are asserted, both through the real
    /// `search_loops_cohorts` + `assemble_cohort_findings`:
    ///
    /// - ACROSS findings — `shared_uncertainty_two_terminals_fixture` reaches two
    ///   terminals (two findings) from the SAME loop, so both findings' witnesses
    ///   open with the same loop step. This is the strongest form: the sharing
    ///   survives per-finding assembly rather than being an artifact of one call.
    /// - WITHIN a finding — `severity_race_fixture` reaches ONE terminal from two
    ///   loops of different severity, so the finding's two cohorts carry the same
    ///   terminal step.
    ///
    /// Each `assert_eq!` on the step VALUE runs before its `ptr_eq`, so a real
    /// divergence fails legibly instead of as an opaque pointer mismatch.
    /// Replacing `steps.intern_witness(&rep.witness)` with `rep.witness.clone()`
    /// turns this red and nothing else.
    #[test]
    fn cohort_witness_steps_are_shared() {
        // --- across two findings, one shared loop ---
        let (routines, edges, summaries) = shared_uncertainty_two_terminals_fixture();
        let findings = assemble_cohorts(&routines, edges, summaries);
        assert_eq!(findings.len(), 2, "two terminals -> two findings");
        let loop_steps: Vec<&std::sync::Arc<EvidenceStep>> = findings
            .iter()
            .map(|f| {
                &f.cohort_contexts
                    .as_ref()
                    .expect("d1 emits cohort_contexts")[0]
                    .witness
                    .first_steps[0]
            })
            .collect();
        assert_eq!(
            loop_steps[0], loop_steps[1],
            "both findings are reached from the same loop, so the loop step is equal"
        );
        assert!(
            std::sync::Arc::ptr_eq(loop_steps[0], loop_steps[1]),
            "…and must be ONE hash-consed allocation, not two equal copies"
        );

        // --- within one finding, two cohorts of one terminal ---
        let (routines, edges, summaries) = severity_race_fixture();
        let findings = assemble_cohorts(&routines, edges, summaries);
        assert_eq!(findings.len(), 1);
        let cohorts = findings[0]
            .cohort_contexts
            .as_ref()
            .expect("d1 emits cohort_contexts");
        assert!(cohorts.len() >= 2, "the fixture must produce >1 cohort");
        assert_eq!(
            cohorts[0].witness.terminal_step, cohorts[1].witness.terminal_step,
            "every cohort of one terminal ends on the same terminal step"
        );
        assert!(
            std::sync::Arc::ptr_eq(
                &cohorts[0].witness.terminal_step,
                &cohorts[1].witness.terminal_step
            ),
            "…and must be ONE hash-consed allocation"
        );
    }

    /// **Pins the SHARED identity/advice text at the use site.** Across a run,
    /// `d1` emits 563,126 `affected_objects` entries over 2,042 distinct values,
    /// 22,383 titles of ONE distinct value, and 22,383 fix options drawn from
    /// TWO — so these are interned (`object_ids`) or hoisted out of the emit loop
    /// (`title`, `setup_fix`/`general_fix`) rather than rebuilt per finding.
    ///
    /// Like the witness-step and note claims, equality cannot see this: a version
    /// that allocated per finding produces byte-identical output. So this asserts
    /// pointer identity across TWO findings from the real pipeline, with the value
    /// asserted first so a genuine divergence fails legibly. Replacing
    /// `intern_id(&mut object_ids, o)` with `Arc::from(o)`, or `Arc::clone(&title)`
    /// with a fresh `Arc::from(...)`, turns the corresponding half red.
    #[test]
    fn cohort_ids_and_advice_text_are_shared_across_findings() {
        let (routines, edges, summaries) = shared_uncertainty_two_terminals_fixture();
        let findings = assemble_cohorts(&routines, edges, summaries);
        assert_eq!(findings.len(), 2, "two terminals -> two findings");
        let (a, b) = (&findings[0], &findings[1]);

        // affected_objects: both terminals live in the fixture's single object.
        assert_eq!(a.affected_objects.len(), 1);
        assert_eq!(&*a.affected_objects[0], "app/Codeunit/1");
        assert_eq!(a.affected_objects, b.affected_objects);
        assert!(
            Arc::ptr_eq(&a.affected_objects[0], &b.affected_objects[0]),
            "an object id repeated across findings must be ONE run-level allocation"
        );

        // title: one distinct value across every d1 finding in a run.
        assert_eq!(&*a.title, "Database operation inside a loop");
        assert_eq!(a.title, b.title);
        assert!(
            Arc::ptr_eq(&a.title, &b.title),
            "the title must be hoisted out of the emit loop, not rebuilt per finding"
        );

        // fix_options: drawn from a two-entry menu, so the text is shared too.
        assert_eq!(a.fix_options.len(), 1);
        assert_eq!(a.fix_options[0], b.fix_options[0]);
        assert!(
            Arc::ptr_eq(&a.fix_options[0].description, &b.fix_options[0].description),
            "fix-option description must be hoisted, not rebuilt per finding"
        );
        assert!(Arc::ptr_eq(
            &a.fix_options[0].safety,
            &b.fix_options[0].safety
        ));
    }

    /// **Pins the SHARED-note representation at the use site.** Every
    /// `Finding.confidence.evidence` record is a `ConfidenceEvidence` whose
    /// `note` is an `Arc<str>` cloned from the run-level `UncertaintyTable` — one
    /// allocation per DISTINCT uncertainty rather than one per record. That is the entire
    /// memory claim of this change (7,418,849 records, 3,073 distinct notes on
    /// Base App 8020), and equality alone cannot see it: a version that rebuilt
    /// the string per record would still produce equal notes and identical
    /// output, just a gigabyte heavier.
    ///
    /// So this asserts pointer identity, through the real
    /// `search_loops_cohorts` + `assemble_cohort_findings`, across TWO separate
    /// findings — the strongest available statement that the sharing survives
    /// finding assembly rather than being an artifact of one `to_confidence`
    /// call. The `assert_eq!` on the note text first keeps a failure legible:
    /// if the text diverges, that fires before the pointer check.
    #[test]
    fn cohort_evidence_notes_are_shared_across_findings() {
        let (routines, edges, summaries) = shared_uncertainty_two_terminals_fixture();
        let uncertainties: Vec<(String, Vec<Uncertainty>)> = vec![(
            "Y".to_string(),
            vec![unc("dynamic-dispatch", None, Some("Y"))],
        )];

        let ctx = minimal_ctx_with_uncertainties(&routines, edges, summaries, uncertainties);
        let findings = cohort_findings(&ctx, &routines);

        assert_eq!(findings.len(), 2, "two terminals -> two findings");
        let notes: Vec<&std::sync::Arc<str>> = findings
            .iter()
            .map(|f| {
                assert_eq!(
                    f.confidence.evidence.len(),
                    1,
                    "each winner path crosses Y exactly once"
                );
                f.confidence.evidence[0]
                    .note
                    .as_ref()
                    .expect("the uncertainty produces a note")
            })
            .collect();
        assert_eq!(&**notes[0], "dynamic-dispatch at Y");
        assert_eq!(&**notes[1], "dynamic-dispatch at Y");
        assert!(
            std::sync::Arc::ptr_eq(notes[0], notes[1]),
            "both findings' evidence notes must be clones of ONE table allocation, \
             not two independently formatted Strings"
        );
    }

    /// A cohort-bearing finding no longer STORES `evidence_path`/
    /// `additional_paths`, and `finding::evidence_path_of` must reconstruct
    /// EXACTLY what `assemble_cohort_findings` used to store there — the WINNER
    /// cohort's flattened representative witness.
    ///
    /// `severity_race_fixture` is chosen because it makes the two candidate
    /// answers visibly different: ONE terminal (`H/op0`) reached by two loops of
    /// DIFFERENT severity, `L2` (depth-2 → critical) and `L1` (high). So the
    /// winner cohort's path starts in `L2` and the loser's in `L1`. A
    /// `evidence_path_of` that read `cohort_contexts[1]`, or that fell back to
    /// the (now empty) field, fails on the `L2` assertion rather than passing
    /// vacuously — which the `.first()` → `.last()` perturbation confirms.
    ///
    /// The 4-step shape (`loop`, `call`, `hop`, `terminal`) is the same one the
    /// pre-change `contexts` oracle pins for this fixture in
    /// `winner_context_drives_finding_fields`, so the two paths agree on the
    /// answer as well as on the mechanism.
    #[test]
    fn cohort_evidence_path_is_the_winner_witness() {
        use crate::engine::l5::d1_witness::flatten_witness;
        use crate::engine::l5::finding::evidence_path_of;

        let (routines, edges, summaries) = severity_race_fixture();
        let findings = assemble_cohorts(&routines, edges, summaries);
        assert_eq!(findings.len(), 1, "one terminal -> one cohort finding");
        let f = &findings[0];

        assert!(
            f.evidence_path.is_empty(),
            "a cohort-bearing finding must NOT build evidence_path — it is 59.9 MiB \
             of retained duplicate the projection already discards"
        );
        assert!(
            f.additional_paths.is_none(),
            "…and must not build additional_paths either (36.0 MiB more)"
        );

        let cohorts = f
            .cohort_contexts
            .as_ref()
            .expect("d1 always emits cohort_contexts");
        assert!(
            cohorts.len() >= 2,
            "the fixture reaches one terminal from two loops of different severity"
        );
        assert_eq!(cohorts[0].severity, "critical", "winner class first");
        assert_eq!(
            f.severity, "critical",
            "the finding lifts the winner severity"
        );

        let path = evidence_path_of(f);
        assert_eq!(
            &*path,
            flatten_witness(&cohorts[0].witness).as_slice(),
            "the derived path IS the winner cohort's flattened witness"
        );

        // The discriminating half: the winner is L2's route, not L1's.
        assert_eq!(path[0].routine_id, "L2", "path starts in the WINNER's loop");
        assert_eq!(path[0].loop_id.as_deref(), Some("L2/loop0"));
        assert_eq!(path.len(), 4, "loop, call, hop, terminal");
        let last = path.last().expect("non-empty");
        assert_eq!(last.routine_id, "H");
        assert_eq!(last.operation_id.as_deref(), Some("H/op0"));

        // `additional_paths` is gone too, and `pathCount` — which reaches the
        // default `analyze` JSON — must still be `1 + (non-winner cohorts)`.
        // This fixture has MORE than one cohort, so a `realizing_path_count`
        // that fell back to the (now `None`) field would answer 1 here.
        assert_eq!(
            crate::engine::l5::finding::realizing_path_count(f),
            cohorts.len(),
            "pathCount must still count every realizing path, not just the winner"
        );
        assert!(
            crate::engine::l5::finding::realizing_path_count(f) > 1,
            "the fixture must actually exercise the >1 case or the assertion above \
             passes vacuously"
        );
    }

    /// The same pipeline with NO uncertainties anywhere must leave the finding at
    /// the base level with empty evidence — the negative half of the assertion
    /// above, so a change that always emits (or never emits) evidence cannot pass
    /// both.
    #[test]
    fn cohort_confidence_is_base_level_without_uncertainties() {
        let (routines, edges, summaries) = uncertain_winner_fixture();
        let findings = assemble_cohorts(&routines, edges, summaries);
        assert_eq!(findings.len(), 1);
        let c = &findings[0].confidence;
        assert_eq!(c.level, "likely");
        assert!(c.capped_by.is_none());
        assert!(c.evidence.is_empty());
    }

    /// Bullet: terminal-based `id = root_cause_key = d1/{terminal}/{op}` AND the
    /// fingerprint INPUTS (detector, terminal primary location, affected tables,
    /// root_cause_key) are unchanged vs the OLD pipeline, so a new finding's
    /// fingerprint equals the OLD premerge finding's fingerprint for the same key.
    #[test]
    fn terminal_based_id_and_stable_fingerprint_inputs() {
        let (routines, edges, summaries) = two_loops_one_terminal();
        let ctx = minimal_ctx(&routines, edges, summaries);
        let workspace = ws(&routines);
        let mut memo = HashMap::new();
        let (graph, seeds) = build_d1_graph(&ctx, &workspace, &mut memo);
        let (direct_ops, _stats) = enumerate_direct_ops(&workspace, &ctx);
        let aggs = search_loops(
            &graph,
            &seeds,
            &direct_ops,
            &ctx,
            &ctx.closed_world_temp_params,
        );
        let mut new_findings = assemble_findings(&aggs, &ctx, &role_map(&routines));

        // OLD pipeline (shadow oracle) over the SAME content, separate owned clone.
        let resolved = L3Resolved {
            workspace: ws(&routines),
            root_classifications: vec![],
            primary_app: None,
            infra_diagnostics: vec![],
            precomputed_calls: None,
            precomputed_events: None,
        };
        let premerge = detect_d1_premerge(&resolved, &ctx);
        // rootCauseKey -> fingerprint (all premerge findings sharing a key hash equal).
        let mut old_fp: HashMap<String, String> = HashMap::new();
        for rec in &premerge {
            let fp = ctx.fingerprint_index.fingerprint_of(&rec.finding);
            old_fp.insert(rec.finding.root_cause_key.clone(), fp);
        }
        assert!(!old_fp.is_empty(), "old premerge must produce a finding");

        for (f, _ctxs) in &mut new_findings {
            assert_eq!(f.id, f.root_cause_key, "id == root_cause_key");
            assert_eq!(f.root_cause_key, "d1/H/H/op0", "terminal-based key");
            let new_fp = ctx.fingerprint_index.fingerprint_of(f);
            assert_eq!(
                Some(&new_fp),
                old_fp.get(&f.root_cause_key),
                "fingerprint inputs unchanged vs the old pipeline for {}",
                f.root_cause_key
            );
        }
    }

    /// Bullet (G-7): the dead-routine down-confidence roots = EVERY context
    /// witness's first-step routine. Two dead LOCAL loop routines reach one shared
    /// terminal -> ONE finding with TWO contexts; both loop roots are d14-dead, so
    /// the finding down-confidences (note appended), proving G-7 spans all
    /// contexts, not just the winner. Real AL source (d14 needs the reachability
    /// substrate).
    #[test]
    fn dead_routine_downconfidence_spans_all_contexts() {
        const SRC: &str = r#"
table 50790 "T5 G7 Rec"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50790 "T5 D1 G7"
{
    local procedure DoInsert()
    var R: Record "T5 G7 Rec";
    begin
        R.Insert();
    end;

    local procedure DeadLoopA()
    var i: Integer;
    begin
        for i := 1 to 10 do
            DoInsert();
    end;

    local procedure DeadLoopB()
    var i: Integer;
    begin
        for i := 1 to 5 do
            DoInsert();
    end;
}
"#;
        let files = vec![("src/T5D1G7.al".to_string(), SRC.to_string())];
        let resolved = crate::engine::l3::l3_workspace::assemble_and_resolve_default(
            &files,
            "11111111-0000-0000-0000-0000000g7d1a",
        );
        let d1: Vec<_> = crate::engine::l5::detectors::registered_detectors()
            .into_iter()
            .filter(|d| d.name == "d1-db-op-in-loop")
            .collect();
        assert_eq!(d1.len(), 1, "d1 registered once");
        let out = crate::engine::l5::registry::run_detectors(&resolved, &d1);
        let findings: Vec<&Finding> = out
            .findings
            .iter()
            .filter(|f| f.detector == "d1-db-op-in-loop")
            .collect();

        assert_eq!(
            findings.len(),
            1,
            "both dead loops reach the SAME terminal -> one finding. {:#?}",
            findings
        );
        let f = findings[0];
        // C6 cohort schema: the two reaching loops live in `cohort_contexts` (one
        // verdict class, loop_count 2) — decompress its `loop_set` via the run
        // catalog to recover the two DISTINCT dead loop roots (the population G-7
        // spans).
        //
        // The `assert!(f.contexts.is_none(), "cutover: per-loop contexts retired")`
        // that stood here is DELETED, not moved: `Finding` no longer has a
        // `contexts` field, so the state it guarded against is unrepresentable.
        // A guard whose violation cannot be constructed is not coverage.
        let idx = out
            .d1_cohort_index
            .as_ref()
            .expect("d1 cohort index present");
        let ccs = f.cohort_contexts.as_ref().expect("cohort_contexts present");
        let total_loops: u64 = ccs.iter().map(|c| c.loop_count).sum();
        assert_eq!(total_loops, 2, "two reaching loops");
        let mut roots: Vec<&str> = Vec::new();
        for cc in ccs {
            for g in idx.registry.iter(cc.loop_set) {
                roots.push(idx.catalog[g as usize].loop_routine_id.as_str());
            }
        }
        roots.sort();
        roots.dedup();
        assert_eq!(roots.len(), 2, "two DISTINCT dead loop roots");
        assert!(
            f.root_cause
                .contains("appears unreachable from any entry point"),
            "G-7 down-confidence note applied across ALL contexts: {}",
            f.root_cause
        );
    }
}
