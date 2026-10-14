# Issue #71: digest changed roots by routine declaration span — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `alsem digest --file` / `--diff` (and prove's anchor) resolve a changed routine by its whole declaration span (attributes, header, body), read hunk bodies line by line, treat Interface/ControlAddIn procedures as declaration-only roots, and anchor every entry at its declaration.

**Architecture:** Spans come from ONE place, the model (`ModelRoutine.source_anchor` plus a new `declaration_start_line` that reaches up to the first attached attribute, and a new `ModelObject.declaration_only_routines` carrier). `resolve_changed_roots` builds a `RoutineSpanIndex` from the `Model` instead of the snapshot's evidence start lines and returns, per root, which declarations were touched; the shared entry builder anchors each entry at the last touched declaration. The diff parser becomes a count-bounded hunk state machine that records `Added(n)` / `DeletedAfter(l)` changes.

**Tech Stack:** Rust (crate `al-sem`, package name with a HYPHEN), `crates/al-syntax` (the lowerer), serde_json, the `alsem` binary.

**Spec:** `.agent/issue-71/ledger.md` (sections Design, Acceptance matrix, Golden license, Acceptance tests (step 6)). Read the Design section before Task 1; every rule below is copied from it.

## Global Constraints

- Every cargo command runs with `TREE_SITTER_AL_PATH=U:/Git/al-call-hierarchy/tree-sitter-al` (this is a worktree; it has no grammar clone).
- Package filter is `-p al-sem` (hyphen). Format each touched file with `rustfmt <file>`, never `cargo fmt`.
- Edit with the Edit/Write tools only, never `sed -i` / python rewrites (the discrimination breaks too).
- Spans are 0-based inclusive `[decl_start, end]`; diff line numbers are 1-based. `Added(n)` touches a span iff `start+1 <= n <= end+1`; `DeletedAfter(L)` iff `start+1 <= L` and `L+1 <= end+1`.
- The indexed population is model routines whose `source_anchor.source_unit_id` starts with `ws:` plus declaration-only procedures of `ws:` object rows. No app-guid filter.
- `source_anchor` of `ModelRoutine` is NOT changed (census and event joins key on it).
- Anchor columns are UTF-8 byte columns exactly as `source_anchor` stores them.
- Existing diagnostic kinds, JSON keys and exit codes keep their spelling; no diagnostic kind is added. New keys: `routine.declarationOnly` (only on declaration-only entries), `routine.anchors` (only on multi-declaration ids), coverage reason `declaration-only`.
- Digest `schemaVersion` 1.4.0 → **1.5.0**; prove stays **1.3.0**.
- Golden license (ledger "Golden license (exact)"): only the 21 digest JSON goldens' `schemaVersion` line, plus the listed fields of `ws-d8-commit-in-tx`, `ws-d34`, `ws-d14-dead-routine` (json + human), `ws-d35`, `ws-txn-d46-neg`, `ws-txn-d48-pos` (json). No prove golden moves. Regenerate only with `scripts/check-goldens --regen`, and only after a written triage receipt at `.agent/golden-triage.md` in the worktree (run the tests WITHOUT regen first to learn the moved-file list).
- Never touch `.github/`, `scripts/`, `.claude/`, `CLAUDE.md`, `.gitignore`, the `Cargo.toml` version, `tree-sitter-al`, or `.agent/` other than `.agent/issue-71/ledger.md`, `.agent/issue-71/findings.json` and the regen receipt `.agent/golden-triage.md`.
- Before every commit: `scripts/ci-steps task` (green). Record each discrimination proof (mutation, fail output, pass output) in the ledger's "Discrimination proofs" table.

## Review Focus

1. A hunk header with omitted counts (`@@ -5 +5 @@`): both counts default to 1 and the body is still count-bounded. Test added in Task 1 (`omitted_counts_bound_the_body`).
2. A modified file whose diff carries headers but no hunk (a mode-only change): unchanged behaviour, every declaration of the file is a root, like a pure rename. Pinned in Task 5 (`hunkless_modified_file_takes_every_declaration`).
3. The same id reached by several inputs at once (`--routine Dup` plus a `--diff` on one arm): touched sets are unioned, so the anchor is the last declaration. Test added in Task 5 (`mixed_inputs_union_touched_declarations`).
4. A `\ No newline at end of file` marker after a hunk is already complete: it is a top-level line and is skipped, never an error. Covered by A1g(e) and Task 1's `no_newline_marker_is_not_counted`.
5. A selector root with no `ws:` span (cannot happen for primary-app source routines, but the entry must not panic): no anchor, entry still built. Covered by the `Option` path in Task 5 (`routine_anchor` is `None` when the id has no span); no separate test, the code has no `unwrap` there.

## Acceptance tests already in the tree (step 6, uncommitted)

`tests/cli/cli_digest_changed_spans.rs` (registered in `tests/cli/main.rs`), the A2c test
`dependency_routines_never_become_changed_roots` in `tests/cli/cli_digest_query_dependencies.rs`,
and the two `schemaVersion` pins moved to `"1.5.0"` (`cli_digest_query_dependencies.rs:169`,
`cli_digest_temp_state.rs:142`). They are red today for the reasons quoted in the ledger.
`scripts/ci-steps task` runs the whole suite, so red tests cannot sit in a commit. Task 1
therefore commits them PARKED: each red test gets `#[ignore = "#71: lands in Task N"]`
(N from the table below), the two pins go back to `"1.4.0"` (Task 6 moves them), and each
later task deletes its own `#[ignore]` lines as its red step. The two green guards
(`object_level_hunk_is_outside_routines`, `limits_are_pinned`) are never ignored.

| test | lands in |
|---|---|
| `hunk_on_assignment_line_is_a_changed_root`, `every_routine_shape_line_maps_to_its_routine`, `context_lines_do_not_create_roots`, `deletions_resolve_by_interior_gap`, `one_inside_one_outside_hunk`, `hunk_body_is_count_bounded`, `bare_header_pair_inside_short_hunk_is_body`, `minus_line_past_old_count_ends_the_hunk`, `bare_header_pair_exact_fill_is_complete`, `rename_with_and_without_hunks`, `file_with_only_opless_routines_is_matched`, `file_digest_returns_every_routine_of_the_file`, `entry_anchor_is_the_declaration`, `duplicate_id_anchor_follows_the_changed_arm`, `dependency_routines_never_become_changed_roots` | Task 5 |
| the two `"1.5.0"` pins | Task 6 |
| `declaration_only_procedures_are_roots` | Task 7 |

Run them: `cargo test -p al-sem --test cli -- cli_digest_changed_spans:: cli_digest_query_dependencies:: cli_digest_temp_state::`

---

### Task 1: Count-bounded hunk bodies in the diff parser

**Files:**
- Modify: `src/engine/l5/diff_parser.rs` (`DiffHunk` :10-16, `parse_hunk_header` :150-202, `parse_unified_diff` :208-400, tests :402-)

**Interfaces:**
- Produces: `pub enum HunkChange { Added(i32), DeletedAfter(i32) }` (derive `Debug, Clone, Copy, PartialEq, Eq`); `DiffHunk { pub new_start: i32, pub new_count: i32, pub old_count: i32, pub changes: Vec<HunkChange> }`; `DiffParseResult.errors` now holds `"hunk at +N truncated"` (N = `new_start`) for every incomplete hunk.

- [ ] **Step 1: Write the failing unit tests** in `diff_parser.rs`'s `mod tests`:

```rust
    use super::HunkChange::{Added, DeletedAfter};

    const HEAD: &str = "--- a/x.al\n+++ b/x.al\n";

    fn changes(diff: &str) -> (Vec<Vec<HunkChange>>, Vec<String>) {
        let r = parse_unified_diff(diff);
        (
            r.files
                .iter()
                .flat_map(|f| f.hunks.iter().map(|h| h.changes.clone()))
                .collect(),
            r.errors,
        )
    }

    #[test]
    fn hunk_changes_follow_the_body() {
        // old: ctx, -, -, blank = 4; new: ctx, +, blank, + = 4.
        let (c, e) = changes(&format!(
            "{HEAD}@@ -3,4 +3,4 @@\n ctx\n-old1\n-old2\n+new\n\n+tail\n"
        ));
        assert_eq!(c, vec![vec![DeletedAfter(3), Added(4), Added(6)]]);
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn pure_deletion_cursor_starts_after_new_start() {
        let (c, _) = changes(&format!("{HEAD}@@ -9,1 +8,0 @@\n-x\n"));
        assert_eq!(c, vec![vec![DeletedAfter(8)]]);
        let (c, _) = changes(&format!("{HEAD}@@ -1,1 +0,0 @@\n-x\n"));
        assert_eq!(c, vec![vec![DeletedAfter(0)]]);
    }

    #[test]
    fn truncated_hunk_is_an_error_with_or_without_final_newline() {
        for d in [
            format!("{HEAD}@@ -3,2 +3,1 @@\n-old\n"),
            format!("{HEAD}@@ -3,2 +3,1 @@\n-old"),
        ] {
            let (c, e) = changes(&d);
            assert_eq!(e, vec!["hunk at +3 truncated".to_string()], "{d:?}");
            assert_eq!(c, vec![vec![DeletedAfter(2)]], "{d:?}");
        }
    }

    #[test]
    fn header_looking_lines_inside_a_body_are_body() {
        let r = parse_unified_diff(&format!("{HEAD}@@ -9,1 +9,1 @@\n--- x\n+++ y\n"));
        assert_eq!(r.files.len(), 1);
        assert_eq!(r.files[0].hunks[0].changes, vec![DeletedAfter(8), Added(9)]);
        assert!(r.errors.is_empty());
    }

    #[test]
    fn the_line_ending_a_truncated_hunk_is_redispatched() {
        let r = parse_unified_diff(&format!(
            "{HEAD}@@ -3,2 +3,1 @@\n-old\ndiff --git a/y.al b/y.al\n--- a/y.al\n+++ b/y.al\n@@ -9,1 +9,1 @@\n-x\n+y\n"
        ));
        let paths: Vec<&str> = r.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["x.al", "y.al"]);
        assert_eq!(r.errors.len(), 1);
        assert_eq!(r.files[1].hunks[0].changes, vec![DeletedAfter(8), Added(9)]);
    }

    #[test]
    fn minus_line_past_old_count_ends_the_hunk() {
        let r = parse_unified_diff(&format!(
            "{HEAD}@@ -3,1 +3,2 @@\n-old\n+a\n--- a/y.al\n+++ b/y.al\n@@ -9,1 +9,1 @@\n-x\n+y\n"
        ));
        let paths: Vec<&str> = r.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["x.al", "y.al"]);
        assert_eq!(r.errors, vec!["hunk at +3 truncated".to_string()]);
    }

    #[test]
    fn exact_fill_completes_the_hunk() {
        let r = parse_unified_diff(&format!(
            "{HEAD}@@ -3,2 +3,1 @@\n-old\n--- a/y.al\n+++ b/y.al\n@@ -9,1 +9,1 @@\n-x\n+y\n"
        ));
        assert_eq!(r.files.len(), 1);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let c: Vec<_> = r.files[0].hunks.iter().map(|h| h.changes.clone()).collect();
        assert_eq!(c, vec![vec![DeletedAfter(2), Added(3)], vec![DeletedAfter(8), Added(9)]]);
    }

    #[test]
    fn no_newline_marker_is_not_counted() {
        let (c, e) = changes(&format!(
            "{HEAD}@@ -9,1 +9,1 @@\n-x\n\\ No newline at end of file\n+y\n\\ No newline at end of file\n"
        ));
        assert_eq!(c, vec![vec![DeletedAfter(8), Added(9)]]);
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn omitted_counts_bound_the_body() {
        let r = parse_unified_diff(&format!("{HEAD}@@ -5 +5 @@\n-a\n+b\n ctx-after\n"));
        let h = &r.files[0].hunks[0];
        assert_eq!((h.old_count, h.new_count), (1, 1));
        assert_eq!(h.changes, vec![DeletedAfter(4), Added(5)]);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
    }
```

Also correct the two short bodies (P16) so they test well-formed input: in `simple_modified_file` replace the body lines `-old\n\` / `+new\n";` with

```rust
            @@ -10,3 +10,4 @@ SomeContext\n\
            \x20ctx1\n\
            -old\n\
            +new\n\
            +new2\n\
            \x20ctx2\n";
```

and add `assert!(r.errors.is_empty(), "{:?}", r.errors);` at its end; do the same in `rename_with_hunks_is_renamed` (`@@ -1,3 +1,4 @@` with `\x20ctx1`, `-old`, `+new`, `+new2`, `\x20ctx2`). A context line's leading space must be written `\x20`: a `\` line continuation eats the next line's leading whitespace.

- [ ] **Step 2: Run them, verify they fail to compile / fail**

Run: `cargo test -p al-sem --lib diff_parser`
Expected: compile error `cannot find type HunkChange` / `no field changes on DiffHunk`.

- [ ] **Step 3: Implement.** In `DiffHunk` add the two fields with doc comments; add `HunkChange` (doc: "1-based new-file numbers; `DeletedAfter(L)` = a gap between new lines L and L+1"). In `parse_hunk_header` keep the old count instead of discarding it:

```rust
    eat_digits(bytes, &mut i)?;
    // (?:,(\d+))?  — omitted count → 1
    let mut old_count = 1;
    if i < bytes.len() && bytes[i] == b',' {
        i += 1;
        old_count = eat_digits(bytes, &mut i)?;
    }
```

and return `DiffHunk { new_start, new_count, old_count, changes: Vec::new() }`. Add the body state:

```rust
/// The hunk whose body is being read: lines left on each side, and the next
/// new-file line (`cursor`). Git writes `new_start` as the line BEFORE the gap
/// when `new_count == 0`, so the cursor then starts one later.
struct OpenHunk {
    new_start: i32,
    old_left: i32,
    new_left: i32,
    cursor: i32,
    in_deletion_run: bool,
}

impl OpenHunk {
    fn new(h: &DiffHunk) -> Self {
        OpenHunk {
            new_start: h.new_start,
            old_left: h.old_count,
            new_left: h.new_count,
            cursor: if h.new_count == 0 { h.new_start + 1 } else { h.new_start },
            in_deletion_run: false,
        }
    }

    fn is_complete(&self) -> bool {
        self.old_left <= 0 && self.new_left <= 0
    }

    /// Consume one line as body. `None`: the line is not body (its side has no
    /// count left, or it is not a body line), so the hunk ends INCOMPLETE.
    /// `Some(change)`: accepted; `change` is the change it records, if any.
    fn accept(&mut self, line: &str) -> Option<Option<HunkChange>> {
        match line.as_bytes().first() {
            Some(b'\\') => Some(None),
            None | Some(b' ') if self.old_left > 0 && self.new_left > 0 => {
                self.old_left -= 1;
                self.new_left -= 1;
                self.cursor += 1;
                self.in_deletion_run = false;
                Some(None)
            }
            Some(b'-') if self.old_left > 0 => {
                self.old_left -= 1;
                let starts_run = !self.in_deletion_run;
                self.in_deletion_run = true;
                Some(starts_run.then_some(HunkChange::DeletedAfter(self.cursor - 1)))
            }
            Some(b'+') if self.new_left > 0 => {
                self.new_left -= 1;
                self.in_deletion_run = false;
                let c = HunkChange::Added(self.cursor);
                self.cursor += 1;
                Some(Some(c))
            }
            _ => None,
        }
    }
}
```

In `parse_unified_diff`: make `errors` `mut`; drop the terminator element —

```rust
    let mut lines: Vec<&str> = normalized.split('\n').collect();
    // A final '\n' is the last line's terminator, not an empty context line.
    if normalized.ends_with('\n') {
        lines.pop();
    }
    let mut open: Option<OpenHunk> = None;
```

— and at the TOP of the loop body, before the `"\\ "` skip and every header test:

```rust
        if let Some(h) = open.as_mut() {
            if let Some(change) = h.accept(line) {
                if let Some(c) = change {
                    current_file
                        .as_mut()
                        .and_then(|f| f.hunks.last_mut())
                        .expect("an open hunk belongs to the current file")
                        .changes
                        .push(c);
                }
                if h.is_complete() {
                    open = None;
                }
                continue;
            }
            errors.push(format!("hunk at +{} truncated", h.new_start));
            open = None;
            // The line that ended the hunk is a top-level line: fall through.
        }
```

Replace the hunk-header arm with:

```rust
        if line.starts_with("@@")
            && let Some(hunk) = parse_hunk_header(line)
            && let Some(ref mut f) = current_file
        {
            let state = OpenHunk::new(&hunk);
            f.hunks.push(hunk);
            if !state.is_complete() {
                open = Some(state);
            }
        }
```

and before the end-of-input block: `if let Some(h) = open { errors.push(format!("hunk at +{} truncated", h.new_start)); }`. Update the module doc (line 4: "Only the new-side line ranges are extracted" → the hunk body is read, count-bounded; header tests do not apply inside a body).

- [ ] **Step 4: Run, verify green.** `cargo test -p al-sem --lib diff_parser` → all pass. Then `scripts/check-goldens` → green (the committed `ws-d8-commit-in-tx.changed.diff` is well-formed: old 14 = 3+1+10, new 15 = 3+2+10, so no error appears and `resolve_changed_roots` still reads only `new_start`/`new_count`). The acceptance tests' parser-level assertions (A1g, c2, c2-overrun, c2-exact) now pass; their digest assertions stay red until Task 5.

- [ ] **Step 5: Discrimination (from the matrix, A1g).** With Edit: (a) move the `if let Some(h) = open.as_mut()` block BELOW the `--- ` / `+++ ` arms → `header_looking_lines_inside_a_body_are_body` fails (2 files); revert. (b) Make `accept` reject `None` (empty line) → `hunk_changes_follow_the_body` fails; revert. (c) Delete the `lines.pop()` → `truncated_hunk_is_an_error_with_or_without_final_newline` fails on the with-newline case; revert. (d) Change the `-` arm guard to `self.old_left > 0 || self.new_left > 0` → `minus_line_past_old_count_ends_the_hunk` fails; revert. Record each in the ledger.

- [ ] **Step 6: Park the acceptance tests.** With Edit, add `#[ignore = "#71: lands in Task N"]` under each red test's `#[test]` (table above), and set `cli_digest_query_dependencies.rs:169` and `cli_digest_temp_state.rs:142` back to `"1.4.0"`. Before parking, run the parser-level lines of the three bare-header tests once by hand (they now pass up to their first `digest(` call).

- [ ] **Step 7: Gate and commit.** `rustfmt` the touched files; `scripts/ci-steps task` green.

```bash
git add src/engine/l5/diff_parser.rs tests/cli/main.rs tests/cli/cli_digest_changed_spans.rs tests/cli/cli_digest_query_dependencies.rs tests/cli/cli_digest_temp_state.rs
git commit -m "digest: read diff hunk bodies line by line, bounded by the header counts (#71)"
```

No golden regen in this task.

---

### Task 2: Attribute origin in the IR (the lowerer change the design names)

**Files:**
- Modify: `crates/al-syntax/src/ir/decl.rs:95-102` (`AttributeIr`), `crates/al-syntax/src/lower/mod.rs:1452` (its one construction site), tests in `lower/mod.rs`'s `mod tests` (:2842)

**Interfaces:**
- Produces: `AttributeIr { name, raw, args, pub origin: Origin }` — the `attribute_item` node's origin.

- [ ] **Step 1: Failing test** (append to `lower/mod.rs` `mod tests`):

```rust
    /// #71: an attribute keeps its own origin, so a routine's declaration span can
    /// start at its first attached attribute.
    #[test]
    fn attribute_origin_is_the_attribute_item() {
        let src = "codeunit 1 C\n{\n    [IntegrationEvent(false, false)]\n    [Obsolete('x', '1.0')]\n    procedure P()\n    begin\n    end;\n}\n";
        let af = parse(src);
        let r = &af.objects[0].routines[0];
        let rows: Vec<(u32, u32)> = r
            .attributes_parsed
            .iter()
            .map(|a| (a.origin.start.row, a.origin.start.column))
            .collect();
        assert_eq!(rows, vec![(2, 4), (3, 4)]);
        assert_eq!(r.origin.start.row, 4, "the routine node starts at the keyword");
    }
```

- [ ] **Step 2:** `cargo test -p al-syntax --lib attribute_origin` → compile error `no field origin`.
- [ ] **Step 3:** Add `pub origin: super::Origin,` (doc: "The `attribute_item` node's origin (#71: the routine's declaration span starts at its first attribute).") and at `lower/mod.rs:1452` add `origin: origin_of(item),`.
- [ ] **Step 4:** Test passes; `cargo test -p al-syntax --lib` green.
- [ ] **Step 5: Discrimination.** Replace `origin_of(item)` with the routine's own node origin (the `node`/arm node in scope) → the test fails on rows `[(4, 4), (4, 4)]`; revert. Record.
- [ ] **Step 6:** Ask the `lowerer-reviewer` agent to review the diff (CLAUDE.md: any `lower/` change). `rustfmt` both files; `scripts/ci-steps task`; commit:

```bash
git add crates/al-syntax/src/ir/decl.rs crates/al-syntax/src/lower/mod.rs
git commit -m "al-syntax: keep each routine attribute's origin (#71)"
```

No golden moves (`AttributeIr` is not serialized).

---

### Task 3: `ModelRoutine.declaration_start_line`

**Files:**
- Modify: `src/program/model/workspace.rs` (`ModelRoutine` :412-500; routine loop :1257 and the push at :1320-1350; new helper)
- Modify (literal sites, add the field): `src/program/model/abi_rows.rs`, `src/program/model/program_calls.rs`, `src/program/model/program_calls_adapter_tests.rs`, `src/engine/gate/format_pr_summary.rs`, `src/engine/l4/cfg_walker.rs`, `src/engine/l4/db_effect_solver.rs`, `src/engine/l5/d1_temp.rs`, `src/engine/l5/detectors/d17.rs`, `src/engine/l5/fingerprint.rs`, `src/engine/l5/table_display.rs`, `src/engine/l5/test_support.rs`, `src/engine/return_summary.rs`, `src/engine/root_classification.rs`, `tests/l4_summary_differential.rs`, `tests/temp_state/temp_state_path.rs`, `tests/temp_state/temp_state_shadowing.rs`
- Test: `tests/cli/cli_digest_changed_spans.rs`

**Interfaces:**
- Consumes: `AttributeIr.origin` (Task 2).
- Produces: `ModelRoutine.declaration_start_line: u32` (0-based); `pub(crate) fn declaration_start_line(r: &al_syntax::ir::RoutineDecl) -> u32` in `workspace.rs` (Task 4 reuses it).

- [ ] **Step 1: Failing test** (append to `cli_digest_changed_spans.rs`):

```rust
/// Plan Task 3: a routine's declaration starts at its first attached attribute;
/// `source_anchor` still starts at the keyword.
#[test]
fn model_routine_declaration_start_includes_attributes() {
    let dir = tempfile::tempdir().unwrap();
    d8_workspace(dir.path(), &d8_posting());
    let model_id = compute_gate_model_instance_id(dir.path()).unwrap();
    let model = assemble_and_resolve_workspace_program(dir.path(), &model_id, false).unwrap();
    let r = |name: &str| {
        model
            .workspace
            .routines
            .iter()
            .find(|r| r.name == name)
            .unwrap()
    };
    // 0-based: `[IntegrationEvent]` row 17, `procedure OnAfterPostSalesDoc` row 18.
    let p = r("OnAfterPostSalesDoc");
    assert_eq!((p.declaration_start_line, p.source_anchor.start_line), (17, 18));
    let q = r("PostSalesDoc");
    assert_eq!((q.declaration_start_line, q.source_anchor.start_line), (2, 2));
}
```

- [ ] **Step 2:** `cargo test -p al-sem --test cli cli_digest_changed_spans::model_routine` → compile error `no field declaration_start_line`.
- [ ] **Step 3:** Add the field after `source_anchor` with the doc "0-based first line of the declaration: the minimum of the routine node's start row and every attached attribute's start row (#71). `source_anchor` is unchanged." Add the helper:

```rust
/// The 0-based first line of a routine's declaration: its own node, or its first
/// attached attribute when one sits above it (#71).
pub(crate) fn declaration_start_line(r: &al_syntax::ir::RoutineDecl) -> u32 {
    r.attributes_parsed
        .iter()
        .map(|a| a.origin.start.row)
        .fold(r.origin.start.row, u32::min)
}
```

Set `declaration_start_line: declaration_start_line(ir_routine),` in the push at :1320. In every other literal listed above add `declaration_start_line: 0,` (none of them is a `ws:` source routine of the real assembly, so the span index never reads them).

- [ ] **Step 4:** Test passes; `cargo check -p al-sem --all-targets` clean.
- [ ] **Step 5: Discrimination (matrix A1b mutation (i)).** Make the helper return `r.origin.start.row` → the test fails `(18, 18)` vs `(17, 18)`; revert. Record.
- [ ] **Step 6:** `rustfmt` each touched file; `scripts/ci-steps task`; commit.

```bash
git add src/program/model/workspace.rs src/program/model/abi_rows.rs src/program/model/program_calls.rs src/program/model/program_calls_adapter_tests.rs src/engine/gate/format_pr_summary.rs src/engine/l4/cfg_walker.rs src/engine/l4/db_effect_solver.rs src/engine/l5/d1_temp.rs src/engine/l5/detectors/d17.rs src/engine/l5/fingerprint.rs src/engine/l5/table_display.rs src/engine/l5/test_support.rs src/engine/return_summary.rs src/engine/root_classification.rs tests/l4_summary_differential.rs tests/temp_state/temp_state_path.rs tests/temp_state/temp_state_shadowing.rs tests/cli/cli_digest_changed_spans.rs
git commit -m "model: ModelRoutine.declaration_start_line reaches the first attribute (#71)"
```

No golden moves.

---

### Task 4: `ModelObject.declaration_only_routines` for Interface / ControlAddIn

**Files:**
- Modify: `src/program/model/workspace.rs` (`ModelObject` :37-102, new struct, the object loop :710-905)
- Modify (literal sites, add `declaration_only_routines: Vec::new()`): `src/program/model/abi_rows.rs`, `src/engine/l5/detectors/d17.rs`, `src/engine/l5/detectors/d50.rs`, `src/engine/l5/fingerprint.rs`, `src/engine/return_summary.rs`, `src/engine/root_classification.rs`
- Test: `tests/cli/cli_digest_changed_spans.rs`

**Interfaces:**
- Consumes: `declaration_start_line(&RoutineDecl)` (Task 3), `ir_parameter_symbols`, `normalized_signature_hash`, `to_stable_routine_id_from_parts`, `to_stable_object_id`, `anchor_from_origin`.
- Produces:

```rust
/// An Interface / ControlAddIn procedure declaration (#71). Not a `ModelRoutine`
/// (the model keeps skipping those object kinds); digest reads these to make a
/// changed declaration a root.
#[derive(Debug, Clone)]
pub struct ModelDeclarationOnlyRoutine {
    pub name: String,
    /// `{stableObjectId}#{64 hex}`, the signature hash folded with the declaring
    /// object's NAME (every unnumbered object shares `<guid>:<Kind>:0`).
    pub stable_routine_id: String,
    pub parameters: Vec<ModelParameter>,
    pub return_type: Option<String>,
    pub source_anchor: crate::program::body::features::PAnchor,
    pub declaration_start_line: u32,
}
```

and `ModelObject.declaration_only_routines: Vec<ModelDeclarationOnlyRoutine>` (empty for every other kind).

- [ ] **Step 1: Failing test** (append to `cli_digest_changed_spans.rs`):

```rust
/// Plan Task 4: Interface/ControlAddIn procedures ride on their object row with a
/// name-folded stable id; they are still not `ModelRoutine`s.
#[test]
fn interface_object_rows_carry_declaration_only_routines() {
    let dir = tempfile::tempdir().unwrap();
    decl_only_workspace(dir.path());
    let model_id = compute_gate_model_instance_id(dir.path()).unwrap();
    let model = assemble_and_resolve_workspace_program(dir.path(), &model_id, false).unwrap();
    let obj = |name: &str| {
        model
            .workspace
            .objects
            .iter()
            .find(|o| o.name == name)
            .unwrap_or_else(|| panic!("{name}: no object row"))
    };
    let rows = |name: &str| -> Vec<(String, String, u32, u32)> {
        obj(name)
            .declaration_only_routines
            .iter()
            .map(|d| {
                (
                    d.name.clone(),
                    d.stable_routine_id.clone(),
                    d.source_anchor.start_line,
                    d.declaration_start_line,
                )
            })
            .collect()
    };
    let ifoo = rows("IFoo");
    assert_eq!(ifoo.len(), 2, "{ifoo:?}");
    assert_eq!(
        ifoo[0],
        ("Bar".to_string(), decl_only_id("Interface", "IFoo", "Bar"), 2, 2)
    );
    assert_eq!((ifoo[1].0.as_str(), ifoo[1].2), ("Baz", 3));
    assert_ne!(ifoo[1].1, ifoo[0].1);
    assert_eq!(
        rows("IQux"),
        vec![("Bar".to_string(), decl_only_id("Interface", "IQux", "Bar"), 2, 2)]
    );
    assert_eq!(rows("MyAddIn")[0].1, decl_only_id("ControlAddIn", "MyAddIn", "Init"));
    assert!(rows("IEmpty").is_empty());
    assert!(rows("DeclCu").is_empty());
    assert!(
        !model.workspace.routines.iter().any(|r| r.name == "Bar"),
        "the Interface skip stays"
    );
}
```

- [ ] **Step 2:** compile error `no field declaration_only_routines`.
- [ ] **Step 3:** Add the struct and field. In the object loop, just before `workspace.objects.push(ModelObject { … })` (:839), compute:

```rust
        // #71: Interface / ControlAddIn procedure declarations, carried on the object
        // row (the routine loop below still skips these kinds). The id folds in the
        // object NAME: every unnumbered object shares the object id `<guid>/<Kind>/0`.
        let declaration_only_routines: Vec<ModelDeclarationOnlyRoutine> =
            if object_type == "Interface" || object_type == "ControlAddIn" {
                let stable_object_id = to_stable_object_id(&object_id);
                o.routines
                    .iter()
                    .filter(|r| !r.name.is_empty())
                    .map(|r| {
                        let syms = crate::program::body::ir_walk::ir_parameter_symbols(r);
                        let specs: Vec<crate::engine::ids::ParamSpec> = syms
                            .iter()
                            .map(|p| crate::engine::ids::ParamSpec {
                                type_text: p.type_text.clone(),
                                is_var: p.is_var,
                            })
                            .collect();
                        let hash = crate::engine::ids::normalized_signature_hash(
                            &r.name,
                            &specs,
                            r.return_type.as_deref(),
                        );
                        ModelDeclarationOnlyRoutine {
                            name: r.name.clone(),
                            stable_routine_id: to_stable_routine_id_from_parts(
                                &stable_object_id,
                                &hash,
                                Some(&name),
                            ),
                            parameters: syms
                                .iter()
                                .map(|p| ModelParameter {
                                    index: p.index,
                                    name: p.name.clone(),
                                    type_text: p.type_text.clone(),
                                    is_var: p.is_var,
                                    is_record: p.is_record,
                                    table_name: p.table_name.clone(),
                                })
                                .collect(),
                            return_type: r.return_type.clone(),
                            source_anchor: anchor_from_origin(&r.origin, source_unit_id, cols),
                            declaration_start_line: declaration_start_line(r),
                        }
                    })
                    .collect()
            } else {
                Vec::new()
            };
```

and add `declaration_only_routines,` to the push. Rewrite the stale skip comment (:880-903) to two lines: the skip keeps these signature-only methods out of `ModelRoutine` so body detectors never see them; digest reads them from `ModelObject.declaration_only_routines` (#71); see the "stale Interface/ControlAddIn skip" discovery. Add `Vec::new()` at the six other literal sites.

- [ ] **Step 4:** Test passes; `cargo check -p al-sem --all-targets` clean.
- [ ] **Step 5: Discrimination (matrix A2d mutation (ii)).** Pass `None` instead of `Some(&name)` → the IQux assertion fails (the two `Bar` ids are equal and differ from `decl_only_id`); revert. Record.
- [ ] **Step 6:** `rustfmt` touched files; `scripts/ci-steps task`; commit the 7 source files and `tests/cli/cli_digest_changed_spans.rs`.

```bash
git commit -m "model: Interface/ControlAddIn procedure declarations on ModelObject (#71)"
```

No golden moves (`ModelObject` is not serialized).

---

### Task 5: Root resolution and entry anchors from the routine span index

**Files:**
- Modify: `src/engine/l5/digest_cli.rs` (delete `build_routine_file_index` :191-227; `ChangedRootsResult` :181-184; `resolve_changed_roots` :236-366; `DigestEntryFull` :415-426; `run_digest_query_full_from_entries` :529-768; entry JSON :943-950; caller :1340-1366), `src/engine/l5/prove.rs` (:908, :965-971, the test literal :1054)
- Test: `tests/cli/cli_digest_changed_spans.rs`, `tests/cli/main.rs`, `tests/cli/cli_digest_query_dependencies.rs` (A2c test)
- Goldens: `tests/cli-b-goldens/digest/` (regen #1)

**Interfaces:**
- Consumes: `HunkChange`/`DiffHunk.changes` (Task 1), `ModelRoutine.declaration_start_line` (Task 3).
- Produces:

```rust
pub struct RoutineSpan {
    pub routine: String,          // stable routine id
    pub source_unit_id: String,   // ORIGINAL unit, e.g. "ws:src/posting.al"
    pub start: u32,               // 0-based decl_start
    pub end: u32,                 // 0-based last row of the routine node
    pub anchor_line: u32,         // source_anchor.start_line
    pub anchor_column: u32,       // source_anchor.start_column (UTF-8 bytes)
    pub declaration_only: Option<DeclOnlyInfo>, // Task 7 fills it
}
pub struct DeclOnlyInfo { pub display: String }
pub struct RoutineSpanIndex { pub spans: Vec<RoutineSpan>, by_file: HashMap<String, Vec<usize>>, by_routine: HashMap<String, Vec<usize>> }
pub fn build_routine_span_index(resolved: &Model) -> RoutineSpanIndex;
pub struct ChangedRootsResult {
    pub roots: Vec<String>,
    pub diagnostics: Vec<ChangedRootsDiagnostic>,
    /// Per root id: indices into `spans.spans` of the declarations the input touched.
    pub touched: HashMap<String, std::collections::BTreeSet<usize>>,
    pub spans: RoutineSpanIndex,
}
pub fn resolve_changed_roots(snap: &CapabilitySnapshot, resolved: &Model, input: &ChangedInput, primary_app_guid: Option<&str>) -> ChangedRootsResult;
pub fn run_digest_query_full_from_entries(snap, roots, base_entries, tx_ctx_map, temp_states, spans: &RoutineSpanIndex, touched: &HashMap<String, BTreeSet<usize>>) -> DigestQueryFullResult;
// DigestEntryFull gains `pub routine_anchors: Vec<FullAnchor>` (empty for a single-declaration id).
```

- [ ] **Step 1: Tests.** Delete the `#[ignore = "#71: lands in Task 5"]` lines (15 tests, table above). Add the two Review Focus tests to `cli_digest_changed_spans.rs`:

```rust
/// Review focus 3: touched sets from several inputs are unioned per id.
#[test]
fn mixed_inputs_union_touched_declarations() {
    let dir = tempfile::tempdir().unwrap();
    shapes_workspace(dir.path(), SHAPES);
    let (doc, code) = digest(
        dir.path(),
        &["--routine", "Dup"],
        Some(&one_line_diff("src/Shapes.al", 21, "    begin")),
    );
    assert_eq!(code, 0, "{doc}");
    assert_eq!(anchor(entry(&doc, "Dup")).1, 24, "{doc}");
}

/// Review focus 2: a modified file with headers and no hunk takes every
/// declaration of the file, like a pure rename (unchanged behaviour).
#[test]
fn hunkless_modified_file_takes_every_declaration() {
    let dir = tempfile::tempdir().unwrap();
    d8_workspace(dir.path(), &d8_posting());
    let (doc, code) = digest(
        dir.path(),
        &[],
        Some("diff --git a/src/posting.al b/src/posting.al\nold mode 100644\nnew mode 100755\n--- a/src/posting.al\n+++ b/src/posting.al\n"),
    );
    assert_eq!(code, 0, "{doc}");
    assert_eq!(roots(&doc), vec!["OnAfterPostSalesDoc", "PostSalesDoc"], "{doc}");
}
```

- [ ] **Step 2:** Run the acceptance command; expected failures are the ones quoted in the ledger's "Acceptance tests (step 6)" table (A1a … A4(c), A2c) plus `hunkless_modified_file_takes_every_declaration` (`PostSalesDoc` only).

- [ ] **Step 3: Implement.** Replace `build_routine_file_index` with:

```rust
impl RoutineSpan {
    /// Does a diff change touch this span? `Added(n)`: n in the span (1-based).
    /// `DeletedAfter(l)`: the gap between new lines l and l+1 is INTERIOR, so
    /// deleting a whole routine, or blank lines between routines, blames no one.
    fn touched_by(&self, c: &HunkChange) -> bool {
        let (first, last) = (i64::from(self.start) + 1, i64::from(self.end) + 1);
        match *c {
            HunkChange::Added(n) => (first..=last).contains(&i64::from(n)),
            HunkChange::DeletedAfter(l) => first <= i64::from(l) && i64::from(l) + 1 <= last,
        }
    }

    fn anchor(&self) -> FullAnchor {
        FullAnchor {
            source_kind: "source",
            file: Some(self.source_unit_id.clone()),
            line: Some(self.anchor_line),
            column: Some(self.anchor_column),
            excerpt: None,
        }
    }
}

impl RoutineSpanIndex {
    fn of_file(&self, normalized: &str) -> &[usize] {
        self.by_file.get(normalized).map_or(&[], Vec::as_slice)
    }
    fn of_routine(&self, id: &str) -> &[usize] {
        self.by_routine.get(id).map_or(&[], Vec::as_slice)
    }
}

/// One span per routine DECLARATION (never collapsed by id), in document order,
/// from the model's workspace routines. Only `ws:` units: dependency source
/// routines are `dep:<guid>:<path>`, ABI-only ones have an empty unit.
pub fn build_routine_span_index(resolved: &Model) -> RoutineSpanIndex {
    let mut spans: Vec<RoutineSpan> = resolved
        .workspace
        .routines
        .iter()
        .filter(|r| r.source_anchor.source_unit_id.starts_with("ws:"))
        .map(|r| RoutineSpan {
            routine: r.stable_routine_id.clone(),
            source_unit_id: r.source_anchor.source_unit_id.clone(),
            start: r.declaration_start_line,
            end: r.source_anchor.end_line,
            anchor_line: r.source_anchor.start_line,
            anchor_column: r.source_anchor.start_column,
            declaration_only: None,
        })
        .collect();
    spans.sort_by(|a, b| {
        (&a.source_unit_id, a.anchor_line, a.anchor_column)
            .cmp(&(&b.source_unit_id, b.anchor_line, b.anchor_column))
    });
    let mut by_file: HashMap<String, Vec<usize>> = HashMap::new();
    let mut by_routine: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, s) in spans.iter().enumerate() {
        by_file.entry(normalize_input(&s.source_unit_id)).or_default().push(i);
        by_routine.entry(s.routine.clone()).or_default().push(i);
    }
    RoutineSpanIndex { spans, by_file, by_routine }
}
```

(`sort_by` is stable, so the arms of an `#if`-split header, which share one origin, keep model order.)

In `resolve_changed_roots` (add `resolved: &Model` as the second parameter; fix the doc: "Files are scoped by the span index's `ws:` filter"): build `let spans = build_routine_span_index(resolved);`, keep `roots: HashSet<String>`, add `touched: HashMap<String, BTreeSet<usize>>` and a local `fn touch(roots, touched, spans, i)` that inserts `spans.spans[i].routine` into both. Then:
- files: `let ixs = spans.of_file(&normalize_input(file));` empty → `FileUnmatched`, else touch each.
- selector unique match `id`: `roots.insert(id.clone()); let t = touched.entry(id.clone()).or_default(); t.extend(spans.of_routine(&id));`
- diff: after the `Deleted` check, `let ixs = spans.of_file(&normalize_input(&diff_file.path));` empty → `DiffFileUnmatched`; `diff_file.hunks.is_empty()` → touch every `ixs`; else per hunk `let hit: Vec<usize> = ixs.iter().copied().filter(|&i| hunk.changes.iter().any(|c| spans.spans[i].touched_by(c))).collect();` empty → `HunkOutsideRoutines` with the SAME `start_line`/`end_line` expressions as today; else touch each.
- return `ChangedRootsResult { roots: sorted, diagnostics, touched, spans }`.

In `run_digest_query_full_from_entries` add the two parameters, DELETE the `anchor_by_rid` block (root slots, ops, call sites) and compute per root:

```rust
        let decls = spans.of_routine(rid);
        // The LAST touched declaration in document order (a single-declaration id:
        // that one). See the ledger's Entry anchor for why last, not first.
        let routine_anchor = touched
            .get(rid.as_str())
            .and_then(|t| t.iter().next_back())
            .map(|&i| spans.spans[i].anchor());
        let routine_anchors: Vec<FullAnchor> = if decls.len() > 1 {
            decls.iter().map(|&i| spans.spans[i].anchor()).collect()
        } else {
            Vec::new()
        };
```

(`touched` holds the indices in ascending order; the vector is in document order, so the last index is the last declaration.) Add `routine_anchors` to `DigestEntryFull` and its push, and to prove's test literal (`routine_anchors: Vec::new(),`). In the entry JSON after the `anchor` insert:

```rust
    if !entry.routine_anchors.is_empty() {
        routine_m.insert(
            "anchors".into(),
            serde_json::Value::Array(entry.routine_anchors.iter().map(anchor_to_value).collect()),
        );
    }
```

Callers: digest `resolve_changed_roots(&snap, resolved, &changed_input, primary_guid_of(resolved))` and pass `&changed_roots.spans, &changed_roots.touched`; prove the same with `&resolved`. Import `HunkChange` and `BTreeSet`.

- [ ] **Step 4: Run.** The acceptance command → every test green except the still-ignored A2d test. Then run `scripts/check-goldens` WITHOUT regen and save the list of failing goldens: expected exactly the json + human of `ws-d8-commit-in-tx`, `ws-d34`, `ws-d14-dead-routine`, and the json of `ws-d35`, `ws-txn-d46-neg`, `ws-txn-d48-pos` (`ws-d8-commit-in-tx.diff.json` and all prove goldens stay green).

- [ ] **Step 5: Golden regen point #1 (licensed).** Run the `golden-diff-triager` agent over the would-be diff and write `.agent/golden-triage.md` in the worktree: one `MOVED:` line per file, each classified against the license (d8: + `OnAfterPostSalesDoc` anchored `ws:src/posting.al` 18:4, `rootsRequested`/`rootsResolved` 2→3, `totalEffects` 8→11, `totalUnresolved` recomputed, + human block; d34: + `DoWork` 43:4, `Persist` anchor 47→45:4, roots 5→6, + human block; d14: + `LiveHelper` 7:1, + `DeadHelper` 9:1, roots 1→3, + 2 human blocks; d35 `PersistAudit` 31→29:4; d46-neg `DoSetup` 31→27:4; d48-pos second `SendOne` 39:8→35:4). The `schemaVersion` line does NOT move yet. Any other moved byte → stop, find out why. Then `scripts/check-goldens --regen`, inspect `git diff --stat tests/`, then `scripts/check-goldens` green.

- [ ] **Step 6: Discrimination (matrix).** Each with Edit, run, revert, run:
  - A1a: replace `build_routine_span_index`'s spans with one per op/call-site start line (`start = end = op.start_line`) → `hunk_on_assignment_line_is_a_changed_root` exit 2.
  - A1b (ii): `HunkChange::Added(n) => (first..last).contains(..)` → the `end;` rows fail. (iii): dedupe `spans` by `routine` → the line-21 `Dup` row fails. (iv) keep only the first span per `(source_unit_id, start)` → the `Split` row fails.
  - A1c: add `|| true` header-window fallback (touch every span overlapping `[new_start, new_start+new_count-1]`) → `context_lines_do_not_create_roots` fails.
  - A1d (ii): `DeletedAfter(l) => (first..=last).contains(&l) || (first..=last).contains(&(l+1))` → (c) gives `PostSalesDoc`.
  - A2c: drop the `starts_with("ws:")` filter → `dependency_routines_never_become_changed_roots` gets entries.
  - A4(c) (i)/(iii): `.next()` instead of `.next_back()` → (c1)/(c3)/(c4) fail; (ii) `decls.last()` regardless of touch → (c2) fails; (iv) drop the `anchors` insert → the `anchors` assertions fail.
  - A4(b): build the anchor file from `normalize_input(...)` → `entry_anchor_is_the_declaration` (b) fails.
  Record each.

- [ ] **Step 7: Gate and commit.** `rustfmt` touched files; `scripts/ci-steps task`.

```bash
git add src/engine/l5/digest_cli.rs src/engine/l5/prove.rs tests/cli/cli_digest_changed_spans.rs tests/cli/cli_digest_query_dependencies.rs tests/cli-b-goldens/digest/
git commit -m "digest: changed roots and entry anchors by routine declaration span (#71)"
```

---

### Task 6: Digest schema 1.5.0

**Files:**
- Modify: `src/engine/l5/digest_cli.rs:1141-1143`, `tests/cli/cli_digest_query_dependencies.rs:169`, `tests/cli/cli_digest_temp_state.rs:142`
- Goldens: the 21 `tests/cli-b-goldens/digest/*.json` (regen #2)

- [ ] **Step 1:** Set both pins to `"1.5.0"` with Edit (parked at `"1.4.0"` in Task 1). Run `cargo test -p al-sem --test cli -- cli_digest_query_dependencies::symbol_only cli_digest_temp_state::` → fail `left: String("1.4.0") right: "1.5.0"`.
- [ ] **Step 2:** Set `"1.5.0"` with the comment `// 1.5.0: additive routine.declarationOnly, routine.anchors and the "declaration-only" coverage reason (#71).` above it (keep the 1.4.0 line of history).
- [ ] **Step 3:** Pins green. `scripts/check-goldens` without regen → exactly the 21 digest JSON goldens fail, each only on `schemaVersion`.
- [ ] **Step 4: Golden regen point #2 (licensed).** Triage receipt at `.agent/golden-triage.md`: 21 `MOVED:` lines, each `"schemaVersion": "1.4.0"` → `"1.5.0"` and nothing else; no prove golden, no human file. `scripts/check-goldens --regen`; `git diff --stat` shows 21 files, 1 line each; `scripts/check-goldens` green.
- [ ] **Step 5: Discrimination.** Set the version back to `"1.4.0"` → both pins and `check-goldens` fail; revert. Record.
- [ ] **Step 6:** `scripts/ci-steps task`; commit the three files and the 21 goldens: `git commit -m "digest: schemaVersion 1.5.0 for the #71 additive keys"`.

---

### Task 7: Declaration-only roots in digest

**Files:**
- Modify: `src/engine/l5/digest_cli.rs` (`build_routine_span_index`, `DigestEntryFull`, `run_digest_query_full_from_entries`, entry JSON), `src/engine/l5/prove.rs` (test literal)
- Test: `tests/cli/cli_digest_changed_spans.rs` (remove the `#[ignore]` from `declaration_only_procedures_are_roots`)

**Interfaces:**
- Consumes: `ModelObject.declaration_only_routines` (Task 4), `RoutineSpan.declaration_only` (Task 5).
- Produces: `DigestEntryFull.declaration_only: bool`; JSON `routine.declarationOnly: true` on those entries only.

- [ ] **Step 1:** Delete the `#[ignore = "#71: lands in Task 7"]` line. Run it → fails at (a): `"changedRootsDiagnostics":[{"file":"src/IFoo.Interface.al","kind":"diff-file-unmatched"}] … left: 2 right: 0`.
- [ ] **Step 2: Implement.** In `build_routine_span_index`, before the sort:

```rust
    for o in &resolved.workspace.objects {
        if !o
            .source_anchor
            .as_ref()
            .is_some_and(|a| a.source_unit_id.starts_with("ws:"))
        {
            continue;
        }
        for d in &o.declaration_only_routines {
            spans.push(RoutineSpan {
                routine: d.stable_routine_id.clone(),
                source_unit_id: d.source_anchor.source_unit_id.clone(),
                start: d.declaration_start_line,
                end: d.source_anchor.end_line,
                anchor_line: d.source_anchor.start_line,
                anchor_column: d.source_anchor.start_column,
                declaration_only: Some(DeclOnlyInfo { display: d.name.clone() }),
            });
        }
    }
```

(make `spans` `mut`). In `run_digest_query_full_from_entries`, move the anchor computation to the top of the loop and, right after it and BEFORE the identity-table display lookup, add:

```rust
        // Declaration-only roots (Interface / ControlAddIn methods) are not in the
        // snapshot: build them from the span index first, so they push no
        // `root-not-in-snapshot` / `no-coverage-record` and count once (P19).
        if let Some(info) = decls
            .first()
            .and_then(|&i| spans.spans[i].declaration_only.as_ref())
        {
            out.push(DigestEntryFull {
                routine_id: rid.clone(),
                routine_display: info.display.clone(),
                object_display: String::new(),
                routine_anchor,
                routine_anchors,
                coverage_status: "unknown".to_string(),
                coverage_reasons: vec!["declaration-only".to_string()],
                effects: Vec::new(),
                unresolved: Vec::new(),
                unresolved_traversal: UnresolvedTraversal {
                    truncated: false,
                    max_depth: 64,
                    visited_routines: 0,
                },
                entry_diagnostics: Vec::new(),
                declaration_only: true,
            });
            continue;
        }
```

Add `pub declaration_only: bool` to `DigestEntryFull` (`false` in the model-routine push and prove's test literal) and in the entry JSON: `if entry.declaration_only { routine_m.insert("declarationOnly".into(), true.into()); }`.

- [ ] **Step 3:** The acceptance command → all green. `scripts/check-goldens` → green with no regen (no golden fixture has an interface or controladdin).
- [ ] **Step 4: Discrimination (matrix A2d).** (i) skip the object loop in `build_routine_span_index` → (a) `diff-file-unmatched`; (iii) build the id with `to_stable_routine_id_from_parts(&stable_object_id, &hash, None)` in Task 4's code → the (a) stableId assertion fails; (iv) move the build-first branch below the display lookup → (a) loses its entry (`rootsResolved` 0, a `DIAG-digest-root-not-in-snapshot`). Revert each. Record.
- [ ] **Step 5:** `rustfmt`; `scripts/ci-steps task`; commit `src/engine/l5/digest_cli.rs src/engine/l5/prove.rs tests/cli/cli_digest_changed_spans.rs`: `git commit -m "digest: Interface/ControlAddIn procedures are declaration-only roots (#71)"`.

---

### Task 8: CHANGELOG and the branch gate

**Files:**
- Modify: `CHANGELOG.md` (`[Unreleased]`)

- [ ] **Step 1:** Under `[Unreleased]` add, in plain English:
  - `### Changed`: `alsem digest --file` / `--diff` resolve changed roots by routine declaration span (attribute lines, header, `var`, body, `end;`), not by the start lines of indexed operations; `file-unmatched` now means the file declares no routine at all, `diff-file-unmatched` the same for a diff path, `hunk-outside-routines` that the hunk's changed lines touch no span. Interface and ControlAddIn procedures are declaration-only roots (`routine.declarationOnly`, coverage reason `declaration-only`, no effects). Entries in digest and prove anchor at the declaration (`routine.anchor`, the last touched declaration of a multi-declaration id), with `routine.anchors` listing every declaration of such an id. Digest `schemaVersion` 1.4.0 → 1.5.0 (additive keys); prove stays 1.3.0.
  - `### Fixed`: hunk bodies are read line by line and bounded by the header counts, so context lines no longer create roots, pure deletions inside a routine resolve, header-looking body lines (`--- x` deleted, `+++ y` added) are body, and a truncated hunk is reported as `diff-parse-error`. `--file dep:<guid>:<path>` no longer selects dependency routines.
  - A one-line note for bc-dev-mcp (separate repo): `risk.ts` can stop avoiding `--diff`, and `parseAlSemDigest` should expand an entry over `routine.anchors`.
- [ ] **Step 2: Branch gate.** `scripts/ci-steps all`; `scripts/check-goldens --verify-coverage`; `scripts/check-goldens`; `scripts/cdo-gate U:/Git/DO-cdo-baseline/Cloud` (redirect to a log and `grep` it; never pipe through `tail`). A3: `git diff --stat <base>..HEAD -- tests/` shows exactly 21 digest JSON + 3 digest human goldens and no prove golden; the last triage receipt covers them.
- [ ] **Step 3:** Ledger: fill "Gate results" and "Base/Head SHAs". Commit `CHANGELOG.md`: `git commit -m "changelog: #71 digest routine spans"`.

---

## Self-review

- Spec coverage: Data plumb 1 → Task 2; 2 → Task 3; 3 + Matching + Entry anchor → Task 5; 4 → Task 1; 5 → Task 4; 6 → Tasks 5 (model routines, prove) and 7 (declaration-only, build first); 7 → Task 6; Also-in-the-change → Task 8; A1a-A1i, A2a-A2c, A4, A4(c) → Task 5; A1g parser parts → Task 1; A2d → Task 7; A2d(f) pins → Task 6; A3 → Tasks 5, 6, 8.
- Every type used later is defined in an earlier task: `HunkChange` (1), `AttributeIr.origin` (2), `declaration_start_line` field and helper (3), `ModelDeclarationOnlyRoutine` (4), `RoutineSpan`/`RoutineSpanIndex`/`DeclOnlyInfo`/`ChangedRootsResult.touched`/`.spans`/`DigestEntryFull.routine_anchors` (5), `DigestEntryFull.declaration_only` (7).
- No lowerer change beyond the one the design names (Task 2).
