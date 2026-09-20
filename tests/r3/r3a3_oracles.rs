//! R3a-3 EXIT-GATE — native L4-direct structural oracle for the capability cone +
//! coverage.
//!
//! Ground-truth-free, STRUCTURAL oracles run NATIVELY against the Rust R3a-3
//! projection (`project_r3a3`) — NOT a transitive byte-match against the al-sem
//! goldens. The byte-parity differential (`r3a3_differential.rs`) is necessary but
//! not sufficient: if BOTH engines made the same structural mistake (a stray
//! inherited fact, a duplicate inheritedFactKey, a non-monotone coverage roll-up, a
//! provenance/via invariant break), a pure equality diff would still pass. These
//! oracles assert the cone/coverage CONTRACT in ABSOLUTE terms over the Rust output.
//!
//! ## The invariants (plan Task 3 Step 2)
//!   1. every INHERITED fact `via != "self"`, carries `provenance == "inherited"`
//!      AND a `witnessCallsiteId` (the first-hop callsite from subject); every
//!      DIRECT fact carries `provenance == "direct"` AND `via == "self"`;
//!   2. every inherited fact traces to a callee carrying a DIRECT (or inherited)
//!      fact with the SAME inheritedFactKey
//!      (op|resourceKind|resourceId|confidence|tempClass — the last component
//!      ⟨issue 33⟩)
//!      at the recorded MINIMUM call-distance — no shorter path exists (cross-check
//!      against the independent BFS matrix: `genuine >1-hop count ≤ total inherited`,
//!      `routines_with_inherited` agrees);
//!   3. `inheritedFactKey` dedup holds — no two inherited facts on one routine share
//!      an inheritedFactKey;
//!   4. coverage.inheritedStatus is the MONOTONE roll-up — `complete` ⟹ no reasons
//!      and no unknownTargets; a non-`complete` inheritedStatus carries ≥1 reason OR
//!      ≥1 unknownTarget; directStatus `complete` never has reasons;
//!   5. capabilityFactsDirect is SELF-CONSISTENT with the cone — every inheritedFactKey
//!      present in some routine's inherited set is produced as a DIRECT fact by at
//!      least one routine in the corpus (the cone invents no key).
//!
//! The corpus is the full SOURCE-ONLY `ws-*` set; the oracles run over EVERY fixture.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;

use al_sem::engine::l3::l3_workspace::assemble_and_resolve_workspace_default;
use al_sem::engine::l4::capability_cone::{
    PCapabilityFact, PRoutineConeCoverage, compute_r3a3_real_matrix, project_r3a3,
};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn corpus_dir() -> PathBuf {
    repo_root().join("tests").join("r0-corpus")
}

fn goldens_dir() -> PathBuf {
    repo_root().join("tests").join("r3a3-goldens")
}

/// Every source-only fixture that has a committed R3a-3 golden (sorted).
fn discover_fixtures() -> Vec<String> {
    let dir = goldens_dir();
    let mut out = Vec::new();
    let entries = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read R3a-3 goldens dir {}: {e}", dir.display()));
    for entry in entries {
        let name = entry
            .expect("dir entry")
            .file_name()
            .to_string_lossy()
            .to_string();
        if let Some(fx) = name.strip_suffix(".r3a3.golden.json") {
            out.push(fx.to_string());
        }
    }
    out.sort();
    out
}

/// The inheritedFactKey (op|resourceKind|resourceId|confidence|tempClass) —
/// mirrors `capability_cone::inherited_fact_key` over the PROJECTED fact, whose
/// `extra` is already JSON.
///
/// ⟨issue 33⟩ The temp class is load-bearing for BOTH oracles that use this
/// mirror, and for different reasons — do NOT un-widen it for either of them.
///
///   - `oracle_r3a3_inherited_factkey_dedup`: the cone now keeps a known-temp
///     and a non-known-temp fact about one table as two SEPARATE inherited
///     facts, which share a base key and would read as a broken dedup through
///     the old 4-part mirror.
///   - `oracle_r3a3_inherited_keys_trace_to_a_direct_producer`: NOT inert here,
///     despite what an earlier draft of this comment said. With the class in the
///     key, that oracle now asserts "every inherited fact traces to a SAME-CLASS
///     direct producer" — which is exactly the soundness precondition issue 32's
///     temp-class guards in `digest::reconstruct_witness_paths` rest on, made
///     executable for free. It holds because `retag` copies `extra` through
///     untouched; un-widen this mirror and the pin silently disappears while the
///     oracle keeps passing. Stated limit: it runs over the source-only fixture
///     corpus, so a cross-app scope mismatch between the cone's node set and
///     `snap.capability_facts` would still degrade witnesses undetected.
fn inherited_fact_key(f: &PCapabilityFact) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        f.op,
        f.resource_kind,
        f.resource_id.as_deref().unwrap_or(""),
        f.confidence,
        if projected_fact_is_known_temp(f) {
            "kt"
        } else {
            "nt"
        }
    )
}

/// `l4::cone_derived::fact_is_known_temp` over the PROJECTED fact — only the
/// exact `tempState: {kind: "known", value: true}` signal qualifies.
///
/// Known gap, faithful TODAY: the production predicate matches
/// `CapabilityExtra::Table` specifically, while this reads `extra.tempState`
/// without checking `extra.kind == "table"`. The two agree because the `Table`
/// arm is the only one that emits `tempState` — so if another `extra` variant
/// ever gains one, this mirror silently stops mirroring.
fn projected_fact_is_known_temp(f: &PCapabilityFact) -> bool {
    f.extra
        .as_ref()
        .and_then(|e| e.get("tempState"))
        .map(|ts| {
            ts.get("kind").and_then(|k| k.as_str()) == Some("known")
                && ts.get("value").and_then(|v| v.as_bool()) == Some(true)
        })
        .unwrap_or(false)
}

/// Load the Rust R3a-3 projection for one fixture (source-only).
fn rust_projection(fixture: &str) -> Vec<PRoutineConeCoverage> {
    let dir = corpus_dir().join(fixture);
    match assemble_and_resolve_workspace_default(&dir) {
        Some(resolved) => project_r3a3(&resolved).summaries,
        None => vec![],
    }
}

#[test]
fn oracle_r3a3_provenance_via_invariants() {
    let fixtures = discover_fixtures();
    let mut checked = 0usize;
    let mut direct_seen = 0usize;
    let mut inherited_seen = 0usize;
    for fx in &fixtures {
        for s in rust_projection(fx) {
            for f in &s.capability_facts_direct {
                assert_eq!(
                    f.provenance, "direct",
                    "[{fx}] {} direct fact provenance != direct",
                    s.routine_id
                );
                assert_eq!(
                    f.via, "self",
                    "[{fx}] {} direct fact via != self",
                    s.routine_id
                );
                direct_seen += 1;
            }
            for f in &s.capability_facts_inherited {
                assert_eq!(
                    f.provenance, "inherited",
                    "[{fx}] {} inherited fact provenance != inherited",
                    s.routine_id
                );
                assert_ne!(
                    f.via, "self",
                    "[{fx}] {} inherited fact via == self",
                    s.routine_id
                );
                // The first-hop callsite witness is present for the CALL-shaped edge
                // kinds (call / object-run / dependency). Edge kinds that carry no
                // callsite — `event-dispatch` / `implicit-trigger` — legitimately
                // have NO witnessCallsiteId (al-sem `callsiteIdForEdge` returns
                // undefined for them; the goldens confirm this).
                let callsite_bearing =
                    !matches!(f.via.as_str(), "event-dispatch" | "implicit-trigger");
                if callsite_bearing {
                    assert!(
                        f.witness_callsite_id.is_some(),
                        "[{fx}] {} inherited fact via={} lacks witnessCallsiteId (first-hop)",
                        s.routine_id,
                        f.via
                    );
                }
                inherited_seen += 1;
            }
            checked += 1;
        }
    }
    assert!(direct_seen > 0, "no direct facts observed across corpus");
    assert!(
        inherited_seen > 0,
        "no inherited facts observed across corpus"
    );
    eprintln!(
        "R3a-3 oracle (provenance/via): {checked} routine(s), {direct_seen} direct + \
         {inherited_seen} inherited facts — all invariants hold."
    );
}

#[test]
fn oracle_r3a3_inherited_factkey_dedup() {
    let fixtures = discover_fixtures();
    for fx in &fixtures {
        for s in rust_projection(fx) {
            let mut seen: HashSet<String> = HashSet::new();
            for f in &s.capability_facts_inherited {
                let k = inherited_fact_key(f);
                assert!(
                    seen.insert(k.clone()),
                    "[{fx}] {} has DUPLICATE inheritedFactKey `{k}` — dedup broken",
                    s.routine_id
                );
            }
        }
    }
    eprintln!("R3a-3 oracle (inheritedFactKey dedup): holds across all fixtures.");
}

#[test]
fn oracle_r3a3_inherited_keys_trace_to_a_direct_producer() {
    // Invariant 2 + 5: every inheritedFactKey present in ANY routine's inherited set
    // is produced as a DIRECT fact by at least one routine in the SAME fixture — the
    // cone invents no key (every inherited fact descends from some direct emit).
    let fixtures = discover_fixtures();
    for fx in &fixtures {
        let summaries = rust_projection(fx);
        let mut direct_keys: HashSet<String> = HashSet::new();
        for s in &summaries {
            for f in &s.capability_facts_direct {
                direct_keys.insert(inherited_fact_key(f));
            }
        }
        for s in &summaries {
            for f in &s.capability_facts_inherited {
                let k = inherited_fact_key(f);
                assert!(
                    direct_keys.contains(&k),
                    "[{fx}] {} inherited key `{k}` is produced by NO direct fact in the \
                     fixture — the cone invented it",
                    s.routine_id
                );
            }
        }
    }
    eprintln!(
        "R3a-3 oracle (inherited→direct producer): every inherited key descends from a \
         direct emit."
    );
}

#[test]
fn oracle_r3a3_coverage_monotone_rollup() {
    // Invariant 4: coverage roll-up is monotone + well-formed.
    let fixtures = discover_fixtures();
    let mut non_trivial = 0usize;
    for fx in &fixtures {
        for s in rust_projection(fx) {
            let cov = &s.coverage;
            // subject == routineId.
            assert_eq!(
                cov.subject, s.routine_id,
                "[{fx}] coverage.subject != routineId for {}",
                s.routine_id
            );
            // directStatus complete ⟹ no reasons.
            if cov.direct_status == "complete" {
                assert!(
                    cov.reasons.is_empty() || cov.inherited_status != "complete",
                    "[{fx}] {} directStatus=complete but carries reasons {:?} with \
                     inheritedStatus=complete",
                    s.routine_id,
                    cov.reasons
                );
            }
            // inheritedStatus complete ⟹ no reasons AND no unknownTargets (monotone:
            // a clean cone forwards nothing).
            if cov.inherited_status == "complete" {
                assert!(
                    cov.reasons.is_empty(),
                    "[{fx}] {} inheritedStatus=complete but reasons={:?}",
                    s.routine_id,
                    cov.reasons
                );
                assert!(
                    cov.unknown_targets.is_empty(),
                    "[{fx}] {} inheritedStatus=complete but unknownTargets={:?}",
                    s.routine_id,
                    cov.unknown_targets
                );
            } else {
                non_trivial += 1;
                // non-complete inheritedStatus ⟹ ≥1 reason OR ≥1 unknownTarget.
                assert!(
                    !cov.reasons.is_empty() || !cov.unknown_targets.is_empty(),
                    "[{fx}] {} inheritedStatus={} but NO reasons and NO unknownTargets \
                     (non-monotone roll-up)",
                    s.routine_id,
                    cov.inherited_status
                );
                // reasons + unknownTargets are sorted + deduped.
                let mut sorted = cov.reasons.clone();
                sorted.sort();
                sorted.dedup();
                assert_eq!(
                    sorted, cov.reasons,
                    "[{fx}] {} coverage.reasons not sorted/deduped",
                    s.routine_id
                );
                let ut: BTreeSet<&String> = cov.unknown_targets.iter().collect();
                assert_eq!(
                    ut.len(),
                    cov.unknown_targets.len(),
                    "[{fx}] {} coverage.unknownTargets has duplicates",
                    s.routine_id
                );
            }
        }
    }
    assert!(
        non_trivial > 0,
        "no non-trivial inheritedStatus observed — coverage roll-up never exercised"
    );
    eprintln!(
        "R3a-3 oracle (coverage monotone roll-up): {non_trivial} non-trivial inheritedStatus \
         record(s), all well-formed."
    );
}

#[test]
fn oracle_r3a3_real_bfs_consistency() {
    // Invariant 2: the GENUINE BFS-derived counts are consistent with the projected
    // inherited facts — `routines_with_inherited_facts` agrees, and the genuine
    // >1-hop count never exceeds the total inherited facts (a >1-hop witness IS an
    // inherited fact). Cross-validates the cone's distance/witness selection against
    // an INDEPENDENT BFS oracle.
    let fixtures = discover_fixtures();
    let mut total_real_routines = 0usize;
    let mut total_proj_routines = 0usize;
    let mut total_more_than_1_hop = 0usize;
    let mut total_inherited = 0usize;
    let mut total_ties = 0usize;
    for fx in &fixtures {
        let dir = corpus_dir().join(fx);
        let Some(resolved) = assemble_and_resolve_workspace_default(&dir) else {
            continue;
        };
        let real = compute_r3a3_real_matrix(&resolved);
        let summaries = project_r3a3(&resolved).summaries;

        let proj_routines_with_inherited = summaries
            .iter()
            .filter(|s| !s.capability_facts_inherited.is_empty())
            .count();
        let proj_inherited: usize = summaries
            .iter()
            .map(|s| s.capability_facts_inherited.len())
            .sum();

        // The INDEPENDENT BFS oracle and the production cone must agree on WHICH
        // routines carry inherited facts.
        assert_eq!(
            real.routines_with_inherited_facts, proj_routines_with_inherited,
            "[{fx}] BFS oracle routinesWithInherited ({}) != projected ({})",
            real.routines_with_inherited_facts, proj_routines_with_inherited
        );
        // A genuine >1-hop witness IS an inherited fact ⟹ count ≤ total inherited.
        assert!(
            real.facts_with_more_than_1_hop_witness <= proj_inherited,
            "[{fx}] BFS >1-hop count ({}) exceeds total inherited facts ({})",
            real.facts_with_more_than_1_hop_witness,
            proj_inherited
        );

        total_real_routines += real.routines_with_inherited_facts;
        total_proj_routines += proj_routines_with_inherited;
        total_more_than_1_hop += real.facts_with_more_than_1_hop_witness;
        total_inherited += proj_inherited;
        total_ties += real.equal_distance_ties;
    }
    assert_eq!(total_real_routines, total_proj_routines);
    assert!(
        total_more_than_1_hop > 0,
        "no genuine >1-hop witnesses across the corpus"
    );
    assert!(
        total_ties > 0,
        "no genuine equal-distance ties across the corpus"
    );
    eprintln!(
        "R3a-3 oracle (real BFS consistency): routinesWithInherited={total_real_routines} \
         (agrees with cone), genuine >1hop={total_more_than_1_hop} (≤ {total_inherited} \
         inherited), genuine ties={total_ties}."
    );
}

#[test]
fn oracle_r3a3_family_coverage() {
    // The 13 capability families: assert the op/resourceKind families the source-only
    // corpus exercises are all present (a degenerate port emitting only table facts
    // would pass an equality diff if the goldens were equally degenerate). The
    // corpus does NOT exercise telemetry/background/hyperlink (no fixtures), so we
    // gate on the resourceKinds that DO appear in the manifest's opDistribution.
    let fixtures = discover_fixtures();
    let mut ops: HashMap<String, usize> = HashMap::new();
    let mut kinds: HashMap<String, usize> = HashMap::new();
    for fx in &fixtures {
        for s in rust_projection(fx) {
            for f in s
                .capability_facts_direct
                .iter()
                .chain(s.capability_facts_inherited.iter())
            {
                *ops.entry(f.op.clone()).or_insert(0) += 1;
                *kinds.entry(f.resource_kind.clone()).or_insert(0) += 1;
            }
        }
    }
    // Ops present in the manifest opDistribution (the families the corpus exercises):
    // table (read/insert/modify/delete), dispatch (execute), commit, error (error-throw),
    // events (publish/subscribe), http (send), storage (store-write), file (write-blob),
    // ui (ui-confirm/ui-message/ui-error/ui-window-open).
    for op in [
        "read",
        "insert",
        "modify",
        "delete",
        "execute",
        "commit",
        "error-throw",
        "publish",
        "subscribe",
        "send",
        "store-write",
        "write-blob",
        "ui-message",
        "ui-error",
        "ui-confirm",
        "ui-window-open",
    ] {
        assert!(
            ops.get(op).copied().unwrap_or(0) > 0,
            "capability op `{op}` absent from the corpus — a family is not firing (have: {:?})",
            {
                let mut v: Vec<_> = ops.keys().cloned().collect();
                v.sort();
                v
            }
        );
    }
    // resourceKinds covering ≥10 of the 13 families.
    for kind in [
        "table",
        "transaction",
        "codeunit",
        "page",
        "event",
        "http",
        "isolated-storage",
        "file",
        "ui",
        "error",
    ] {
        assert!(
            kinds.get(kind).copied().unwrap_or(0) > 0,
            "resourceKind `{kind}` absent from the corpus (have: {:?})",
            {
                let mut v: Vec<_> = kinds.keys().cloned().collect();
                v.sort();
                v
            }
        );
    }
    eprintln!(
        "R3a-3 oracle (family coverage): {} distinct ops, {} distinct resourceKinds — the \
         ported families fire.",
        ops.len(),
        kinds.len()
    );
}
