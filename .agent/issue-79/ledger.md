# Issue #79 ledger

## Claim

```json
{"run_id":"20261017-133951-0d1cad","issue":79,"attempt":1,"session":"claude-code-session:cf05ae92-26c8-4a5d-a439-ddec4928220c","branch":"issue/79-agent-discovery-l5-digest-chang-a1","worktree":"U:\\Git\\al-sem-issue-79-a1","body_hash":"efa8c85c020f08113fd19c7b0d72478b29a385d6339835f8c7650c41e3262080"}
```

Issue body hash: `efa8c85c020f08113fd19c7b0d72478b29a385d6339835f8c7650c41e3262080`.

Reviewer roster: on 2026-10-15 the operator replaced `gpt-6-astra` and `gemini-3.8-flash` with `gpt-6.1-sol` and `claude-opus-5.5`. Marks are recorded as `reviews.sol` and `reviews.opus`, and we attest with `--substitute astra=sol --substitute flash=opus`.

## Classification

**architectural.** The fix crosses four layers:
- the al-syntax lowerer (`FieldDecl.origin`);
- the model (member declaration spans, plus an opt-in per-routine reference fact);
- digest changed-root resolution;
- the digest output contract (the additive `rootedBy`, a new diagnostic, human rendering, schema 1.7.0).

The spec is `docs/superpowers/specs/2026-10-17-issue-79-declaration-change-roots-design.md`.

## Assumption probes

All probes ran on master `a744cd61`. Details are in `.agent/issue-79/investigation.md`.

| probe | result | evidence |
|---|---|---|
| the issue reproduces | Confirmed. A field declaration, a codeunit global and a table global each give exit 2 with one `hunk-outside-routines` and zero roots. | investigation §1 |
| the model has declaration spans | **Falsified for fields.** `FieldDecl` has no `origin`. Global `VarDecl.origin` exists, but it is dropped before `ModelVariable`. | investigation §3 |
| existing reference facts can answer "who references G / F" | **Falsified.** `identifier_references` misses quoted names and parenless member receivers. `field_accesses` misses `xRec.F` and bare implicit-Rec fields. Record-op field args miss `ModifyAll`, `FieldNo`, `FieldCaption`. Probed in `ws-shapes`. | investigation §2a/2b |
| widening the existing facts is safe | **Falsified.** They feed the `l2_features.snapshot` golden and d3/d19/d22/d56. | investigation §6 |
| shadowing data exists | Confirmed. In `RoutineVariables` the first entry is the binding, and `scope == "global"` means the global. | investigation §2a |
| a field's table can be established | Confirmed for typed record vars (`record_variables.table_id`, global records promoted), implicit Rec via the object's own table, and tableextension fields merged onto the base. **Not** for codeunit `TableNo` implicit Rec. | investigation §2e |
| a fixture exists | Confirmed. `d8_workspace` in `cli_digest_changed_spans.rs` covers lines 29, 39 (only via `ModifyAll`), 47 (unreferenced) and 48 (cross-file). The remaining shapes use temp workspaces. | investigation §6 |
| goldens move | Only `--test cli`: the `schemaVersion` line in 21 digest JSON goldens, the human goldens with diagnostics (because diagnostics are now rendered), and the rewritten pins A1e/A1f. | investigation §6, spec D5 |

## Design

See the spec file. D1 covers the lowerer field origin, D2 the model member declarations, D3 the opt-in `routine_references` fact, D4 digest resolution, and D5 the output contract.

## Discoveries

(filled at the end)

## Timeline

- Worktree created at master `a744cd61`.
- Investigation (opus, read-only; subagents 1): `.agent/issue-79/investigation.md`.
- Classified as architectural; spec written.
- Spec panel (sol + opus, the operator roster; register hash = sha256(spec)[:16]):
  - r1 (hash efee00724b24c30f): both REJECTED. 1 critical finding (a declaration-only file was dropped as unmatched) and many important ones (binding, A3 discrimination, boundary lines), merged as S1-S19. Revision 2.
  - r2 (hash a548ee16bdebd5db): 17 accepted. S5/S6/S9 re-raised. New: N1-N8 (dataitems, `Record 18`/`"18"`, declared vs implicit Rec, deletion gap, `this.G` chains, rename, attribute fields, `#if` anchor). A study (`.agent/issue-79/resolver-reuse.md`) showed the program resolver already implements every rule. Revision 3 replaced the hand-written binder with an opt-in reference-collection pass that reuses the resolver's scope and receiver typing (N9).
  - r3 (hash 618b1e7257972028): opus accepted 26, sol accepted most. New: N10-N15 (whole-routine `with` gate, `Record{None}` selectors, `#if` `MalformedDuplicate` global, break 18, helper split, app guid source). Revision 4.
  - r4 (hash ec548865411c6ad3): both accepted 32 of 34; S15 (quote normalization) and N13 (break 18) re-raised. Revision 5.
  - r5 (spec_rounds 5, the cap; hash 5587f49e8fc58316): BOTH ACCEPTED ALL 34 ENTRIES. Sol found nothing new. Opus raised one new minor, N16 (strip outer quotes only on raw text), "nothing blocks the spec". It was applied after the cap (spec hash 3cedcade9ad44d26) and is registered as `unreviewed`.
- NOT converged at the cap, only because N16 is unreviewed. The handoff route needs every mark to be accepted or re-raised, and `finish --outcome handoff` cannot read sol/opus marks (the executor gap filed as #141, found on #81). So the issue is parked: `blocked spec-panel-cap`. The design (the spec file), this ledger and the register are committed on the branch for a human to resume. A resumed attempt only needs N16 marked in its final panel.
