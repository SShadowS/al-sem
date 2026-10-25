# Issue #111 plan: `digest --diff` reads plain `diff -u` / `diff -ru` headers

The design is the ledger's `## Design` (bounded issue; panel-converged, design hash `f2d7215c6520765b`):
`.agent/issue-111/ledger.md`. One task.

## Task 1 — end a `---`/`+++` header at the first raw TAB

**Files:** `src/engine/l5/diff_parser.rs` (`extract_path`, ~:165; unit tests U1, U2 beside the existing ones),
`tests/cli/cli_digest_changed_spans.rs` (CLI tests C1-C4 beside #76 T3, ~:2353, through the `digest()` helper ~:83;
update the stale doc comment ~:2354-2356), `CHANGELOG.md` (`[Unreleased]` → Fixed).

**Steps:**
1. Red: write C1, C2, C3, C4, U1, U2 exactly as the Design states, header bytes as literals. C1-C4 and U1 fail on the
   base for the stated reason (C1/C3: `diff-file-unmatched` on a path with `\t<time>` glued on; C2: `diff-file-unmatched`
   on `/dev/null\t<time>`; C4: the quoted path is not unquoted; U1: the added file's path keeps the timestamp). U2
   passes on the base by design.
2. Green: `let rest = rest.split_once('\t').map_or(rest, |(path, _)| path);` before the `/dev/null` test and
   `unquote_path`, with a one-line comment stating the supported wire format and the residual (P7).
3. Discrimination proofs, one break per test (made with Edit, grep-checked, reverted with Edit): C1-C3 and U1 drop
   the TAB cut; C4 and U2-tab cut after `unquote_path`; U2-space cut at the first space instead of TAB.
4. Update the stale comment; add the CHANGELOG entry (with the non-goals).
5. Gate `bash scripts/ci-steps task-affected` exit 0; `check-diff` by the conductor; one commit.

**Expected moves:** no golden (no golden diff carries a TAB header); CDO unaffected.

**Review focus:** the cut happens before the `/dev/null` test and before unquoting; no git form is damaged (U2); every
test asserts an exact result and fails under its own break.
