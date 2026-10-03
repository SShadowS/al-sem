//! Plan 1B.1: building the whole-program node graph over the real CDO snapshot
//! is panic-free and yields a deep, app-qualified node set.

// Task T0.2: shared CDO_WS/ENFORCE_CDO_WS gating helper — see
// `tests/common/cdo.rs` for why this is `#[path]`-included rather than a
// regular crate dependency (separate test-binary crates can't `use` each
// other's `mod`s).
use crate::cdo;
use cdo::cdo_ws_or_enforce;

#[test]
fn cdo_program_graph_is_app_qualified_and_panic_free() {
    let Some(ws) = cdo_ws_or_enforce() else {
        return;
    };
    let snap = al_sem::snapshot::SnapshotBuilder {
        workspace_root: ws,
        local_providers: vec![],
    }
    .build()
    .expect("snapshot");
    let g =
        al_sem::program::build_program_graph(&snap, &al_sem::program::abi_ingest::AbiCache::new());
    // Print diagnostic counts for the task report.
    let apps: std::collections::BTreeSet<_> = g.objects.iter().map(|o| o.id.app).collect();
    println!(
        "objects={} routines={} apps_spanned={}",
        g.objects.len(),
        g.routines.len(),
        apps.len()
    );
    // Deep node set across workspace + source-bearing deps.
    assert!(g.objects.len() > 500, "objects: {}", g.objects.len());
    assert!(g.routines.len() > 2000, "routines: {}", g.routines.len());
    // App-qualified: nodes span more than one app.
    assert!(
        apps.len() >= 2,
        "nodes should span multiple apps, got {}",
        apps.len()
    );
    // Deterministic: objects sorted by NodeId.
    assert!(
        g.objects
            .iter()
            .zip(g.objects.iter().skip(1))
            .all(|(a, b)| a.id <= b.id),
        "objects must be sorted by NodeId"
    );
}
