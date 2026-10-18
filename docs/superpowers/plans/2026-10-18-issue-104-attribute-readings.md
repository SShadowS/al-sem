# Issue #104: several per-build readings of one attribute — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Consumers of a routine's publisher/subscriber attributes use EVERY per-build reading of one attribute: IncludeSender and Isolated are unioned, and readings that name one event give one subscription.

**Architecture:** One shared IncludeSender union helper in `event.rs` (used by `publisher_include_sender` and #103's `EventFacts::union`); subscriptions merged by event key inside `SubscriberIndex::build`, the one place every edge and subscription-inventory consumer reads; `parse_isolated` reads every reading.

**Tech Stack:** Rust (crate `al-sem`), owned AL syntax IR.

**Spec:** `.agent/issue-104/ledger.md` sections "## Design" and "## Acceptance matrix" (converged spec panel, attempt 2 round 2).

## Global Constraints

- Format only touched files with `rustfmt <file>`; never `cargo fmt`.
- Protected paths, never edited: `.github/ scripts/ .claude/ CLAUDE.md .gitignore Cargo.toml version tree-sitter-al .agent/`.
- Every shell in the worktree: `export TREE_SITTER_AL_PATH=U:/Git/al-call-hierarchy/tree-sitter-al`.
- `RoutineNode::event_subscribers` is NOT changed (one entry per reading / per #103 arm stays).
- No `#[allow(...)]` additions.
- Every new or changed test gets a discrimination proof (break → fail, revert → pass), recorded in the ledger's proof table.
- Commits: the acceptance tests of Tasks 1-3 are already in the tree and red, and Task 1's and Task 2's share `tests/cli/cli_attribute_trivia.rs`. A commit after one task would leave the others' tests red, and a pre-commit golden run over a partly staged tree can reject it. So Tasks 1-3 are implemented in order WITHOUT committing; their "commit" steps become one commit of the code and the tests once all three are green (after `check-diff`), then Task 4's CHANGELOG commit.

## Review Focus

1. A subscription whose two readings resolve to the same publisher by name and by number: one key (covered by `a3b_…`).
2. Readings that differ only in element on a NON-platform publisher, including a real table publisher of a platform event name: one subscription (covered by `a3e_codeunit_…`, `a3e_real_table_publisher_…`).
3. Readings of two different events on one routine: both kept, ordinals unchanged (control `a3c_control_…`).
4. A reading whose IncludeSender arg is not a bool literal: contributes unknown, never true (row in `include_sender_is_the_union_of_every_reading`).
5. #103's two-arm case with different skip flags: one edge, node list unchanged (`two_entries_of_one_event_are_one_subscription_at_the_first_ordinal` plus #103's existing build.rs tests A4/A6b stay green).

---

### Task 1: IncludeSender union helper (Design item 1)

**Files:**
- Modify: `src/program/resolve/event.rs` (`publisher_include_sender`, ~:301; add the helper next to it)
- Modify: `src/program/build.rs` (`EventFacts::union`, ~:953-964: replace the inline rule with the helper)
- Test (exists, red): `src/program/resolve/event.rs` `include_sender_is_the_union_of_every_reading`; `tests/cli/cli_attribute_trivia.rs` `a1_sender_in_the_second_reading_binds_the_sender_subscriber`, control `a2_control_sender_in_the_first_reading_binds_the_sender_subscriber`

**Interfaces:**
- Produces: `pub fn union_include_sender(readings: impl IntoIterator<Item = Option<bool>>) -> Option<bool>` in `crate::program::resolve::event`: `Some(true)` if any is `Some(true)`; `Some(false)` if at least one and all are `Some(false)`; else `None`.

- [ ] **Step 1:** Run the two red tests; confirm they fail (`include_sender` is `Some(false)`; 5 of 8 truth-table rows wrong).
- [ ] **Step 2:** Implement `union_include_sender`; make `publisher_include_sender` map EVERY attribute named `integrationevent`/`businessevent`/`internalevent` (case-insensitive) to its arg-0 bool literal (non-literal → `None`) and return the union; make `EventFacts::union` call the helper over publisher arms' `include_sender`.
- [ ] **Step 3:** Add a `union_include_sender` truth-table unit test (all eight cases of the ledger's rule, incl. empty → `None`).
- [ ] **Step 4:** Run: `cargo test -p al-sem --lib include_sender` and `cargo test --test cli cli_attribute_trivia::a1 cli_attribute_trivia::a2` and #103's `cargo test -p al-sem --lib if_duplicate_publisher_include_sender_is_true_when_any_arm_is` → all PASS.
- [ ] **Step 5:** Discrimination proofs (Edit, run, revert): e.g. helper returns the first reading only → A1 and the truth-table test fail.
- [ ] **Step 6:** `check-diff`, commit `fix(program): union IncludeSender over every per-build reading (#104)`.

### Task 2: Merge subscriptions by event key in `SubscriberIndex::build` (Design item 2)

**Files:**
- Modify: `src/program/resolve/index.rs` (`SubscriberIndex::build`, ~:233-380)
- Modify: `src/program/build.rs` test A6b doc comment (~:2661-2662): "the node keeps both entries; `SubscriberIndex` wires them as one subscription" (S3-3)
- Test (exists, red): `src/program/resolve/index.rs` `subscriber_readings_keep_only_skip_flags_set_in_every_reading`, `two_entries_of_one_event_are_one_subscription_at_the_first_ordinal`; `tests/cli/cli_attribute_trivia.rs` `a3_two_readings_of_one_subscriber_are_one_subscription`, `a3b_readings_naming_the_publisher_by_name_and_by_number_are_one_subscription`, `a3e_codeunit_event_readings_differing_only_in_element_are_one_subscription`, `a3e_real_table_publisher_readings_differing_only_in_element_are_one_subscription`; controls `a3c_control_readings_naming_two_events_keep_both_subscriptions`, `a3e_control_platform_table_event_readings_keep_one_subscription_per_element`

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: no new public API. Behaviour per the ledger's Design item 2: per subscriber routine, take each entry through the existing object resolution + candidate selection WITHOUT recording; key = (resolved publisher `ObjectNodeId`, else textual `(publisher_object_type, publisher_name, publisher_id)`; `event_name` folded; `element` only when the outcome is `Bound(rid)` and that routine's `publisher_kind == Some(PublisherKind::Platform)`); record each group once, in first-ordinal order, with the first entry's ordinal, textual fields and element, and conditions = the AND of the group's skip flags (`ManualBinding` is routine-level, unchanged).

- [ ] **Step 1:** Run the six red tests; confirm the stated failures (two routes / two subscriptions / `subscribers_of` len 2).
- [ ] **Step 2:** Implement the two-pass merge inside `SubscriberIndex::build` (keep `build_entry` as the one place conditions are built; derive the merged conditions from it). Keep `subscriptions()` order as documented (`index.rs:393-394`).
- [ ] **Step 3:** Reword the A6b doc comment in `build.rs`.
- [ ] **Step 4:** Run: `cargo test -p al-sem --lib program::resolve::index` and `cargo test --test cli cli_attribute_trivia::` and `cargo test -p al-sem --lib program::build` → all PASS (incl. #103's A4/A6b unchanged).
- [ ] **Step 5:** Discrimination proofs: drop the element-only-for-Platform rule (always key on element) → `a3e_codeunit_…` fails; key on textual publisher instead of resolved id → `a3b_…` fails; OR the flags instead of AND → the skip-flag test fails; record per entry (no merge) → `a3_…` and `two_entries_…` fail.
- [ ] **Step 6:** `check-diff`, commit `fix(program): one subscription per distinct event across per-build readings (#104)`.

### Task 3: Isolated from every reading (Design item 3)

**Files:**
- Modify: `src/program/model/events.rs` (`parse_isolated`, ~:116-140)
- Test (exists, red): `src/program/model/events.rs` `isolated_in_the_second_reading_makes_the_event_isolated`, `isolated_is_read_from_every_reading_of_every_kind`

**Interfaces:**
- Consumes/produces: none new. `parse_isolated` reads every `IntegrationEvent` (arg 2) and `BusinessEvent` (arg 1) reading; `Some(true)` if any is `true` or carries the arg non-literal (existing conservative rule); else `None`. `InternalEvent` stays unread (discovery, ledger).

- [ ] **Step 1:** Run the two red tests; confirm "not isolated".
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** Run: `cargo test -p al-sem --lib program::model::events` → PASS.
- [ ] **Step 4:** Discrimination: read only the first reading of each kind → both fail.
- [ ] **Step 5:** `check-diff`, commit `fix(model): Isolated from every per-build reading (#104)`.

### Task 4: CHANGELOG

**Files:** Modify: `CHANGELOG.md` (`## [Unreleased]` → `### Fixed`).

- [ ] **Step 1:** One entry naming the three fixes (IncludeSender union, one subscription per distinct event incl. name-vs-number and the platform-element rule, Isolated from every reading), the `count_unknown_include_sender_plus1_subscribers` note, and #104.
- [ ] **Step 2:** `check-diff`, commit `docs: CHANGELOG for #104`.
