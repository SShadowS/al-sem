//! Dependency-coverage preflight — port of al-sem `src/cli/preflight.ts`, now
//! rewired to consume the FRESH resolver's own coverage status
//! ([`crate::program::resolve::full::FreshCoverage`]) instead of the legacy
//! L3-derived `unresolved_callsites`/`opaque_apps` pair. See
//! `docs/superpowers/specs/` (the preflight-fresh-coverage plan) for the design.
//!
//! `degraded` = the fresh resolver's run cannot vouch for a complete analysis
//! cone. Any of the following independently degrades:
//!   - one or more TRUE unknown resolution edges (`FreshCoverage::unknown > 0`),
//!   - the resolve run's own coverage contract was violated
//!     (`!FreshCoverage::coverage_holds`),
//!   - one or more files were parsed with recovery
//!     (`FreshCoverage::recovered_files > 0` — the IR may have dropped content, so
//!     `unknown == 0` does not prove completeness over them), OR
//!   - one or more dependency apps in the primary's reachable closure were only
//!     available as opaque symbol packages (`FreshCoverage::opaque_apps` non-empty).
//!
//! A THIRD, first-class state exists alongside "verified clean" and "degraded":
//! **could-not-verify** — the fresh resolve pipeline itself failed to run (`fresh`
//! is `Err`). This is never silently folded into "clean": it always degrades (and
//! fails when `required`), carries its own message, and is surfaced separately via
//! `verify_error` so a caller can distinguish "the instrument ran and found holes"
//! from "the instrument itself never produced an answer".
//!
//! `failed` = `degraded && required` (the `--require-dependencies` flag). Default is
//! fail-OPEN: always surface the signal to stderr, never block by default.

use crate::program::resolve::full::FreshCoverage;

/// `PreflightResult` (al-sem `preflight.ts`, extended with the could-not-verify state).
pub struct PreflightResult {
    pub degraded: bool,
    pub failed: bool,
    pub message: String,
    /// The fresh resolver's `unknown` edge count (0 when `verify_error.is_some()`).
    pub unknown_edges: usize,
    /// Symbol-only dependency app names, already sorted (0-length when
    /// `verify_error.is_some()`).
    pub opaque_apps: Vec<String>,
    /// `Some(e)` when the fresh coverage pipeline itself failed to run — the
    /// could-not-verify state. `None` whenever `fresh` was `Ok`, degraded or not.
    pub verify_error: Option<String>,
}

/// `evaluatePreflight(coverage, required)`, rewired onto [`FreshCoverage`].
///
/// `fresh` is the fresh resolver's own coverage status for this run
/// (`crate::program::resolve::full::build_program_with_coverage`, computed on
/// every run; there is no verdict cache) — `Err(e)` when the pipeline
/// itself could not produce one (could-not-verify, handled first and separately
/// below, never laundered into "clean").
pub fn evaluate_preflight(
    fresh: &Result<FreshCoverage, String>,
    required: bool,
) -> PreflightResult {
    match fresh {
        Err(e) => PreflightResult {
            degraded: true,
            failed: required,
            message: format!("coverage could not be verified: {e}"),
            unknown_edges: 0,
            opaque_apps: vec![],
            verify_error: Some(e.clone()),
        },
        Ok(fc) => {
            let mut clauses: Vec<String> = Vec::new();
            if fc.unknown > 0 {
                clauses.push(format!("{} unknown resolution edge(s)", fc.unknown));
            }
            if !fc.coverage_holds {
                clauses.push("coverage contract violated".to_string());
            }
            if fc.recovered_files > 0 {
                clauses.push(format!("{} recovered file(s)", fc.recovered_files));
            }
            if !fc.opaque_apps.is_empty() {
                clauses.push(format!(
                    "{} symbol-only dependency app(s): {}",
                    fc.opaque_apps.len(),
                    fc.opaque_apps.join(", ")
                ));
            }
            let degraded = !clauses.is_empty();
            let message = if degraded {
                format!("analysis coverage degraded — {}", clauses.join(", "))
            } else {
                "resolution coverage verified".to_string()
            };
            PreflightResult {
                degraded,
                failed: degraded && required,
                message,
                unknown_edges: fc.unknown,
                opaque_apps: fc.opaque_apps.clone(),
                verify_error: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::resolve::full::FreshCoverage;

    fn clean() -> FreshCoverage {
        FreshCoverage {
            unknown: 0,
            coverage_holds: true,
            recovered_files: 0,
            opaque_apps: vec![],
            ledger: vec![],
            unidentified_packages: vec![],
        }
    }

    #[test]
    fn clean_verified() {
        let pf = evaluate_preflight(&Ok(clean()), true);
        assert!(!pf.degraded && !pf.failed);
        assert_eq!(pf.message, "resolution coverage verified");
    }

    #[test]
    fn unknown_edges_degrade_failed_only_when_required() {
        let fc = FreshCoverage {
            unknown: 3,
            ..clean()
        };
        let pf = evaluate_preflight(&Ok(fc.clone()), false);
        assert!(pf.degraded && !pf.failed);
        assert_eq!(
            pf.message,
            "analysis coverage degraded — 3 unknown resolution edge(s)"
        );
        let pf = evaluate_preflight(&Ok(fc), true);
        assert!(pf.failed);
    }

    #[test]
    fn contract_violation_never_reports_clean() {
        let fc = FreshCoverage {
            coverage_holds: false,
            ..clean()
        };
        let pf = evaluate_preflight(&Ok(fc), false);
        assert!(pf.degraded);
        assert_eq!(
            pf.message,
            "analysis coverage degraded — coverage contract violated"
        );
    }

    #[test]
    fn recovered_files_degrade() {
        let fc = FreshCoverage {
            recovered_files: 2,
            ..clean()
        };
        let pf = evaluate_preflight(&Ok(fc), false);
        assert_eq!(
            pf.message,
            "analysis coverage degraded — 2 recovered file(s)"
        );
    }

    #[test]
    fn opaque_only_degrades_with_sorted_names() {
        let fc = FreshCoverage {
            opaque_apps: vec!["A App".into(), "B App".into()],
            ..clean()
        };
        let pf = evaluate_preflight(&Ok(fc), true);
        assert!(pf.degraded && pf.failed);
        assert_eq!(
            pf.message,
            "analysis coverage degraded — 2 symbol-only dependency app(s): A App, B App"
        );
    }

    #[test]
    fn all_signals_retained_in_fixed_order() {
        let fc = FreshCoverage {
            unknown: 1,
            coverage_holds: false,
            recovered_files: 2,
            opaque_apps: vec!["Dep".into()],
            ledger: vec![],
            unidentified_packages: vec![],
        };
        let pf = evaluate_preflight(&Ok(fc), false);
        assert_eq!(
            pf.message,
            "analysis coverage degraded — 1 unknown resolution edge(s), \
             coverage contract violated, 2 recovered file(s), \
             1 symbol-only dependency app(s): Dep"
        );
    }

    #[test]
    fn could_not_verify_is_first_class_and_never_silent() {
        let pf = evaluate_preflight(&Err("snapshot build failed: boom".into()), false);
        assert!(pf.degraded && !pf.failed);
        assert_eq!(
            pf.message,
            "coverage could not be verified: snapshot build failed: boom"
        );
        assert_eq!(
            pf.verify_error.as_deref(),
            Some("snapshot build failed: boom")
        );
        let pf = evaluate_preflight(&Err("boom".into()), true);
        assert!(pf.failed);
    }
}
