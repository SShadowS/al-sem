# Issue #289 — digest 1.7–5.8× slower since #160 step 6 (enumerate_proofs after F1-S1)

## Claim

```json
{"run_id":"20261024-163804-d1d6b6","issue":289,"attempt":1,"session":"claude-code-session:57653578-e111-4b15-8684-017371f816fa","branch":"issue/289-digest-slower-since-160-step-6-a1","worktree":"/work/al-sem-issue-289-a1","body_hash":"f921ec8662bc6130aae8148271010dec15e415ca14ba9e477c47338237c51b6e","reconciled":[]}
```

Issue body hash (claim `body_hash`): `f921ec8662bc6130aae8148271010dec15e415ca14ba9e477c47338237c51b6e`.
Base (`make-worktree`): `b2c73d4f3ad16defdc84a8c3f2d45aac351113d3` (master after #271).
Reviewers (operator instruction, confirmed for this queue): `gpt-6.1-sol` in the astra slot, Opus in the flash slot; attest with `--substitute astra=sol --substitute flash=opus`. Marks stored as `reviews: {sol: …, opus: …}`.
Operator (2026-10-24, AskUserQuestion on alsem-main's request): "Finish #271 first" → #271, then #289, then #272. Claimed with a one-off `--loop-max 5` (4 used since the reset).
Scope rule (alsem-main): keep the fix inside `enumerate_proofs` (`src/engine/l5/digest.rs`); #160 step 9 (in `/work/al-sem-issue-160-a10`) also touches digest.rs (fingerprint query paths) — whoever merges second rebases.

Rules from alsem-main (2026-10-24 ~18:20):
- Heavy work is allowed for #289; one CDO process at a time; judge benches on CPU-s, walls reported only (OD-4); send alsem-second a one-line heads-up before a long bench round.
- Target stays ≤ 1.1 × `c7e5f97e` CPU-s on `h`; if the fix lands just above, bring the operator the numbers with the reason and a stated default action after 15 min.
- **Rebase onto `6172492f` (#160 step 9, PR #290) BEFORE the final CDO bench**, not only before the gates: step 9 changed digest's hot path (`gaps_in_cone` now `Option` and prove-only, `group_resources` moved, `path_boundary` explicit `ev:` arm). The prototype rounds on `b2c73d4f` are root-cause evidence only.
- alsem-main reviews the design (F1-S1 guarantee; interaction with step 9's digest edits) once it is in the ledger.

## Classification

**Bounded.** A local data-structure fix inside `enumerate_proofs` (plus `ProofOutcome` / a new `ProofKey` just above it); the enumeration's meaning is unchanged.

## Assumption probes

Run by the design author (opus), release-fast, CDO `h` unless stated, mimalloc purge delay 0, the issue's command; evidence under `/work/bench/289/`.

| probe | result | evidence |
|---|---|---|
| counters at base `b2c73d4f` (`ep-counters.patch`) | 301,335 calls; 16,399 hit `MAX_STATES` (25,000 pops) and account for 1,255 of the 1,320 s in `enumerate_proofs` (run total 1,511 CPU-s); crossed pops 425M, crossed drops at pop 165M, crossed pushes 649M, uncrossed pops 2.9M, revisit steps 18.0e9; the cap is never reached (max 210 distinct keys, cap 256) | `ctr-h.err` |
| same counters at `c7e5f97e` | nearly the same work (449M pops, 594M pushes, 13.0e9 revisit steps, 17,670 limited calls) but only 68–85 s in `enumerate_proofs` | `c7e-h.err`, `revctr-h.err` |
| what F1-S1 adds per pop | `found.contains(&**k)` hashes the full candidate key (a `Vec<String>` of projected-hop JSON + `BoundaryEnd`) on ~590M crossed pops (425M expanded + 165M dropped); nothing else per pop changed this much | interning removes ~75% of base CPU |
| prototype: intern each key once to a dense id, `found: Vec<bool>` (`fix-intern.patch`) | `h` 379–421 CPU-s vs base 1,530–1,624; 122 s in `enumerate_proofs` vs the F1-S1-reverted control's 68 s | `fixctr-h.err`; res/res2/res3 |
| the remaining ~54 s (1.1–1.15×) | crossed-first order makes paths deeper (average max length 29 vs 23.6): 38% more revisit steps, 10% more pushes, Node 32 → 56 bytes | counter files |
| extras (packed chain array fix3, 64-bit Bloom fix2, both fix4) | no gain beyond noise (fix3 388; fix4 405; fix2 387–467) → dropped | res.txt, res3.txt |
| control: master with F1-S1 reverted (`bin-rev`) | 357–379 CPU-s ≈ `c7e5f97e` (351–374): the whole gap is F1-S1 | res2.txt, res3.txt |
| byte-identity | fix, fix2, fix3, fix4 byte-identical to base on `h` (441,330,873 B, sha256 `6d6505f6…`) and `d` (20,016,778 B, `1e853d95…`) | `cmp` of `out-*` |
| scope vs #160 step 9 | `b2c73d4f..6172492f` touches digest.rs only from ~line 3,161 (`DigestEffectResult`, `digest_one_root`, `ProjectedPath`, `*_pub`); none in the fix region (~897–1151), incl. the `gaps_in_cone` Option change, `group_resources` move, `path_boundary` `ev:` arm | git diff |

Prototype numbers (CPU-s / wall s): `h` — `c7e5f97e` 374/56, 353/43, 351/43, 368/53, 369/50, 369/51, 355/42, 363/49 (median ≈ 365); rev 373/53, 368/53, 379/50, 357/45, 365/50; base 1,624/139, 1,583/133, 1,530/124; fix 403/54, 421/66 (load 46), 379/44, 414/56, 414/53, 388/47, 391/46, 411/53 (median ≈ 407, ratio ≈ 1.115 — inside the F1 band, see Design); peaks 3.2–3.5 GB, unchanged. `d` wall — `830a150c` 28.0, 28.3, 27.7; base 48.5, 53.5, 51.8; fix 29.8, 32.5, 30.8 (target ≤ ~36 s: passes); peaks ~3.14 GB. Operator (relayed by alsem-main, 19:20 UTC): stop tuning, write the design; fallback F1 below.

## Design

**Goal.** Restore `digest`'s pre-F1-S1 cost on CDO while keeping F1-S1's guarantee (the proof cap counts DISTINCT candidate keys after the boundary cut) and byte-identical output.

**Non-goals.** The remaining ~1.1× from crossed-first ordering (deeper paths); chain-array / Bloom extras (measured no gain); any change outside `enumerate_proofs`, `ProofOutcome` and the new `ProofKey`.

**Design.**
1. One stable id per distinct candidate key: `HashMap<ProofKey, usize>` + `found: Vec<bool>` + `found_count`. A key is hashed at a crossing push and at an uncrossed-seed lookup, never per crossed partial-path pop (revision 2 wording). Memory (M289-1): the crossing push interns BEFORE the `found` test, so `key_ids` keeps one entry per distinct crossing key a call meets, including keys never seeded (master built such a key and dropped it); bounded by the distinct crossing keys of one call and freed at return; the final measurement note reports peaks. `Node.crossed` holds `(key_id, level)` instead of `Rc<Key>`. At a pop the dropped-path test is `found[id]`; at a crossing push `intern(key_of(..))` then `found[id]` (same test as today's `found.contains(&key)`); at a seed a crossed path reuses its id, an uncrossed path interns its key; `found_count` rises only on a false → true change, so the cut equals `found.insert(key) && found.len() >= cap`. Pops, push order, the `MAX_STATES` points and `out.paths` order are unchanged → output identical by construction.
2. Perf guard: the key becomes a module-level newtype `ProofKey(Vec<String>, Option<BoundaryEnd>)` next to `ProofOutcome`, whose hand-written `Hash` bumps a `#[cfg(test)]` thread-local `PROOF_KEY_HASHES`; `ProofOutcome` gains `popped: usize`. New test `proof_walk_hashes_keys_per_crossing_not_per_partial_path` on the CDO shape: `R→P`; `P` calls `DepA` (`cs-a`) reaching seed `H1` at depth 1 plus a dead branch `X1→X2`; `P` also calls `DepW` (`cs-w`) opening a 6-layer × 3-routine fully linked dependency lattice with no seed (1,092 crossed partial paths, all popped after key A is found). Asserts `paths == [(root, a, h1)]`, not truncated/incomplete, `popped == 1097`, `hashes <= 4` (confirmed on the prototype by the Opus reviewer: 2 hash calls, no map resize). The doc comment states the guard's limit: it pins per-pop hashing and the drop-at-pop on this fixture, not enumeration work in general (an extra-pop regression shows only as `popped`, only here).
3. Ship `fix-intern.patch` + `ProofKey` + `popped` + the guard test only — NOT the prototype's `chain` / `bloom` / `bit` extras. CHANGELOG `Fixed` entry.

**Acceptance matrix.**

| # | item | test / measurement | break that must fail it |
|---|---|---|---|
| A1 | root cause | probe table; counter patches | — |
| A2 | F1-S1 kept | `proof_cap_counts_distinct_candidates_after_the_boundary_cut`, `twin_proofs_count_once_toward_the_cap` stay green | re-run F1-S1's breaks on the new code: (i) raw counting (`found_count += 1` on every new raw proof) → twin row fails; (ii) crossed paths pushed onto `queue` instead of `crossed_q` → fails at cap 2; (iii) raw counting + both found-checks removed → fails at cap 3; revert → green. Record F1-S1's stated limit: under raw counting alone the NEW guard test still passes |
| A3 | perf | 3 interleaved rounds on the REBASED branch: `h` judged on median CPU-s (≤ 1.1 × `c7e5f97e`'s median); `d` judged on WALL against step 6's B1′ (≤ `830a150c` + 8 s) — the issue's own `d` bound is a wall bound; peaks reported; `maxrss.py`; numbers in a measurement note | see F1 |
| A4 | byte-identity | the evidence that matters is the `cmp` of CDO `d`/`h` output against a base binary rebuilt at the same rebased master (no corpus golden holds a rule-B boundary, so `check-goldens` (no regen) only shows nothing else moved) | unit break: test "the key is interned" instead of `found[id]` (confuses interned with found; `k < found.len()` at the pop and crossing-push tests) → fails `proof_cap_counts_distinct_candidates_after_the_boundary_cut`, the guard and `digest_cli::tests_rule_b::b8_proofs_are_cut_at_the_boundary_and_keep_their_class`; `twin_proofs_count_once_toward_the_cap` stays green (its fixture has no crossing — measured by the Opus reviewer, `review-logs/r2-break-interned.log`); the twin test is covered by A2's raw-counting break (i). (Dropping the push-time found-check is NOT a discriminating break: such nodes are dropped at pop and the output is unchanged — measured: all 79 `l5::digest` tests green.) |
| A5 | perf guard | `proof_walk_hashes_keys_per_crossing_not_per_partial_path` | (a) restore master's `HashSet<Rc<ProofKey>>` with the per-pop `contains` (multi-site edit; assert every site changed) → `hashes` far above 4 (record the measured value; ≈ 1,094 expected) fails; (b) delete the drop-at-pop `if found[k] { continue }` → `popped` 1,099 fails (measured by the Opus reviewer; both F1-S1 tests stay green under it, so the guard alone catches it); revert → green |

**Pre-stated fallbacks.**
- F1 (operator, relayed by alsem-main): the final measurement is 3 interleaved rounds on `h` on the branch REBASED onto `6172492f`, judged on median CPU-s. If median ≤ 1.1 × `c7e5f97e`'s median, done. If just above — defined as > 1.1× and ≤ 1.15× (revision 2) — the numbers go to the operator via alsem-main, and the stated default after 15 min is to accept the fix (≈4× faster than master's ~1,530) and file the remaining few % as a follow-up issue. When F1 fires, A3's CPU half is NOT delivered: it is listed under `## Not delivered`, the follow-up issue (the deeper paths from crossed-first ordering, ~1.1×) is filed in the same change as a discovery, and the PR body says `Refs #289` instead of `Closes #289` (the flow allows `Closes` only when every acceptance row is met), leaving the issue for the operator to close. Above 1.15×: no default — stop and ask the operator.
- Current state (prototype, old base): `h` median ≈ 407 vs `c7e5f97e` ≈ 365 → ≈ 1.115×, i.e. inside the F1 band, not at the bar; the rebased measurement decides.
- F2: if byte-identity ever fails, it is a bug in the fix (the design keeps the enumeration's meaning exactly): find and fix the difference, never rebaseline.
- F3: if `hashes <= 4` proves fragile (HashMap resizing rehashes), loosen to `hashes <= 2 * (crossings + seeds)` (today effectively `2 * 3`: the guard has no uncrossed seed), never towards a per-pop number (master ≈ 1,094).

Probe-table notes (revision 2, sol): the counters count key LOOKUP attempts, not hash calls; `pop_u` in `ep-counters.patch` increments before checking that an uncrossed node exists or survives the cut (an over-count of uncrossed pops); the `MAX_STATES` limit is 25,000 permitted expansions plus the terminating 25,001st counted pop (digest.rs:1019-1029).

**Measurement plan.** Before = base / `c7e5f97e` / `830a150c` binaries (`/work/bench/289/` and `/work/bench/160-6/regress*/`); after = the branch REBASED onto `6172492f`, release-fast. One CDO process at a time, interleaved, 3 rounds each on `h` and `d`, `maxrss.py` (CPU = user+sys); `h` walls reported only (`h` is judged on median CPU-s); `d` wall judged against step 6's B1′ (≤ `830a150c` + 8 s); heads-up to alsem-second before the round. Byte-identity `cmp` of `d`/`h` against a base binary built at the same rebased master.

Patches: `/work/bench/289/fix-intern.patch` (the fix), `/work/bench/289/fix-design-prototype.patch` (fix + the dropped extras + `ProofKey` counter + `popped` + guard test; drop the extras when implementing), `ep-counters*.patch` (counters).

## Timeline

- 16:38 claim #289 attempt 1, worktree at `b2c73d4f`. Probes + design dispatched (opus).
- ~19:20 operator (relayed by alsem-main): stop tuning, write the design. ~19:45 design in the ledger; alsem-main reviewed it: ACCEPT (+ minor M1).
- Spec panel r1: both approve-with-fixes (S2 important: d is a wall bound; O1 important: A4 break non-discriminating; O2 important: F1 band undefined / waiver). Revision 2.
- Spec panel r2: sol 10/11 (S2 re-raised: a leftover sentence), opus 11/11 + O8, O9 minor. Revision 3.
- Spec panel r3: both approve. **Converged**, 13 entries, all fixed, all accepted. spec_rounds 3 of 5. Measurement note will judge `d` on the median of 3 rounds (Opus wording point).
- Plan (1 task) `b56f8a51` → rebased `be30a3bd`. Rebase onto `6172492f` (#160 step 9) clean; `enumerate_proofs` region unchanged by it.
- Task 1 (opus) `b086d5fc`, 1 attempt: ships fix-intern + `ProofKey` + `popped` + guard + CHANGELOG (no extras). One deviation: a local `type Row` alias in the guard test for clippy (after the proofs; no behaviour change). `task-affected` 4210 passed; `check-goldens` 1170 passed, nothing moved; check-diff clean.
- ci-steps-task at `b086d5fc` exit 0 (53 s, supervised; `logs/ci-steps-task/20261024T200504-2f0ffdbb.log`). Draft PR #295.
- Final round 1 (final_rounds 1 of 5): sol approve (no findings); Opus approve with fixes (3 minor wording: ×15 comment, `popped` doc, guard call-site ids); code-review skill (7, no correctness bug): CR1 guard counts only `ProofKey` hashes (bypassable) → fix; CR2 pin `hashes == 2` → fix; CR3 + CR5 `IndexSet::insert_full` (no hidden length rule; stored hashes, no growth rehash) → fix; CR4 remaining ~10% (hop-string interning) → out of scope, covered by F1; CR6 = Opus 1; CR7 `Node.crossed` doc → fix. The final CDO bench, started at `b086d5fc`, was STOPPED before the `d` rounds because CR3/CR5 change the measured path; it re-runs after the fix.
- `e4b5659d` (FC1, FC2, FC3+CR5, FC6, FC7): guard measures exactly 2 hashes with IndexSet; A5(a)/(b) and A2(i) re-run (proof table). ci-steps-task exit 0 (`logs/ci-steps-task/20261024T201956-f42db880.log`), pushed.
- Final round 2 (final_rounds 2 of 5): both approve, all six ids accepted. **Final panel converged**: 19 entries; FC4 deferred (covered by F1), the rest fixed; all accepted by sol and opus. Final CDO bench re-started on `e4b5659d` (phase 1: `h` + byte-identity; phase 2: timed `d` after alsem-second pauses its builds).
- Bench done (see Measurements): `h` 1.121 (F1 band) → operator accepted 20:41 UTC; `d` passes B1′; byte-identical.
- Step 11: origin/master unmoved → no rebase. **B = `6172492f383bf666d9cd70cf8dceaeac914337a2`, H = `e4b5659d909cf0244c02f0042c499431379cd478`.** Full set started.

## Discrimination proofs

| test | mutation | fail output | pass output | commit |
|------|----------|-------------|-------------|--------|
| guard `proof_walk_hashes_keys_per_crossing_not_per_partial_path` (red on master: `ProofKey` + `popped` + test on master's `HashSet<Rc<ProofKey>>` walk) | — (master behaviour) | `1094 key hashes for 1097 pops` (paths and `popped == 1097` hold; only the hash check fails) | fix measures 2 key hashes for 1097 pops (temporary `<= 0` bound, reverted to `<= 4`) | `b086d5fc` |
| A2(i) `twin_proofs_count_once_toward_the_cap` | raw counting (`found[key] = true; found_count += 1` on every new raw proof) | twin fails (left `[([0,1],0),([0,1],1)]`, right 4 paths); other 85 pass incl. the guard (F1-S1's stated limit) | 86/86 `l5::digest*` | `b086d5fc` |
| A2(ii) `proof_cap_counts_distinct_candidates_after_the_boundary_cut` | crossed paths onto `queue` instead of `crossed_q` | fails `cap 2` (got `cs-a, cs-b`, want `cs-a, cs-c`); guard fails too (popped 1099) | 86/86 | `b086d5fc` |
| A2(iii) same | raw counting + both found-checks removed (3 sites) | fails `cap 3`; twin and guard (popped 1099) fail | 86/86 | `b086d5fc` |
| A4 proof_cap, guard, `b8_proofs_are_cut_at_the_boundary_and_keep_their_class` | `k < found.len()` at the pop and crossing-push tests (2 sites) | all three fail (proof_cap `([], false)`; guard `paths == []`; b8 `("-", true, 0)` vs `("XRec.Fill", true, 0)`); twin green, as designed | 86/86 | `b086d5fc` |
| A5(a) guard | master's `HashSet<Rc<ProofKey>>` restored at 5 sites (0 `key_ids` / `found_count` / `intern(` / `found[` left) | only the guard fails: `1094 key hashes for 1097 pops` | 86/86; revert `cmp`-identical to the clean file | `b086d5fc` |
| A5(b) guard | drop-at-pop `if …found[k] { continue }` deleted | only the guard fails: popped 1099 vs 1097 (both F1-S1 tests green) | 86/86 | `b086d5fc` |
| guard on IndexSet (`assert_eq!(hashes, 2)`) | — | measured 2 key hashes for 1097 pops | ok | `e4b5659d` |
| A5(a) guard (IndexSet code) | master's `HashSet<Rc<ProofKey>>` + per-pop `contains` at all 5 sites (no `insert_full` / `found_count` / `intern(` left) | only the guard fails: `1094 key hashes for 1097 pops` (left 1094, right 2) | 86/86; revert `cmp`-identical | `e4b5659d` |
| A5(b) guard (IndexSet code) | drop-at-pop deleted | only the guard fails: popped 1099 vs 1097 | 86/86 | `e4b5659d` |
| A2(i) twin (IndexSet code) | raw counting | only the twin fails (left 2 paths, right 4); guard green (its stated limit) | 86/86 | `e4b5659d` |

## Measurements (final, rebased head `e4b5659d` on `6172492f`; `/work/bench/289/final/`)

Release-fast; one CDO process at a time; `MIMALLOC_PURGE_DELAY=0`; CPU = user+sys via `maxrss.py`; the pinned CDO workspace @ bc3ccb18. `h`: 3 interleaved rounds of [ref `c7e5f97e`, after `e4b5659d`, base-rebased `6172492f`].

| `h` | round 1 | round 2 | round 3 | median |
|---|---|---|---|---|
| ref `c7e5f97e` CPU-s | 351.92 | 364.83 | 377.10 | **364.83** |
| after `e4b5659d` CPU-s | 408.84 | 409.11 | 411.41 | **409.11** |
| base-rebased (master) CPU-s | 1536.74 | 1632.51 | 1639.21 | **1632.51** |
| after wall s | 46.54 | 52.04 | 55.07 | 52.04 |
| after / base peak MB | 3351 / 3216 | 3345 / 3197 | 3314 / 3209 | 3345 / 3209 (+4%; ref 3402–3476) |

`h` ratio median(after)/median(ref) = 409.11 / 364.83 = **1.121** → inside the F1 band (> 1.1, ≤ 1.15); 4.0× faster than master. Byte-identity: `h` after = base-rebased in all 3 rounds (441,330,873 B, sha256 `6d6505f6af15…`); `d` after = base-rebased (20,016,778 B, `1e853d95291e…`).

**F1 fired.** Numbers sent to alsem-main for the operator; **operator ACCEPTED at 20:41 UTC** (AskUserQuestion "Accept (recommended)", relayed by alsem-main) — the same outcome as the pre-stated 15-min default. Consequences per F1: A3's `h` CPU half under `## Not delivered`; follow-up issue for the remaining ~12% filed in the same change (Discoveries); PR body `Refs #289`; the operator closes #289 after the merge.

`d` timed rounds (B1′, wall ≤ median(`830a150c`) + 8 s), 3 interleaved rounds [d-ref `830a150c`, after], other session's builds paused:

| `d` | round 1 | round 2 | round 3 | median |
|---|---|---|---|---|
| d-ref wall s | 27.31 | 25.98 | 27.47 | 27.31 |
| after wall s | 30.28 | 31.16 | 31.91 | **31.16** |
| d-ref / after CPU-s | 117.89 / 129.30 | 116.75 / 126.98 | 120.31 / 130.47 | 117.89 / 129.30 |
| d-ref / after peak MB | 3037 / 3088 | 3060 / 3096 | 3070 / 3087 | ~3060 / ~3088 |

B1′ bound = 27.31 + 8 = 35.31 s; after median 31.16 s → **passes** (4.15 s under; every run inside). Raw rows `/work/bench/289/final/p2-res.txt`.

## Findings register summary

`findings.json`: 19 entries. Spec panel (3 rounds): S1–S3 (sol), O1–O9 (opus), M1 (alsem-main's design review) — all `fixed`. Final panel (2 rounds; code-review skill at high + sol + opus): FC1, FC2, FC3 (incl. CR5), FC6, FC7 `fixed`; FC4 `deferred` (minor; the remaining gap is F1 + the follow-up discovery). Every entry accepted by sol and opus.

## Gate results

| gate | head | exit | log |
|---|---|---|---|
| ci-steps-task | `b086d5fc` | 0 (53 s) | `.agent/runs/20261024-163804-d1d6b6/logs/ci-steps-task/20261024T200504-2f0ffdbb.log` |
| ci-steps-task | `e4b5659d` | 0 (78 s) | `.agent/runs/20261024-163804-d1d6b6/logs/ci-steps-task/20261024T201956-f42db880.log` |
| **ci-steps-all** (H) | `e4b5659d` | **0** (417 s, supervised) | `.agent/runs/20261024-163804-d1d6b6/logs/ci-steps-all/20261024T204335-22495b60.log` |
| **check-goldens-coverage** (H) | `e4b5659d` | **0** (supervised) | `.agent/runs/20261024-163804-d1d6b6/logs/check-goldens-coverage/20261024T205032-cc4793c1.log` |
| **check-goldens** (H) | `e4b5659d` | **0** (tree stamp trusted; supervised) | `.agent/runs/20261024-163804-d1d6b6/logs/check-goldens/20261024T205032-23780321.log` |
| **cdo-gate** (H) | `e4b5659d` | **0** (659 s, supervised) | `.agent/runs/20261024-163804-d1d6b6/logs/cdo-gate/20261024T205032-77497a3f.log` |

Tree clean after the full set. Grammar c89c2ecb; the pinned CDO workspace @ bc3ccb18.

## Discoveries

```json
[
  {
    "subsystem": "l5 digest (enumerate_proofs candidate keys)",
    "locator": "src/engine/l5/digest.rs enumerate_proofs / key_of / ProofKey (after #289)",
    "symptom": "After #289, alsem digest on CDO h still costs ~1.12x the pre-F1-S1 baseline (median 409 vs 365 CPU-s; master was 1,633). Remaining cost: crossed-first ordering makes partial paths deeper (avg max length 29 vs 23.6: ~38% more revisit steps, ~10% more pushes, Node 32 -> 56 bytes), and key_of clones every hop's JSON string at each crossing.",
    "kind": "enhancement",
    "origin_issue": 289,
    "reproducer": "alsem digest <pinned CDO workspace> --file src/Features/HandledState/CDOUnhandledSalesPagesMgt.Codeunit.al --format json --deterministic (release-fast, MIMALLOC_PURGE_DELAY=0), 3 interleaved rounds against bin-c7e5f97e; CPU via maxrss.py. Bench driver /work/bench/289/final/phase1.sh.",
    "pre_existing": true,
    "capability": "enumerate_proofs keeps F1-S1's distinct-candidate cap without the deeper-path overhead: e.g. intern hop JSON to small ids so ProofKey is a Vec<u32> (no string clones or string hashing), or revisit the crossed-first queue order.",
    "acceptance": "CDO h digest median CPU-s <= 1.1 x c7e5f97e's median over 3 interleaved rounds; output byte-identical; F1-S1's two tests and the #289 guard stay green."
  }
]
```

## Not delivered

- **A3, `h` CPU half:** median ratio 1.121 against the 1.1 bar (fix 409.11 vs ref 364.83 CPU-s; 4.0× faster than master's 1,632.51). F1 fired; the operator accepted at 20:41 UTC. The remaining ~12% is the discovery above. `d` (B1′) IS delivered; byte-identity (A4), F1-S1 (A2), the guard (A5) and the root cause (A1) are delivered. Hence the PR says `Refs #289`; the operator closes #289 after the merge.
- FC4 (hop-string interning) deferred into that discovery.
