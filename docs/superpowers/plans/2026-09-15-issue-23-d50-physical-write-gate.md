# Plan — issue #23: d50's transaction-managing gate counts physical writes

Spec: `.agent/issue-23/ledger.md` (design v3, spec panel CONVERGED — 16 register entries,
all `fixed`, all accepted by both reviewers).

Acceptance tests are already committed to the working tree and RED in exactly the predicted
shape: A2, A7, the population test and two A1 unit tests fail; A3, A5, A6, A9 and the A1
control pass. That asymmetry is the precondition for both tasks below.

Two tasks. The order is load-bearing: T2 cannot run before T1, because regenerating the
goldens while the gate is still temp-inclusive would bless the four-finding bug as the
expected output.

---

## Task 1 — the gate swap, its doc corrections, and the CHANGELOG

**Change.** `src/engine/l5/detectors/d50.rs:74`:

```rust
ctx.cone_derived.writes_tables_count_of(&summary.routine_id) >= TRANSACTION_THRESHOLD_TABLES
```
becomes
```rust
ctx.cone_derived.writes_physical_tables_count_of(&summary.routine_id) >= TRANSACTION_THRESHOLD_TABLES
```

with a comment stating why: a temporary record never dirties the transaction, so counting
temp writes toward "this routine manages a transaction" admits routines that manage none;
d8 already gates on the physical count (`d8.rs:40-42`) and this aligns d50 with it.

**Doc corrections that ship in the same commit.**

1. `d50.rs:59-60` documents the gate as `writesTablesOf(summary).length >= TRANSACTION_THRESHOLD_TABLES`.
   That sentence is false after the swap; it must name the physical accessor.
2. Do NOT touch `affected_tables` (`d50.rs:342`). It stays the temp-inclusive span footprint
   and is not a second gate. The CHANGELOG must not describe the result as "proven dirty
   physical tables", which would overstate both this count's conservative semantics
   (unknown / parameter-dependent / absent temp states still count) and `affectedTables`.

**CHANGELOG** under `## [Unreleased]` → `Fixed`, scoped precisely: physical-count alignment
of the COUNT branch; the NAME heuristic is unchanged, so a posting-named temp-only routine
still qualifies. Note that the narrowing is not limited to temp-only routines — two physical
plus one temporary also drops 3 → 2.

**Exit condition.** Every non-golden test green:
- `cargo test --test gap gap_issue23 -- --test-threads=1` → 7 passed, 0 failed
- `cargo test -p al-sem --lib d50::tests::a1` → 3 passed, 0 failed
- `cargo test --test gap` whole umbrella, and `-p al-sem --lib` whole lib, green
- `--test r4` and `--test l2_ir` still RED — that is T2's work, not a regression

**A4, the discrimination proof, is this task's deliverable and not optional.** The current
tree already records the "broken" state, so the proof is its mirror image: after the fix,
break `d50.rs:74` back to `writes_tables_count_of`, ASSERT the break applied
(`s.count(old) == 1` before writing), run the two commands above, record that
`a2_temp_only_writer_produces_no_finding`, `a7_mixed_writer_below_physical_threshold_produces_no_finding`,
the population test and the two A1 tests FAIL while `a3`/`a5`/`a6`/`a9` and the A1 control
stay green, restore by byte-copy, re-run, record green. A proof that comes back green is
evidence about the TEST — investigate and report it, never shrug.

**Before committing:** `check-diff --base master --head HEAD --issue 23 --cwd <worktree>`.

---

## Task 2 — regenerate both golden families together, after triage

Two families move, and only these two:

- `tests/r4-goldens/ws-d50-temp-gate.r4.golden.json` — seeded at `findingCount` 0 / empty
  `findings` because the regeneration rewrites but cannot mint. It becomes **2 findings**:
  `StageRows` and `PostBuffers`.
- `tests/ir-l2-goldens/l2_features.snapshot` — grows by **5 routines** (the four writers plus
  the worker's `OnRun`). Purely fixture-driven; nothing to do with the gate.

**`ws-d50-pos` and `ws-d50-neg` must be BYTE-UNCHANGED.** `pos` is transaction-managing by
NAME and `neg`'s Run is unchecked, so neither depends on the count. If either moves, that is
an unexplained line and a discovery — not a rebaseline.

**Order of operations.**
1. `check-goldens --verify-coverage` first, so an unobserved family is never mistaken for an
   unaffected one.
2. Dispatch `golden-diff-triager` over the moved lines. Every line classified, or it is not
   blessed.
3. Write the triage receipt to `<main checkout>/.agent/golden-triage.md` — the repo's
   `golden-regen-guard.sh` requires a receipt under 90 minutes old naming every MOVED path,
   with WHY each moved. Write it in the MAIN checkout even though the work is in a worktree.
   Do not use the `GOLDEN_TRIAGED=1` escape hatch.
4. Regenerate all families together via `scripts/check-goldens` with the regenerate flag —
   never one family at a time, which is the trap CLAUDE.md names by example.
5. Re-run `scripts/check-goldens` clean, and confirm **A8 now becomes observable**: with the
   golden correct, `run_smoke_entry` no longer panics on the byte compare, so the
   end-of-test assertion that `ws-d50-temp-gate` appears exactly once in completed ported
   results finally executes. Its own proof — remove the Smoke entry and watch THAT assertion
   fail — belongs here, once it is reachable.

**Exit condition.** `scripts/check-goldens` exit 0 with `ws-d50-pos` / `ws-d50-neg`
byte-identical, A8 executing and passing, and the triage receipt committed alongside the
regenerated goldens with the classification in the commit message.
