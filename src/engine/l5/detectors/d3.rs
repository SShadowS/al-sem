//! D3 — missing / incomplete `SetLoadFields` before a record retrieval. Port of
//! al-sem `src/detectors/d3-missing-setloadfields.ts` + the d3-LOCAL
//! `deriveLoadStates` helper (`src/detectors/d3-load-state.ts`).
//!
//! Detects retrievals (`FindSet` / `FindFirst` / `FindLast` / `Get`) whose loaded
//! field set does not cover the fields later accessed — same-routine, and through
//! directly-resolved callees via `RecordRoleSummary`. Emits only on a complete
//! witness (a concrete retrieval + a concrete access); bails conservatively, never
//! claiming a false "clean".
//!
//! `deriveLoadStates` walks the routine's record ops in SOURCE order tracking the
//! per-record-variable load-field state: `SetLoadFields` sets a partial load set,
//! `AddLoadFields` unions, `Reset`/`Copy`/`TransferFields` invalidate. It yields one
//! `LoadStateAtRetrieval` per retrieval op with the load state of its record var at
//! that point.

use std::collections::{HashMap, HashSet};

use al_syntax::IdentifierFoldExt;

use crate::engine::l2::features::{PAnchor, PCallSite, PCallee};
use crate::engine::l3::l3_workspace::{L3RecordOperation, L3Resolved, L3Routine};
use crate::engine::l5::confidence::{UncertaintyLite, to_confidence};
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::{
    anchor_of, is_platform_loaded_trigger_rec, normalize_load_field_arg, primary_key_field_names_lc,
};
use crate::engine::l5::finding::{Evidence, EvidenceStep, Finding, FixOption};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};

const DETECTOR: &str = "d3-missing-setloadfields";

/// Retrieval ops `deriveLoadStates` yields a state for.
const RETRIEVAL_OPS: &[&str] = &["FindSet", "FindFirst", "FindLast", "Get"];

/// Ops that close the access window (and bail in deriveLoadStates).
#[allow(dead_code)] // future design: G-15(b) access-window detector
const INVALIDATING_OPS: &[&str] = &["Reset", "Copy", "TransferFields"];

/// G-15(b): ops that close the post-retrieval ACCESS window. Besides the
/// invalidating ops, `Init` re-initialises the buffer — accesses after it
/// operate on the constructed row, not the loaded one, so the prior load is
/// irrelevant to them. (`Init` does NOT clear the SetLoadFields selection,
/// so `deriveLoadStates` keeps using `INVALIDATING_OPS` unchanged.)
const WINDOW_CLOSING_OPS: &[&str] = &["Reset", "Copy", "TransferFields", "Init"];

/// G-15(b): `Clear(<var>)` — a bare platform call that resets the record
/// variable entirely. Same buffer-reset semantics as `Init`: it closes the
/// post-retrieval access window for that variable.
fn is_clear_call_on(cs: &PCallSite, var_key: &str) -> bool {
    matches!(&cs.callee, PCallee::Bare { name } if name.eq_fold_identifier("Clear"))
        && cs.argument_texts.len() == 1
        && cs.argument_texts[0].trim().to_lowercase() == var_key
}

/// `before(a, b)` — strictly before in source order (line then column). Mirrors
/// al-sem d3's local `before` (== `before_anchor`).
fn before(a: &PAnchor, b: &PAnchor) -> bool {
    if a.start_line != b.start_line {
        return a.start_line < b.start_line;
    }
    a.start_column < b.start_column
}

/// The load-field state of a record variable at a point in the op stream.
#[derive(Debug, Clone)]
enum LoadState {
    /// No SetLoadFields seen — the full record is loaded.
    None,
    /// A partial load set is active.
    Loaded(HashSet<String>),
    /// Reset / Copy / TransferFields cleared the analysable state.
    Invalidated,
}

/// A retrieval op paired with the load state of its record variable at that site.
struct LoadStateAtRetrieval<'a> {
    retrieval_op: &'a L3RecordOperation,
    record_variable_name: String,
    load_state: LoadState,
}

/// Source order: line then column. Mirrors d3-load-state.ts `inSourceOrder`.
fn in_source_order(a: &L3RecordOperation, b: &L3RecordOperation) -> std::cmp::Ordering {
    let ra = &a.source_anchor;
    let rb = &b.source_anchor;
    ra.start_line
        .cmp(&rb.start_line)
        .then_with(|| ra.start_column.cmp(&rb.start_column))
}

/// Reconstruct per-record-variable load-field state by walking the routine's record
/// operations in source order. Port of `deriveLoadStates`.
fn derive_load_states(routine: &L3Routine) -> Vec<LoadStateAtRetrieval<'_>> {
    let mut ops: Vec<&L3RecordOperation> = routine.record_operations.iter().collect();
    ops.sort_by(|a, b| in_source_order(a, b));

    let mut state_by_var: HashMap<String, LoadState> = HashMap::new();
    let mut out: Vec<LoadStateAtRetrieval> = Vec::new();

    for op in ops {
        let var_key = op.record_variable_name.to_lowercase();
        // current = state_by_var.get(varKey) ?? { kind: "none" }
        let current = state_by_var
            .get(&var_key)
            .cloned()
            .unwrap_or(LoadState::None);

        if op.op == "SetLoadFields" {
            let fields: HashSet<String> = op
                .field_arguments
                .as_ref()
                .map(|fa| fa.iter().map(|f| normalize_load_field_arg(f)).collect())
                .unwrap_or_default();
            state_by_var.insert(var_key, LoadState::Loaded(fields));
            continue;
        }
        if op.op == "AddLoadFields" {
            let mut next: HashSet<String> = match &current {
                LoadState::Loaded(fields) => fields.clone(),
                _ => HashSet::new(),
            };
            if let Some(fa) = &op.field_arguments {
                for f in fa {
                    next.insert(normalize_load_field_arg(f));
                }
            }
            state_by_var.insert(var_key, LoadState::Loaded(next));
            continue;
        }
        if op.op == "Reset" || op.op == "Copy" || op.op == "TransferFields" {
            state_by_var.insert(var_key, LoadState::Invalidated);
            continue;
        }
        if RETRIEVAL_OPS.contains(&op.op.as_str()) {
            let load_state = match &current {
                LoadState::Loaded(fields) => LoadState::Loaded(fields.clone()),
                LoadState::None => LoadState::None,
                LoadState::Invalidated => LoadState::Invalidated,
            };
            out.push(LoadStateAtRetrieval {
                retrieval_op: op,
                record_variable_name: op.record_variable_name.clone(),
                load_state,
            });
        }
    }

    out
}

pub fn detect_d3(
    resolved: &L3Resolved,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_parse_incomplete = 0u64;
    let mut skipped_temporary_record = 0u64;
    let mut skipped_unknown_reads = 0u64;

    for routine in &ws.routines {
        // roleOf(routine) !== "primary" → skip. Source-only ⇒ all primary.
        if !routine.body_available {
            continue;
        }
        if routine.parse_incomplete {
            skipped_parse_incomplete += 1;
            continue;
        }
        candidates_considered += 1;

        // G-15(a): assignment WRITE targets. `Rec.Field := ...` records BOTH a
        // PVarAssignment (anchored at the statement start, which IS the LHS
        // member expression's start) and a PFieldAccess at that same anchor. A
        // field access whose (position, member name) matches an assignment LHS
        // is the WRITE target — a write needs no SetLoadFields. The lhs_name is
        // stored lowercased but may keep quotes; normalize both sides. RHS
        // reads sit at different positions and are never excluded.
        let write_targets: HashSet<(u32, u32, String)> = routine
            .var_assignments
            .iter()
            .map(|va| {
                (
                    va.source_anchor.start_line,
                    va.source_anchor.start_column,
                    normalize_load_field_arg(&va.lhs_name),
                )
            })
            .collect();

        for state in derive_load_states(routine) {
            // Bailout — cannot prove.
            if matches!(state.load_state, LoadState::Invalidated) {
                continue;
            }

            let var_key = state.record_variable_name.to_lowercase();
            // d22-FN companion: the IMPLICIT `Rec` of a page/table trigger is
            // platform-loaded — the platform fetches the row before the trigger
            // runs, so SetLoadFields advice on it is moot. Skip (mirrors d11/d37).
            if is_platform_loaded_trigger_rec(routine, &state.record_variable_name) {
                continue;
            }
            let rec_var = routine
                .record_variables
                .iter()
                .find(|rv| rv.name.to_lowercase() == var_key);

            // Temp records live in memory; SetLoadFields has no SQL benefit.
            // recVar?.tempState.kind === "known" && recVar.tempState.value === true
            // G-19: a ParameterDependent param record (keyword-less by-var)
            // CLOSED-WORLD PROVEN temp (`local` routine, all resolved callers
            // pass Known(true) temp) is treated exactly like Known(true).
            if let Some(rv) = rec_var
                && (crate::engine::l2::features::known_temp_suppresses(rv.temp_state_known_value())
                    || crate::engine::l5::closed_world_temp::pd_state_proven_temp(
                        Some(&rv.temp_state),
                        &routine.id,
                        &ctx.closed_world_temp_params,
                    ))
            {
                skipped_temporary_record += 1;
                continue;
            }
            let table_id = match rec_var.and_then(|rv| rv.table_id.clone()) {
                Some(t) => t,
                None => continue, // unresolved table — bailout
            };
            let table = match ctx.table_by_id.get(table_id.as_str()) {
                Some(t) => *t,
                None => continue,
            };
            // fieldNameById: id → lowercased name.
            let field_name_by_id: HashMap<&str, String> = table
                .fields
                .iter()
                .map(|f| (f.id.as_str(), f.name.to_lowercase()))
                .collect();

            // G-12 refinement 1: the table's PRIMARY KEY (first key) fields are
            // always loaded regardless of SetLoadFields — accessing them after a
            // retrieval wastes nothing. Lowercased names; an empty/missing key
            // set excludes nothing (keep firing). Shared with d42 (G-15(c)).
            let pk_fields: HashSet<String> = primary_key_field_names_lc(table);
            // G-12 refinement 2: FlowFields need CalcFields, not SetLoadFields —
            // an uncovered FlowField read is d22's domain, not d3's. EXACT
            // structural signal (`field_class == "FlowField"`); anything else
            // (Normal / FlowFilter / unknown name) stays in the accessed set.
            let flowfield_names: HashSet<String> = table
                .fields
                .iter()
                .filter(|f| f.field_class == "FlowField")
                .map(|f| f.name.to_lowercase())
                .collect();
            // A field whose load is never d3's concern. Unresolved field names
            // are NOT excluded (suppression-direction: when unsure, keep firing).
            let excluded_field =
                |name_lc: &String| pk_fields.contains(name_lc) || flowfield_names.contains(name_lc);

            let retrieval_anchor = &state.retrieval_op.source_anchor;

            // The window closes at the first window-closing op on this record
            // var after the retrieval (Reset/Copy/TransferFields, plus G-15(b)
            // `Init`) — or at a `Clear(<var>)` bare call, whichever comes first.
            let mut window_end: Option<&PAnchor> = None;
            for op in &routine.record_operations {
                if op.record_variable_name.to_lowercase() == var_key
                    && WINDOW_CLOSING_OPS.contains(&op.op.as_str())
                    && before(retrieval_anchor, &op.source_anchor)
                    && window_end.is_none_or(|we| before(&op.source_anchor, we))
                {
                    window_end = Some(&op.source_anchor);
                }
            }
            for cs in &routine.call_sites {
                if is_clear_call_on(cs, &var_key)
                    && before(retrieval_anchor, &cs.source_anchor)
                    && window_end.is_none_or(|we| before(&cs.source_anchor, we))
                {
                    window_end = Some(&cs.source_anchor);
                }
            }
            let in_window = |anchor: &PAnchor| -> bool {
                before(retrieval_anchor, anchor) && window_end.is_none_or(|we| before(anchor, we))
            };

            let mut accessed_fields: HashSet<String> = HashSet::new();
            let mut access_steps: Vec<EvidenceStep> = Vec::new();
            let mut uncertainties: Vec<UncertaintyLite> = Vec::new();
            let mut bailout = false;

            // --- same-routine field accesses in the window ---
            for fa in &routine.field_accesses {
                if fa.record_variable_name.to_lowercase() != var_key {
                    continue;
                }
                if !in_window(&fa.source_anchor) {
                    continue;
                }
                let name_lc = fa.field_name.to_lowercase();
                // G-15(a): an access that is the TARGET of an assignment is a
                // WRITE — it never needs the field loaded. Exact structural
                // match: same start position AND same member name as a recorded
                // assignment LHS. RHS reads (different position) keep counting.
                let is_write_target = write_targets.contains(&(
                    fa.source_anchor.start_line,
                    fa.source_anchor.start_column,
                    name_lc.clone(),
                ));
                // G-12: PK / FlowField accesses never need a SetLoadFields —
                // they don't count toward the "unloaded fields accessed" witness
                // (the access step is still recorded as context for findings
                // carried by OTHER, normal-field accesses).
                if !excluded_field(&name_lc) && !is_write_target {
                    accessed_fields.insert(name_lc);
                }
                access_steps.push(EvidenceStep {
                    routine_id: routine.id.clone(),
                    operation_id: None,
                    callsite_id: None,
                    loop_id: None,
                    source_anchor: anchor_of(&fa.source_anchor, routine),
                    note: format!("accesses {}.{}", state.record_variable_name, fa.field_name),
                });
            }

            // --- cross-routine: record passed by simple identifier to a
            // directly-resolved callee ---
            for cs in &routine.call_sites {
                if !in_window(&cs.source_anchor) {
                    continue;
                }
                let arg_index = cs
                    .argument_texts
                    .iter()
                    .position(|a| a.trim().to_lowercase() == var_key);
                let arg_index = match arg_index {
                    Some(i) => i,
                    None => continue,
                };
                // edge = (graph.edgesByFrom.get(routine.id) ?? []).find(callsiteId === cs.id)
                let edge = ctx.graph.edges_by_from.get(&routine.id).and_then(|edges| {
                    edges
                        .iter()
                        .find(|e| e.callsite_id.as_deref() == Some(cs.id.as_str()))
                });
                let edge = match edge {
                    Some(e) => e,
                    None => {
                        bailout = true;
                        uncertainties.push(UncertaintyLite::new("interface-dispatch", &cs.id));
                        continue;
                    }
                };
                if edge.kind == "interface" || edge.kind == "dynamic" {
                    bailout = true;
                    uncertainties.push(UncertaintyLite::new("interface-dispatch", &cs.id));
                    continue;
                }
                let callee = ctx.routine_by_id.get(edge.to.as_str()).copied();
                let param_effect = callee.and_then(|c| {
                    ctx.parameter_roles_by_routine.get(&c.id).and_then(|roles| {
                        roles
                            .iter()
                            .find(|pe| pe.parameter_index as usize == arg_index)
                    })
                });
                let (callee, param_effect) = match (callee, param_effect) {
                    (Some(c), Some(pe)) => (c, pe),
                    _ => continue,
                };
                let callee_param = callee.parameters.get(arg_index);
                let passed_by_var = callee_param.map(|p| p.is_var).unwrap_or(false);
                // A by-var callee that resets/changes load fields / assigns / uses
                // RecordRef invalidates the caller's state — bail.
                if passed_by_var
                    && (param_effect.may_reset_filters
                        || param_effect.may_change_load_fields
                        || param_effect.may_assign_record
                        || param_effect.may_use_record_ref)
                {
                    bailout = true;
                    uncertainties.push(UncertaintyLite::new(
                        "recordref-or-variant",
                        &cs.operation_id,
                    ));
                    continue;
                }
                // reads = paramEffect.readsFields ("unknown" | field-id list)
                let reads = match &param_effect.reads_fields {
                    // al-sem `readsFields` is `FieldId[] | "unknown"` — never "full". The
                    // base summary engine only ever yields Unknown/Known for reads_fields,
                    // so `Full` is unreachable here; treat it like Unknown (skip
                    // conservatively) rather than mapping "reads all fields" to an empty
                    // set — the latter would silently suppress a d3 finding (a false clean,
                    // which d3 must never produce) if a future change ever emits it.
                    crate::engine::l4::summary::FieldList::Unknown
                    | crate::engine::l4::summary::FieldList::Full => {
                        skipped_unknown_reads += 1;
                        continue;
                    }
                    crate::engine::l4::summary::FieldList::Known(ids) => ids.clone(),
                };
                for fid in &reads {
                    if let Some(name) = field_name_by_id.get(fid.as_str()) {
                        // G-12: PK / FlowField reads in the callee are equally
                        // exempt from SetLoadFields coverage.
                        if !excluded_field(name) {
                            accessed_fields.insert(name.clone());
                        }
                    }
                }
                if !reads.is_empty() {
                    let trigger_note = if edge.kind == "implicit-trigger" {
                        format!(" (via implicit {} trigger)", callee.name)
                    } else {
                        String::new()
                    };
                    access_steps.push(EvidenceStep {
                        routine_id: routine.id.clone(),
                        operation_id: None,
                        callsite_id: Some(cs.id.clone()),
                        loop_id: None,
                        source_anchor: anchor_of(&cs.source_anchor, routine),
                        note: format!(
                            "passes {} to {}{}, which reads {} field(s)",
                            state.record_variable_name,
                            callee.name,
                            trigger_note,
                            reads.len()
                        ),
                    });
                }
            }

            if accessed_fields.is_empty() {
                // No concrete access — no witness, no emit. Also G-12
                // refinement 3: an existence-check Get (no normal field read
                // after it, only PK / FlowField touches or nothing at all)
                // loads no wasted field — suppress.
                continue;
            }

            // --- determination ---
            let kind: Option<&str>;
            let mut missing_list: Vec<String>;
            match &state.load_state {
                LoadState::None => {
                    kind = Some("missing");
                    missing_list = accessed_fields.iter().cloned().collect();
                    missing_list.sort();
                }
                LoadState::Loaded(fields) => {
                    let mut missing: Vec<String> = accessed_fields
                        .iter()
                        .filter(|f| !fields.contains(*f))
                        .cloned()
                        .collect();
                    if !missing.is_empty() {
                        missing.sort();
                        kind = Some("incomplete");
                        missing_list = missing;
                    } else {
                        kind = None;
                        missing_list = Vec::new();
                    }
                }
                LoadState::Invalidated => {
                    kind = None;
                    missing_list = Vec::new();
                }
            }
            let kind = match kind {
                Some(k) => k,
                None => continue, // loaded set covers all accesses — silent
            };

            let retrieval_step = EvidenceStep {
                routine_id: routine.id.clone(),
                operation_id: Some(state.retrieval_op.id.clone()),
                callsite_id: None,
                loop_id: None,
                source_anchor: anchor_of(retrieval_anchor, routine),
                note: format!(
                    "{} on {}{}",
                    state.retrieval_op.op,
                    state.record_variable_name,
                    if kind == "missing" {
                        " with no SetLoadFields"
                    } else {
                        " with a partial SetLoadFields"
                    }
                ),
            };

            let mut evidence_path = vec![retrieval_step];
            evidence_path.extend(access_steps);

            let title = if kind == "missing" {
                "Missing SetLoadFields before a record retrieval"
            } else {
                "Incomplete SetLoadFields — accessed fields not loaded"
            };
            let root_cause = format!(
                "{} runs {} on {} and then accesses field(s) [{}] — {}.",
                routine.name,
                state.retrieval_op.op,
                state.record_variable_name,
                missing_list.join(", "),
                if kind == "missing" {
                    "no SetLoadFields was set"
                } else {
                    "an incomplete SetLoadFields"
                }
            );
            let fix_description = if kind == "missing" {
                format!(
                    "Add SetLoadFields({}) before the retrieval.",
                    missing_list.join(", ")
                )
            } else {
                format!(
                    "Extend SetLoadFields to include: {}.",
                    missing_list.join(", ")
                )
            };

            let base_level = if bailout { "possible" } else { "likely" };
            let id = format!("d3/{}", state.retrieval_op.id);

            let mut finding = Finding {
                id: id.clone(),
                root_cause_key: id,
                detector: DETECTOR.to_string(),
                title: title.to_string().into(),
                root_cause,
                severity: "medium".to_string(),
                confidence: to_confidence(&uncertainties, base_level),
                primary_location: anchor_of(retrieval_anchor, routine),
                evidence_path,
                additional_paths: None,
                affected_objects: vec![routine.object_id.as_str().into()],
                affected_tables: vec![table_id.into()],
                fix_options: vec![FixOption {
                    description: fix_description.into(),
                    safety: "high".into(),
                }],
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
            finding.fingerprint = Some(fp_index.fingerprint_of(&finding));
            findings.push(finding);
        }
    }

    // Dedupe by id (keep first-seen) then sort by id (compareStrings == byte order).
    let mut seen: HashSet<String> = HashSet::new();
    let mut deduped: Vec<Finding> = Vec::new();
    for f in findings {
        if seen.contains(&f.id) {
            continue;
        }
        seen.insert(f.id.clone());
        deduped.push(f);
    }
    deduped.sort_by(|a, b| a.id.cmp(&b.id));

    let emitted = deduped.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates_considered, emitted);
    stats.add_skip("temporaryRecord", skipped_temporary_record);
    stats.add_skip("parseIncomplete", skipped_parse_incomplete);
    stats.add_skip("unknownReads", skipped_unknown_reads);
    Ok(DetectorOutput {
        findings: deduped,
        stats,
        diagnostics: vec![],
        d1_cohort_index: None,
    })
}
