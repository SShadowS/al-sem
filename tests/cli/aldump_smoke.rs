//! R0 Task 4 smoke test: run the identity-subset extraction on the vendored
//! ws-d2 fixture and assert the output matches the committed golden's identity
//! subset.
//!
//! Task 3.3 (al-sem parity retirement) vendored the ws-d2 fixture tree in-repo
//! (`tests/fixtures/ws-d2/`); this test no longer reads from any al-sem
//! checkout and hard-requires its inputs (no skip-gate).

use al_sem::engine::snapshot::snapshot_workspace;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Vendored ws-d2 fixture (Task 3.3; see `tests/fixtures/ws-d2/PROVENANCE.md`).
fn ws_d2_dir() -> PathBuf {
    repo_root().join("tests").join("fixtures").join("ws-d2")
}

#[test]
fn ws_d2_identity_subset_matches_golden() {
    let ws = ws_d2_dir();
    assert!(
        ws.is_dir(),
        "vendored ws-d2 fixture missing at {} (Task 3.3 vendoring)",
        ws.display()
    );

    let snap = snapshot_workspace(&ws).expect("snapshot_workspace should succeed on ws-d2");

    // Serializes cleanly as JSON.
    let json = serde_json::to_string_pretty(&snap).expect("snapshot serializes to JSON");
    let _parsed: serde_json::Value =
        serde_json::from_str(&json).expect("emitted output parses as JSON");

    // --- Objects: the three from ws-d2.golden.json with exact ids + fingerprints. ---
    let find_obj = |id: &str| {
        snap.objects
            .iter()
            .find(|o| o.stable_object_id == id)
            .unwrap_or_else(|| panic!("missing object {id}"))
    };

    let pub_obj = find_obj("22222222-d200-0000-0000-000000000002:Codeunit:64101");
    assert_eq!(pub_obj.name, "D2 Publisher");
    assert_eq!(pub_obj.kind, "Codeunit");
    assert_eq!(
        pub_obj.signature_fingerprint,
        "377fb0f90a7fd7704067c8f976cd5436ee1dcb4a57b9cd0acf61cdcaaf7b0c4a"
    );

    let sub_obj = find_obj("22222222-d200-0000-0000-000000000002:Codeunit:64102");
    assert_eq!(sub_obj.name, "D2 Subscriber");
    assert_eq!(sub_obj.kind, "Codeunit");
    assert_eq!(
        sub_obj.signature_fingerprint,
        "bfc4e34885feeb6a82dd67e03cca121cab27224b653eede5ab2160a91b209cd3"
    );

    let cust_obj = find_obj("22222222-d200-0000-0000-000000000002:Table:64100");
    assert_eq!(cust_obj.name, "Customer");
    assert_eq!(cust_obj.kind, "Table");
    assert_eq!(
        cust_obj.signature_fingerprint,
        "c89886eb4c10302d7de10838ebfff1b3c7651f9b409b89ecfd1301f9697a8999"
    );

    // --- Routines: RaiseInLoop (procedure) + OnQuietEvent (event-publisher). ---
    let find_routine = |id: &str| {
        snap.routines
            .iter()
            .find(|r| r.stable_routine_id == id)
            .unwrap_or_else(|| panic!("missing routine {id}"))
    };

    let raise = find_routine(
        "22222222-d200-0000-0000-000000000002:Codeunit:64101#299663ee14d29f43470da2f218237c42dc9923d39062c86dbc2982a454f2e0ac",
    );
    assert_eq!(raise.name, "RaiseInLoop");
    assert_eq!(raise.kind, "procedure");
    assert_eq!(raise.canonical_signature_text, "raiseinloop():");

    let on_quiet = snap
        .routines
        .iter()
        .find(|r| r.name == "OnQuietEvent")
        .expect("missing OnQuietEvent");
    assert_eq!(on_quiet.kind, "event-publisher");
}
