# Issue #78: digest, fingerprint and prove name the routine's owning object — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `routine.objectDisplay` in `alsem digest`, the `objectDisplay` of `alsem fingerprint` blocks and of `alsem prove` are `"<ObjectType> <ObjectName>"`, exactly as `alsem query effects` prints them. Today all three are always `""`.

**Architecture:** The rule lives once in `src/engine/l4/effect_query_cli.rs`, next to `RoutineJoin`.
- `object_display(type, name)` is shared by `RoutineJoin::of` and by a new index, `object_display_by_stable_routine_id(&Model)`.
- The index covers primary-app routines (through `RoutineJoin::of`) and declaration-only routines (from their declaring object).
- Digest and fingerprint read the index instead of splitting the identity table's bare name. Prove inherits the fix through the digest query.

**Tech Stack:** Rust; `tests/cli` umbrella (`CARGO_BIN_EXE_alsem`).

**Spec:** `## Design` and the acceptance matrix (A1-A4) in `.agent/issue-78/ledger.md`. The spec panel converged in round 5.

## Global Constraints

- Only these change: `src/engine/l4/effect_query_cli.rs`, `src/engine/l5/digest_cli.rs`, `src/engine/l5/fingerprint_query.rs`, `src/engine/l5/fingerprint_cli.rs`, `src/engine/l5/prove.rs`, any test or bench that calls the two changed signatures, `tests/cli/*` (the acceptance tests), the regenerated goldens, and `CHANGELOG.md`.
- No `#[allow(`; `rustfmt <file>` only; Edit/Write tools only.
- Never touch `.github/ scripts/ .claude/ CLAUDE.md .gitignore Cargo.toml tree-sitter-al .agent/`.
- The snapshot, its identity table, selectors, `owningRoutineDisplay` and the fingerprint CBOR/`.cbor.gz`/shard encodings do not change.

## Review Focus

1. Only one owner rule exists: no second `format!("{} {}", type, name)`.
2. Declaration-only owners come from the declaring object, never from a shared `<guid>/<Kind>/0` id.
3. The identity table is never read for the owner.
4. Golden movement is limited to `objectDisplay` values and the human header lines.

### Task 1: the shared owner index, wired into digest, prove and fingerprint

**Files:** `src/engine/l4/effect_query_cli.rs` (`object_display`, `object_display_by_stable_routine_id`, `RoutineJoin::of`); `src/engine/l5/digest_cli.rs` (delete `split_object_routine`; new parameter on `run_digest_query_full_from_entries`; both entry kinds; caller at `:1632`); `src/engine/l5/prove.rs` (caller at `:968`); `src/engine/l5/fingerprint_query.rs` (delete `split_object_routine`; `fingerprint_query` and `build_block` read the index); `src/engine/l5/fingerprint_cli.rs` (caller); test/bench call sites of the two signatures; `CHANGELOG.md` (`### Fixed`).

- [ ] Confirm the acceptance tests (A1, A2, A3, A4, already written) are red for the stated reason.
- [ ] Implement as the ledger's `## Design` states.
- [ ] All green: the acceptance tests, `cargo test --test cli`, `cargo test -p al-sem --lib`, `cargo clippy --all-targets -- -D warnings`. Moved goldens are expected; leave them for Task 2.
- [ ] Discrimination proof per acceptance test. Mutation (i): revert the lookup to `""`. Mutation (ii): take the owner from the identity table's object name (the old `<guid>:<Kind>:<n>` path). Record both outcomes.
- [ ] CHANGELOG `### Fixed` entry; `check-diff`; commit together with the tests.

### Task 2: regenerate and triage the goldens

**Files:** the goldens under `tests/cli-b-goldens/{digest,fingerprint,prove}/` that move.

- [ ] `scripts/check-goldens --regen` (through the supervisor), then `golden-diff-triager` over the diff.
- [ ] Every moved line is an `objectDisplay` value or a human header line. CBOR/`.cbor.gz`/shards are byte-identical. An unexplained line is a discovery, and that golden is not blessed.
- [ ] Commit the regenerated goldens with the triage summary in the message.
