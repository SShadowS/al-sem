//! #277 D12: how many batches run at once. Every job kind carries its
//! measured peak (`KIND_PEAK_KIB`); a run's `W` is as many workers as fit the
//! E16 budget at its heaviest kind, at most 4, at most 2 with `policy check`.
//!
//! Limit L1: the budget holds for manifests no heavier than the measured
//! shapes (spec D12); the rows are CDO's, so a larger workspace needs them
//! re-measured.

use super::job::ViewUniverse;
use crate::engine::perf_trace as pt;

/// E16's budget: the shared cgroup's 32 GiB less 4 GiB (12A-6), in KiB.
pub const E16_BUDGET_KIB: u64 = 29_360_128;

/// A job kind: its command (`"events fanout"`, …) and its view's universe.
pub type JobKind = (&'static str, ViewUniverse);

use ViewUniverse::{CrossApp as X, SingleApp as S};

/// 256 MiB in KiB: every row is rounded up to a multiple of it.
const STEP: u64 = 262_144;

/// policy check cross-app, the largest row: max(`p` 8,279,568, `U`
/// 10,056,180 KiB) → 9.75 GiB.
const POLICY_X: u64 = 39 * STEP;

/// One row per (command, universe), rounded up to the next 256 MiB: each the
/// max of the kind's standalone CDO `VmHWM` median and the median peak of
/// every measured shape whose heaviest standalone member it is (spec D12).
/// Numbers: Task 12's bench, `docs/measurements/2026-10-25-issue-277-160-step12.md`
/// Part 4b (release-fast, `maxrss.py`, H medians over 3–5 rounds; the
/// letters are its manifests). A kind with no row takes the largest row.
pub static KIND_PEAK_KIB: &[(JobKind, u64)] = &[
    // Part 4b: max(`a` 3,162,240, `aa`, `AS`) KiB → 3.25 GiB.
    (("analyze", X), 13 * STEP),
    // Part 4b: `s` 727,772 KiB → 0.75 GiB.
    (("analyze", S), 3 * STEP),
    // Part 4b: max(`d`, `h`, `AD`, `ADF`, `O`, `Up` 3,711,548) KiB; #246
    // (`docs/measurements/2026-10-25-issue-246-digest-include-reads.md`): `Up`
    // with `--include-reads` on d and h, median 4,062,196 (max 4,075,056) KiB
    // → 4.0 GiB. The row is per kind, not per flag, so it covers both.
    (("digest", X), 16 * STEP),
    // Part 4b: `ds` 747,896 KiB → 0.75 GiB.
    (("digest", S), 3 * STEP),
    (("policy check", X), POLICY_X),
    // Part 4b: `ps` 727,824 KiB → 0.75 GiB.
    (("policy check", S), 3 * STEP),
    // Part 4b: `fr` 3,144,176, `fi` 3,146,120 KiB → 3.25 GiB.
    (("fingerprint", X), 13 * STEP),
    // Part 4b: max(`f` 1,467,800, `frs`, `fis`) KiB → 1.5 GiB.
    (("fingerprint", S), 6 * STEP),
    // Part 4b: `pr` 3,155,308 KiB → 3.25 GiB.
    (("prove", X), 13 * STEP),
    // Part 4b: `prs` 731,948 KiB → 0.75 GiB.
    (("prove", S), 3 * STEP),
    // Part 4b: max(`df`, `dfs` 1,473,272) KiB → 1.5 GiB.
    (("diff", S), 6 * STEP),
    // Part 4b: `ev` 3,184,072 KiB → 3.25 GiB.
    (("events fanout", X), 13 * STEP),
    // Part 4b: `evs` 719,668 KiB → 0.75 GiB.
    (("events fanout", S), 3 * STEP),
    // Part 4b: `ec` 3,161,128 KiB → 3.25 GiB.
    (("events chains", X), 13 * STEP),
    // Part 4b: `ecs` 725,472 KiB → 0.75 GiB.
    (("events chains", S), 3 * STEP),
    // Part 4b: `qt` 3,167,556 KiB → 3.25 GiB.
    (("query touches", X), 13 * STEP),
    // Part 4b: `qts` 735,544 KiB → 0.75 GiB.
    (("query touches", S), 3 * STEP),
    // Part 4b: `qe` 3,166,676 KiB → 3.25 GiB.
    (("query effects", X), 13 * STEP),
    // Part 4b: `qes` 738,864 KiB → 0.75 GiB.
    (("query effects", S), 3 * STEP),
    // Part 4b: `g` 2,596,904 KiB → 2.5 GiB.
    (("graph", X), 10 * STEP),
];

/// `P`: the largest row of any of `kinds` (a kind with no row takes the
/// table's largest; no kind at all, 0).
pub fn peak_kib(kinds: &[JobKind], table: &[(JobKind, u64)]) -> u64 {
    let largest = table.iter().map(|(_, kib)| *kib).max().unwrap_or(0);
    let row = |k: &JobKind| table.iter().find(|(r, _)| r == k).map_or(largest, |r| r.1);
    kinds.iter().map(row).max().unwrap_or(0)
}

/// `W = max(1, min(batches, 4, ⌊E16_BUDGET_KIB / P⌋))`, then at most 2 when
/// the manifest has a `policy check` (the cap never raises `W`).
pub fn worker_count(
    kinds: &[JobKind],
    batches: usize,
    has_policy: bool,
    table: &[(JobKind, u64)],
) -> usize {
    let fit = E16_BUDGET_KIB.checked_div(peak_kib(kinds, table));
    let fit = fit.map_or(usize::MAX, |n| usize::try_from(n).unwrap_or(usize::MAX));
    let w = batches.min(4).min(fit).max(1);
    if has_policy { w.min(2) } else { w }
}

/// Whether the run has a `policy check` job (D12's cap of 2); no other
/// command caps `W` (R10-3).
pub fn has_policy_check(kinds: &[JobKind]) -> bool {
    kinds.iter().any(|(command, _)| *command == "policy check")
}

/// The run's `W` over the committed table, with its `batch.workers` instant
/// and, when one job's peak alone exceeds the budget, the one stderr line.
pub fn admit(kinds: &[JobKind], batches: usize) -> usize {
    let has_policy = has_policy_check(kinds);
    let p = peak_kib(kinds, KIND_PEAK_KIB);
    if p > E16_BUDGET_KIB {
        let _ = super::output::write_stderr(
            b"al-sem batch: the largest measured job peak exceeds the 28 GiB budget; running one worker\n",
        );
    }
    let w = worker_count(kinds, batches, has_policy, KIND_PEAK_KIB);
    pt::instant_lazy(
        "batch",
        "batch.workers",
        || serde_json::json!({ "w": w, "p_kib": p, "policy_cap": has_policy }),
    );
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    const K: JobKind = ("digest", X);

    /// A one-row table whose row fits the budget `n` times.
    fn fits(n: u64) -> Vec<(JobKind, u64)> {
        let p = E16_BUDGET_KIB.checked_div(n).unwrap_or(E16_BUDGET_KIB + 1);
        vec![(K, p)]
    }

    /// T17 (lib, injected table): ⌊budget / P⌋ = 0, 1, 3, ≥ 4 with enough
    /// batches → 1, 1, 3, 4 without policy, 1, 1, 2, 2 with.
    #[test]
    fn worker_count_over_an_injected_table() {
        for (n, alone, policy) in [(0, 1, 1), (1, 1, 1), (3, 3, 2), (4, 4, 2), (7, 4, 2)] {
            let table = fits(n);
            assert_eq!(E16_BUDGET_KIB / table[0].1, n, "precondition: fits {n}");
            assert_eq!(worker_count(&[K], 8, false, &table), alone, "fits {n}");
            assert_eq!(
                worker_count(&[K], 8, true, &table),
                policy,
                "fits {n}, policy"
            );
        }
        assert_eq!(
            worker_count(&[K], 2, false, &fits(7)),
            2,
            "at most the batches"
        );
    }

    /// The spec's bench manifests as job kinds (§7.1).
    fn kinds(names: &str) -> Vec<JobKind> {
        names
            .chars()
            .flat_map(|c| match c {
                'a' => vec![("analyze", X)],
                's' => vec![("analyze", S)],
                'd' | 'h' => vec![("digest", X)],
                'f' => vec![("fingerprint", S)],
                'p' => vec![("policy check", X)],
                'e' => vec![("events fanout", X)],
                'q' => vec![("query touches", X)],
                'F' => vec![("fingerprint", X)],
                'P' => vec![("prove", S)],
                'c' => vec![("events chains", X)],
                'Q' => vec![("query effects", X)],
                _ => unreachable!("no job {c}"),
            })
            .collect()
    }

    fn committed(names: &str) -> usize {
        let kinds = kinds(names);
        worker_count(&kinds, 4, has_policy_check(&kinds), KIND_PEAK_KIB)
    }

    /// R10-3: only `policy check` caps `W` at 2, in either universe.
    #[test]
    fn only_policy_check_caps() {
        assert!(has_policy_check(&[("analyze", X), ("policy check", S)]));
        assert!(!has_policy_check(&[
            ("events chains", X),
            ("query effects", S)
        ]));
        assert_eq!(committed("adf"), 4, "no policy check: no cap");
    }

    /// T17 (lib, the committed table, O16): a shape's peak raised only its
    /// heaviest member's row, so `AD`, `ADF`, `O` ×4 run at 4 and `U` at 2.
    #[test]
    fn worker_count_over_the_committed_table() {
        assert_eq!(committed("ad"), 4, "AD");
        assert_eq!(committed("adf"), 4, "ADF");
        assert_eq!(committed("shfa"), 4, "O");
        assert_eq!(committed("adhpeq"), 2, "U");
    }

    /// T17's `U−p` case: with Step 1's events and query rows, `U` without
    /// policy runs at 4.
    #[test]
    fn worker_count_u_without_policy() {
        assert_eq!(committed("adheq"), 4, "U−p");
    }

    /// Task 12: the kinds that had placeholders or no row now carry their
    /// measured rows, so a case-B manifest (cross-app fingerprint, single-app
    /// prove) and the chains/effects kinds run at 4; with policy, still 2.
    #[test]
    fn worker_count_measured_case_b_rows() {
        assert_eq!(committed("FP"), 4, "case B");
        assert_eq!(committed("cQ"), 4, "events chains, query effects");
        assert_eq!(committed("FPcQp"), 2, "with policy");
        assert_eq!(peak_kib(&[("fingerprint", X)], KIND_PEAK_KIB), 13 * STEP);
        assert_eq!(peak_kib(&[("prove", S)], KIND_PEAK_KIB), 3 * STEP);
        assert_eq!(peak_kib(&[("events chains", X)], KIND_PEAK_KIB), 13 * STEP);
        assert_eq!(peak_kib(&[("query effects", S)], KIND_PEAK_KIB), 3 * STEP);
    }

    /// #246: digest's cross-app row covers `--include-reads` (its `Up` median
    /// 4,062,196 KiB), and every T17 shape keeps its `W`: 16 steps still fit
    /// the budget 7 times.
    #[test]
    fn digest_row_covers_include_reads() {
        assert_eq!(peak_kib(&[("digest", X)], KIND_PEAK_KIB), 16 * STEP);
        assert_eq!(E16_BUDGET_KIB / (16 * STEP), 7);
        assert_eq!(committed("adheq"), 4, "Up");
    }

    #[test]
    fn rows_are_whole_steps_and_one_per_kind() {
        for (i, (k, kib)) in KIND_PEAK_KIB.iter().enumerate() {
            assert_eq!(kib % STEP, 0, "{k:?} rounded to 256 MiB");
            assert!(
                !KIND_PEAK_KIB[..i].iter().any(|(r, _)| r == k),
                "{k:?} twice"
            );
        }
    }

    #[test]
    fn a_kind_with_no_row_takes_the_largest() {
        // Every command has rows; `graph` single-app is not a job kind.
        assert!(!KIND_PEAK_KIB.iter().any(|(k, _)| *k == ("graph", S)));
        assert_eq!(peak_kib(&[("graph", S)], KIND_PEAK_KIB), POLICY_X);
        assert_eq!(
            KIND_PEAK_KIB.iter().map(|(_, kib)| *kib).max(),
            Some(POLICY_X),
            "policy check cross-app is the largest row"
        );
        assert_eq!(peak_kib(&[], KIND_PEAK_KIB), 0);
    }
}
