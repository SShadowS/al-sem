# TLS-27: the findings sort builds each key once

Task: TLS-27 (backlog entry in `docs/OUTSTANDING.md`, from the 2026-07-31 attribution work).
Branch `tools/tls-27-findings-sort`, base master `af0612da`.

## What changed

The findings sort (detector name in natural order, then location text, then root cause) used a
comparator that ran `compare_natural` and formatted both location strings on every comparison.
With ~144 k findings on CDO that is millions of tokenizations and string builds.

`sort_by_detector_location_cause` (`src/engine/l5/registry.rs`):
- `natural_ranks` sorts the distinct detector names once with `compare_natural` and gives each a
  `usize` rank; names that compare Equal share a rank, so ties fall through to the next key
  exactly as before.
- `sort_by_cached_key` builds each finding's key `(rank, location, root cause)` once. It is a
  stable sort, so ties on all three keys keep input order, as the old `sort_by` did.

Used by `role_scope_and_sort` and by `project_r4_findings` / `project_r4_findings_cross_app`
(`src/engine/l5/finding.rs`, each with its own location helper, unchanged).

## Tests (registry tests module, each with a discrimination proof)

- `natural_ranks_are_an_exact_monotone_image_of_compare_natural`: all pairs of 14 names,
  including Equal spellings (`d1`/`d01`).
- `findings_sort_by_natural_detector_then_location_text_then_cause`: the production path through
  `role_scope_and_sort`.
- `decorated_sort_equals_the_old_comparator`: 2,000 generated items against the old comparator,
  input index included.

Review: gpt sol and local Opus, two rounds each, both READY; no input found where the order
differs. Folded in: no second findings buffer (`sort_by_cached_key`), `usize` rank, `HashSet` of
names, test rework.

## Measurement (2026-11-12, CDO `/work/src/cdo-baseline/Cloud`, release-fast, 22 cores)

Command: `alsem analyze <CDO> --scope all --format json --deterministic`. Per binary 3 traced
runs (`ALSEM_TRACE=1`) and 1 untraced run under `timeit.py`; master first, then head. Before each
run: MemAvailable ≥ 35 GiB (it was ~51-53 GB) and 1-minute load < 10 (6.5-9.9).

Under `--scope all` (cross-app, the default) the sort runs inside the `crossapp.role_scope` span,
which wraps only `role_scope_and_sort`; the `l4_l5.role_scope_and_sort` span is the single-app
path and does not fire here.

| | master `af0612da` | head `d14dd1c7` |
|---|---|---|
| `crossapp.role_scope`, runs 1/2/3 | 269.6 / 250.9 / 202.4 ms | 24.0 / 22.5 / 21.7 ms |
| median | 250.9 ms | 22.5 ms (×0.090) |
| untraced wall | 24.13 s | 22.88 s |
| untraced CPU | 129.8 s | 124.5 s |
| untraced peak RSS | 3,208.9 MiB | 3,223.9 MiB |

The wall, CPU and peak rows are one run each, so their differences are within noise; the sort
was ~1 % of the run. All 8 stdout files (51,064,082 bytes) have the same sha256
`5e6eac80fbb0611ed123be8215508fec3cd840d3a3f5cc236cb61d772c23132f`.
