# #160 step 3: fold from uncertainty edges — plan

The spec is the converged design in `.agent/issue-160/ledger.md` (attempt 6, design hash
`b26a93a8acc688be`): sections Assumption probes, Design, Acceptance matrix (A3.1-A3.10).
Epic: `docs/superpowers/specs/2026-10-18-issue-160-analysis-session-design.md` §9 row 3.

Every test below is written red first and given a discrimination proof (break, watch it
fail, revert, watch it pass; record both in the ledger's proof table). Format with
`rustfmt <file>` only. Per-task gate: `scripts/ci-steps task-affected`.

## Task 1 — the scan, its oracle, and the stderr move (A3.3-A3.7, A3.10; Design 1-3, 5-7)

Files: `src/engine/l4/capability_cone.rs`, `src/engine/l4/db_effect_solver.rs`,
`src/engine/l4/analysis.rs`, `tests/cli/cli_build_counters.rs` (span name only).

1. Red: in `capability_cone.rs`'s test module add
   - `callsite_local_scan_hand_stated` (A3.7): a hand-built `CombinedGraph` (A: six kinds,
     `member-not-found` twice; B: `external-target`; orphan `from`: `ambiguous-overload`),
     expected map written literally.
   - the oracle helper: for one `Model`, build the graph as the callers do, run the old fold
     (`#[cfg(test)]`) and the new scan, compare per distinct `ws.routines` id (absence = ∅),
     count extra new keys, and count routines per (kind × {non-recursive singleton,
     self-loop singleton, multi-member SCC}) using `graph_scc`'s `recursive` and member count.
     One model build per input serves comparison and counts.
   - `callsite_local_scan_matches_the_fold_on_fixtures` (A3.4, A3.5, A3.6): r0-corpus dirs
     (207) + the 32 dirs under `tests/fixtures`, `tests/r3a4-fixtures`, `tests/r3a5-fixtures`,
     `tests/r2-5b-fixtures` + the inline cells fixture (each of the four kinds in a two-member
     cycle, a self-loop and a non-recursive routine). Asserts each input set ran > 0
     workspaces, all 12 cells > 0 over the union, `external-target` > 0 on these inputs.
     Prints its wall time (A3.10).
   - `callsite_local_scan_matches_the_fold_on_cdo` (A3.4/A3.5 CDO half): include
     `tests/common/cdo.rs` as `src/program/dep_cache.rs:300` does; `cdo_ws_or_enforce`; prints
     the 12-cell table.
   - `callsite_local_scan_matches_the_fold_under_a_roles_cap` (A3.3 i): a COPY of the
     `CYC_SRC` shape whose `HandleCyc` adds an `external-target` and a `member-not-found`
     call; under `with_roles_iteration_cap(1, …)` assert the precondition via
     `compute_summaries_v2` (non-empty diagnostics, a member with `fixpoint-capped`), then
     old == new with both sets non-empty.
   - `snapshot_base_prints_no_capped_line` (A3.3 ii): child-process pattern of
     `build_l4_prints_the_capped_line_once` (`summary_runner.rs:2577-2612`) running
     `build_r3a3_source_only_base` on the SAME copied fixture under cap 1; zero
     `warning: summarize:` lines.
   Add a stub `callsite_local_coverage_reasons` returning an empty map so everything
   compiles; record the red output.
2. Green: `is_callsite_local_kind` → `pub(crate)` with the doc of Design 1; implement the
   scan; both callers use it; the old fold becomes `#[cfg(test)]` with only its attribute
   and doc changed; rename span `r3a3.direct_facts` → `r3a3.direct_facts_scan` and update
   `r3a3_spans_open_once_per_base`; Design 7 comments.
3. Proofs: A3.6 (remove the `external-target` arm of the old fold's `match`), A3.7 (add
   `|| kind == "unresolved-call"` to the scan's own filter), A3.3 ii (restore the
   `render_warnings` loop on the scan path), one each for the cap precondition and the cell
   assertion (drop the inline fixture → a cell is 0).

## Task 2 — counters, the empty-diff guard, the restated table, CHANGELOG (A3.2, A3.9; Design 4, 8)

Files: `tests/cli/cli_build_counters.rs`,
`docs/superpowers/specs/2026-10-18-issue-160-steps-0-2-design.md`, `CHANGELOG.md`.

1. Rows per A3.2 (process totals): `analyze_cross_app_d47` v2 1→0, scc 3→2; `digest_cross_app`,
   `digest_single_app`, `prove`, `FINGERPRINT_ROW` v2 2→1, scc 4→3; `diff_ws_ws` v2 2→0,
   scc 4→2; `diff_ws_ws_strict` v2 4→2, scc 8→6. Header comment (`:178-180`) updated.
2. A3.9: `run_digest_empty_diff_builds_no_context` asserts `build.snapshot == 0`, doc fixed;
   proof: compose the snapshot before the no-input check → fails.
3. Child spec §4.4.8: an "after step 3" table restating the moved rows.
4. CHANGELOG `## [Unreleased]`: the solve removed from the snapshot path and `project_r3a3`,
   the counter moves, the stderr move (Design 6, exactly), the census/trace-instant move,
   the span rename, the gap deviation.
