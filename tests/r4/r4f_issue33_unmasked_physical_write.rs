//! ⟨issue 33, acceptance A5⟩ The unmasked physical write must not resurface as a
//! `WRITE_PENDING_AT_EXTERNAL_IO` false positive.
//!
//! Issue 33 puts the temp class into `capability_cone::inherited_fact_key`, so a
//! physical write of a table no longer disappears behind a known-temp write of the
//! same table. That unmasking is the whole point — and on its own it would open a
//! new false-positive class, because the two layers downstream disagree about which
//! object they read:
//!
//!   - the ordering engine grades an occurrence physical-or-not from the FACT's
//!     `temp_state` (the cone representative's), while
//!   - the witness terminal is chosen by `digest::fact_equivalent`, which compares
//!     op / resourceKind / resourceId / resourceArgSource / objectType and **never**
//!     temp state.
//!
//! So a physical fact would terminate on the nearest equivalent direct fact — which,
//! in `ws-d33-mixed-temp-key`, is a TEMP insert sitting before an HTTP call. The
//! occurrence would be graded physical, anchored at an in-memory write, and d47 would
//! report `WRITE_PENDING_AT_EXTERNAL_IO` at CRITICAL for a body that never opens a
//! write transaction before the IO. That is issue 32's finding. It is NOT unreachable
//! on master: `fact_equivalent` compares `resource_id` only when BOTH sides are
//! `Some`, so a routine whose only direct fact is a `None`-rid known-temp write
//! mis-terminates a physical fact on it, with no ordering argument needed. What the
//! cone's key collapse hid was the MIXED case specifically, so this change widens the
//! guard's reach rather than creating its need.
//!
//! The guard is the temp-class check on the terminal `find` in
//! `reconstruct_witness_paths` (and its twin on the reverse-BFS prune seed).
//!
//! Two tests, with different jobs, because ONE of them cannot do both:
//!   - the ordering test pins the guard itself — drop the terminal check and `Run`
//!     gains `WRITE_PENDING_AT_EXTERNAL_IO` (verified, not predicted);
//!   - the fingerprint test is the anti-degenerate half — it proves the physical fact
//!     IS there and its witness is COMPLETE, so a silent `Run` cannot be read as "the
//!     fix is absent" or "the guards shredded the witness". It is discriminated by the
//!     key widening, NOT by the guard; see its own doc for why, measured.
//!
//! And the ordering test carries its own control: a physical write genuinely before
//! the IO, which fires on the pre-issue-33 engine and must keep firing here.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use al_sem::engine::l3::l3_workspace::assemble_and_resolve_workspace_default;
use al_sem::engine::l5::fingerprint_cli::{
    FingerprintFormat, FingerprintOptions, FingerprintOutput, run_fingerprint_pipeline,
};
use al_sem::engine::l5::fingerprint_query::WitnessLimit;
use al_sem::engine::l5::ordering_facts::project_r4f_ordering_facts;
use serde_json::Value;

/// NAMING COLLISION, stated rather than fixed. `tests/r0-corpus/` names its
/// fixtures `ws-d<NN>-*` where `NN` is a DETECTOR id, and `ws-d33/` already
/// exists — it is detector `d33`'s own fixture, and it is itself temp-related.
/// This one is named for ISSUE 33, not detector d33. The two sit adjacent in
/// `l2_features.snapshot` and read as siblings. It should be renamed to
/// `ws-issue33-mixed-temp-key`; that costs a directory rename, the r3a3 golden's
/// filename, the `l2_features.snapshot` lines and this constant, and it is far
/// cheaper before someone triages detector d33 against the wrong fixture.
const FIXTURE: &str = "ws-d33-mixed-temp-key";
/// The table both the temp write (`Inner`) and the physical write (`PhysWriter`)
/// target — the one whose two obligations used to collapse into one.
const MIXED_TABLE: &str = "11111111-0000-0000-0000-000000000033/table/50100";

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("r0-corpus")
        .join(FIXTURE)
}

/// The fingerprint-query JSON for the fixture (includeInherited, witnessLimit 3 —
/// the same query flags the cli-b differential uses).
fn fingerprint_json() -> Value {
    let ws = fixture_dir();
    let opts = FingerprintOptions {
        workspace: &ws,
        driver_version: "issue33",
        format: FingerprintFormat::Json,
        out: None,
        shard: None,
        witness_limit: Some(WitnessLimit::Capped(3)),
        roots: None,
        routine_selectors: Vec::new(),
        include_inherited: true,
        is_query_requested: true,
        deterministic: true,
        strict: false,
        verbosity: "compact",
        inventory_only: false,
        no_roots_config: false,
    };
    let result = run_fingerprint_pipeline(&opts).expect("fingerprint pipeline");
    let text = match result.output {
        FingerprintOutput::Text(t) => t,
        _ => panic!("fingerprint query must produce text output"),
    };
    serde_json::from_str(&text).expect("fingerprint query output is JSON")
}

/// `routine display name -> (evidence-anchor excerpt, witness incomplete)` for every
/// `insert` fact on [`MIXED_TABLE`].
fn mixed_table_inserts(doc: &Value) -> HashMap<String, Vec<(String, bool)>> {
    let mut out: HashMap<String, Vec<(String, bool)>> = HashMap::new();
    for block in doc["payload"]["blocks"].as_array().expect("blocks") {
        let name = block["routine"]["display"]
            .as_str()
            .expect("routine display")
            .to_string();
        for family in block["families"].as_array().expect("families") {
            for resource in family["resources"].as_array().expect("resources") {
                if resource["display"].as_str() != Some(MIXED_TABLE) {
                    continue;
                }
                for fact in resource["facts"].as_array().expect("facts") {
                    if fact["op"].as_str() != Some("insert") {
                        continue;
                    }
                    let witness = &fact["witness"];
                    out.entry(name.clone()).or_default().push((
                        witness["evidence"]["anchor"]["excerpt"]
                            .as_str()
                            .unwrap_or("<no excerpt>")
                            .to_string(),
                        witness["incomplete"].as_bool().expect("witness.incomplete"),
                    ));
                }
            }
        }
    }
    out
}

/// ⟨A5, anti-degenerate half⟩ Both same-table obligations are PRESENT, each carries
/// its own evidence anchor, and both witnesses are COMPLETE.
///
/// Both `Run` (the root) and `Inner` (the routine holding the temp write and the HTTP
/// call) carry two `insert` facts on the mixed table after issue 33 — the physical one
/// anchored at `PhysWriter`'s `Rec.Insert`, the known-temp one at `Inner`'s
/// `TempRec.Insert`. This is what stops the sibling test's silence from meaning "the
/// physical fact went missing again", and it is what would catch the guards degrading
/// a witness to `terminal-not-found` (which sets `incomplete`).
///
/// **What discriminates it, measured rather than assumed.** Un-widen
/// `inherited_fact_key` and this FAILS: `Run` is left with a single `("TempRec.Insert",
/// false)` entry, the physical obligation gone. Dropping the terminal temp-class guard
/// does NOT fail it — verified, not predicted. The fingerprint query's
/// `witness.evidence.anchor` is projected from the FACT's own `witness_operation_id`,
/// which `retag` carries through, not from the BFS terminal; `project_path` strips
/// terminal hops out of the emitted `paths`, so the mis-terminal this guard prevents is
/// simply not visible on this surface. It is visible on the ordering surface, and that
/// is what the sibling test below pins.
#[test]
fn issue33_both_temp_classes_are_present_with_complete_witnesses() {
    let doc = fingerprint_json();
    let inserts = mixed_table_inserts(&doc);

    for routine in ["Run", "Inner"] {
        let facts = inserts
            .get(routine)
            .unwrap_or_else(|| panic!("{routine} carries no insert fact on {MIXED_TABLE}"));

        // Anti-degenerate: a silent `Run` must not mean "the physical fact is
        // missing again". Both obligations are present.
        let excerpts: BTreeSet<&str> = facts.iter().map(|(e, _)| e.as_str()).collect();
        assert_eq!(
            excerpts,
            BTreeSet::from(["Rec.Insert", "TempRec.Insert"]),
            "{routine}: the physical and known-temp inserts of {MIXED_TABLE} must BOTH \
             be present, each anchored at its own write, got {facts:?}"
        );

        // ...and the guards narrowed the terminal match without destroying the
        // witness (`terminal-not-found` would set `incomplete`).
        for (excerpt, incomplete) in facts {
            assert!(
                !incomplete,
                "{routine}: the witness for `{excerpt}` is INCOMPLETE — the temp-class \
                 guards must narrow the terminal match, not break it"
            );
        }
    }
}

/// ⟨A5⟩ The unmasked physical write produces NO `WRITE_PENDING_AT_EXTERNAL_IO`,
/// while the control — a physical write genuinely before the IO — still does.
///
/// `Run`/`Inner` hold `TempRec.Insert(); Client.Get(); PhysWriter()`. Nothing dirties
/// a physical write transaction before the HTTP call, so no ordering fact may exist
/// for them. `ControlRun`/`ControlInner` hold `Ctrl.Insert(); Client.Get()` on a
/// different table, which genuinely does — verified to fire on the pre-issue-33
/// engine too, so its presence here proves the harness can produce the label at all.
///
/// **Stated limit: the negative half can pass VACUOUSLY.** `labels_by_routine.get(..)
/// .unwrap_or_default()` means the `!contains` assertion on `Run`/`Inner` also holds
/// when those routines produce NO ordering facts whatsoever. D3 proves the test
/// discriminates against the specific break it exists for (drop the terminal guard
/// and `Run` gains the label), and the sibling fingerprint test covers "the physical
/// fact went missing", so the gap is narrow — but it is real. Asserting that `Run`'s
/// label set is NON-EMPTY would close the last vacuous-pass path for free, and is the
/// right follow-up edit.
#[test]
fn issue33_the_unmasked_physical_write_does_not_fire_write_pending_at_external_io() {
    let resolved = assemble_and_resolve_workspace_default(&fixture_dir())
        .unwrap_or_else(|| panic!("{FIXTURE} must resolve"));
    let name_by_stable: HashMap<&str, &str> = resolved
        .workspace
        .routines
        .iter()
        .map(|r| (r.stable_routine_id.as_str(), r.name.as_str()))
        .collect();

    let doc: Value = serde_json::from_str(&project_r4f_ordering_facts(&resolved, FIXTURE))
        .expect("ordering-facts projection is JSON");

    let mut labels_by_routine: HashMap<&str, BTreeSet<&str>> = HashMap::new();
    for entry in doc["entries"].as_array().expect("entries") {
        let stable = entry["routineId"].as_str().expect("routineId");
        let name = name_by_stable
            .get(stable)
            .unwrap_or_else(|| panic!("ordering fact for unknown routine {stable}"));
        let set = labels_by_routine.entry(name).or_default();
        for fact in entry["facts"].as_array().expect("facts") {
            set.insert(
                fact["guarantee"]["label"]
                    .as_str()
                    .expect("guarantee.label"),
            );
        }
    }

    for routine in ["Run", "Inner"] {
        let labels = labels_by_routine.get(routine).cloned().unwrap_or_default();
        assert!(
            !labels.contains("WRITE_PENDING_AT_EXTERNAL_IO"),
            "{routine}: a known-temp write before the IO and a physical write AFTER it \
             is not a pending physical write at the IO point — got {labels:?}"
        );
    }

    // The control, verified to fire on the pre-issue-33 engine.
    for routine in ["ControlRun", "ControlInner"] {
        let labels = labels_by_routine.get(routine).cloned().unwrap_or_default();
        assert!(
            labels.contains("WRITE_PENDING_AT_EXTERNAL_IO"),
            "{routine} is the CONTROL — a physical write genuinely before the IO. If it \
             stops firing, the silence asserted above proves nothing. Got {labels:?}"
        );
    }
}
