//! Context-server design r3 §4.2 / r4 §11 (stage 1, T-phase-alloc): the
//! owned harvest arm of the phase functions. The borrowed phase path these
//! tests once drove through a test-only `Prepared` is the resident revision
//! now (`engine::resident::revision`, whose tests pin T-order and
//! T-prepared on real artifacts).

use std::collections::BTreeSet;
use std::sync::Arc;

use super::*;
use crate::engine::l4::analysis::build_l4;
use crate::engine::l5::detector_context::{CapsInput, CoreInput, L4Input};
use crate::program::inputs::{WorkspaceInputs, WorkspaceNeeds};

/// The binary's read for digest and prove (`FINGERPRINT_ROWS`).
const READ: WorkspaceNeeds = WorkspaceNeeds {
    build_only: false,
    fingerprint_rows: true,
};

fn r0(ws: &str) -> Arc<WorkspaceInputs> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus")
        .join(ws);
    WorkspaceInputs::read(&path, &READ)
}

/// The det's core-summary harvest only (the fields a drained
/// `CoreSummaries` empties), comparable across harvests.
fn core_harvest(det: &DetectorArtifacts) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
    let unc = det
        .uncertainties_by_node
        .iter()
        .map(|(id, set)| {
            let values: Vec<&_> = det
                .uncertainties
                .elements(*set)
                .iter()
                .map(|u| det.uncertainties.value(*u))
                .collect();
            (id.clone(), format!("{values:?}"))
        })
        .collect();
    let roles = det
        .parameter_roles_by_routine
        .iter()
        .map(|(id, r)| (id.clone(), format!("{r:?}")))
        .collect();
    (unc, roles)
}

/// The payload a lent harvest clones, cloned the same way: each distinct
/// id's direct facts and coverage (when it has direct facts) and each
/// distinct id's core uncertainties and parameter roles, plus the roles of
/// ids no row reaches.
fn clone_payloads(model: &Model, caps: &CapabilityAnalysis, core: &CoreSummaries) -> usize {
    let mut kept = 0usize;
    let mut seen = std::collections::HashSet::new();
    for r in &model.workspace.routines {
        if !seen.insert(r.id.as_str()) {
            continue;
        }
        if let Some(d) = caps.direct_full.get(&r.id) {
            let pair = (d.clone(), caps.coverage.get(&r.id).cloned());
            kept += pair.0.len();
            std::hint::black_box(pair);
        }
        if let Some(s) = core.0.get(&r.id) {
            let pair = (s.uncertainties.clone(), s.parameter_roles.clone());
            kept += pair.0.len() + pair.1.len();
            std::hint::black_box(pair);
        }
    }
    for (id, s) in &core.0 {
        if !seen.contains(id.as_str()) && !s.parameter_roles.is_empty() {
            std::hint::black_box((id.clone(), s.parameter_roles.clone()));
        }
    }
    kept
}

/// T-phase-alloc (r4 §11, as corrected by review r3 finding 14): the OWNED
/// harvest arm (the one-shot path's) moves the composition's payloads and
/// drains the core summaries; it never copies them. Measured with the lib's
/// thread-gated counting allocator, on one thread: the lent harvest's
/// allocations exceed the owned harvest's by AT LEAST the payload clones
/// themselves (measured by cloning exactly what the lent arm clones), in
/// bytes and in count. The intended `Arc` wrapper is counted explicitly: the
/// owned harvest moves the composition's ONE `Arc<ConeDerivedStore>` into the
/// artifacts (the same allocation, no new wrapper, no copy); the lent one
/// shares it (two handles, one allocation).
///
/// DISCRIMINATION: cloning `direct_full` in the owned arm (`get(..).cloned()`
/// in place of `remove`) raises the owned bytes by the direct facts' clone,
/// and the byte margin falls below the payload ("owned 48498 B, lent 50670
/// B, payload 14897 B: the owned arm copied"; unbroken: owned 35773 B).
#[test]
fn owned_harvest_moves_payloads_and_adds_only_the_counted_arc() {
    let inputs = r0("ws-d37");
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    pool.install(|| {
        let target = AnalysisTarget::build(&inputs, true, "t-phase-alloc").unwrap();
        let model = target.resolved();
        let rel = relation_of(&target);
        let scope = || ConeScope {
            derived: true,
            raw: WitnessScope::None,
        };
        let (owned_caps, _) = compose_cone(model, &rel, scope());
        let (lent_caps, _) = compose_cone(model, &rel, scope());
        let req = || L4Request {
            extra_diagnostics: vec![],
            control: None,
        };
        let (owned_l4, owned_core) = build_l4(model, &rel, req(), None);
        let (lent_l4, lent_core) = build_l4(model, &rel, req(), None);
        let mask = digest_plan(
            crate::engine::l5::detectors::default_detectors(),
            BTreeSet::new(),
            false,
        )
        .union();
        assert!(mask & substrate::CORE_SUMMARIES != 0 && mask & substrate::SUMMARIES != 0);

        let (kept, n_payload, b_payload) =
            crate::test_alloc::measure(|| clone_payloads(model, &lent_caps, &lent_core));
        assert!(
            kept > 0 && b_payload > 0,
            "precondition: the fixture has payloads to copy"
        );

        let derived_ptr = Arc::as_ptr(&owned_caps.derived);
        let ((owned, _), n_owned, b_owned) = crate::test_alloc::measure(|| {
            dc::harvest_detector_artifacts(
                model,
                mask,
                &rel,
                Some(CapsInput::Owned(owned_caps)),
                L4Input::Solved(&owned_l4, CoreInput::Owned(owned_core)),
                None,
            )
        });
        let ((lent, _), n_lent, b_lent) = crate::test_alloc::measure(|| {
            dc::harvest_detector_artifacts(
                model,
                mask,
                &rel,
                Some(CapsInput::Borrowed(&lent_caps)),
                L4Input::Solved(&lent_l4, CoreInput::Borrowed(&lent_core)),
                None,
            )
        });
        // The counted Arc: moved, never re-wrapped or copied.
        assert_eq!(Arc::as_ptr(&owned.cone_derived), derived_ptr);
        assert_eq!(Arc::strong_count(&owned.cone_derived), 1);
        assert!(Arc::ptr_eq(&lent.cone_derived, &lent_caps.derived));
        assert_eq!(Arc::strong_count(&lent_caps.derived), 2);
        // No payload copy in the owned arm.
        assert!(
            b_lent >= b_owned + b_payload,
            "owned {b_owned} B, lent {b_lent} B, payload {b_payload} B: the owned arm copied"
        );
        assert!(
            n_lent >= n_owned + n_payload,
            "owned {n_owned}, lent {n_lent}, payload {n_payload} allocations"
        );
        eprintln!(
            "[t-phase-alloc] owned={n_owned} allocs/{b_owned} B lent={n_lent}/{b_lent} B \
             payload={n_payload}/{b_payload} B"
        );
        assert_eq!(
            core_harvest(&owned),
            core_harvest(&lent),
            "same harvest either way"
        );
    });
}
