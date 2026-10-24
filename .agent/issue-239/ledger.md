# Issue #239 — an `<owner>::<routine>` selector whose owner names no match falls back to other objects' routines

## Claim

```json
{"run_id":"20261023-134435-ba7359","issue":239,"attempt":1,"session":"claude-code-session:e2bf58b3-5d1b-467f-8c3b-1c59a8d42f0c","branch":"issue/239-agent-discovery-l5-selector-inde-a1","worktree":"/work/al-sem-issue-239-a1","body_hash":"71d7fa836ed771122b52d17f08b28a8380307bc1d42860df9ef2e54804a24d94","title":"…","reconciled":[]}
```

Issue body hash: `71d7fa836ed771122b52d17f08b28a8380307bc1d42860df9ef2e54804a24d94` (claim `body_hash`).
Ponytail: the smallest complete fix.

## Classification

**bounded.** One rule in one function (`narrow_to_owner`, `src/engine/l5/selector_index.rs`), its two call sites, and the two tests from #78 that pin the old fallback.

## Assumption probes

Run 2026-10-23 at base `3a2d5923`, worktree `/work/al-sem-issue-239-a1`.

| probe | result | evidence |
|---|---|---|
| P1 where the cascade lives | `resolve_selector` (`selector_index.rs:140-197`) runs forms 1-5. A real routine's identity display is the bare routine name, so `<owner>::<routine>` falls through forms 2-4 and matches in form 5 (`:186-194`), which looks up only the segment after the last `::` and ignores the object part | code read; A6 matches via form 5 |
| P2 where the owner narrows | `narrow_to_owner` (`selector_index.rs:204-227`) keeps matches whose `owner_of(id)` display key equals the object part; **when none does it returns all matches** (`:222-226`). The owner display is `"<Type> <Name>"`, unquoted (`object_display`, `src/engine/l4/effect_query_cli.rs:436-438`), from `OwnerIndex` (`:465-520`), which covers every primary routine | code read |
| P3 callers | Two: digest `resolve_changed_roots` (`digest_cli.rs:403-406`), which prove also calls (`prove.rs:898`); and `fingerprint_query` (`fingerprint_query.rs:670-671`). No other caller | `grep -rn narrow_to_owner` |
| P4 how "unmatched" is reported today | **digest**: `ChangedRootsDiagnostic::SelectorUnmatched { selector }` (`digest_cli.rs:170-172`, pushed `:407-410`), JSON `{"kind":"selector-unmatched","selector":…}` (`:1258-1261`), exit 2 when no root (pinned by `tests/cli/cli_b_digest_exit_oracles.rs:75-108`). **fingerprint**: `FingerprintQueryDiagnostic::SelectorUnresolved { selector }` (`fingerprint_query.rs:183-185`, pushed `:673-676`), JSON `selector-unresolved` with `triedForms` detail (`:1090-1094`), human `error: --routine '<sel>' did not match any routine (tried: …)` on stderr (`fingerprint_cli.rs:298-303`), exit 2 (`:526-534`). **prove**: zero roots -> a dummy document, `answer: unknown`, exit 2 (`prove.rs:906-944`); prove has no selector-diagnostic channel | code read |
| P5 dependency routines | digest's selector index keeps primary routines only (`build_selector_indexes(snap, primary_app_guid)`); fingerprint and prove build single-app models (`fingerprint_cli.rs:375`, `prove.rs:874`). So every matched id has an owner in `OwnerIndex`; the old comment's "a dependency object" case cannot reach `narrow_to_owner` | code read |
| P6 repro on master (debug `alsem` built at `3a2d5923`) | **Shape 1** (`/work/bench/239/ws`: only `Codeunit "Sel B"` declares `Persist`), selector `Codeunit Sel A::Persist`: prove exit 0, `objectDisplay: "Codeunit Sel B"`; digest exit 0, no diagnostic, one entry for `Codeunit Sel B`; fingerprint exit 0, one block for `Codeunit Sel B`. **Shape 2** (`/work/bench/239/ws2`: both declare `Persist`): `Sel A::Persist` and `Codeunit "Sel A"::Persist` each exit 2 `selector-ambiguous` (both candidates); control `Codeunit Sel A::Persist` exit 0 for Sel A | `/work/bench/239/{p1,d1,f1,d2,d3,d4}.json` |
| P8 other tests/goldens with a qualified selector | none outside `cli_object_display.rs` (the `{TABLE_OWNER}::OnInsert` test there uses the full owner, still kept) | grep of `tests/cli/*.rs` and `tests/` goldens for `"selector": "…::"` |
| P7 tests that pin the fallback | **A6** `tests/cli/cli_object_display.rs:415` `owner_qualified_selector_narrows_to_its_owner` pins only the narrowing (owner present): unchanged by this fix. **U3** `selector_index.rs:238` `narrow_to_owner_keeps_the_named_owner_or_everything`: its last assert (`:250-253`) pins `"Sel A::Persist"` (bare object name) -> both ids. This is the fallback; it must be rewritten on purpose (bare name now narrows to `a`). #78 ledger `.agent/issue-78/ledger.md:157-159` records both proofs and states the fallback as a known limit | code read |

**Verdicts.** P1, P2, P3 hold as the issue says. P4: the unmatched path exists in all three surfaces; prove reports it only as exit 2 + `unknown`. P5 removes the reason the fallback existed. P6 confirms both shapes on all three surfaces, plus a third: the AL-quoted owner (`Codeunit "Sel A"`) does not narrow either. P7: only U3's last assert pins the fallback; A6 stays green. P8: no golden pins the fallback.

## Design

**The rule** (in `narrow_to_owner`, the ONE place; both callers inherit it):
1. Only an object-qualified match (form 5) is narrowed, as today.
2. The object part `want` is a display key (`normalize_display_key`) with `"` removed, so the AL spelling `Codeunit "Sel A"::Persist` works too (owner displays are unquoted; AL names cannot contain `"`).
3. **Full owner first, then bare name** (O3). Keep the matches whose owner key equals `want` (`Codeunit Sel A`). Only when there are none, keep the matches whose owner key with the type word stripped (`strip_type_word_prefix`, already in this file) equals `want` (bare name `Sel A`). So `Codeunit Sel B::Persist` never turns ambiguous through a `Table "Codeunit Sel B"`. Case and whitespace fold the same way the identity table's keys do.
4. **No fallback.** When nothing is kept the result is empty, so each caller's existing unmatched path fires: digest `selector-unmatched` (exit 2 with no root), fingerprint `selector-unresolved` (exit 2), prove `unknown` (exit 2). `narrow_to_owner` keeps its signature (`-> Vec<String>`); the fallback (`selector_index.rs:222-226`) is deleted. No diagnostic type, renderer or caller changes (O1, O2). The stale doc-comment sentence ("When no match has that owner, the matches stay…", `:201-203`) goes too.

**Ambiguity.** A bare name that two owners share (`Codeunit "Sel B"` and `Table "Sel B"` both declare `Persist`; selector `Sel B::Persist`) keeps both -> 2 matches -> the existing `len() >= 2` paths (`digest_cli.rs:411`, `fingerprint_query.rs:677`), with owner-labelled candidates (#78). No new code. Per command (S2): digest `selector-ambiguous` whose `candidates` name both owners, exit 2; fingerprint human `is ambiguous` plus one `  - <owner>::Persist  (` line per owner, exit 2; fingerprint JSON `selector-ambiguous` (no candidates: its shape today, `fingerprint_query.rs:1095-1101`); prove exit 2, `answer: unknown` (prove drops selector diagnostics, `prove.rs:906-944`).

**Fixtures** (O4).
- `same_name_workspace` (A6, A7, M2, M3) is not changed, so A7's exact two-candidate assert (`cli_object_display.rs:474-477`) still holds.
- A new `one_owner_workspace` for M1, the issue's exact shape (the P6 repro, `/work/bench/239/ws`): `Codeunit "Sel A"` (50101) declaring only `Other`, `Codeunit "Sel B"` (50102) declaring `Persist`. One form-5 match, so the old fallback answers with Sel B at exit 0, which is the bug.
- A new `shared_name_workspace` for M4/M6: `Codeunit "Sel B"` (50101), `Table "Sel B"` (50111) and `Table "Codeunit Sel B"` (50112), each declaring `Persist` (tables with one field).

**Acceptance matrix** (digest, fingerprint and prove on each row):

| row | selector | workspace | expected |
|---|---|---|---|
| M1 | `Codeunit Sel A::Persist` | `one_owner_workspace` (Sel A has no `Persist`) | digest `selector-unmatched`, exit 2, no entry; fingerprint exit 2 + `did not match`; prove exit 2, `answer: unknown`. Never Sel B's routine |
| M2 | `Sel A::Persist` | `same_name_workspace` | Sel A's routine (S1): digest exit 0, one entry with `routine.objectDisplay == "Codeunit Sel A"` and Sel A's stable id; fingerprint exit 0, one block with the same owner and id; prove exit 0, `payload.routine.objectDisplay == "Codeunit Sel A"` and the same id |
| M3 | `Codeunit "Sel A"::Persist` | `same_name_workspace` | as M2 (S1) |
| M4 | `Sel B::Persist` | `shared_name_workspace` | ambiguous, per command as in Ambiguity (S2); candidates are exactly `Codeunit Sel B::Persist` and `Table Sel B::Persist` (`Table Codeunit Sel B`'s bare name is `Codeunit Sel B`, which does not fit) |
| M5 | `Codeunit Sel A::Persist` | `same_name_workspace` | unchanged: A6 |
| M6 | `Codeunit Sel B::Persist` | `shared_name_workspace` | the codeunit's routine alone (full owner wins over `Table "Codeunit Sel B"`'s bare name): digest/fingerprint/prove exit 0, owner `Codeunit Sel B` |

**Tests** (in `tests/cli/cli_object_display.rs`, except U3), each with its break -> fail:
- **A1** (M1). Break: restore the fallback (`if narrowed.is_empty() { matches } else { narrowed }`) -> all three exit 0 with Sel B's routine (the P6 repro). A1 also asserts the digest diagnostic kind `selector-unmatched` and fingerprint's `did not match` line, not just the exit code.
- **A2** (M2 + M3, owner and stable id on all three commands). Breaks: (i) drop the bare-name step -> M2 exit 2; (ii) drop the `"` removal -> M3 exit 2; (iii) make the bare-name step keep a match other than the named owner's (e.g. `matches.last()`) -> the owner/id asserts read Sel B (the wrong-owner mutation S1 asks for).
- **A3** (M4 + M6, all three commands). Breaks: (i) **precedence (O3)**: compare full owner OR bare name in one pass -> M6 keeps the codeunit and `Table "Codeunit Sel B"`, so exit 2 `selector-ambiguous` instead of exit 0; (ii) keep only the first bare-name match -> M4 digest and fingerprint exit 0 instead of ambiguous.
- **U3** rewritten (unit, `selector_index.rs`, O6), owners `a = "Codeunit Sel A"`, `b = "Codeunit Sel B"`, `c = "Table Codeunit Sel A"`:
  - `Sel A::Persist` on `[a, b]` -> `[a]` (bare name);
  - `Codeunit "Sel A"::Persist` on `[a, b]` -> `[a]` (quote fold);
  - `Codeunit Sel A::Persist` on `[a, b, c]` -> `[a]` (full owner wins; `c`'s bare name `Codeunit Sel A` also fits);
  - `Codeunit Sel C::Persist` on `[a, b]` -> `[]` (no fallback);
  - `full-display` form on `[a, b]` -> `[a, b]` (untouched).
  Breaks: restore the fallback -> the `[]` assert reads `[a, b]`; one-pass OR -> the precedence assert reads `[a, c]`; drop the `"` removal -> the quote assert reads `[]`.
- A6, A7 stay as they are (regression).

**Failure shapes.** (a) A selector that used to resolve through the fallback (wrong owner, or an owner spelled another way, e.g. with an object number) is now exit 2. Intended; no hint names the real owners (Not delivered). (b) An object with an empty model name has the owner display `<Type>` alone; it is selectable by `<Type>::<routine>` only (full owner), never by a bare name (there is none). Unchanged corner. (c) **Stated limit (O5).** Dropping the fallback assumes every id the selector index keeps has an owner in `OwnerIndex`. The two decide "primary" differently: the selector index by the stable id's app guid (`role_of_stable_id`, `selector_index.rs:88-91`), `OwnerIndex` by the model role (`is_primary_routine` -> `routine_role`, `effect_query_cli.rs:355-357`, used at `:484-487`). An id the second misses has owner `""`, so it can never be selected in `<owner>::` form (its bare routine name and its stable id still work). No probe found such an id; nothing pins it.

**Pre-stated fallbacks.**
- If full-owner-first (rule 3) moves any test or golden other than U3/A3, keep the rule (it is the correct one) and record the moved test as an intended change with its reason.
- If the `"` removal (rule 2) moves any golden, drop it, drop M3, A2's break (ii), and U3's quote assert and its break (SN1), and record "AL-quoted owner spelling" under Not delivered.
- If the bare-name step cannot be made to select Sel A on all three commands, stop and report: it is half the Acceptance, so it is not dropped.
- If `shared_name_workspace` fails to build, or its owner displays differ from `Codeunit Sel B` / `Table Sel B` / `Table Codeunit Sel B`, rename the objects (keeping the shape: two owners sharing a bare name, and one owner whose bare name equals another's full owner) and update M4/M6.
- If `one_owner_workspace` on the new build does not give the M1 result, the fallback is still reached by some other path: stop and report, do not loosen A1.

**Tasks (1).**
- T1: the rule in `narrow_to_owner` (quote fold, full owner then bare name, no fallback), its doc comment, U3 rewrite, the two fixtures, A1/A2/A3, CHANGELOG `Fixed` under Unreleased (the 2.0.1 #78 entry says "the selector matches as before"; the new entry supersedes it).

**Golden moves expected.** None. No cli golden uses an object-qualified selector (P8; the only selector-error golden, `tests/cli-b-goldens/fingerprint/ws-d8-commit-in-tx.selector-error.json`, uses a selector with no `::`), and no diagnostic shape changes. If `scripts/check-goldens` moves anything, it is unexplained and stops the task.

## Base/Head SHAs

Base `3a2d592391ca6550974288d3c7029ddddc3d8d57`. Head: —

## Timeline

- 2026-10-23: claim, probes (master debug build, repro under `/work/bench/239/`), design.
- 2026-10-23: spec panel r1 (sol S1-S2, opus O1-O6): all 8 fixed; hint dropped (O2, conductor), tasks 2 -> 1.

## Acceptance matrix

See Design. Results:

| row | test | result at `ccd7c687` |
|---|---|---|
| M1 | A1 | green: digest `selector-unmatched` exit 2, no entry; fingerprint exit 2 `did not match`, no Sel B; prove exit 2 `unknown` |
| M2, M3 | A2 | green: all three exit 0, owner `Codeunit Sel A`, Sel A's stable id |
| M4, M6 | A3 | green: M4 ambiguous with exactly `Codeunit Sel B` / `Table Sel B` (digest, fingerprint), prove exit 2 `unknown`; M6 the codeunit alone on all three |
| M5 | A6 | green, unchanged |
| unit | U3 | green |

## Discrimination proofs

Each break made with Edit in `narrow_to_owner`, run (U3 + `cli_object_display::`), reverted with Edit; after the last revert `git diff` showed only the rule and both runs green (`/work/bench/239/revert-green.log`). Logs `/work/bench/239/<break>.log`.

| break | U3 | A1 | A2 | A3 |
|---|---|---|---|---|
| B1 restore the fallback (bare empty -> all matches) `b1-fallback` | FAIL `:275` `["a","b"]` vs `[]` | FAIL `:621` digest exit 0 vs 2 | pass | pass |
| B2 one-pass OR (full owner OR bare name) `b2-one-pass-or` | FAIL `:258` `["a","c"]` vs `["a"]` | pass | pass | FAIL `:586` M6 `Codeunit Sel B::Persist` exit 2 vs 0 |
| B3 drop the `"` removal `b3-no-quote-removal` | FAIL `:262` `[]` vs `["a"]` | pass | FAIL `:586` M3 `Codeunit "Sel A"::Persist` exit 2 vs 0 | pass |
| B4 drop the bare-name step `b4-no-bare-name` | FAIL `:261` `[]` vs `["a"]` | pass | FAIL `:586` M2 exit 2 vs 0 | FAIL `:681` M4 `selector-unmatched` vs `selector-ambiguous` |
| B5 keep only the first bare-name match (`.take(1)`) `b5-first-bare-only` | **pass** (U3 has no shared-bare-name row; the design names A3 for this break) | pass | pass | FAIL `:679` M4 digest exit 0 vs 2 |
| B6 bare-name step keeps another owner's match (`keyed.last()`) `b6-bare-wrong-owner` | FAIL `:262` `["b"]` vs `["a"]` | pass | FAIL `:589` owner `Codeunit Sel B` vs `Codeunit Sel A` | FAIL `:689` candidates `Table Codeunit Sel B` twice |

Every new or changed test fails under at least one break. A1 stops at its first assert (digest), so B1's fingerprint/prove exit 0 was not observed through the test; the P6 repro shows all three.

## Findings register summary

Spec r1: 8 findings (sol S1-S2 important; opus O1 important, O2-O6 minor), all `fixed` in `## Design`. Register: `.agent/issue-239/findings.json`.

## Gate results

| gate | result | log |
|---|---|---|
| `scripts/check-goldens` (no regen) | exit 0, 989 passed, 4 skipped; no golden moved | `/work/bench/239/check-goldens.log` |
| `scripts/ci-steps task-affected` | exit 0, 3819 passed, 8 skipped | `/work/bench/239/task-affected.log` |
| `agentflow check-diff` before / after the task commit | `"reasons": []` both | `/work/bench/239/check-diff-pre.log` |
| `scripts/ci-steps task` at `ccd7c687` | exit 0, 3819 passed, 8 skipped | `/work/bench/239/task.log` |

Commits: plan `6e890aae`, task `ccd7c687`. No pre-stated fallback fired.

## Discoveries

—

## Not delivered

- **No hint listing the real owners** on an unmatched `<owner>::<routine>` selector, in any command. Reason: the issue's Acceptance asks only for "selector-unmatched, not another object's routine"; a hint would widen two public diagnostic enums, change `narrow_to_owner` to a `Result`, add renderer branches and break the `item16_unresolved_selector_human_message` oracle (`tests/cli/cli_b_fingerprint_oracles.rs:683`), and prove has no channel for it anyway (spec r1 O1, O2; conductor decision).

## Acceptance tests (red)

Run 2026-10-23 on the base rule (tests written, `narrow_to_owner` unchanged). Logs: `/work/bench/239/red-u3.log`, `/work/bench/239/red-cli.log`.

| test | red failing line | observed |
|---|---|---|
| U3 `selector_index::tests::narrow_to_owner_keeps_the_named_owner_only` | `src/engine/l5/selector_index.rs:253` (`Sel A::Persist` on `[a, b]`) | `left: ["a", "b"]`, `right: ["a"]` (the fallback) |
| A1 `cli_object_display::owner_qualified_selector_with_no_such_routine_is_unmatched` (M1) | `tests/cli/cli_object_display.rs:621` (digest exit code) | `left: 0`, `right: 2`: digest selected Sel B's routine |
| A2 `cli_object_display::bare_and_quoted_owner_selectors_select_that_owner` (M2, M3) | `tests/cli/cli_object_display.rs:586` in `assert_selects` for `Sel A::Persist` (digest exit) | `left: 2`, `right: 0`: `selector-ambiguous` |
| A3 `cli_object_display::shared_bare_name_is_ambiguous_and_full_owner_wins` (M4, M6) | `tests/cli/cli_object_display.rs:689` (M4 candidates) | `left: ["Codeunit Sel B::Persist", "Table Codeunit Sel B::Persist", "Table Sel B::Persist"]` (all three, the fallback), `right:` the two `Sel B` owners |

A6, A7 and the other `cli_object_display` tests stay green on the base.

## Attempt 2

**Base.** `b3686bde` (master after #83, #240, #253, #254, #160's digest rewrite), worktree
`/work/al-sem-issue-239-a2`, branch `issue/239-selector-owner-a2`. Plan commit `1227e910`
(carried from attempt 1). Run `20261023-212135-0a81a5`. Logs: `/work/bench/239a2/`.

**Re-checked against the current code.**
- `OwnerIndex` (`src/engine/l4/effect_query_cli.rs:466-521`) still keys owners by stable
  routine id from the model. `object_display` is `"<Type> <Name>"` with the LOGICAL name
  since #240, so a name may hold `"` (e.g. `A "B" C`). Its `name_by_object_id` is still a
  last-insert-wins `HashMap` (#241 is open).
- The owner cannot come from the snapshot identity table (the tableextension stub
  overwrite, `effect_query_cli.rs:451-456`), so `build_selector_indexes(snap, ..)` cannot
  know owners.
- The callers are unchanged: digest `resolve_changed_roots` (`digest_cli.rs:405-408`,
  which prove reaches) and fingerprint (`fingerprint_query.rs:670-671`). Both call
  `resolve_selector(sel, &idx)` and then `narrow_to_owner(sel, form, matches, owner_of)`.

**What changed versus attempt 1.**
1. **CR6 under the hands-off rule.** fingerprint_query.rs may not be edited. So neither
   `resolve_selector`'s nor `narrow_to_owner`'s argument list can change, and the index
   has no owner source. The narrowing now lives in the cascade's RESULT:
   `resolve_selector` returns `SelectorMatches` (private `ids`, plus the parsed form-5
   object part), and `narrow_to_owner` is the only way to read the ids. A caller that
   skips it does not compile (proof B10). Neither caller's file changed.
2. **Rule.** Attempt 1's "full owner first, then bare name" became one decision on the
   object part (CR1):
   - empty: no narrowing (CR4);
   - first word an object type word: full owner only;
   - otherwise a bare object name: the owner without its type word.
   A quoted part (`"Codeunit Sel B"`) is a bare name (quote position). An AL quoted
   name is unquoted with `""` -> `"` (#240 logical names), not by deleting every `"`.
   This applies to the object part and the routine part (CR2). M6 (typed part: the
   codeunit only) and M4 (bare: both `Sel B` owners) still hold.
3. **Tests.** Every CLI test asserts digest, fingerprint and prove in ONE `assert_eq!`
   (`per_command`), so a failure shows all three commands (sol F1, opus G1). A4 is new
   (CR1, sol F2's shape), and so is A5 (CR4). U3 now runs through `resolve_selector` +
   `narrow_to_owner`.

**Commits.** See "commit and rebase" below. At first there were none: `agentflow check-diff` returned `cargo-version-changed` before
the first commit (see Gate). The work is in the working tree.

**Gate.**

| gate | result | log |
|---|---|---|
| `bash scripts/ci-steps task` (working tree, all changes) | exit 0, 3930 passed, 8 skipped; no golden moved (no golden path in `git status`) | `/work/bench/239a2/task.log` |
| `agentflow check-diff --base origin/master --head HEAD` | exit 1, `"reasons": ["cargo-version-changed"]` | `/work/bench/239a2/check-diff-1.log` |

The check-diff cause: `origin/master` moved to `0b0b95c9` ("Bump version to 2.0.3": Cargo.toml,
Cargo.lock, CHANGELOG) after this branch's base `b3686bde`. The two-point diff reads
master's bump as a change on this branch. The branch does not touch Cargo.toml. Per the
run rule, I stopped before committing. A rebase onto `0b0b95c9` would put this entry
under the released `## [2.0.3]` heading. It must move to a new `### Fixed` under
`## [Unreleased]`, and the one-sentence #240 bullet edit would then sit in a released
section.

**Red on the base rule** (`selector_index.rs` stashed, tests kept; `red-base-cli.log`):
- A1: all three commands exit 0 with `Codeunit Sel B`.
- A2: `Sel A::Persist` is ambiguous.
- A3: the M4 candidates include `Table Codeunit Sel B`.
- A4: all three commands select `Table Codeunit Sel B`.
- A5 passes on the base (the old fallback kept `::Persist`'s one match). It pins
  attempt 1's CR4 regression, which B5 shows.

**Discrimination proofs.** Each break was made with Edit in `selector_index.rs` (B10 in
`digest_cli.rs`). The runner checked the break applied by `diff` against the saved good
file and refuses a no-op break. Each break ran U3 + `cli_object_display::` and was
reverted with Edit; `diff` was then empty. The final green run after all reverts:
`green-u3.log`, `green-cli.log`.

| break | U3 | CLI tests failing (each shows all three commands) | log |
|---|---|---|---|
| B1 restore the fallback (`kept` empty -> all) | FAIL `["a#","b#"]` vs `[]` | A1 (digest/fingerprint/prove exit 0, `Codeunit Sel B`), A4 (all three select the table) | `b1-fallback.log` |
| B2 a typed part also matches bare names | FAIL `["a#","c#"]` vs `["a#"]` | A3 M6 (ambiguous with `Table Codeunit Sel B`), A4 | `b2-typed-matches-bare.log` |
| B3 keep the owner part's quotes | FAIL `[]` vs `["a#"]` | A2 `Codeunit "Sel A"::Persist` unmatched on all three | `b3-owner-quotes.log` |
| B4 keep the routine part's quotes (CR2) | FAIL `[]` vs `["a#"]` | A2 `Codeunit "Sel A"::"Persist"` unmatched on all three | `b4-routine-quotes.log` |
| B5 narrow an empty object part (CR4) | FAIL `[]` vs `["a#","b#"]` | A5 unmatched on all three | `b5-empty-part-narrows.log` |
| B6 drop the bare-name match | FAIL `[]` vs `["a#"]` | A2, A3 M4, A4 second assert | `b6-no-bare-name.log` |
| B7 keep only the first kept match (`.take(1)`) | **pass** (no U3 row keeps two) | A3 M4 selects one routine at exit 0 on all three | `b7-first-only.log` |
| B8 read a quoted bare name as typed | FAIL `[]` vs `["c#"]` | A4 `"Codeunit Sel B"::Persist` unmatched | `b8-quoted-bare-read-as-typed.log` |
| B9 no `""` unescape (#240) | FAIL `[]` vs `["d#"]` | none (no CLI row for an escaped owner; U3 pins it) | `b9-no-unescape.log` |
| B10 a caller reads the ids without `narrow_to_owner` (`digest_cli.rs`) | does not compile: `E0616 field ids of struct SelectorMatches is private` | — | `b10-skip-narrowing.log` |

Every new or changed test fails under at least one break.

**Round-1 finding dispositions** (register: `findings.json`, round `final-1`).
- CR1: fixed. A typed part never takes the bare-name step (A4, B2, B8).
- CR2: fixed. The routine part is unquoted (A2, B4).
- CR3: fixed as a stated Limit (L1; #241).
- CR4: fixed. An empty object part is the bare routine name (A5, B5).
- CR5: fixed as a stated Limit (L2).
- CR6: fixed within the hands-off rule (B10); Limit L3.
- CR7: fixed. The plan no longer cites the untracked ledger path, and it describes
  attempt 2.
- sol F1: fixed (one assert per row over all three commands).
- sol F2: fixed. CR1 closes the table case, and the CHANGELOG names each command's
  output.
- opus G1: fixed. The test docs state what the break shows, as observed in the logs.
- opus G2: fixed. The CHANGELOG says an owner spelled by number is unmatched.

**Limits.**
- L1 (CR3, #241): an `#if`-split object pair that shares one id still reports the
  last-inserted name for every routine (`OwnerIndex::build`, `name_by_object_id`). With
  no fallback, `Codeunit Foo::P` for the shadowed name is now unmatched (exit 2). The
  stable id and the bare routine name still select it. #241 fixes this, not this change.
- L2 (CR5): the unmatched diagnostic does not list the owners that declare the routine.
  That needs new fields on `FingerprintQueryDiagnostic` (fingerprint_query.rs, which is
  hands-off) and on digest's `ChangedRootsDiagnostic` beyond the call site. The
  CHANGELOG states it.
- L3 (CR6): the narrowing is still a second call each caller makes, but the type forces
  it. Folding `owner_of` into `resolve_selector` needs fingerprint_query.rs's call
  changed. Until then `narrow_to_owner`'s `_selector`/`_matched_form` parameters are
  unused (a `shortcut:` comment says so).
- L4: an unquoted bare name whose first word is a type word (`Page Management::X`)
  reads as typed; `"Page Management"::X` works. A logical name that is itself wrapped
  in `"` (`"""X"""`) matches only by its AL spelling.
- L5 (carried, design (c)): an id with no `OwnerIndex` owner (`""`) is never selected in
  `<owner>::` form.

### Attempt 2: commit and rebase (coordinator instruction)

- `check-diff --base b3686bde` (the merge-base): `"reasons": []` before each commit
  (`check-diff-2.log`). Before committing, the one-sentence edit to #240's bullet was
  dropped. One sentence in the #239 entry says the owner fallback that the #240 entry
  mentions is gone.
- Commits on `b3686bde`: `f1a46c1d` (plan, CR7) and `52b7754b` (the fix, tests and
  CHANGELOG).
- `git rebase origin/master` onto `0b0b95c9` (the 2.0.3 bump) had no conflict, but it left
  the #239 entry under the released `## [2.0.3]`. I moved it with Edit to a new
  `### Fixed` under `## [Unreleased]` and folded the move into the fix commit
  (`--fixup` + autosquash). `git diff origin/master -- CHANGELOG.md` is only that
  new section.
- Head: `c2255c94` (attempt-1 plan), `9cafb2e2` (plan), `cd265a02` (fix). Not pushed.
- `bash scripts/ci-steps task` on `cd265a02`: exit 0, 3930 passed, 8 skipped
  (`task-rebased.log`). `check-diff --base origin/master`: `"reasons": []`
  (`check-diff-rebased.log`).
- The register anchors were re-checked on `cd265a02`. Three CHANGELOG anchors moved and
  were re-hashed: CR5 -> `CHANGELOG.md:31-31` (same text, same hash), F2 ->
  `CHANGELOG.md:12-31` (new hash, since the range now includes the fallback sentence),
  G2 -> `CHANGELOG.md:17-18` (same hash). All other anchors match unchanged.

## Operator decision (2026-10-23, direct)

- Reviewer sol (gpt-6.1-sol) hit its provider usage limit during final round 1 (two failed calls). The operator said directly: "This one time i trust Opus, ship." For #239 only, Opus is the sole reviewer: both register slots carry Opus's marks, and the attestation uses `--substitute astra=opus --substitute flash=opus`. sol stays in slot astra for later issues.

### Attempt 2: final round 1 fixes (opus M1, code-review B1-B10)

Fixed on top of `cd265a02`, without a rebase, in commit `a2be7f32`. At check-diff time
`origin/master` had moved to `678d6540` (#262), and check-diff gave
`protected-path:.agent/issue-256/*`: master's own files, read through the two-point
diff, not this branch's. Master had moved and check-diff gave a reason, so I rebased
(no conflicts). The new head is **`140c5b2e`** (`3cf3497e` plan a1, `a8045ce6` plan,
`8e6d2502` fix, `140c5b2e` final-1 fixes). The CHANGELOG entry stays under
`## [Unreleased]` / `### Fixed`.

Naming: the attempt-2 proof labels "B1-B10" in the proof table above are breaks, not
these findings. The new breaks below are C1-C6.

| finding | decision |
|---|---|
| M1 / B8 | kept: `unquote` calls `scope::unescape_al_identifier`; it now serves only the object part |
| B1 | confirmed on the CLI (display `Post ""X""`, selector unmatched). Fixed: `strip_quotes` removes one outer pair and unescapes nothing; the doc comment says routine displays are raw |
| B2 | fixed: `OwnerPart.wants` = [unquoted, as written]; the first spelling that keeps a match wins |
| B3 | fixed: `last_separator`, the last `::` outside quotes |
| B4 | fixed: a selector with no unquoted `::` loses one outer quote pair before forms 2-4; nothing else changes (only `"..."` selectors, which matched no raw display before) |
| B5 | fixed in `fingerprint_cli.rs` (human only): the hint line `declared as: <owner>::<routine>` via `selector_index::owner_check_candidates`; `format_selector_errors_human` is unchanged. Digest, fingerprint JSON and prove stay without it (L2) |
| B6 | refuted: fingerprint and prove use `ProgramScope::Workspace` (the single-app model, workspace routines only), and every workspace routine is Primary. L5 extended: should a non-primary row ever reach the index, it has owner `""` and is unselectable in `<owner>::` form, by design (no fallback) |
| B7 | fixed: `cascade` returns `Option<OwnerPart>` from its form-5 branch |
| B9 | kept the list (only a display string reaches `narrow_to_owner` through the hands-off call); `object_type_words_are_the_model_types` pins it to `ir_object_type` with an exhaustive `ObjectKind` match |
| B10 | refuted: the five #239 CLI tests run 0.12-0.22 s each, about 0.84 s in all (nextest, `cli-r2.log`) |

CLI probe after the fix (`/work/bench/239a2/ws-b1`, digest): `Codeunit "Sel A"::"Post ""X"""`,
`"Post ""X"""`, `Codeunit "Legacy"::Run` and `Codeunit """Legacy"""::Run` each select one
entry (before: the first three were unmatched).

**Proofs** (Edit-made, refused if not applied (`run2.sh` diffs against `si.good`/`fc.good`),
reverted with Edit, then the diff was empty):

| break | unit | CLI | log |
|---|---|---|---|
| C1 unescape the routine part (B1) | U3 FAIL `[]` vs `["a#"]` (`:408`) | pass | `c1-routine-unescape.log` |
| C2 bare forms keep quotes (B4) | U3 FAIL `[]` vs `["a#"]` (`:409`) | pass | `c2-bare-forms-keep-quotes.log` |
| C3 `rfind` split (B3) | U3 FAIL `[]` vs `["a#"]` (`:412`) | pass | `c3-rfind-split.log` |
| C4 drop the as-written spelling (B2) | U3 FAIL `[]` vs `["l#"]` (`:422`) | pass | `c4-no-as-written.log` |
| C5 drop the hint line (B5) | pass | A1 FAIL (no `declared as`) | `c5-no-hint.log` |
| C6 `xmlport` -> `xmlports` (B9) | B9 test FAIL | pass | `c6-type-word-list-drift.log` |

**Gate.** `bash scripts/ci-steps task`: exit 0, 3931 passed before the rebase
(`task-r2.log`) and 3936 passed, 8 skipped on `140c5b2e` (`task-r2-rebased.log`).
`check-diff --base origin/master`: `"reasons": []` on `140c5b2e`.

**Register.** Added M1 and B1-B10 (round final-1). Re-hashed after the code moved: CR1
-> `selector_index.rs:212-233`, CR2 -> `:320-331` (its evidence now says strip_quotes),
CR4 -> `:215-219`, CR5 -> `CHANGELOG.md:36-39` (evidence: partly delivered by B5),
F2 -> `CHANGELOG.md:12-39`, G1 -> `cli_object_display.rs:635-641`. CR3, CR6, CR7, F1
and G2 are unchanged. Every final-1 anchor was verified against `140c5b2e`.

## Final round 2 (Opus, sole reviewer per the operator)

- Opus accepted M1 and B1-B10 on 140c5b2e (B6, B10 as refuted) and said ship. New minor findings, both deferred as stated limits on Opus's own suggestion, and filed as one discovery:
  - M2: B2's as-written fallback is tried when the unquoted spelling keeps no MATCH, not when no such OWNER exists, so with `Codeunit Legacy` (no `Persist`) and `Codeunit """Legacy"""` (displayed `Codeunit "Legacy"`, declares `Persist`), `Codeunit "Legacy"::Persist` selects the second. Needs two objects whose names differ only by wrapping quotes.
  - M3: `last_separator` finds no `::` outside quotes for a header-copied selector whose logical object name holds an odd number of `"` (`Codeunit A "B::Persist`), which `rfind` split before this round; the AL spelling `Codeunit "A ""B"::Persist` works.
- Step 11 gates on 140c5b2e (base 678d6540, supervised, run 20261023-212135-0a81a5): check-goldens-coverage exit 0 (0.0 s); ci-steps-all exit 0 (395.7 s); check-goldens exit 0 (0.1 s, tree-stamp skip); cdo-gate exit 0 (313.2 s). Attest with `--substitute astra=opus --substitute flash=opus` (operator decision above).

## Attempt 2 stopped: wall-clock cap (2026-10-24 07:26 UTC)

- Waiting overnight for sol (the operator chose "wait for sol" after the executor refused one reviewer in two slots: `bad-substitute`) exhausted this run's wall-clock budget (`charge` → `exhausted: wall-clock`). State at stop: head 28f9024e (evidence commit on code head 140c5b2e), Opus accepted all 33 entries and said ship, four gates green on 140c5b2e (base 678d6540). Open: sol's marks on CR1-CR7, F1, F2, G1, G2, M1-M3, B1-B10. Master has since moved to 9bd2996f (#250 ObjKey logical names, #252), so attempt 3 must rebase and re-run the gates.

## Attempt 3 (run 20261024-072841-9427cc, base 9bd2996f)

- Claimed after the operator ran `unblock 239`. Attempt 2's four code commits cherry-picked onto 9bd2996f (master gained #250 ObjKey logical names and #252); one CHANGELOG conflict (both `### Fixed` entries kept under `[Unreleased]`). check-diff `[]`; task gate exit 0 (222.9 s). Re-anchored CR5, F2, G2 (CHANGELOG lines shifted; text unchanged, hashes unchanged). Remaining: sol's marks (the operator's choice), the four gates, attest with sol and Opus in their normal slots.
- Final panel, sol (attempt 3, head f2e84c10): accepted all 24 ids (CR1-CR7, F1, F2, G1, G2, M1-M3, B1-B10; M2/M3 deferrals accepted), no new findings, ship. With Opus's earlier marks, every register entry is accepted by both reviewers. Attest uses the normal `--substitute astra=sol --substitute flash=opus`.
- Step 11 gates on f2e84c10 (base 9bd2996f, supervised, run 20261024-072841-9427cc): check-goldens-coverage exit 0 (0.0 s); ci-steps-all exit 0 (404.9 s); check-goldens exit 0 (0.1 s, tree-stamp skip); cdo-gate exit 0 (364.8 s).
