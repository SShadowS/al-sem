# Issue 10 — ROOT_KIND_VALUES is defined twice

## Claim

```json
{"run_id":"20260918-121406-d91f37","issue":10,"attempt":1,
 "branch":"issue/10-l5-root-kind-values-defined-twice-a1",
 "worktree":"U:\\Git\\al-sem-issue-10-a1",
 "body_hash":"f392c81ffe5c12333c5bc71ba32e1c199836625c02e1d73c9e4640d3e1761add"}
```

Worktree from `master` @ `0e9d09aa`.

## Classification

**BOUNDED**, and the deliverable is a net DELETION. The vocabulary is declared twice; the fix is
to delete one declaration and import the other.

## Assumption probes

| probe | result | evidence |
|---|---|---|
| P1 the vocabulary is declared twice | **holds** | `root_classification.rs:31` `pub const ROOT_KIND_VALUES: [&str; 12]`; `fingerprint_cli.rs:76` `pub const ROOT_KIND_VALUES: &[&str]` — same twelve values, same order |
| P2 the CLI copy has exactly two uses, both local | **holds** | `fingerprint_cli.rs:96` (`contains`) and `:99` (`join`), both inside `validate_roots` |
| P3 nothing outside those two files names the CLI copy | **holds** | Repo-wide grep for `ROOT_KIND_VALUES` returns only `root_classification.rs`, `fingerprint_cli.rs`, and one test in `root_classification.rs` (see P5) |
| P4 the type difference is inert at the call sites | **holds** | The classifier's is an ARRAY `[&str; 12]`, the CLI's a SLICE `&[&str]`. Both `contains` and `join` are slice methods reachable on an array by deref, so neither call site changes |
| P5 issue #11 left a test that this change makes redundant | **holds** | `root_classification.rs:1117` — `the_two_root_kind_vocabularies_are_identical`, added by #11, asserts the two lists are equal. With one list it is tautological |

### The defect

Adding a 13th kind to the classifier makes the engine emit a kind that shipped
`alsem fingerprint --roots <kind>` refuses as "unknown root kind". The two lists agree TODAY, so
nothing is currently broken — the defect is that nothing prevents them diverging.

## Design

Delete the CLI's copy; import the classifier's.

```rust
use crate::engine::root_classification::ROOT_KIND_VALUES;
```

Both call sites are unchanged (P4). Net effect: fourteen lines removed, one added.

**Also delete `the_two_root_kind_vocabularies_are_identical`.** Issue #11 added it as the best
guard available while two lists existed: it asserted they were equal. With a single definition the
assertion cannot fail, and a test that cannot fail is worse than no test — it reads as coverage.
The structural fix subsumes it, which is the point: drift becomes impossible by construction
rather than detected after the fact.

That matters for what this change actually buys. The issue frames the remedy as "the CLI validator
and the classifier can never disagree". A test can only ever observe that they currently agree;
deleting one declaration is what makes disagreement unrepresentable. The compiler is the guard.

### On the issue's proposed discrimination proof

The issue asks for "a test that adds a hypothetical kind to the one array and asserts the CLI
validator accepts it (revert the import and the test fails)". A Rust test cannot append to a
`const` at runtime, so that test cannot be written as described. The honest equivalent is a test
asserting `validate_roots` accepts EVERY value in the single `ROOT_KIND_VALUES` and rejects one
outside it — and the discrimination proof is the structural one the issue intends: restore a
second, divergent literal in `fingerprint_cli.rs` and watch the test fail.

## Acceptance matrix

| # | item | proof |
|---|---|---|
| A1 | `fingerprint_cli.rs` has no `ROOT_KIND_VALUES` of its own | Repo-wide grep finds exactly ONE definition |
| A2 | `validate_roots` checks against that single definition | Test: every value in `root_classification::ROOT_KIND_VALUES` is accepted |
| A3 | A value outside it is still rejected, with the same message | Test asserts the existing `unknown root kind '<v>'; valid: <list>` text, which `cli_b_fingerprint_oracles.rs:188` also pins |
| A4 | Discrimination | Reintroduce a divergent second literal in `fingerprint_cli.rs`, assert the edit applied, watch A2 fail; byte-restore under a hash check |
| A5 | The redundant #11 test is gone | `the_two_root_kind_vocabularies_are_identical` deleted, not left tautological |
| A6 | No behaviour change | The vocabularies are identical today, so no golden moves; `check-goldens` byte-clean |

## Spec panel

One round, both reviewers, **no findings** — `findings.json` is therefore empty rather than
padded with entries nobody raised.

- **astra**: "No critical, important, or minor findings. The proposed import is the right shape."
  It settled the layering question by evidence rather than opinion: `fingerprint_cli.rs:364-365`
  ALREADY calls `crate::engine::root_classification::roots_config_was_loaded`, so the import
  follows an established direction and introduces no new dependency.
- **flash**: verified the same and added that deleting the #11 parity test REMOVES an upward
  `root_classification -> l5` dependency, leaving the hierarchy strictly one-way.

**One refinement adopted, from astra:** the discrimination proof must restore a CLI-local list
that is MISSING a canonical value. An extra CLI-only value would not fail an accepts-all test —
only a missing one does. A4 below is written that way, and the measured result shows why it
matters.

## Discrimination proofs

### A4 — the single definition

| field | value |
|---|---|
| tests | `cargo test --test cli cli_b_fingerprint_oracles::item3` |
| mutation | replace `use crate::engine::root_classification::ROOT_KIND_VALUES;` with a CLI-local list missing `"test-procedure"` |
| break asserted? | YES — anchor matched once, the import confirmed absent, and the replacement block confirmed present |
| fail output | exit 101 — **2 failed, 1 passed** |
| pass output | exit 0 — **3 passed** |
| restore | sha256 MATCH |

**Which failed, and why both are correct.** `item3_every_classifier_root_kind_is_accepted_by_the_cli`
failed because the CLI no longer accepts a kind the classifier emits — the defect this issue is
about. `item3_unknown_root_kind_errors_exit1` also failed, because the missing value changes the
joined `valid: ...` text it pins literally; that is astra's second guard catching the same
divergence from the other side.

**The one that PASSED is the interesting one.** `item3_valid_root_kinds_pass` checks exactly two
kinds, neither of them `"test-procedure"`, so a list missing that value cannot move it. It was the
pre-existing coverage, and on its own it would have let this divergence through — which is
precisely why the new test was needed rather than relying on what was already there.

### A guard of mine that was wrong, and caught itself

The proof script first asserted the break by checking `"test-procedure"` was absent from
`broken.split(b"];")[0]` — everything up to the first `];` in the file. That substring spans
earlier arrays, so the assertion failed on a break that had applied correctly. Replaced with a
direct `NEW in broken` check.

Recorded because it is the doctrine working in the awkward direction: the guard fired, and the
thing it caught was the guard. An over-clever assertion is its own failure mode, and had it been
written the other way round — silently true — it would have certified a break that never applied.
