//! CI exit-code contract — port of al-sem `src/cli/exit-code.ts`.
//!
//! ```text
//!   0  — clean (no policy-failing findings)
//!   1  — policy-failing findings present (at/above --fail-on threshold)
//!   2  — analysis failure (parser/graph crash or error-severity diagnostic), or a
//!        failed write of analyze's report (analyze only; every other command exits
//!        1 on a write failure)
//!   3  — invalid config/usage (bad flags, unknown preset, mutual-exclusion)
//!   4  — coverage/dependency preflight failed (--require-dependencies)
//! ```
//!
//! Precedence (highest priority wins, never overwrite a higher with a lower):
//!   CONFIG_ERROR (3) > ANALYSIS_FAILURE (2) > PREFLIGHT_FAILED (4) > FINDINGS (1) > CLEAN (0)
//!
//! PREFLIGHT_FAILED (4) is a higher NUMERIC value than CONFIG_ERROR (3) but LOWER
//! priority — the ordering above is by semantic priority, not numeric value.
//!
//! `alsem batch` (#277 D2.2) exits for the RUN, not for its jobs: each job's own
//! exit (by the table above, a write failure included) is on that job's stdout
//! line.
//!
//! ```text
//!   0  — every job ran, whatever its own exit
//!   1  — a run-transport failure: the run's own stdout line write failed
//!   3  — a manifest or usage error (an argv clap rejects, a refused command, an
//!        empty manifest or batch): nothing runs
//! ```

/// `EXIT` const map (al-sem `exit-code.ts`).
pub mod exit {
    pub const CLEAN: u8 = 0;
    pub const FINDINGS: u8 = 1;
    pub const ANALYSIS_FAILURE: u8 = 2;
    pub const CONFIG_ERROR: u8 = 3;
    pub const PREFLIGHT_FAILED: u8 = 4;
}

/// Severity rank — higher is more severe (al-sem `SEV_RANK`).
fn sev_rank(sev: &str) -> u8 {
    match sev {
        "info" => 0,
        "low" => 1,
        "medium" => 2,
        "high" => 3,
        "critical" => 4,
        _ => 0,
    }
}

const VALID_SEVERITIES: &[&str] = &["critical", "high", "medium", "low", "info"];

/// `computeFindingExit(findings, failOn)` — `EXIT.FINDINGS` iff any finding is at/above
/// the `failOn` severity; else `EXIT.CLEAN`. No `failOn` → `EXIT.CLEAN` always.
pub fn compute_finding_exit<S: AsRef<str>>(severities: &[S], fail_on: Option<&str>) -> u8 {
    let Some(fail_on) = fail_on else {
        return exit::CLEAN;
    };
    let min = sev_rank(fail_on);
    if severities.iter().any(|s| sev_rank(s.as_ref()) >= min) {
        exit::FINDINGS
    } else {
        exit::CLEAN
    }
}

/// `parseFailOn` — validate a `--fail-on` string. `Ok(severity)` or `Err(usage message)`
/// (the caller maps the error to `EXIT.CONFIG_ERROR`).
pub fn parse_fail_on(input: &str) -> Result<String, String> {
    if VALID_SEVERITIES.contains(&input) {
        Ok(input.to_string())
    } else {
        Err(format!(
            "invalid --fail-on '{input}'. Expected: critical | high | medium | low | info"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_fail_on_is_clean() {
        assert_eq!(compute_finding_exit(&["critical"], None), exit::CLEAN);
    }

    #[test]
    fn at_or_above_threshold_is_findings() {
        assert_eq!(
            compute_finding_exit(&["high"], Some("high")),
            exit::FINDINGS
        );
        assert_eq!(
            compute_finding_exit(&["critical"], Some("high")),
            exit::FINDINGS
        );
    }

    #[test]
    fn below_threshold_is_clean() {
        assert_eq!(
            compute_finding_exit(&["high"], Some("critical")),
            exit::CLEAN
        );
        assert_eq!(
            compute_finding_exit(&[] as &[&str], Some("info")),
            exit::CLEAN
        );
    }

    #[test]
    fn parse_fail_on_validation() {
        assert_eq!(parse_fail_on("high").unwrap(), "high");
        assert!(parse_fail_on("bogus").is_err());
    }
}
