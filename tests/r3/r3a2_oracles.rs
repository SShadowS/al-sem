//! R3a-2 EXIT-GATE — native L4-direct structural oracle for the fixed-point
//! summary core.
//!
//! Ground-truth-free, STRUCTURAL oracles run NATIVELY against the Rust R3a-2
//! projection (`project_r3a2`) + the R3a-1 combined-graph/SCC projection — NOT a
//! transitive byte-match against the al-sem goldens. The byte-parity differential
//! (`r3a2_differential.rs`) is necessary but not sufficient: if BOTH engines made
//! the same structural mistake (a stray inherited effect, a duplicate effectKey, a
//! mis-merged via, a mis-flagged `inRecursiveCycle`), a pure equality diff would
//! still pass. These oracles assert the summary-core CONTRACT in ABSOLUTE terms over
//! the Rust output.
//!
//! ## The five invariants (plan Task 3 Step 2)
//!   1. every INHERITED dbEffect (`via != "direct"`) traces to a combined-graph
//!      CALLEE carrying an effect with the SAME effectKey (the composition didn't
//!      invent it);
//!   2. `effectKeyOf` dedup holds — no two effects on one routine share an effectKey;
//!   3. the `via` of a merged effect is the MAX over the via-precedence ladder of
//!      every contributing source (the routine's own direct emit + every callee's
//!      effect with that key);
//!   4. `inRecursiveCycle` ⟺ the routine's SCC is RECURSIVE (cross-check R3a-1);
//!   5. a routine carrying ≥1 uncertainty has `hasUnresolvedCalls = true` (every
//!      uncertainty source co-sets the flag in al-sem; the reverse does not hold —
//!      the flag also PROPAGATES from callees without a local uncertainty).
//!
//! The corpus is the full SOURCE-ONLY `ws-*` set; the oracles run over EVERY fixture
//! so the inherited-effect / recursive-SCC / opaque-callee cases are all exercised.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use al_sem::engine::l4::effect_lattice::via_for_edge_kind;
use al_sem::engine::l4::summary::{PRoutineSummaryCore, R3a2Projection, project_r3a2};
use al_sem::program::model::program_calls::assemble_and_resolve_workspace_with_program_calls;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// True if a callee effect with key `callee_key` is a valid SOURCE for an
/// inherited effect with key `inherited_key`.
///
/// Pre-Task-7 the inherited copy shared the callee's key BYTE-FOR-BYTE (the fold
/// was verbatim). Task 7 (G5 / RV-7) SUBSTITUTES a callee `ParameterDependent(i)`
/// (tempfrag `p<i>`) per-callsite through the caller's argument binding, so the
/// inherited key's tempfrag becomes the resolved `t`/`f`/`u` while the callee's
/// stays `p<i>`. The `op|tableId|operationId` prefix is invariant under
/// substitution (only the tempfrag changes), so a valid source is either:
///   - the EXACT same key (non-PD effects fold unchanged), OR
///   - a callee key with the SAME prefix whose tempfrag is `p<...>` (the PD
///     effect that substituted into this inherited tempfrag).
fn callee_key_sources_inherited(callee_key: &str, inherited_key: &str) -> bool {
    if callee_key == inherited_key {
        return true;
    }
    // Split off the final `|tempfrag` segment; compare the invariant prefix and
    // require the callee tempfrag to be a parameter-dependent fragment (`p<i>`).
    // Confirm the numeric index too — not merely a leading 'p' — so a non-PD frag
    // that happens to start with 'p' can never mask a real regression here.
    match (callee_key.rsplit_once('|'), inherited_key.rsplit_once('|')) {
        (Some((callee_prefix, callee_frag)), Some((inh_prefix, _inh_frag))) => {
            let is_pd_frag = callee_frag.starts_with('p')
                && callee_frag
                    .get(1..)
                    .is_some_and(|s| s.parse::<u32>().is_ok());
            callee_prefix == inh_prefix && is_pd_frag
        }
        _ => false,
    }
}

fn corpus_dir() -> PathBuf {
    repo_root().join("tests").join("r0-corpus")
}

fn goldens_dir() -> PathBuf {
    repo_root().join("tests").join("r3a2-goldens")
}

/// Every source-only fixture that has a committed R3a-2 golden (sorted).
fn discover_fixtures() -> Vec<String> {
    let dir = goldens_dir();
    let mut out = Vec::new();
    let entries = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read R3a-2 goldens dir {}: {e}", dir.display()));
    for entry in entries {
        let name = entry
            .expect("dir entry")
            .file_name()
            .to_string_lossy()
            .to_string();
        if name.ends_with(".r3a2-trace.golden.json") {
            continue;
        }
        if let Some(fx) = name.strip_suffix(".r3a2.golden.json") {
            out.push(fx.to_string());
        }
    }
    out.sort();
    out
}

/// The via-precedence rank — mirrors al-sem `VIA_RANK` / the Rust `via_rank`:
/// `direct=4 > implicit-trigger=3 > event-subscriber=2 > dynamic=1 > inherited=0`.
fn via_rank(via: &str) -> u8 {
    match via {
        "direct" => 4,
        "implicit-trigger" => 3,
        "event-subscriber" => 2,
        "dynamic" => 1,
        "inherited" => 0,
        _ => 0,
    }
}

/// One fixture's oracle input from the RUST resolved model: the R3a-2 summaries
/// (by stable routineId) + the R3a-1 combined-graph callees + the recursive-SCC
/// member set.
struct OracleInput {
    /// stable routineId → its projected summary.
    summaries: HashMap<String, PRoutineSummaryCore>,
    /// stable routineId → its combined-graph callee (stable routineId, edge kind).
    callees: HashMap<String, Vec<(String, String)>>,
    /// stable routineIds that are members of a RECURSIVE SCC.
    recursive_members: HashSet<String>,
}

fn build(fixture: &str) -> Option<OracleInput> {
    let resolved = assemble_and_resolve_workspace_with_program_calls(&corpus_dir().join(fixture))?;

    let R3a2Projection { summaries } = project_r3a2(&resolved);
    let summaries: HashMap<String, PRoutineSummaryCore> = summaries
        .into_iter()
        .map(|s| (s.routine_id.clone(), s))
        .collect();

    // R3a-1 combined graph (stable ids) — the callee relation + the recursive SCCs.
    let r3a1 = resolved.project_r3a1_combined_graph();
    let mut callees: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for e in &r3a1.combined_edges {
        callees
            .entry(e.from.clone())
            .or_default()
            .push((e.to.clone(), e.kind.clone()));
    }
    let mut recursive_members: HashSet<String> = HashSet::new();
    for scc in &r3a1.sccs {
        if scc.recursive {
            for m in &scc.members {
                recursive_members.insert(m.clone());
            }
        }
    }

    Some(OracleInput {
        summaries,
        callees,
        recursive_members,
    })
}

// ============================================================================
// 1. Every inherited dbEffect (via != "direct") traces to a callee carrying an
//    effect with the SAME effectKey.
// ============================================================================

#[test]
fn every_inherited_effect_traces_to_a_callee_effect() {
    let mut checked = 0usize;
    for fixture in discover_fixtures() {
        let Some(input) = build(&fixture) else {
            continue;
        };
        for (rid, summary) in &input.summaries {
            for e in &summary.db_effects {
                if e.via == "direct" {
                    continue;
                }
                // Some combined-graph callee of `rid` must carry an effect that is
                // a valid SOURCE for this inherited effect — either the SAME
                // effectKey (non-PD effects fold verbatim) or a `parameter-dependent`
                // callee effect with the same op|table|op-id prefix that was
                // SUBSTITUTED per-callsite into this inherited tempfrag (Task 7).
                let found = input
                    .callees
                    .get(rid)
                    .map(|cs| {
                        cs.iter().any(|(callee_id, _kind)| {
                            input
                                .summaries
                                .get(callee_id)
                                .map(|cs_sum| {
                                    cs_sum.db_effects.iter().any(|ce| {
                                        callee_key_sources_inherited(&ce.effect_key, &e.effect_key)
                                    })
                                })
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false);
                assert!(
                    found,
                    "[{fixture}] routine {rid} has an inherited effect (via={}) with effectKey \
                     {} that NO combined-graph callee carries — the composition invented it",
                    e.via, e.effect_key,
                );
                checked += 1;
            }
        }
    }
    assert!(
        checked > 0,
        "no inherited effects checked — the corpus must carry ≥1 (anti-degenerate)"
    );
    eprintln!("R3a-2 oracle #1: {checked} inherited effects all trace to a callee effect");
}

// ============================================================================
// 2. effectKeyOf dedup holds — no two effects on one routine share an effectKey.
// ============================================================================

#[test]
fn no_two_effects_on_a_routine_share_an_effect_key() {
    let mut checked = 0usize;
    for fixture in discover_fixtures() {
        let Some(input) = build(&fixture) else {
            continue;
        };
        for (rid, summary) in &input.summaries {
            let mut seen: HashSet<&str> = HashSet::new();
            for e in &summary.db_effects {
                assert!(
                    seen.insert(e.effect_key.as_str()),
                    "[{fixture}] routine {rid} carries TWO effects with the same effectKey {} \
                     — the via-precedence merge failed to dedup",
                    e.effect_key,
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 0, "no effects checked — corpus degenerate?");
    eprintln!("R3a-2 oracle #2: {checked} effects, effectKey dedup holds per routine");
}

// ============================================================================
// 3. The via of a merged effect is the MAX over the via-precedence ladder of
//    every contributing source (this routine's direct emit + every callee effect
//    with that key).
// ============================================================================

#[test]
fn merged_via_is_the_max_over_contributing_sources() {
    let mut checked = 0usize;
    for fixture in discover_fixtures() {
        let Some(input) = build(&fixture) else {
            continue;
        };
        for (rid, summary) in &input.summaries {
            for e in &summary.db_effects {
                // The CALLER inherits a callee's effect tagged with the EDGE's via —
                // `via_for_edge_kind(edge.kind)` — NOT the callee's own via. So the
                // contributing vias for THIS effectKey are exactly:
                //   - `via_for_edge_kind(edge.kind)` for every callee edge whose callee
                //     summary carries the key, and
                //   - "direct" if this routine self-emits the key (unobservable here
                //     directly, BUT: merged via == "direct" ⟺ self-emitted, since
                //     "direct" can only originate from the routine's own emission).
                // INVARIANT: the merged via is the MAX over those contributions.
                let mut max_edge_rank: Option<u8> = None;
                let mut callee_carries = false;
                if let Some(cs) = input.callees.get(rid) {
                    for (callee_id, kind) in cs {
                        if let Some(cs_sum) = input.summaries.get(callee_id) {
                            // Task 7: a PD callee effect is SUBSTITUTED into this
                            // inherited tempfrag, so match on the substitution-aware
                            // source relation, not byte-equality.
                            if cs_sum.db_effects.iter().any(|ce| {
                                callee_key_sources_inherited(&ce.effect_key, &e.effect_key)
                            }) {
                                callee_carries = true;
                                let r = via_rank(via_for_edge_kind(kind));
                                max_edge_rank = Some(max_edge_rank.map_or(r, |m| m.max(r)));
                            }
                        }
                    }
                }
                let merged_rank = via_rank(&e.via);
                if e.via == "direct" {
                    // Self-emitted: "direct" (rank 4) dominates every edge contribution.
                    // Nothing more to assert (direct is the top of the ladder).
                } else {
                    // Purely inherited: a callee must carry the key, and the merged via
                    // must EQUAL the max edge-via over the carrying callees.
                    assert!(
                        callee_carries,
                        "[{fixture}] routine {rid} effectKey {} has via={} but NO callee carries \
                         the key",
                        e.effect_key, e.via,
                    );
                    let expected = max_edge_rank.expect("callee_carries ⟹ max_edge_rank set");
                    assert_eq!(
                        merged_rank, expected,
                        "[{fixture}] routine {rid} effectKey {} merged via={} (rank {merged_rank}) \
                         != max edge-via over carrying callees (rank {expected}) — the via-precedence \
                         merge did not keep the max",
                        e.effect_key, e.via,
                    );
                }
                checked += 1;
            }
        }
    }
    assert!(checked > 0, "no effects checked for via-precedence");
    eprintln!("R3a-2 oracle #3: {checked} effects respect via = max over contributing sources");
}

// ============================================================================
// 4. inRecursiveCycle ⟺ the routine's SCC is recursive (cross-check R3a-1).
// ============================================================================

#[test]
fn in_recursive_cycle_iff_scc_is_recursive() {
    let mut recursive_seen = 0usize;
    for fixture in discover_fixtures() {
        let Some(input) = build(&fixture) else {
            continue;
        };
        for (rid, summary) in &input.summaries {
            let is_recursive_member = input.recursive_members.contains(rid);
            assert_eq!(
                summary.in_recursive_cycle, is_recursive_member,
                "[{fixture}] routine {rid}: inRecursiveCycle={} but its R3a-1 SCC \
                 recursive-membership={is_recursive_member}",
                summary.in_recursive_cycle,
            );
            if summary.in_recursive_cycle {
                recursive_seen += 1;
            }
        }
    }
    assert!(
        recursive_seen > 0,
        "no recursive-cycle routine seen across the corpus (anti-degenerate: need ≥1)"
    );
    eprintln!(
        "R3a-2 oracle #4: inRecursiveCycle⟺recursive-SCC holds; {recursive_seen} recursive-cycle routine(s)"
    );
}

// ============================================================================
// 5. A routine carrying ≥1 uncertainty has hasUnresolvedCalls = true.
//    (Every uncertainty source co-sets the flag in al-sem; the reverse does not
//    hold — the flag PROPAGATES from callees without a local uncertainty.)
// ============================================================================

#[test]
fn any_uncertainty_implies_has_unresolved_calls() {
    let mut with_uncertainty = 0usize;
    let mut with_flag = 0usize;
    for fixture in discover_fixtures() {
        let Some(input) = build(&fixture) else {
            continue;
        };
        for (rid, summary) in &input.summaries {
            if !summary.uncertainties.is_empty() {
                assert!(
                    summary.has_unresolved_calls,
                    "[{fixture}] routine {rid} carries {} uncertaint(y/ies) but \
                     hasUnresolvedCalls=false — an uncertainty source must co-set the flag",
                    summary.uncertainties.len(),
                );
                with_uncertainty += 1;
            }
            if summary.has_unresolved_calls {
                with_flag += 1;
            }
        }
    }
    assert!(
        with_uncertainty > 0,
        "no routine with an uncertainty seen across the corpus (anti-degenerate: need ≥1)"
    );
    assert!(
        with_flag > 0,
        "no routine with hasUnresolvedCalls seen (anti-degenerate: need ≥1)"
    );
    eprintln!(
        "R3a-2 oracle #5: uncertainty ⟹ hasUnresolvedCalls holds; {with_uncertainty} \
         routine(s) with uncertainty, {with_flag} with the flag set"
    );
}
