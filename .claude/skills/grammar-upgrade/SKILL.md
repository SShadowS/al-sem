---
name: grammar-upgrade
description: Use when tree-sitter-al main moves or a grammar bump/lowerer change is needed: the gen-syntax / kind_policy / NAMED_KIND_COUNT / CACHE_VERSION_GRAMMAR checklist and the lowerer shape notes.
---

# Upgrading the grammar

**Upgrading the grammar (the checklist the v4.3.0, v4.4.0, v4.4.1, `bf72a2d0`, `8682cb5f`, `4d75846c` and `960ed312` bumps followed):**

1. `git -C tree-sitter-al pull` — moves the local clone to the new `main` tip. `main` keeps
   moving (it moved twice during the `bf72a2d0` upgrade): check `origin/main` again before
   you commit, since CI builds whatever `main` is when it runs.
2. `cargo run -p xtask -- gen-syntax` — regenerates the raw vocabulary AND the
   `node-types.sha256` sidecar. Skipping this is not an option: `crates/al-syntax/build.rs`
   sha256s the grammar's `node-types.json` and PANICS on mismatch, so the build refuses
   before a single test runs.
3. **Triage any new `RawKind` in `crates/al-syntax/src/schema/kind_policy.rs`** (the
   exhaustive match fails the build on purpose — classify it, never wildcard it; new
   `Preproc*` nodes are triaged `Structural`, the lowerer owns descent) and update
   `crates/al-syntax/src/raw/mod.rs`'s `NAMED_KIND_COUNT` anchor in the same commit. That
   anchor is a `--lib` test, so `scripts/check-goldens` will NOT catch it.
4. **Bump `CACHE_VERSION_GRAMMAR`** (`src/engine/gate/cache_prune.rs`) and its mirror in
   `tests/cli/cli_c_cache_differential.rs`, plus the `grammar` stamp in the two
   `tests/cli-c-goldens/cache/fixture-cache/` artifacts that carry the CURRENT tuple
   (`1234...` and `cafecafe...`; `babebabe...` is deliberately stale). The `cafecafe`
   Kept fixture's `artifactContentHash` is a sha256 over its own raw text, so it must be
   RECOMPUTED after the stamp changes — edit the stamp, then rehash. This constant keys
   dependency-cache invalidation: leave it stale and caches minted under the old grammar
   are silently reused under the new one. `cache_version_grammar_tracks_the_linked_grammar`
   is the guard, and it is a `--lib` unit test, so **`scripts/check-goldens` does NOT run
   it** — run `cargo test -p al-sem --lib` too. The stamp carries the grammar COMMIT
   (`tree-sitter-al-v4.4.1-960ed312-native`) because `main` changes shapes without moving
   the package version; the guard checks only the version part, so a same-version bump
   passes it with a stale commit — update the commit by hand. A longer stamp can push
   `1234….json` across a 0.1 KB boundary in `tests/cli-c-goldens/cache/dry-run.txt`.
5. **Measure both sides.** Run the full suite on the OLD grammar and the NEW one and diff
   the per-test outcomes; that is the only way to attribute a moved golden to the grammar
   rather than to whatever else is in your diff. `gen-syntax` makes the revert cheap: check
   the old grammar commit out in `tree-sitter-al/`, re-run it, and the vocabulary returns to the committed bytes —
   no `git checkout --` of generated files needed.
6. Update CLAUDE.md's Grammar section version numbers and the Prerequisites line.

# Notes if you touch the lowerer itself

**Notes still relevant if you touch the lowerer itself** (`crates/al-syntax/src/lower/mod.rs`,
the one place that still reads raw grammar shapes):
- A scoped member trigger (`Object::Member` triggers) is a single named
  `member_trigger_name` CST node (`object`/`member` fields, no literal `::` token in its
  field set) rather than a plain `(identifier)`/`(quoted_identifier)` — `routine_name_text`
  joins the two fields back into `Object::Member` text. Editor consumers
  (`tree-sitter-al/queries/highlights.scm` + `tree-sitter-al/queries/tags.scm`) capture `member_trigger_name` directly;
  the engine does not use `.scm` queries at all (IR-only).
- A `case` branch's `pattern` field binds one `_single_pattern` value per branch value —
  the `,` separators are never tagged `pattern` — and an `in`-as-case-pattern lowers to
  the named `in_expression` node, never an inline `seq` leaking `left`/`operator`/`right`.
  The lowerer keeps a defensive `is_named()` filter on `children_by_field(Pattern)` as
  belt-and-suspenders (an anonymous `,` token has no `RawKind` and would panic if it ever
  reached `lower_expr`).
- Object/field/action bodies wrap their content in a `declaration_body` node (repeat1 of
  `_body_element`); a code block's statements wrap in `statement_block`; report dataitem
  bodies in `report_body`; `var_section`'s body in `var_body`. `grammar.js`'s own rule
  definitions are the source of truth — grep it rather than trusting this list to stay
  current.
- **Validate a lowerer change with the differential goldens** (`cargo test`): zero
  divergences = behaviour-preserving. Goldens are **Rust-owned baselines** regenerated
  from this engine — see Testing Philosophy & Goldens in CLAUDE.md.
- Dump a real tree with `tree-sitter parse <file.al>` from `tree-sitter-al/` — always
  verify a grammar-shape claim against real `tree-sitter parse` output, not just a read
  of `grammar.js` (two owned-grammar field-pollution bugs upstream of the lowerer were
  found exactly this way; see CHANGELOG history).
- **v4.0.0 shapes (2026-08-12 upgrade — what moved and what to rely on):**
  - The statement terminator `;` lives OUTSIDE `code_block`, `if_statement` and the
    branch rules — statement/block spans end at their last real token (1 byte earlier
    than v3 when a `;` followed). Body/branch fields (`body`, `then_branch`,
    `else_branch`, …) hold exactly ONE named node, never `[stmt, ';']`.
  - A dangling `else` binds to the INNER `if` (was: outer construct) — the one v4
    change that alters what a program means. `case … else` bodies are one
    `statement_block`.
  - Operator precedence is Pascal-correct: `and`/`or`/`xor` bind TIGHTER than
    comparison, unary minus tighter than `*` (`-a * b` = `(-a) * b`), and `..` is no
    longer an expression operator. Binary IR shapes reflect this.
  - ~70 more keywords became NAMED nodes (`record_keyword`, `field_keyword`, …), plus
    `assignment_operator` and `filter_operator`. They appear as named children
    everywhere — `schema/kind_policy.rs` classifies them `Trivia` and the shared
    `structural_children` filter drops them, so positional reads stay clean. A new
    RawKind variant fails `kind_policy.rs`'s exhaustive match (the loudness gate):
    triage it there, never wildcard it.
  - Keyword nodes have a UNIFORM shape (one anonymous child, canonical lowercase
    spelling) — read a keyword's text from the node itself, never a child. Queries
    naming non-lowercase anonymous spellings (`"IF"`, `"Then"`) no longer compile.
  - Dotted-reference fields no longer include the `.` separator tokens.
