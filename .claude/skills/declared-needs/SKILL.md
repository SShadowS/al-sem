---
name: declared-needs
description: Use when adding or changing any fact the engine builds (an IR field, a body fact, a summary, a detector input, a command's computation), adding a detector or command, or making a build cheaper. Explains build profiles, substrate bits and View plans.
---

# Declared needs: nothing is built unless someone asks for it

The engine builds only what the tools that run declare they need. Each consumer states
its needs, the build takes the **union** of those needs, and everything else is
skipped. There is no "build everything by default" switch.

This rule is why a default `alsem analyze` or the LSP server does not pay for facts
only one opt-in detector reads. A new fact that skips this mechanism quietly makes
every run slower and bigger. Follow the steps below, every time.

Source of the design: `docs/superpowers/specs/2026-10-04-compact-graph-core-design.md`
§4 (build profiles), the `substrate` module doc in `src/engine/l5/registry.rs`, and
`src/engine/session.rs` (View and Plan, #160 step 2).

## The three levels

A need is declared at the level where the fact is **produced**:

| Level | What it controls | Where | Declared by |
|---|---|---|---|
| 1. Build profile | What parsing, lowering and the dependency tier keep (syntax trees, text, per-file facts) | `src/program/profile.rs` (`BuildProfile`) | Every graph build call site. Presets: `FULL` (everything), `LIGHT` (the LSP server) |
| 2. Substrate bit | Which expensive derived facts the detector context builds (summaries, transaction spans, ordering facts, …) | `src/engine/l5/registry.rs` (`pub mod substrate`) | Each detector's `requires`; `run_detectors` folds them into `demanded` |
| 3. View plan | What one command computes over its model (L4 analysis, trees, witness facts, after-run reads) | `src/engine/session.rs` (`Plan`, `View`) | The command's plan factory: `analyze_plan`, `digest_plan`, `prove_plan`, `policy_plan`, `query_plan`, … |

**Choose the level:**

- The fact comes from the source text or from lowering one file (label text, captions,
  dependency bodies)? → **Level 1**, a `BuildProfile` field.
- The fact is derived across routines or the call graph, and detectors read it? →
  **Level 2**, a substrate bit.
- One command's own computation that is not a detector input (a digest tree, a
  fingerprint read)? → **Level 3**, a `Plan` field set by that command's factory.

A fact can need more than one level. For example, rules with messages need level 1
(the label text) and level 2 (the rule index). Then the higher level states which
lower level it depends on, and a mismatch fails loudly (see rule 5).

## Rules

1. **No default.** `BuildProfile` has no `Default`, on purpose. Every call site names its
   profile. Plans are built only by the factories in `session.rs` (their fields are
   private).
2. **`FULL` keeps everything.** It is today's complete behaviour. Never add a preset
   that keeps "more than `FULL`". If a fact is new, `FULL` keeps it and smaller
   profiles may skip it. (Rev 1 of the intent-evidence spec broke this rule with an
   `INTENT` preset "above `FULL`", and rev 2 had to undo it.)
3. **Skipping a fact must not change any other fact.** A smaller profile may leave a
   fact empty but must never compute any other fact differently. This is tested:
   `full_and_light_profiles_give_the_same_report` (`src/program/dep_summary.rs`) and
   `cdo_light_and_full_profiles_give_the_same_report_and_answers`
   (`src/program/dep_cache.rs`). A new profile field extends both tests.
4. **Skip the fact where it is made.** Don't build it and then drop it. A skipped
   fact should never be allocated (for example, the lowerer does not read the text at
   all).
5. **Reading an unbuilt fact fails loudly.** In debug builds and tests, reading a
   cell or field that was not declared panics with a message naming the missing
   declaration. The model to copy is `get_ordering_facts`:
   `.expect("ordering cell not filled: declare substrate::ORDERING_FACTS")`.
   **Never let "not built" look like "empty".** An empty `HashMap` or a `None` that
   could mean either one is the bug this rule prevents.
6. **A profile field that changes the dependency tier joins the cache key.**
   `DepKey` (`src/program/dep_cache.rs`) carries `keep_bodies` for this reason. A new
   field that changes what a dependency summary (`PackedFile`) holds must join
   `DepKey`. Otherwise a cache hit serves a tier built under another profile.
7. **Never compute a substrate lazily inside the detector loop.** Fill it once,
   before the parallel loop. A lazy `OnceLock` fill inside rayon deadlocked
   `r4_differential` (2026-10-04). The whole story is in the `ORDERING_FACTS` doc.
8. **`substrate::ALL` is a closed list, not "every bit".** An expensive bit with few
   readers stays **out** of `ALL`, like `ORDERING_FACTS` and `DB_EFFECT_REVERSE_INDEX`,
   so the many `build_detector_context(.., substrate::ALL)` callers don't pay for it.
   When you do that, add the bit to the parity test's full mask
   (`tests/gap/gap_detector_substrate_parity.rs`, today `ALL | ORDERING_FACTS`) in the
   same change. Otherwise the full context lacks the bit while the minimal one has it,
   and the test fails against correct code. Read the `DB_EFFECT_REVERSE_INDEX` doc
   before deciding.
9. **A detector declares everything it reads.** Two tests guard this:
   `every_detector_alone_declares_what_it_reads` (registry) runs each detector alone,
   and the per-detector full-against-minimal parity test compares findings. Both must
   stay green. Prove the declaration matters: remove the bit from `requires` and watch
   a test fail (CLAUDE.md's discrimination-proof rule).
10. **Post-run reads go in `Plan::after_run`.** A command that reads a substrate after
    the detectors run (digest reads `TRANSACTION_SPANS`, events reads `SUMMARIES`)
    declares it there. `prepare` asserts each run's union holds it.

## Checklist: adding a build-profile field (level 1)

1. Add the field to `BuildProfile` as an enum with a doc comment for each value. Set
   it in `FULL` (keeps everything) and `LIGHT`.
2. Pass it to where the fact is made, so a skip never allocates (rule 4).
3. If it changes the dependency tier: extend `DepKey` and the `PackedFile` format
   (rule 6).
4. Make reading the field under a skipping profile panic in debug builds and tests
   (rule 5).
5. Extend the two LIGHT-against-FULL tests: list the new field as allowed to differ,
   and keep every other fact identical (rule 3).
6. Check every call site that builds a graph still states its profile on purpose.
   Search for `BuildProfile::`.

## Checklist: adding a substrate bit (level 2)

1. Take the next free bit in `pub mod substrate` (today `1 << 7`). Document what it
   fills, its cost, and any bit or profile it implies.
2. Decide on `ALL` membership (rule 8). If it stays out, update the parity test's full
   mask in the same change.
3. Fill it in `build_detector_context` / the det fill, once, before the detector loop
   (rule 7).
4. Accessor: panic with a "declare substrate::X" message when it is empty (rule 5).
5. Declare it in each reading detector's `requires`, and in `Plan::after_run` for
   post-run readers (rules 9 and 10).
6. If the bit needs a level-1 fact, the command that demands the bit must also pick a
   profile that keeps that fact. Make the mismatch panic in debug builds and tests.

## Checklist: a new command (level 3)

Add a `*_plan` factory in `session.rs`. Don't build `Plan` by hand, and don't
build a `DetectorContext` outside a `View` (`alsem query` has its own pipeline, and
that is documented as the exception, not the pattern). Pick the build profile from
what the plan needs, not `FULL` by habit, once a smaller profile is enough.

## Worked example in progress: intent evidence

`docs/superpowers/specs/2026-10-29-intent-evidence-design.md` (rev 2, design only, not
built) adds both kinds of declaration:

- **Level 1:** `BuildProfile::intent_text: Skip | Workspace | All`. It controls
  whether the lowerer keeps label text, captions, tooltips and doc comments. `LIGHT`
  uses `Skip` and `FULL` uses `All`. It joins `DepKey`.
- **Level 2:** `substrate::RULES`, the per-routine "rejects when" rules and the
  per-field writer index. It stays out of `ALL` and is added to the parity mask. Only
  `d65-guard-bypass` (opt-in) and `alsem explain` demand it, so a default `analyze`
  run costs what it did before.

When that work starts, follow the spec's §4.1b, and update this example once it has
landed.
