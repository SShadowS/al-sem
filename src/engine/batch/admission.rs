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

/// policy check cross-app: max(Task 1's `p` 8,280,524 KiB, step 6's 7.94 GiB,
/// Task 1's projected `U` 8,333,320 KiB) → 8 GiB. The largest known row.
const POLICY_X: u64 = 32 * STEP;

/// One row per (command, universe), rounded up to the next 256 MiB: each the
/// max of the kind's standalone CDO `VmHWM` median and the median peak of
/// every measured shape whose heaviest standalone member it is (spec D12).
/// Numbers: `.agent/issue-277/ledger.md` "Task 1 projection" (release-fast,
/// `maxrss.py`, `/work/bench/160-12/task1/reading.txt`) and spec §7.3 (step
/// 6's final bench). A kind with no row takes the largest row.
/// No row yet: events chains and query effects, cross-app and single-app.
pub static KIND_PEAK_KIB: &[(JobKind, u64)] = &[
    // `a` 3,142,400 KiB → 3 GiB.
    (("analyze", X), 12 * STEP),
    // step 6's ≈ 0.72 GB → 0.75 GiB.
    (("analyze", S), 3 * STEP),
    // max(`d` 3,146,856, `h` 3,222,396 KiB, step 6's 3.35 GiB) → 3.5 GiB.
    (("digest", X), 14 * STEP),
    (("policy check", X), POLICY_X),
    // `f` (B0) 1,465,740 KiB, step 6's 1.40 GiB → 1.5 GiB.
    (("fingerprint", S), 6 * STEP),
    // Plan Task 10 Step 1's provisional rows (release-fast at de5b151e, max
    // of 3 `maxrss.py` rounds; `/work/bench/160-12/task10-step1-peaks.md`).
    // Task 12's Part 4b replaces every row.
    // max 746,136 KiB → 0.75 GiB.
    (("digest", S), 3 * STEP),
    // max 751,848 KiB → 0.75 GiB.
    (("policy check", S), 3 * STEP),
    // max 3,187,260 KiB → 3.25 GiB.
    (("prove", X), 13 * STEP),
    // `diff $WS $WS`: max 1,472,632 KiB → 1.5 GiB.
    (("diff", S), 6 * STEP),
    // max 3,192,604 KiB → 3.25 GiB.
    (("events fanout", X), 13 * STEP),
    // max 747,860 KiB → 0.75 GiB.
    (("events fanout", S), 3 * STEP),
    // max 3,153,424 KiB → 3.25 GiB.
    (("query touches", X), 13 * STEP),
    // max 757,260 KiB → 0.75 GiB.
    (("query touches", S), 3 * STEP),
    // max 2,585,552 KiB → 2.5 GiB.
    (("graph", X), 10 * STEP),
    // shortcut: not measured yet (case B forms), placeholders at the largest
    // known row; Task 12's Part 4b measures them.
    (("fingerprint", X), POLICY_X),
    (("prove", S), POLICY_X),
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
        assert_eq!(peak_kib(&[("events chains", X)], KIND_PEAK_KIB), POLICY_X);
        assert_eq!(peak_kib(&[], KIND_PEAK_KIB), 0);
    }
}
