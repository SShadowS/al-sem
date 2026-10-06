//! Detector presets + selection — port of al-sem `src/detectors/presets.ts` and the
//! `analyze` command's detector-selection logic in `src/cli/index.ts`.
//!
//! `DEFAULT_DETECTOR_NAMES` mirrors al-sem `registry.ts` `DEFAULT_DETECTORS`
//! (the 39 non-opt-in detectors). `OPT_IN_DETECTOR_NAMES` mirrors `OPT_IN_DETECTORS`.
//! `resolve_preset("transaction-integrity")` returns the same 7-name list al-sem's
//! `PRESET_NAMES["transaction-integrity"]` carries (some members are opt-in — the
//! preset IS the explicit opt-in for them).

use crate::engine::l5::detectors::registered_detectors;
use crate::engine::l5::registry::Detector;

/// al-sem `DEFAULT_DETECTORS` names, in al-sem's declaration order. The gate only
/// uses the NAME SET to select the default subset; output order is governed by the
/// post-detector sort, so the order of this list is not load-bearing.
pub const DEFAULT_DETECTOR_NAMES: &[&str] = &[
    "d1-db-op-in-loop",
    "d2-event-fanout-in-loop",
    "d3-missing-setloadfields",
    "d4-repeated-lookup-in-loop",
    "d5-set-based-opportunity",
    "d7-recursive-event-expansion",
    "d8-commit-in-transaction",
    "d9-transaction-span-summary",
    "d10-self-modifying-loop",
    "d11-modify-without-get",
    "d12-dead-integration-event",
    "d13-cross-app-internal-call",
    "d14-dead-routine",
    "d16-obsolete-routine-call",
    "d18-constant-filter-in-loop",
    "d19-unused-parameter",
    "d20-unreachable-after-exit",
    "d21-read-without-load",
    "d22-flowfield-without-calcfields",
    "d29-subscriber-modify-on-event-record",
    "d32-constant-boolean-parameter",
    "d33-unfiltered-bulk-write",
    "d34-commit-in-loop",
    "d35-commit-in-event-subscriber",
    "d36-late-setloadfields",
    "d37-validate-without-persist",
    "d38-subscriber-to-obsolete-event",
    "d39-record-left-dirty-across-chain",
    "d41-transitive-filter-loss",
    "d42-cross-call-wrong-setloadfields",
    "d43-event-ishandled-skip",
    "d44-event-multi-subscriber-overlap",
    "d45-event-transitive-table-exposure",
    "d52-bulk-write-param-no-temp-guard",
    "d53-ignored-tryfunction-result",
    "d54-publish-in-tryfunction-cone",
    "d55-event-publish-in-loop",
    "d56-clone-before-write-in-loop",
    "d57-singleinstance-growing-state",
    "d58-query-filter-after-open",
    "d59-integrationevent-var-boolean-guard",
    "d60-upgrade-loop-should-be-datatransfer",
];

/// al-sem `OPT_IN_DETECTORS` names — not in the default registry. Surfaced only by
/// `--detector <name>` or a preset that lists them.
pub const OPT_IN_DETECTOR_NAMES: &[&str] = &[
    "d40-transitive-load-missing",
    "d46-commit-in-lifecycle",
    "d47-io-unsafe-txn",
    "d48-io-in-loop",
    "d49-uncommitted-write-before-ui",
    "d50-checked-run-implicit-commit",
    "d51-retry-side-effect-duplication",
    "d17-min-version-drift",
    "d61-ishandled-bypasses-critical-write",
    "d62-telemetry-before-success",
    "d63-html-concat-injection",
    "d64-api-page-write-surface",
];

/// The `transaction-integrity` preset members — verbatim from al-sem
/// `presets.ts` `PRESET_NAMES["transaction-integrity"]`.
pub const PRESET_TRANSACTION_INTEGRITY: &[&str] = &[
    "d8-commit-in-transaction",
    "d34-commit-in-loop",
    "d35-commit-in-event-subscriber",
    "d46-commit-in-lifecycle",
    "d47-io-unsafe-txn",
    "d48-io-in-loop",
    "d49-uncommitted-write-before-ui",
];

/// The `bcquality` preset — the full BCQuality wave (d52–d64), including its
/// opt-in members (the preset IS the explicit opt-in for them).
pub const PRESET_BCQUALITY: &[&str] = &[
    "d52-bulk-write-param-no-temp-guard",
    "d53-ignored-tryfunction-result",
    "d54-publish-in-tryfunction-cone",
    "d55-event-publish-in-loop",
    "d56-clone-before-write-in-loop",
    "d57-singleinstance-growing-state",
    "d58-query-filter-after-open",
    "d59-integrationevent-var-boolean-guard",
    "d60-upgrade-loop-should-be-datatransfer",
    "d61-ishandled-bypasses-critical-write",
    "d62-telemetry-before-success",
    "d63-html-concat-injection",
    "d64-api-page-write-surface",
];

/// Known preset names (for the CLI surface + error messages).
pub const PRESET_NAMES_LIST: &[&str] = &["transaction-integrity", "bcquality"];

/// Resolve a preset name to its detector-name list. `Err` on an unknown preset
/// (mirrors al-sem `resolvePreset` throwing).
pub fn resolve_preset(name: &str) -> Result<Vec<String>, String> {
    match name {
        "transaction-integrity" => Ok(PRESET_TRANSACTION_INTEGRITY
            .iter()
            .map(|s| s.to_string())
            .collect()),
        "bcquality" => Ok(PRESET_BCQUALITY.iter().map(|s| s.to_string()).collect()),
        other => Err(format!(
            "Unknown preset '{other}'. Known: {}",
            PRESET_NAMES_LIST.join(", ")
        )),
    }
}

/// All registered `Detector`s whose name is in `names`, preserving the registry's
/// order. A name not found in the registry is an error (mirrors al-sem's
/// `resolveDetectorsByName` / `resolvePreset` throwing on an unknown detector).
pub fn select_detectors(names: &[String]) -> Result<Vec<Detector>, String> {
    let all = registered_detectors();
    let mut out: Vec<Detector> = Vec::new();
    for name in names {
        match all.iter().find(|d| &d.name == name) {
            Some(d) => out.push(Detector {
                name: d.name.clone(),
                run: d.run,
                requires: d.requires,
            }),
            None => return Err(format!("Unknown detector: {name}")),
        }
    }
    Ok(out)
}

/// Resolve the effective detector set for the `analyze` gate, mirroring the
/// selection logic in al-sem `src/cli/index.ts`:
///
/// - `--preset <name>` → `resolve_preset(name)` (mutually exclusive with `--detector`).
/// - `--detector <ids>` → the requested list; if ANY requested name is NOT in the
///   default set, the union ALL_DETECTORS would run in al-sem — but the gate selects
///   exactly the requested names (byte-equivalent: only requested-detector findings
///   reach the SARIF, and `filter_findings` then allow-lists them anyway).
/// - neither → the DEFAULT detector set.
///
/// Returns the resolved `Detector` list. `--preset` + `--detector` together is an error.
///
/// TODO(wirein): opt-in-union detector selection (--detector non-default → run ALL then
/// filter) for production-CLI fidelity — gate preset path is byte-equivalent so deferred.
/// al-sem index.ts:197-208: when `--detector` names a NON-default detector, al-sem runs
/// ALL_DETECTORS then filterFindings allow-lists to the requested set.  The gate PRESET
/// path is byte-equivalent (the preset already selects the right set), so this is NOT a
/// gate bug.  But `alsem analyze --detector <non-default-id>` would under-run vs al-sem.
/// Implement when the gate gains a production-CLI surface for opt-in detectors.
pub fn resolve_analyze_detectors(
    preset: Option<&str>,
    detector: Option<&str>,
) -> Result<Vec<Detector>, String> {
    if preset.is_some() && detector.is_some() {
        return Err(
            "--preset and --detector are mutually exclusive. Use one or the other.".to_string(),
        );
    }
    if let Some(p) = preset {
        let names = resolve_preset(p)?;
        return select_detectors(&names);
    }
    if let Some(d) = detector {
        let names: Vec<String> = d.split(',').map(|s| s.trim().to_string()).collect();
        return select_detectors(&names);
    }
    // Default detector set.
    let names: Vec<String> = DEFAULT_DETECTOR_NAMES
        .iter()
        .map(|s| s.to_string())
        .collect();
    select_detectors(&names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_resolves_transaction_integrity() {
        let names = resolve_preset("transaction-integrity").unwrap();
        assert_eq!(names, PRESET_TRANSACTION_INTEGRITY);
    }

    #[test]
    fn unknown_preset_errors() {
        assert!(resolve_preset("nope").is_err());
    }

    #[test]
    fn default_set_excludes_opt_in() {
        for opt in OPT_IN_DETECTOR_NAMES {
            assert!(
                !DEFAULT_DETECTOR_NAMES.contains(opt),
                "{opt} must not be in the default set"
            );
        }
    }

    #[test]
    fn every_default_and_opt_in_name_is_registered() {
        let all = registered_detectors();
        let registered: std::collections::HashSet<&str> =
            all.iter().map(|d| d.name.as_str()).collect();
        for n in DEFAULT_DETECTOR_NAMES
            .iter()
            .chain(OPT_IN_DETECTOR_NAMES.iter())
        {
            assert!(registered.contains(n), "{n} not registered");
        }
    }

    #[test]
    fn preset_and_detector_mutually_exclusive() {
        assert!(
            resolve_analyze_detectors(Some("transaction-integrity"), Some("d1-db-op-in-loop"))
                .is_err()
        );
    }

    #[test]
    fn preset_resolves_bcquality() {
        let names = resolve_preset("bcquality").unwrap();
        assert_eq!(names, PRESET_BCQUALITY);
        assert_eq!(names.len(), 13);
        // every member must be registered
        let all = registered_detectors();
        let registered: std::collections::HashSet<&str> =
            all.iter().map(|d| d.name.as_str()).collect();
        for n in &names {
            assert!(registered.contains(n.as_str()), "{n} not registered");
        }
    }
}
