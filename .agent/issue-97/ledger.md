# Issue #97 ledger

## Claim

```json
{"run_id":"20261017-054722-98b07a","issue":97,"attempt":1,"session":"claude-code-session:cf05ae92-26c8-4a5d-a439-ddec4928220c","branch":"issue/97-agent-discovery-l5-digest-witne-a1","worktree":"U:\\Git\\al-sem-issue-97-a1","body_hash":"ebf45d14186dc55e8b19658ad1c540d0851ae8e979d1fd4d92c28b89bc3de925"}
```

Issue body hash: `ebf45d14186dc55e8b19658ad1c540d0851ae8e979d1fd4d92c28b89bc3de925`.

Reviewer roster: the operator told us (2026-10-15) to use `gpt-6.1-sol` and `claude-opus-5.5` instead of `gpt-6-astra` and `gemini-3.8-flash`. Marks are recorded as `reviews.sol` and `reviews.opus`, and we attest with `--substitute astra=sol --substitute flash=opus`.

## Classification

**bounded.** One loop condition in one function (`reconstruct_witness_paths`' ordinary-path BFS, `src/engine/l5/digest.rs`) plus a test. It mirrors what #69 already did for PD proofs (`enumerate_proofs`' `cut_len`).

## Design

**Defect.** The ordinary witness BFS (`digest.rs` ~:1357, `while !queue.is_empty() && found.len() < cap`) stops as soon as it has `cap` paths, in BFS discovery order. The paths are then sorted globally (~:1621-1638: by `hop_count`, then raw hop JSON per position), deduplicated, and truncated to `cap` (~:1659). Discovery order is edge order (`edge_compare`, ~:364) at each level, not the final sort order. So with cap 1 the BFS can keep a path that the full sort would have placed second. Example from the issue: `A` calls `Hz` at `cs-a` and `Hb` at `cs-z`, and both write the same table; BFS reaches `Hz` first, while the full sort puts `Hb` first. A capped witness is therefore not a prefix of the uncapped witness.

**Fix** (revision 2, after spec round 1; the #69 pattern adapted to the BFS's hop count):
1. **Record the cut once.** When `found.len()` first reaches `cap`, record `cut_hops = Some(max hop_count among found)`, set once by an explicit `if cut_hops.is_none()` at the loop head; it is never recomputed. `hop_count` is the sort's primary key: a terminal hit at node depth `d` has `d + 1` hops, an opaque-boundary hit at depth `d` has `d`.
2. **Finish only the same-length class.** Keep popping while `front.depth <= cut_hops`; BFS pops in non-decreasing depth. A popped node after the cut:
   - is NEVER expanded (T3, F1): no node pushes a child after the cut. When the cut is recorded after a hit from a node at depth `d`, `d <= cut_hops <= d + 1`, and every queued node has depth `>= d` (FIFO). A child then has depth `>= d + 1`: a terminal child has `>= d + 2` hops, more than the cut, and an opaque child cannot occur, because a child is a non-seed and the `valid_nodes` prune drops every non-seed opaque boundary (it has no out-edge and no direct fact, so it is no ancestor of a carrier). The argument is a comment at the expansion in `digest.rs`;
   - does not push a terminal hit when `depth + 1 > cut_hops`: it would sort after every kept path;
   - may still push an opaque-boundary hit at `depth == cut_hops`, the only same-length path left.
   So the extra work is bounded by the queue at the cut: those nodes are popped, none grows. (T1/T2 still expanded the nodes below the cut's depth; with N callsites on two levels that pushed N² dominated nodes, review F1. W7 pins the bound.)
3. **The sort chooses; truncation counts what the cut dropped** (revision 3). The existing global sort, dedup and `truncate(cap)` pick the `cap` paths. A flag `dropped_beyond_cut` is set whenever the post-cut rules discard a possible path:
   - a terminal is not pushed because `depth + 1 > cut_hops`;
   - a post-cut node at `depth < cut_hops` has at least one eligible edge. "Eligible" means exactly the expansion filters (not visited, in `valid_nodes`, `edge_has_hop`) applied to its out-edges, decided by the same code the expansion uses, but nothing is pushed (T3, F1). So a dead branch that only revisits an ancestor drops nothing (W3 stays exact). This is CONSERVATIVE: `valid_nodes` is plain reverse reachability and ignores the current path's visited ancestors, so an eligible child can still dead-end later (e.g. a cycle back to an ancestor, W5). `truncated` may then read true where the uncapped run finds no further path, which is the same conservative direction as the old flag;
   - a post-cut node at `depth >= cut_hops` has any out-edge (T2, CR1: an O(1) check, no visited set and no filter scan; more conservative than "eligible", which is allowed);
   - a node deeper than `MAX_DEPTH` is popped after the cut (see 4).

   Then `truncated = (cut_hops.is_some() && (!queue.is_empty() || dropped_beyond_cut)) || keyed.len() > cap`, extending `enumerate_proofs`' `cut_len` gating (~:935), and the proof-truncation fold is kept. `path-limit-reached` follows `truncated`. Its POST-LOOP push site (~:1510) is gated on `cut_hops.is_some() && (!queue.is_empty() || dropped_beyond_cut || found.len() > cap)`. BFS paths are distinct, so `found.len() > cap` implies `keyed.len() > cap`. Then the diagnostic is pushed at the same site whichever truncation cause fires, before `depth-exceeded` and the merged proof diagnostics, and the cost-only guards cannot change the diagnostic ORDER. The final `keyed.len() > cap` fallback (~:1662) stays for the proof-only case. The rules never drop a path that sorts before a kept one: anything they drop has `hop_count > cut_hops`.
4. **Limits.**
   - **The loop guard PEEKS:** `front.depth <= cut_hops` is checked on `queue.front()` BEFORE `pop_front` and before the `state_count` increment, as `enumerate_proofs` does (~:878). So a node beyond the cut is never popped or counted.
   - **Pops after the cut do NOT count toward `MAX_STATES`** (T2, CR2; this revises the spec, which counted them). So finishing the length class can never newly trigger `state-limit-exceeded`. Their number is bounded instead by the queue at the cut, since no node pushes a child after it (step 2). One consequence: the uncapped run still counts every pop, so it can hit `MAX_STATES` inside a length class that the capped run finishes uncounted. Then the capped run holds a path the uncapped run never found (W8).
   - **Depth:** after the cut, a popped node with `depth > MAX_DEPTH` (possible when the cut is a terminal at depth `MAX_DEPTH`, which makes `cut_hops = MAX_DEPTH + 1`) is skipped WITHOUT setting `depth_exceeded`, `incomplete` or `limit_hit`. It can only lead to paths longer than the cut, so it counts toward `dropped_beyond_cut`. So no new `depth-exceeded` comes from the extra pops.

**Invariant pinned:** for ordinary paths, when NEITHER the capped NOR the uncapped witness is `incomplete` (two-sided, review F2; W8 shows the prefix failing when only the uncapped run is `incomplete`), `witness(cap = k)` == the first `k` elements of `witness(cap = HARD_PATH_CAP)` in the RAW reported order. Digest's `merge_normalize_via_paths` (~:3040) re-selects its top 3 by projected length and JSON. That projection is downstream of this invariant, and is unchanged and not promised.

## Assumption probes

| probe | result | evidence |
|---|---|---|
| the BFS stops at `found.len() == cap` in discovery order | confirmed: the loop guard is `found.len() < cap` | `digest.rs:~1357` |
| the reported order is a global sort by `hop_count`, then raw hop JSON | confirmed: `keyed.sort_by` | `digest.rs:~1621-1638` |
| the proofs already solve this | confirmed: `enumerate_proofs` records `cut_len` and finishes the length class (`if cut_len.is_some_and(|cut| len > cut) { break }`) | `digest.rs:~873-910` |
| a terminal hit and an opaque-boundary hit differ by one hop at the same node depth | confirmed: a terminal hit pushes `(ni, Some(terminal))` (the chain plus the terminal hop); opaque pushes `(ni, None)` | `digest.rs:~1418-1445` |

## Base/Head SHAs

- B = `9df38c69a00e44fbb72d48332a3ccf403bd41965` (origin/master)
- H = `13c1e5dc122739f40e8fbfae88ca45b1266a32ff` (approved code head; the evidence commit follows it)

## Timeline

- Worktree created at master `9df38c69`.
- Classified as bounded; design written.
- Spec panel (sol + opus). Register hash = sha256(ledger.md)[:16] at the round:
  - r1: both confirmed the defect and the stop rule. W2 was unreachable (S1); the cost was understated (S2); golden/CDO movement (S3); the truncated gating (S4); limits (S5); a corpus oracle (S6); raw-order prefix only (S7); cut recorded once (S8); exact-cap flag (S9).
  - r2: both re-raised S4 and S5. N1: suppressed paths leave no queue entry, so truncated could be false.
  - r3: N3: the exact truncation oracle conflicts with the conservative `dropped_beyond_cut`, so it became an implication, plus W5. Also N4-N6, and S5 re-raised by sol (`MAX_STATES` wording).
  - r4: both accepted all 15. N7 (stale S4 evidence) and N8 (diagnostic order across push sites) were fixed.
  - r5 (spec_rounds 5; pi_calls 10): both accepted all 17; no new findings. CONVERGED.
- T1 (opus; task_attempts T1 = 1): PASS; commit `6b6b9489`. Report: `t1.md`.
  - **Red:** on the old code, W1, W2 and W3 fail (W3 on `truncated`).
  - **Passing on the old code:** W4 passes, because the old code stops with B still queued. It discriminates only the `dropped_beyond_cut` term (proof 2). W5 passes too; it pins the conservative direction.
  - **Corpus oracle:** the r0 corpus has NO witness whose BFS discovery order differs from its sort order, so the extended `check_witness_bfs` oracle passes on both old and new code and cannot discriminate either break. The ledger's earlier claim that "the corpus oracle fails" is corrected here: the unit tests W1-W5 carry the discrimination.
  - **Gates:** lib and clippy green; `check-goldens` green with no golden moved.
  - **`--witness` grep:** only `tests/cli/cli_b_fingerprint_oracles.rs` uses non-default caps (validation oracles, plus a hand-built 2-path cap-1 case that still asserts truncated). No golden runs a non-default cap; digest always uses `HARD_PATH_CAP`.
- `code-review` skill at high (the Claude Code built-in, run as given) found CR1-CR7:
  - CR1/CR3: the cut-depth level was fully processed after the cut;
  - CR2: post-cut pops counted toward `MAX_STATES`, so new `incomplete` results were possible;
  - CR4: opaque diagnostics for dropped paths;
  - CR5: a CHANGELOG over-claim;
  - CR6: stale pushed/popped numbers;
  - CR7: a redundant term and two push sites.
- T2 (opus; task_attempts T2 = 1): PASS; commit `33a79418`. Report: `t2.md`.
  - Post-cut nodes at depth ≥ cut take an O(1) path. Post-cut pops are not counted toward `MAX_STATES`.
  - Post-cut opaque diagnostics are kept only for kept paths, in order; W6 pins this, with a break.
  - The CHANGELOG is scoped to terminal cuts, and the numbers are removed.
  - The T1 breaks 1 and 2 were re-run and still fail; break 3 (W6) fails.
  - Gates: lib 2061, `cli_b_fingerprint_oracles` 35, clippy and `check-goldens` green; no golden moved.
- CDO (sonnet, `cdo.md`), `release-fast`, master `9df38c69` vs `33a79418`, 3 digest files (CDOeDocsSendingProfileSubs, CDOSubscribers, CDOEMailHandler), 2 runs each:
  - **byte-identical** (p=0, t=0, d=0, c=0);
  - wall time 59.9 → 59.5 s, which is noise.
  - Honest limit: these files show `truncated: false` throughout, so they never reach the 256-path cap. This proves no regression on CDO, NOT that the capped path ran there. The capped path is proven by W1-W6.
- Final panel r1 (sol + opus): all 24 register entries accepted except N5 (opus re-raised: no timing on a cap-hitting workload). New: F1 (important, sol: post-cut nodes below the cut's depth still expanded, N² dominated nodes uncounted), F2 (minor: the prefix guarantee needs BOTH runs non-`incomplete`), F3 and opus minors (stale CHANGELOG/ledger/findings text).
- T3 (opus; task_attempts T3 = 1): PASS; commit `13c1e5dc`. Report: `t3.md`.
  - **F1:** after the cut NO node pushes a child. A post-cut node below the cut's depth runs the same eligibility filters without pushing and sets `dropped_beyond_cut` on an eligible edge; a node at or beyond it keeps T2's O(1) path. The argument (cut in `[d, d+1]`, FIFO queue at depth `>= d`, a terminal child has `>= d+2` hops, a non-seed opaque child is pruned by `valid_nodes`) is a comment at the rule. Post-cut work is bounded by the queue at the cut. **W7** pins it through a `#[cfg(test)]` thread-local `WITNESS_ARENA_LEN` (no production cost): arena ≤ 63 with N = 60; with expansion re-allowed it is 3663 (= 63 + 60²) and W7 fails.
  - **F2:** **W8** (`MAX_STATES` = 25,000 siblings; the 10 witness tests take ~1.6 s together): cap 1 returns Hb and is complete, cap 256 is `incomplete` with only Hz, `assert_ne!` on the prefix. Counting post-cut pops again makes W8 fail.
  - W5's message is now "D's edge to E is eligible" (D at depth 2 < cut 3 is no longer expanded; its edge to E passes the filters).
  - Breaks re-run (T3 messages in the table below): break 1 fails W1, W2, W3, W6, W8; break 2 fails W4, W5, W7.
  - Docs: CHANGELOG ("after the cut no node is expanded …; pops after the cut are not counted toward the state limit"; prefix two-sided; conservative flag wording); ledger design steps 2-4, Invariant, Acceptance, proofs table; findings.json S5 and N6 `evidence` only.
  - **CDO timing on a cap-hitting workload (N5).** `release-fast`, master `9df38c69` (temp worktree, shared target dir, removed after) vs branch (33a79418 + T3, `src/lib.rs` touched; binaries differ). Workspace: the pinned CDO baseline.
    - `alsem fingerprint <ws> --witness 1 --format json --deterministic` (whole workspace): 89,225 witnesses, **78,102 `path-limit-reached` / `truncated: true`** (both binaries), 10 `state-limit-exceeded` (both). Wall: master 417.0 s / 454.1 s (mean 435.6), branch 418.0 s / 464.1 s (mean 441.1), +1.3%, inside the run-to-run spread (~9% on both). Each binary's two runs are byte-identical. Master vs branch: 87,034 witnesses identical; **2,191 witnesses in 913 routines differ in the chosen path only**, same hop count, same `truncated`, `incomplete` and diagnostics; everything outside the witnesses is identical. Prefix check on a 14-routine sample of those (every 70th of the 913, `--witness all` on the branch): all **114 changed witnesses: branch cap-1 path == uncapped[0]**, and master's path is at a later index of the uncapped list; no run `incomplete`; `truncated` matches (uncapped > 1 path or truncated) in every case.
    - `alsem fingerprint <ws> --witness all … --routine ×14` (same sample): 1,110 witnesses, **150 `path-limit-reached`** (cap 256). Wall: master 21.5 s / 19.1 s, branch 21.1 s / 19.7 s: no difference. Each binary's two runs byte-identical. Master vs branch: 1,089 identical; **21 witnesses (one routine) differ in path choice only**: all 256 paths, truncated and complete in both, the same first path and the same per-position hop counts, 102-254 paths in common. Only the tail of the cut's length class differs, which is what choosing the sorted prefix of that class (instead of master's first 256 in discovery order) produces. No cap above 256 exists, so this class is checked for shape, not against an uncapped list.
    - No other difference class appeared, so nothing to stop on. A whole-workspace `--witness all` was not timed: a 131-routine sample already wrote 8.4 GB.
    - Gates after T3: lib `2063 passed`, `cli_b_fingerprint_oracles` `35 passed`, clippy `--all-targets -D warnings` clean, rustfmt clean, no `#[allow(` added, `check-goldens` exit 0 (8 × `test result: ok`, 0 FAILED), no golden moved.
- Final panel r2 (final_rounds 2; pi_calls 14): both accepted all 29; no new defects. Opus listed two optional text corrections, both applied: S6's evidence now says "both runs non-`incomplete`", and design step 1 says the cut is set by an explicit `if` at the loop head, not `get_or_insert`. CONVERGED.

## Acceptance matrix

| Acceptance item | Proof |
|---|---|
| The BFS completes the frontier length that reaches the cap, and the global sort chooses (as #69 did for proofs) | The code change described above; reviewed in the spec and final panels. |
| A test asserts capped == prefix(uncapped) for ordinary paths | Unit tests in `digest.rs`, built with the existing helpers (`empty_snapshot`, `call_edge`, `insert_fact`, `build_fingerprint_indexes`):<br>- **W1**, the issue's shape: Root → A; A calls Hz at `cs-a` and Hb at `cs-z`, both ordinary writers of one table. BFS discovers Hz first (`edge_compare` orders callsite first), but the sort puts Hb first (raw JSON compares `routineId` first). It asserts `witness(cap=k) == witness(cap=all)[..k]` for every k.<br>- **W2**, a hop-count tie class that is reachable (replacing the unreachable depth-≥2 opaque shape): seeds `A` (a terminal, 2 hops) and `Z` (an opaque boundary, 1 hop, a hand-built `SnapshotCoverageRecord` with `direct_status "unknown"`), both on the first-hop callsite. A is found first, but `Z` sorts first. Cap 1 must return the Z path.<br>- **W3** (exact cap, dead branch): exactly `cap` paths exist, and a queued branch D only revisits an ancestor. The terminal H's callsite must sort BEFORE D's, so the cut happens while D is still queued. `truncated` must be false; the old code reported true.<br>- **W4** (a dropped path still truncates): cap 1, Root reaches seeds A and B on one callsite. A is a carrier; B → C, and C is a carrier. A sets `cut_hops = 2`, and C's 3-hop terminal is suppressed. `truncated` must be true, the uncapped witness has 2 paths, and capped == uncapped[..1].<br>- **Corpus oracle**: `check_witness_bfs` (~:5340) already runs cap 2 and `HARD_PATH_CAP` over the r0 corpus. When neither run is `incomplete`, it now also asserts `capped(2) == uncapped[..2]` and the IMPLICATION `(uncapped.paths.len() > 2 \|\| uncapped.truncated) ⇒ capped.truncated`. It is not an equality, because the flag is conservative (design step 3).<br>- **W5** (the conservative flag, pinned): Root→A; A→H (a carrier) and A→D, with H's callsite before D's; D→E→F; F→A. Cap 1: `truncated` is true. The uncapped run has exactly 1 path and is not truncated. This documents the conservative direction.<br>- **W6** (T2): opaque diagnostics after the cut only for kept paths.<br>- **W7** (T3, F1): A→Hz first, then A→B at 60 callsites, B→C at 60 callsites; cap 1. The arena (read through the `#[cfg(test)]` hook `WITNESS_ARENA_LEN`) stays at ≤ 63 nodes (sentinel, A, Hz, 60 B), `truncated` is true, and capped == uncapped[..1].<br>- **W8** (T3, F2): A→Hz first, `MAX_STATES` (25,000) siblings that only call back to A, A→Hb last. Cap 1 returns Hb and is not `incomplete`; cap 256 hits `MAX_STATES` before Hb, is `incomplete` and holds only Hz. The guarantee is two-sided. The whole 10-test witness filter runs in about 1.6 s.<br>Discrimination:<br>- restore the old `found.len() < cap` guard: W1, W2, W3, W6 and W8 fail;<br>- drop `dropped_beyond_cut` from `truncated`: W4, W5 and W7 fail;<br>- re-allow expansion after the cut: W7 fails on its arena bound (T3);<br>- count pops after the cut toward `MAX_STATES`: W8 fails (T3);<br>- the corpus oracle passes under every break: the r0 corpus has no witness whose BFS discovery order differs from its sort order, so it cannot discriminate. It is a regression net only;<br>- the post-cut terminal-push guard is COST-only: without it the extra paths are cut by `truncate(cap)`, so no output test can tell. The expansion guard is now pinned by W7's arena-length test hook (`WITNESS_ARENA_LEN`, `#[cfg(test)]` only); its wall-time effect is measured on CDO (T3 timeline). |
| No change elsewhere | **Goldens:** any digest/fingerprint golden may move in `truncated`/`path-limit-reached` at exactly the cap (the W3 semantics), or in path choice where a fixture exceeds its cap. Before claiming anything, grep the goldens and tests for a non-default `--witness`. Every moved line is triaged; there is no blanket "zero" claim. **CDO**, which hits the 256-path cap: `alsem digest` before/after byte-compare on the same file as #96's T5. Every changed witness, evidence, `canonical_key`/occurrence id, `via_paths` or conditionality line is explained as "the sorted prefix now chosen". Also wall time before/after, 2 runs each. |

## Discrimination proofs

Every break was made with Edit, run, reverted with Edit and the file `touch`ed. Logs: `scratchpad/t97/break{1,2}.log` (T1), `t2-break{1,2,3}.log` (T2), `t3-break{1,2,-w7,-w8}.log` (T3). The messages below are from the T3 re-run, against the current tests.

| # | break | failing tests and decisive line | after revert |
|---|---|---|---|
| 1 | restore the old guard (`if found.len() >= cap { break; }` in place of the peek/cut) | W1 `assertion left == right failed: cap 1`; W2 `left == right`; W3 `exactly cap paths exist`; W6 `precondition: the kept path ends at Y`; W8 `cap 1 reaches Hb` (5 failed) | all green |
| 2 | drop `dropped_beyond_cut` from `truncated` | W4 `C's path was dropped by the cut`; W5 `conservative: D's edge to E is eligible`; W7 `the C paths were dropped by the cut` (3 failed) | all green |
| 3 | prune no opaque diagnostic (`if false && cut_hops.is_some()`, T2) | W6 `left: [(…, Some("Y")), (…, Some("Z"))]` | all green |
| W7 | re-allow expansion after the cut (`if false && cut_hops.is_some()` at the no-push rule) | W7 `no node expands after the cut: arena 3663, bound 63` (60 + 3 + 60²) | all green |
| W8 | count pops after the cut (`if cut_hops.is_none() \|\| true`) | W8 `cap 1 pops after the cut uncounted` | all green |

The corpus oracle passes under every break, because the r0 corpus has no distinguishing witness. It is a regression net, not a proof.

## Findings register summary

29 entries (`findings.json`):
- spec panel: S1-S9 and N1-N8;
- code review: CR1-CR7;
- final panel: F1-sol, F2-sol, F3-sol, N9, N10.

All 29 are `fixed`, and both reviewers accepted all of them at code-diff hash `6bf1ad2546685617`. No entry is open, refuted or deferred.

## Gate results

All 4 gates ran under the supervisor on head `13c1e5dc122739f40e8fbfae88ca45b1266a32ff` and returned exit 0. The worktree's `git status --porcelain -- . ':!.agent'` was empty afterwards. Logs are under `.agent/runs/20261017-054722-98b07a/logs/`.

| gate | exit | seconds | log |
|---|---|---|---|
| ci-steps-all | 0 | 767.2 | `ci-steps-all/20261017T084837-51d22993.log` |
| check-goldens-coverage | 0 | 0.4 | `check-goldens-coverage/20261017T090125-deff267f.log` |
| check-goldens | 0 | 69.8 | `check-goldens/20261017T090125-72a5d656.log` |
| cdo-gate | 0 | 268.7 | `cdo-gate/20261017T090235-aad8cf1f.log` |

Earlier gate runs, on `33a79418`, were stopped when the review fixes changed the code; they are not used.

## Discoveries

None.

## Not delivered

Nothing. Both acceptance items are met:
- the BFS finishes the cut's length class and the global sort chooses, so a capped witness is the raw-order prefix of the uncapped one when both runs are complete;
- the tests (W1-W8, with a corpus net) pin it, with discrimination proofs.

The one honest limit is recorded above: the r0 corpus has no witness where the old and new cut differ, so the corpus oracle is a net, not a proof. CDO shows the change on 2,191 cap-1 witnesses.
