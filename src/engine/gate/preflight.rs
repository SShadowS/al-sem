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

/// A dependency the primary needs whose package could not be read: it looks
/// empty (or absent) to every other check.
fn is_unreadable(e: &crate::program::resolve::full::LedgerEntry) -> bool {
    e.unreadable.is_some() || e.found.as_ref().is_some_and(|f| f.ingest_error.is_some())
}

/// Whether the run's inputs were whole, so a finding it did not report is gone
/// rather than unseen (TLS-106, `--update-baseline`): the coverage was
/// verified, its contract held, no file was parsed with recovery, and no
/// dependency package was unreadable, unidentified or encrypted. Unknown call
/// edges, symbol-only dependencies and unbound subscriptions do NOT count: they
/// are the same in every run, and real workspaces always have some (DO: 1,142
/// unknown edges), so a rule that stopped on them would never prune.
pub fn inputs_whole(fresh: &Result<FreshCoverage, String>) -> bool {
    let Ok(fc) = fresh else {
        return false;
    };
    fc.coverage_holds
        && fc.recovered_files == 0
        && fc.unidentified_packages.is_empty()
        && fc.encrypted_packages.is_empty()
        && !fc.ledger.iter().any(is_unreadable)
}

/// A run's declared dependency apps (lowercase guids, from the ledger) by what
/// it found of them (TLS-106). An unreadable package is in none of the three:
/// it already makes the inputs not whole.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DependencyBodies {
    /// Found with source.
    pub bodied: std::collections::BTreeSet<String>,
    /// Found symbol-only.
    pub symbol_only: std::collections::BTreeSet<String>,
    /// Not found at all (its package moved away or never installed).
    pub missing: std::collections::BTreeSet<String>,
}

/// The run's [`DependencyBodies`]. A baseline update compares them with the
/// apps its file's runs had with source and found at all: a body or an app
/// lost since then stops pruning. Empty when the coverage could not be
/// verified (the inputs are then not whole anyway).
pub fn dependency_bodies(fresh: &Result<FreshCoverage, String>) -> DependencyBodies {
    let mut deps = DependencyBodies::default();
    for e in fresh.iter().flat_map(|fc| &fc.ledger) {
        let into = match &e.found {
            Some(f) if f.tier != crate::snapshot::TrustTier::SymbolOnly => &mut deps.bodied,
            Some(_) => &mut deps.symbol_only,
            None if e.unreadable.is_none() => &mut deps.missing,
            None => continue,
        };
        into.insert(e.guid.clone());
    }
    deps
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
            // Engine-switch S5.2b: a subscription that binds to no publisher
            // is a dispatch the analysis cannot follow, like an unknown edge.
            if fc.unbound_subscriptions > 0 {
                clauses.push(format!(
                    "{} unbound event subscription(s)",
                    fc.unbound_subscriptions
                ));
            }
            let unreadable: Vec<&str> = fc
                .ledger
                .iter()
                .filter(|e| is_unreadable(e))
                .map(|e| e.name.as_str())
                .collect();
            if !unreadable.is_empty() {
                clauses.push(format!(
                    "{} unreadable dependency app(s): {}",
                    unreadable.len(),
                    unreadable.join(", ")
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
    use crate::program::resolve::full::{FreshCoverage, LedgerApp, LedgerEntry};

    fn clean() -> FreshCoverage {
        FreshCoverage {
            unknown: 0,
            coverage_holds: true,
            recovered_files: 0,
            opaque_apps: vec![],
            ledger: vec![],
            unidentified_packages: vec![],
            encrypted_packages: vec![],
            unbound_subscriptions: 0,
            files_without_object: vec![],
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
            ledger: vec![entry("Broken", None, Some("bad zip"))],
            unidentified_packages: vec![],
            encrypted_packages: vec![],
            unbound_subscriptions: 3,
            files_without_object: vec![],
        };
        let pf = evaluate_preflight(&Ok(fc), false);
        assert_eq!(
            pf.message,
            "analysis coverage degraded — 1 unknown resolution edge(s), \
             coverage contract violated, 2 recovered file(s), \
             1 symbol-only dependency app(s): Dep, \
             3 unbound event subscription(s), \
             1 unreadable dependency app(s): Broken"
        );
    }

    fn entry(name: &str, found: Option<LedgerApp>, unreadable: Option<&str>) -> LedgerEntry {
        LedgerEntry {
            guid: format!("{name}-guid"),
            name: name.to_string(),
            declared_by: "Primary".to_string(),
            declared_version: "1.0.0.0".to_string(),
            ms_tier: false,
            found,
            unreadable: unreadable.map(str::to_string),
        }
    }

    /// S5.2b policy: a dependency whose symbols failed to ingest degrades; a
    /// missing or older dependency alone does not (calls into it already
    /// degrade as unknown edges; it is reported as a diagnostic instead).
    #[test]
    fn ingest_error_degrades_but_missing_or_older_alone_does_not() {
        let found = |error: Option<&str>| LedgerApp {
            version: "0.5.0.0".to_string(),
            tier: crate::snapshot::TrustTier::SymbolOnly,
            abi_objects: Some(0),
            ingest_error: error.map(str::to_string),
        };
        let quiet = FreshCoverage {
            ledger: vec![
                entry("Missing", None, None),
                entry("Old", Some(found(None)), None),
            ],
            ..clean()
        };
        assert!(!evaluate_preflight(&Ok(quiet), true).degraded);
        let broken = FreshCoverage {
            ledger: vec![entry("Bad", Some(found(Some("parse error"))), None)],
            ..clean()
        };
        let pf = evaluate_preflight(&Ok(broken), true);
        assert!(pf.degraded && pf.failed);
        assert!(pf.message.ends_with("1 unreadable dependency app(s): Bad"));
    }

    /// TLS-106: which inputs stop a baseline update from pruning. Unknown
    /// edges, symbol-only apps and unbound subscriptions do not (every real
    /// workspace has some); an unverified coverage, a broken contract, a
    /// recovered file, and an unreadable, unidentified or encrypted package do.
    ///
    /// Mutation (2026-11-20): `&& fc.unknown == 0` added to `inputs_whole` ->
    /// the unknown-edges case reads not whole (red); restored, green.
    /// Mutation (2026-11-20): `fc.recovered_files == 0 &&` removed -> the
    /// recovered-file case reads whole (red); restored, green.
    #[test]
    fn inputs_whole_ignores_stable_gaps_and_stops_on_lost_input() {
        assert!(inputs_whole(&Ok(clean())));
        let stable = FreshCoverage {
            unknown: 1142,
            opaque_apps: vec!["Base Application".into()],
            unbound_subscriptions: 3,
            ..clean()
        };
        assert!(inputs_whole(&Ok(stable)), "stable gaps do not block");
        let found = LedgerApp {
            version: "1.0.0.0".to_string(),
            tier: crate::snapshot::TrustTier::SymbolOnly,
            abi_objects: Some(0),
            ingest_error: Some("bad zip".to_string()),
        };
        let lost = [
            FreshCoverage {
                coverage_holds: false,
                ..clean()
            },
            FreshCoverage {
                recovered_files: 1,
                ..clean()
            },
            FreshCoverage {
                unidentified_packages: vec!["x.app: no manifest".into()],
                ..clean()
            },
            FreshCoverage {
                encrypted_packages: vec![".alpackages/x.app".into()],
                ..clean()
            },
            FreshCoverage {
                ledger: vec![entry("Gone", None, Some("unreadable"))],
                ..clean()
            },
            FreshCoverage {
                ledger: vec![entry("Bad", Some(found), None)],
                ..clean()
            },
        ];
        for (i, fc) in lost.into_iter().enumerate() {
            assert!(!inputs_whole(&Ok(fc)), "case {i} must not be whole");
        }
        assert!(!inputs_whole(&Err("boom".into())), "unverified");
    }

    /// TLS-106 (review r2 and r3, sol): a declared dependency with source is
    /// bodied, one found symbol-only is symbol-only, one not found at all (the
    /// package moved away) is missing; an unreadable one is none of them.
    ///
    /// Mutation (2026-11-20): the `None if e.unreadable.is_none()` arm made a
    /// no-op -> the missing app is not unbodied (red); restored, green.
    /// Mutation (2026-11-20, round 3): that arm filed under `symbol_only` ->
    /// `missing` is empty (red); restored, green.
    #[test]
    fn dependency_bodies_counts_missing_as_unbodied() {
        let found = |tier| LedgerApp {
            version: "1.0.0.0".to_string(),
            tier,
            abi_objects: Some(1),
            ingest_error: None,
        };
        let mut sourced = entry(
            "Sourced",
            Some(found(crate::snapshot::TrustTier::EmbeddedSource)),
            None,
        );
        sourced.guid = "s".into();
        let mut symbols = entry(
            "Symbols",
            Some(found(crate::snapshot::TrustTier::SymbolOnly)),
            None,
        );
        symbols.guid = "y".into();
        let mut missing = entry("Missing", None, None);
        missing.guid = "m".into();
        let mut broken = entry("Broken", None, Some("bad zip"));
        broken.guid = "b".into();
        let fc = FreshCoverage {
            ledger: vec![sourced, symbols, missing, broken],
            ..clean()
        };
        let deps = dependency_bodies(&Ok(fc));
        let list = |s: &std::collections::BTreeSet<String>| s.iter().cloned().collect::<Vec<_>>();
        assert_eq!(list(&deps.bodied), ["s"]);
        assert_eq!(list(&deps.symbol_only), ["y"]);
        assert_eq!(list(&deps.missing), ["m"]);
        assert_eq!(
            dependency_bodies(&Err("boom".into())),
            DependencyBodies::default()
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
