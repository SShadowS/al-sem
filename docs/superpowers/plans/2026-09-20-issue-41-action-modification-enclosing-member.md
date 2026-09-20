# Issue #41 — action-extension triggers carry their enclosing member

One task. The design and its two review rounds live in `.agent/issue-41/ledger.md`; the
register is `.agent/issue-41/findings.json` (17 entries, A1-A12 + B1-B5).

## Task 1 — capture `modify_action_modification` as a member wrapper, and classify it

### Code changes

1. `crates/al-syntax/src/lower/mod.rs`, `collect_routines` (~:630): the arm becomes
   `RawKind::ModifyModification | RawKind::ModifyActionModification`. Copy the sibling arm
   verbatim, `pending.clear()` included.
2. Same file, `lower_routine`'s enclosing-member fallback (~:711): the `target` fallback matches
   both kinds, and the whole `enclosing_member` is `.filter(|(t, _)| !t.is_empty())` — a
   recovered zero-width `target` (or `name`) must degrade to `None`, never `Some("")`, which
   would mint a different, meaningless stable id (A6, B3).
3. Same file (~:593): one comment line — the `in_dataset_modify_context` gate is deliberately
   `ModifyModification`-only and must not be folded into a shared modify-wrapper helper (B5).
4. Same file (~:705-710): replace the `add*`/`move*` comment. The current wording ("an insertion
   ANCHOR naming a different member") is factually wrong for `add`/`addfirst`/`addlast`, whose
   target is the CONTAINER, and two `views` forms have no target at all. Correct wording (A4,
   ratified in round 2): an `add*`/`move*` wrapper's `target` is never the declaring member of a
   routine in its body — either the body declares that member itself (the ordinary name gate
   finds it) or the routine sits directly in the body and has no declaring member at all
   (`None`, pinned by the control test). `modify` is the only form whose target IS the member the
   body belongs to.
5. `src/engine/root_classification.rs`: add `"modify_action_modification"` to
   `is_page_action_wrapper`, and rewrite the KNOWN GAP paragraph (~:176-180) — it currently says
   the gap "could not be fixed by adding a fifth string here" BECAUSE the lowerer never captures
   the wrapper, which this commit makes false. State instead that it is closed, and state the
   residual: a trigger declared DIRECTLY in an `add*` body has no declaring member, gets no
   `page-action`, and that is the intended answer (A2, B5).
6. `CHANGELOG.md`: correct the `[Unreleased]` bullet at ~:140-146 that repeats the same false
   claim (B2), and add a `Fixed` entry for this change including the de-collision consequence:
   two sibling action `modify()` triggers stop sharing one routine id, so a committed `alsem`
   baseline covering that shape re-fingerprints.
7. `tests/cli/cli_p1_enclosing_member.rs`: the module header's clause (b) still says "the SAME
   `stable_routine_id`"; the test has asserted the opposite since task 4. Fix the header (B4).

### Tests — write them FIRST, run them, capture the red output

In `crates/al-syntax/src/lower/mod.rs`'s test module, beside the three existing
`modify_modification` tests:

- **T1** pageextension `actions { modify(MyAction) { trigger OnAfterAction() … } }` →
  `enclosing_member` is `Some(("MyAction", origin))` AND `origin.kind_text ==
  "modify_action_modification"` (B1 — this assert is the only executable join between the
  lowerer and the hand-stated classifier test; do not omit it). Keep the
  `in_dataset_modify_context == false` assertion but document it as false BY CONSTRUCTION in a
  pageextension, not as a pin of the kind gate (A9).
- **T2** quoted, doubled-quote target `modify("My ""Q"" Action")` → the IR holds the
  outer-stripped, still-escaped text `My ""Q"" Action`; unescaping happens later in
  `ir_walk::ir_enclosing_member`, and this test pins that boundary (A8).
- **T3 CONTROL** `addlast(Processing) { trigger OnDirect() begin end; }` → `enclosing_member` is
  `None`. Document it as a PARSE-SHAPE contract (grammar-admitted, not compilable AL) so nobody
  later "corrects" it into `addlast(X) { action(Y) { trigger … } }`, which is shadowed by the
  inner `action_declaration` and cannot fail (A1, A11, B5).
- **T4** empty `modify() { trigger OnAfterAction() … }` in an `actions` section → `None`, not
  `Some("")`; and **T5** the same for a report-dataset `modify()` (the other kind). Both also
  assert the routine was still found (e.g. `routines.len() == 1`) so a future parse change
  cannot make them vacuous (A6, B5, fable M3).

In `src/engine/root_classification.rs`'s test module: an action-modification page trigger gains
`page-action`, hand-stated through the existing `page_trigger_kinds(Some(...))` helper (A2).

In `tests/cli/cli_p1_enclosing_member.rs`: a pageextension with TWO action `modify()` blocks,
each declaring `OnAfterAction()` → distinct `stable_routine_id`, distinct `enclosing_member`, and
`enclosing_member_range.syntax_kind == "modify_action_modification"`. Mirror the neighbouring
test's legacy-re-mint precondition (re-mint both with `enclosing_member: None` and assert they
COLLIDE), so the test cannot pass for an unrelated reason (A3, B1, fable N3).

### Discrimination proofs (required, recorded)

For each: apply the break, ASSERT the patch actually applied, run, record the failing output,
restore under a sha256 check, re-run green.

- D1 remove the `ModifyActionModification` arm → T1 fails.
- D2 remove the fallback's kind match → T1 fails.
- D3 widen the arm AND fallback to the `add*` family → T3 (control) fails with
  `Some("Processing")`.
- D4 drop the `.filter(|(t, _)| !t.is_empty())` → T4 and T5 fail with `Some("")`.
- D5 change `origin_of(m)` to `origin_of(node)` → T1's `kind_text` assert fails.
- D6 remove `"modify_action_modification"` from `is_page_action_wrapper` → the classifier test
  fails.
- D7 revert the lowerer arm only (classifier string kept) → the `cli_p1` id test fails.

### Constraints

- `rustfmt <file>` per touched file, never `cargo fmt`.
- Never touch `.github/`, `scripts/`, `.claude/`, `CLAUDE.md`, `.gitignore`, `Cargo.toml`
  version fields, `tree-sitter-al`, or `.agent/`.
- `TREE_SITTER_AL_PATH=U:/Git/al-call-hierarchy/tree-sitter-al` in every command.
- No `#[allow(...)]` and no `#[ignore]`: the flow's `check-diff` rejects both.
