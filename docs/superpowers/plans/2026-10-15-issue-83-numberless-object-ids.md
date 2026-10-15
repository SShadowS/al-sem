# Issue #83 — numberless objects get a unique object id — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An object without a number gets the object key `@` + the percent-encoded, folded logical
name. Every id built on the key (model, R0, R1a, `model_object_id`, dependency ABI, internal and
stable routine ids, #71 declaration-only ids, fingerprint object part) becomes unique. Every
numbered id stays byte-identical.

**Architecture:** One function, `engine::ids::object_key(Option<i64>, &str)`, decides the segment.
One helper, `ids::abi_object_number`, states the ABI zero rule. `encode_object_id` and
`CanonicalRoutineKey` take the key; `ModelObject` and `ProjectedObject` carry it.

**Tech Stack:** Rust; `al-syntax`; umbrella test crates (`tests/l2_ir`, `tests/cli`, `tests/r25_abi`),
`tests/differential.rs`.

**Spec:** `docs/superpowers/specs/2026-10-15-issue-83-numberless-object-ids-design.md` (converged, spec
round 3). Every D-, T- and R- reference below is the spec's.

## Global Constraints

- `rustfmt <file>` only, never `cargo fmt`. No `#[allow(`. Edit with the Edit/Write tools; no
  scripted edits.
- Never touch `.github/ scripts/ .claude/ CLAUDE.md .gitignore Cargo.toml tree-sitter-al .agent/`.
- Numbered ids must not move: no golden line for a numbered object may change.
- Package name is `al-sem` (hyphen): `cargo test -p al-sem --lib <filter>`.
- Worktree: export `TREE_SITTER_AL_PATH=U:/Git/al-call-hierarchy/tree-sitter-al`.
- Every new or changed test gets a discrimination proof (break the code with Edit, see it fail,
  restore with Edit, see it pass; record test, break, fail line, pass line).

## Review Focus

1. Every producer P1–P8 (spec Background) and every `CanonicalRoutineKey` site calls `object_key`.
   None keeps a private `unwrap_or(0)` that ends up in an id.
2. The P2 = P3 join (`program_calls.rs:1659`) for a recovered codeunit (T3b).
3. A dependency ModelObject's `object_key` comes from `ProjectedObject`, never re-parsed from the id.
4. Golden triage: only numberless-id lines and rows that were previously collapsed may move.
5. The ordering contract per output (T7): the identity table is ordinal; the contracts use
   `locale_compare`.

### Task 1: the object key and every producer

**Files:**
- `src/engine/ids.rs`: `object_key`, `abi_object_number`, `encode_object_id(app_guid, object_type,
  object_key: &str)`, `CanonicalRoutineKey.object_key: String` (replaces `object_number`), and the
  T1 unit vectors.
- Producers:
  - P1 `src/engine/snapshot.rs:154-161`;
  - P2 `src/program/model/workspace.rs:778-780`, plus the new field `ModelObject.object_key` and
    the re-mint at `:2497`;
  - P3 `src/program/model/mod.rs:32-40`;
  - P5/P6 `src/program/body/l2_workspace.rs:136, 219-229, 280-284, 459-461`;
  - `src/program/body/scope.rs:111-141` (`compute_routine_id` takes the key);
  - P8 `src/engine/deps/projection.rs:202, 282, 341, 350-354` (`ProjectedObject.object_key`);
  - `src/program/model/abi_rows.rs:110-116, 270` (copy `object_key`);
  - `src/program/abi_ingest.rs:421-425` and `src/program/model/program_calls.rs:1530-1534,
    1580-1584` (use `abi_object_number`).
- #71 (D4): `src/program/model/workspace.rs:899-931` passes `None` as the member;
  `tests/cli/cli_digest_changed_spans.rs:1765-1817`, where `decl_only_id`, its doc, its mutation note
  and the shape assertion use `…:Interface:@<name>#`.
- `src/engine/l5/fingerprint.rs:150-152`: the object part is `"{type}/{object_key}"`.
- D5: `tests/differential.rs:168-199`.
- Tests:
  - T3, T3b, T5, T5b: in the existing test module closest to each producer, or a new member of an
    existing umbrella (`tests/l2_ir` or `tests/cli`), listed in that umbrella's `main.rs`;
  - `tests/l2_ir/encoder_vectors.rs:33` and `tests/l2_ir/ir_robustness.rs:235, 273` are updated to
    the new key field.

**Interfaces (produced):**
```rust
pub fn object_key(number: Option<i64>, logical_name: &str) -> String;   // ids.rs
pub fn abi_object_number(n: i64) -> Option<i64>;                         // ids.rs, (n != 0).then_some(n)
pub fn encode_object_id(app_guid: &str, object_type: &str, object_key: &str) -> String;
pub struct CanonicalRoutineKey { /* … */ pub object_key: String, /* … */ }
pub struct ModelObject { /* … */ pub object_key: String, /* … */ }      // object_number: i64 stays
pub struct ProjectedObject { /* … */ pub object_key: String, /* … */ }
```

- [ ] Write T1 in `ids.rs`'s test module. The expected values are the spec's D1 examples:
  - `object_key(Some(50811), "x") == "50811"`;
  - `object_key(None, "IFoo") == "@ifoo" == object_key(None, "ifoo")`;
  - `object_key(None, "Sales Line") == "@sales%20line"`;
  - `object_key(None, "My/If:#x") == "@my%2Fif%3A%23x"`;
  - `object_key(None, "123") == "@123"`;
  - `object_key(None, "a.b_c-d%@") == "@a%2Eb%5Fc%2Dd%25%40"`;
  - `object_key(None, "Ø \"Q\" If") == "@%C3%B8%20%22q%22%20if"`, equal for `ø`;
  - `abi_object_number(0) == None`, `abi_object_number(7) == Some(7)`.
- [ ] Write T3 (temp workspace, interfaces `IEmpty` and `"Ø ""Q"" /:#%@ If"`). Assert each
  interface's literal id from P1 (R0 snapshot), P2 (model), P3 (`model_object_id` of its graph node)
  and P5 (R1a). The literal is `<guid>/Interface/@iempty`, and for the second interface
  `<guid>/Interface/@%C3%B8%20%22q%22%20%2F%3A%23%25%40%20if` (stable form `/`→`:`).
- [ ] Write T3b, which first asserts its precondition in-test: `al_syntax::parse` gives two
  `kind == Codeunit`, `id == None` objects for `codeunit 99999999999999999999 "CU A"` and `"CU B"`.
  Each declares `procedure Run()`. Assert:
  - distinct object ids, internal routine ids and stable routine ids;
  - `"CU A"` implements an interface, and its dispatch meta does not list it as not-found.
- [ ] Update T4 (`cli_digest_changed_spans.rs`): `decl_only_id` builds from the new object id with
  no member. The shape assertion is `…:Interface:@<name>#` + 64 hex. Two interfaces declaring the
  same `Bar()` give distinct ids.
- [ ] Write T5: the real SymbolReference parser reads a JSON with two `Interfaces` without `Id`
  declaring the same method. The projected object ids contain `@<name>` and differ, and their
  internal and stable routine ids differ.
- [ ] Write T5b: a cross-app model with a dependency that declares an interface without `Id`. Assert
  that the dependency Interface graph node and the dependency `ModelObject` both exist, that
  `model_object_id(graph, &node.id) == model_object.id`, and that `model_object.object_key` is the
  id's last segment. Use an existing fixture with a dependency interface if there is one; otherwise
  build the symbol package in the test (see the `tests/r25_abi` vectors).
- [ ] Run them: T1, T3, T3b, T4, T5, T5b fail on the current code (record each failure line).
- [ ] Implement D1, D2, D2a, D3 and D4, and the fingerprint object part. Then D5 in
  `tests/differential.rs`: assert no duplicate `stableObjectId` per snapshot before building the
  maps, key by id, compare `name`.
- [ ] `cargo test -p al-sem --lib`, `cargo test --test l2_ir`, `cargo test --test cli`,
  `cargo test --test r25_abi`, `cargo test --test differential` (expect the R0/R1a goldens to differ),
  `cargo clippy --all-targets -- -D warnings`.
- [ ] `REGEN_TEMP_GOLDENS=1 scripts/check-goldens --regen`, then dispatch `golden-diff-triager` on
  the diff.
  - Expected: the 6 + 6 numberless-id lines in `tests/r0-goldens` / `tests/r1a-goldens`, plus rows
    that were previously collapsed.
  - Any other line is unexplained: stop and report it; never bless it.
  - Then run `scripts/check-goldens` and see it green.
- [ ] Discrimination proofs, recorded:
  - T1: `None` → `"0"`.
  - T3: P3 skips the unescape.
  - T3b: P2 uses `unwrap_or(0)`.
  - T4: pass `Some(name)` again.
  - T5: P8 passes `Some(n)`.
  - T5b: `dep_object_to_l3` sets `object_key` to `""`.
  - D5: inject a duplicate row, and change a name.
- [ ] `rustfmt` every touched `.rs`; `python scripts/agentflow --root U:/Git/al-call-hierarchy
  --run-id <run> check-diff --base master --head HEAD --issue 83 --cwd <worktree>` (after staging a
  commit); commit.

### Task 2: end-to-end retention, ordering contract, CHANGELOG

**Files:** `src/engine/l5/snapshot.rs` (the M8 comment `:25-29, 1136-1138`); a test for T6/T7 next to
the existing full-snapshot tests (`tests/cli/cli_b_snapshot_differential.rs` neighbours or the
`l5::snapshot` test module); `CHANGELOG.md`.

**Interfaces (consumed):** Task 1's `object_key` and ids.

- [ ] T6: run the full-snapshot path (identity table + contracts) on T3's workspace. Both interfaces
  must be present with distinct `stableId`s and their own names. It fails on `master`'s code shape;
  prove it by restoring `object_key(None, …)` → `"0"`.
- [ ] T7: an identity table holding `<guid>:Codeunit:1` and a recovered `<guid>:Codeunit:@a` lists
  `…:1` first (ordinal); the contracts list `…:@a` first (`locale_compare`). Discrimination: sort the
  identity table with `locale_compare` → fail.
- [ ] Rewrite the M8 comment to state the identity table's contract as ordinal (al-sem parity
  retired), naming `@` and `%XX`.
- [ ] CHANGELOG `## [Unreleased]`:
  - `### Fixed`: numberless object ids, dependency interface routine-id collisions, and the
    fingerprint object part;
  - `### Changed`: the #71 declaration-only `stableId` values; the upgrade note from the spec
    (stored baselines and gate snapshots holding numberless ids report those objects as removed and
    re-added once; numbered ids are unchanged).
- [ ] Tests, clippy, `scripts/check-goldens`; rustfmt; check-diff; commit.
