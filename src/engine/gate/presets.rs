//! Detector presets and the `analyze` command's detector selection.
//!
//! The default set is NOT listed here: `analyze` with no `--detector`/`--preset`
//! runs the registry's `default_detectors()` (`engine::l5::detectors`), the one
//! place that decides which detectors are default and which are opt-in.
//! Presets live in `PRESETS`; a preset may name opt-in detectors (the preset IS
//! the explicit opt-in for them).

use crate::engine::l5::detectors::registered_detectors;
use crate::engine::l5::registry::Detector;

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

/// The `business-rules` preset (intent evidence, spec §4.5): the checks that
/// compare a write with the rules the rest of the code applies. Its one member
/// is opt-in (the preset IS the explicit opt-in for it).
pub const PRESET_BUSINESS_RULES: &[&str] = &["d65-guard-bypass"];

/// Every preset: name -> member detector names. The ONLY place a preset is
/// defined; `resolve_preset` and `preset_names` both read it.
pub const PRESETS: &[(&str, &[&str])] = &[
    ("transaction-integrity", PRESET_TRANSACTION_INTEGRITY),
    ("bcquality", PRESET_BCQUALITY),
    ("business-rules", PRESET_BUSINESS_RULES),
];

/// Known preset names, in `PRESETS` order (CLI surface + error messages).
pub fn preset_names() -> Vec<&'static str> {
    PRESETS.iter().map(|(n, _)| *n).collect()
}

/// Resolve a preset name to its detector-name list. `Err` on an unknown preset.
pub fn resolve_preset(name: &str) -> Result<Vec<String>, String> {
    PRESETS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, members)| members.iter().map(|s| s.to_string()).collect())
        .ok_or_else(|| {
            format!(
                "Unknown preset '{name}'. Known: {}",
                preset_names().join(", ")
            )
        })
}

/// All registered `Detector`s whose name is in `names`, preserving the registry's
/// order. A name not found in the registry is an error (mirrors al-sem's
/// `resolveDetectorsByName` / `resolvePreset` throwing on an unknown detector).
pub fn select_detectors(names: &[String]) -> Result<Vec<Detector>, String> {
    let all = registered_detectors();
    let mut out: Vec<Detector> = Vec::new();
    for name in names {
        match all.iter().find(|d| &d.name == name) {
            Some(d) => out.push(d.clone()),
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
/// - neither → the registry's `default_detectors()`.
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
    Ok(crate::engine::l5::detectors::default_detectors())
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

    /// Issue #68 (T1, selector): the no-flag branch of `resolve_analyze_detectors`
    /// (no `--detector`/`--preset`) returns `default_detectors()`, same names in
    /// the same order. It checks only that branch, not which detectors are
    /// default: membership is guarded by T1b
    /// (`detectors::tests::default_and_opt_in_membership`), and the USE (what
    /// `analyze` and the other commands actually run) by T3
    /// (`tests/cli/cli_envelope_detector_set.rs`).
    #[test]
    fn analyze_default_is_registry_default() {
        let selected: Vec<String> = resolve_analyze_detectors(None, None)
            .unwrap()
            .into_iter()
            .map(|d| d.name)
            .collect();
        let registry: Vec<String> = crate::engine::l5::detectors::default_detectors()
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert_eq!(selected, registry);
    }

    /// Issue #68 (T2): every member of every preset is a registered detector.
    /// Walks `PRESETS`, the only place a preset can be defined, so a new
    /// preset is covered without editing this test.
    #[test]
    fn every_preset_member_is_registered() {
        let registered: std::collections::HashSet<String> =
            registered_detectors().into_iter().map(|d| d.name).collect();
        for (preset, members) in PRESETS {
            for m in *members {
                assert!(
                    registered.contains(*m),
                    "{m} not registered (preset {preset})"
                );
            }
        }
        let keys: Vec<&str> = PRESETS.iter().map(|(n, _)| *n).collect();
        assert_eq!(preset_names(), keys);
        for (preset, members) in PRESETS {
            assert_eq!(
                resolve_preset(preset).unwrap(),
                members.to_vec(),
                "resolve_preset({preset}) != PRESETS entry"
            );
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
    }

    #[test]
    fn preset_resolves_business_rules() {
        assert_eq!(
            resolve_preset("business-rules").unwrap(),
            ["d65-guard-bypass"]
        );
    }
}
