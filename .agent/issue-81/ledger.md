# Issue #81 ledger

## Claim

```json
{"run_id":"20261017-130445-9e2bd2","issue":81,"attempt":1,"session":"claude-code-session:cf05ae92-26c8-4a5d-a439-ddec4928220c","branch":"issue/81-agent-discovery-l5-digest-chang-a1","worktree":"U:\\Git\\al-sem-issue-81-a1","body_hash":"b60b8faa6adfe41c9528d9eddbc9037a56f74e31e93450d2035e196cd3d647fa"}
```

Issue body hash: `b60b8faa6adfe41c9528d9eddbc9037a56f74e31e93450d2035e196cd3d647fa`.

Reviewer roster: the operator told us (2026-10-15) to use `gpt-6.1-sol` and `claude-opus-5.5` instead of `gpt-6-astra` and `gemini-3.8-flash`. Marks are recorded as `reviews.sol` and `reviews.opus`. We attest with `--substitute astra=sol --substitute flash=opus`.

## Classification

**bounded.**
- The change lives in digest's changed-root resolution (`resolve_changed_roots`), plus one additive field on the model's declaration-only routine record.
- No resolver, graph or detector change.
- The output gains one additive entry field and one new diagnostic kind, so the digest schema goes 1.6.0 → 1.7.0, by the repo's precedent for additive digest keys (#71: 1.4.0 → 1.5.0).
- The design below is the spec.

## Assumption probes

All probes ran on master `a744cd61`. Details and workspaces are in `.agent/issue-81/investigation.md`.

| probe | result | evidence |
|---|---|---|
| the reproducer (`--file src/IFoo.al` → exit 2, `file-unmatched`) | **falsified, stale.** #71 (merge `049b166c`) made every Interface/ControlAddIn procedure a declaration-only root. `--file`/`--diff` on IFoo.al exit 0 with entries `Bar`/`Baz` (`declarationOnly: true`, no effects, coverage `unknown`/`declaration-only`). | investigation §1 |
| the capability (implementers become roots) is still missing | confirmed. No implementer routine is a root. `--file src/ImplA.al --file src/ImplB.al` shows that the implementers carry the effects the interface digest omits. | investigation §1 |
| an existing helper maps an interface DECLARATION to implementing routines | **falsified.** The resolver's `implementers_of` (`resolve/index.rs:757`) gives implementer OBJECTS on `ProgramGraph` ids, and dispatch works from a call site (name + arity + argument types). `alsem graph` maps a hub to every public routine, which over-approximates. Nothing answers per method on `Model` ids. | investigation §3 |
| the model has what digest needs | confirmed. `ModelObject.implements_interfaces` (`workspace.rs:51`, filled `:803-808` from the IR, `#if` arms unioned). `ModelRoutine` has `name`, `parameters`, `normalized_signature_hash`. The declaration-only record is built at `workspace.rs:899-930` from the same `signature_hash`. | investigation §3 |
| the signature hash picks the right overload | confirmed on the repro. Interface `Baz(X: Integer): Integer` hashes equal to `ImplA.Baz(Integer)` and `ImplB.Baz(X: Integer) R: Integer` (named return included). `ImplA.Baz(Text)` differs. Caveat: the hash is over RAW type text (lowercased and trimmed), so `Record Customer` vs `Record "Customer"` would differ. | investigation §3 |
| enum `implements` adds routines to map | **falsified.** An enum value's implementation names codeunits that must themselves declare `implements IFoo` (compiler rule; the engine relies on it at `index.rs:480-484`). Scanning codeunits covers it. | investigation §3 |
| cross-app needs handling | partly. A dependency interface cannot be named by `--file`/`--diff` (only `ws:` spans). A workspace interface cannot be implemented by a dependency. Digest is cross-app by default, so implementers must be filtered to the primary app. | investigation §3 |
| a golden moves from the mapping | **no.** None of the digest golden workspaces contains an interface. Only the schema-version bump moves the `schemaVersion` line in the 21 `tests/cli-b-goldens/digest/*.json` and the `"1.6.0"` pins in three CLI tests. | investigation §6 |

## Design

**Root cause.** Since #71, a changed interface procedure becomes a declaration-only root. `resolve_changed_roots` (`src/engine/l5/digest_cli.rs`) stops there. Nothing maps the declaration to the routines that run when it is called, so the digest reports no effects for an interface change.

**D1. The model records what a declaration-only routine declares.** `ModelDeclarationOnlyRoutine` (`src/program/model/workspace.rs`) gains:
- the declaring object's kind (Interface | ControlAddIn);
- its logical name, in original case for display and folded for matching;
- the routine's name, in original case and folded;
- the declaration's **interface-match key** (D2.0).

There is no change to the three Interface/ControlAddIn skips (`workspace.rs` routine loop, `engine/snapshot.rs`, `l2_workspace.rs`). There is no change to the stable-id hash.

**D2. Digest expands an interface declaration to its implementers.**

**D2.0, the interface-match key.** The key is the folded routine name, plus a per-parameter list of (`var` flag, normalized type), plus the normalized return type (none if absent). Parameter and return NAMES are excluded.
- **Normalized type (token-aware).** Start with the existing `crate::program::body::scope::canonicalize_type_text` (`scope.rs:54`). It collapses whitespace OUTSIDE quotes only, so a quoted identifier's content is kept byte for byte. Then case-fold the result; AL identifiers, quoted or not, are case-insensitive. Then rewrite each quoted identifier: doubled `""` is unescaped with the existing helper, and the quotes are dropped when the content is a plain identifier (letters, digits and `_`, not starting with a digit).
  - **Equal:** `Record "Customer"`, `Record Customer` and `record  CUSTOMER`.
  - **Not equal:** `Record "A  B"` and `Record "A B"`, because quoted whitespace is identity, as the symbol table treats it (`symbol_table.rs:65-71`). `sig_fp::normalize_type_text` is NOT used, because it collapses whitespace inside quotes (`sig_fp.rs:69-74`).
- **One function.** The key is computed by ONE function, used for both the declaration (D1) and every implementer candidate.
- **Separate from the hash.** It is separate from the stable-id `normalized_signature_hash`, which stays byte-identical, so no id moves.

The expansion goes in `resolve_changed_roots`, after the `--file`/`--routine`/`--diff` hits are collected and before they become roots. For each hit span that is a declaration-only INTERFACE procedure `I.M` (declaration id `d`):
1. **Implementers.** The implementers are the primary-app CODEUNIT `ModelObject`s whose `implements_interfaces` contains `I`, folded, as `index.rs:487`. The `#if` arms of `implements` are unioned by the lowerer (`lower/mod.rs:382-405`). This is deliberately the possible-build over-approximation, the same one the resolver uses, and it is stated as such. Only `ws:` units count. Enums are excluded by design.
2. **Candidates.** Within each implementer, the candidates are the routines with folded name `== M`. They are grouped by STABLE ROUTINE ID, because `#if` duplicate declarations are several `ModelRoutine` rows with one id (`workspace.rs` ~1374). Two rules apply:
   - Triggers are excluded.
   - `local` procedures are excluded, mirroring the resolver's visibility filter (`resolver.rs:437-443`, `:2953-2963`). An interface is dispatched from other objects, and a `local` procedure is invisible there. `internal` and `public` are eligible, because the interface and the implementer are both in the primary app. AL has no `protected` on codeunit procedures.
3. **Pick.** The routine is the unique candidate id whose interface-match key equals the declaration's. This applies to a single candidate too: a lone `Baz(Text)` does NOT implement `Baz(Integer)`. If zero or several ids match, push a new changed-roots diagnostic and make no root for that implementer. A known implementer is never dropped silently (`resolver.rs:2944-2948`). The diagnostic JSON:

   ```json
   {"kind": "interface-implementation-unmatched",
    "interface": "<stable id of the interface declaration d>",
    "interfaceDisplay": "IFoo.Bar",
    "implementer": "<implementer object display, e.g. Codeunit ImplA>",
    "candidates": ["<signature display of each eligible same-name routine id, e.g. ImplA.Baz(Text), sorted>"]}
   ```

   `candidates` is a string array, like `selector-ambiguous`. It holds one item per stable routine id (deduplicated), with a signature display, so overloads stay distinct. It is empty when no same-name eligible routine exists. Diagnostics are deduplicated per (`d`, implementer) and sorted by (`interface`, `implementer`).
4. **Hits.** Every span of the chosen routine id is pushed into `hits` (`spans.of_routine(id)`), following the selector precedent (`digest_cli.rs:426-429`), so its anchor is its own declaration and every `#if` arm is included.
5. **Bookkeeping.** Record `implementer routine id → set of interface declaration ids`. The set is deduplicated, so an interface whose `#if` arms repeat the same procedure counts once. A routine that implements methods of two changed interfaces gets both.

The interface's own declaration-only entry stays. It was changed, which is the #71 contract.

**D3. Explain the root, and show the diagnostic.**
- **The field.** An implementer entry rooted through an interface gets `routine.viaInterface: [{"stableId": "<d>", "display": "IFoo.Bar"}, …]`. It is sorted by `stableId` and omitted when empty. It is kept even when the implementer was also touched directly (mixed input).
- **Display names.** The display uses D1's original-case object and routine names, plumbed to the entry; a declaration-only entry has an empty `objectDisplay`, so it cannot supply them.
- **Human output.** The human format prints `via interface: IFoo.Bar` for each one. `format_digest_human` today takes only the entries (~1438); it gains the changed-roots diagnostics and renders them all, one line each, before the entries. A decision: every kind is rendered, not only the new one, because a human reader today cannot see a `file-unmatched` either.
  - **Line format:** `changed-roots: <kind> <field>=<value> …`. The fields are sorted by key, in the same order the canonical JSON serializer writes them (`format_json.rs:53-71`), with `kind` excluded. An array value is joined with ` | `. The separator is not `,`, because a type such as `Dictionary of [Text, Integer]` contains commas.
  - **Signature display:** each parameter is shown as `[var ]<raw type text>`, and the parameters are separated with `; `, as AL source does, e.g. `ImplC.Baz(Text; var Integer)`. `var` is shown because the match key distinguishes it.
  - **Example:** `changed-roots: interface-implementation-unmatched candidates=ImplC.Baz(Text) implementer=Codeunit ImplC interface=<id> interfaceDisplay=IFoo.Baz`.
  - **No entries.** The lines are printed in the no-entries branch too: before `digest: no changed roots resolved`, which is the exit-2 case (for example, only `file-unmatched`).
- **Schema.** The digest `schemaVersion` goes 1.6.0 → 1.7.0, for the additive `viaInterface` key and the new diagnostic kind. The three test pins move with it.

**Non-goals.**
- **ControlAddIn procedures.** They are implemented in JavaScript, with no AL routine to map to. They stay declaration-only roots, which is honest and unchanged.
- **ControlAddIn `event` declarations.** These are #93.
- **`--routine IFoo.Bar`.** It stays `selector-unmatched`, because the capability snapshot skips declaration-only routines. That is a separate surface, and becomes a discovery.
- **Interface `extends`.** The lowerer never reads `extends_interface`, so an `IFoo extends IBar` implementer is not an `IBar` implementer anywhere, the resolver included. That becomes a discovery.
- **Namespaced same-name interfaces.** They are conflated by the folded bare name, the same imprecision as the resolver. Stated as a limit.
- **Type identity beyond spelling.** The match key normalizes case, whitespace and quotes. It does not resolve a type through namespaces or aliases. A real implementation spelled with a different qualified type name yields the diagnostic, never a wrong root.

**False-positive / failure shapes.**
- (a) **A wrong overload as root.** Prevented: every candidate, even a single one, must equal the key. Several equal-key ids give the diagnostic.
- (b) **A dependency implementer as a root.** Prevented by the primary-app/`ws:` filter.
- (c) **An enum as a root.** Prevented, because only codeunits count.
- (d) **An implementer that does not define M.** The compiler rejects this, but under partial/`#if` code the result is the diagnostic, not a silent drop.
- (e) **Cost.** One pass over primary-app codeunits per changed interface procedure. Negligible.

## Acceptance matrix

All root-set assertions compare `(objectDisplay, display, stableId)` tuples, not display names alone, so ImplA and ImplB are told apart.

| Acceptance item | Proof |
|---|---|
| `--file` on an interface method resolves to every workspace implementation (two implementers) | (A1) CLI test. The workspace has `IFoo { Bar(); Baz(X: Integer): Integer }`, ImplA and ImplB implementing it (ImplB with `implements ifoo` in lower case), and a caller. `digest --file src/IFoo.al` gives EXACTLY the declaration-only `IFoo.Bar`/`IFoo.Baz` plus `ImplA.Bar`, `ImplB.Bar`, `ImplA.Baz(Integer)` and `ImplB.Baz(Integer)`. Each implementer entry has its own effects (ImplB.Bar's `DeleteAll`), its anchor at its own declaration, and `viaInterface == [{IFoo.Bar or IFoo.Baz}]`. |
| `--diff` on an interface method resolves the same way | (A2) a unified diff touching only `procedure Bar();` in IFoo.al gives exactly `IFoo.Bar` plus `ImplA.Bar` and `ImplB.Bar`, not Baz. |
| Overloads | (A3) ImplA declares `Baz(X: Text): Integer` BEFORE `Baz(X: Integer): Integer`, so a first-candidate pick would choose the wrong one. Only the Integer overload is a root. (A3b) Wrong-signature candidates only: ImplC has `Baz(X: Text)` and `Baz(X: Text; Y: Integer)`, and neither matches `Baz(X: Integer)`. The result is no root and `interface-implementation-unmatched` with `candidates: ["ImplC.Baz(Text)", "ImplC.Baz(Text; Integer)"]`. The human line reads `candidates=ImplC.Baz(Text) | ImplC.Baz(Text; Integer)`, and the exact text is asserted. (A3e) Single wrong-signature candidate (unconditional CLI case): ImplD has ONLY `Baz(X: Text)`. The result is no root and the diagnostic with `candidates: ["ImplD.Baz(Text)"]`. (A3c) Quote and whitespace spelling: interface `Qux(C: Record Customer)`; ImplA declares `Qux(C: Record  "Customer")` (quoted, with an extra outer space) AND an overload `Qux(C: Record Vendor)`. ImplA's Customer overload is the root, with no diagnostic. With the normalization dropped (raw lowercased text), there is no key match, so a diagnostic replaces the root. (A3d) Negative: interface `Quux(C: Record "A B")`; ImplA declares only `Quux(C: Record "A  B")`. The result is the diagnostic, not a root. This is a unit test on the key function plus the CLI case, if the table names can be declared. |
| `#if` duplicates | (A7) An implementer with `#if CLEAN … procedure Bar() … #else procedure Bar() … #endif` is ONE root carrying both anchors, not an unmatched diagnostic. An interface whose `#if` arms repeat `Bar` gives one `viaInterface` item per implementer and no duplicate diagnostic. |
| Visibility and triggers | (A8) An implementer whose only `Bar` is `local` gives the diagnostic with `candidates: []`, not a root. A trigger is never a candidate. |
| Enums and dependencies | (A4) An enum `FooKind implements IFoo` adds no root, and the exact `changedRootsDiagnostics == []`. Counting the enum as an implementer would add an unmatched diagnostic, because an enum has no routines. (A9) A source-bearing dependency (`symbol_app::write_source_app`, as #96's A6) declares its own `interface IFoo` and a codeunit `DepImpl implements IFoo` that has NO `Bar`. Digest of the workspace IFoo.al adds no root and gives `changedRootsDiagnostics == []`. Dropping the primary-app filter would make `DepImpl` an implementer with zero candidates, which yields a diagnostic. So the test discriminates, even though the `ws:` span filter already blocks dependency roots. |
| Provenance | (A10) `--file src/IFoo.al --file src/ImplA.al`: `ImplA.Bar` keeps `viaInterface`. A routine implementing methods of two changed interfaces lists both, sorted. |
| Explain, schema, human | (A6) `schemaVersion == "1.7.0"`. `viaInterface` is absent on a non-interface root. The human output prints `via interface: IFoo.Bar` and renders the unmatched diagnostic line, with the exact text asserted (keys sorted). A run with only a `file-unmatched` and no entries prints its `changed-roots:` line before `digest: no changed roots resolved`. The exact `changedRootsDiagnostics` set is asserted in A3b and A8. |
| Discrimination | Each break is made with Edit and must fail its test, then be reverted: (1) remove the expansion: A1/A2 fail. (2) Take the first same-name candidate: A3 fails. (3) Skip the key check for a single candidate: A3e fails. (4) Drop the type normalization: A3c fails. (4b) Use `sig_fp::normalize_type_text`, which collapses whitespace inside quotes: A3d fails. (5) Count rows rather than ids: A7 fails. (6) Allow `local`: A8 fails. (7) Let enums count: A4 fails. (8) Drop the primary-app filter: A9 fails. (9) Drop `viaInterface` or the human diagnostic rendering: A6/A10 fail. |

## Measurement plan

- **Goldens.** The `schemaVersion` line in the 21 `tests/cli-b-goldens/digest/*.json`, and the three CLI pin tests. Also, every `tests/cli-b-goldens/digest/*.human.txt` whose JSON twin has a non-empty `changedRootsDiagnostics` gains one `changed-roots:` line per diagnostic, e.g. `ws-txn-d47-neg-temp` (`file-unmatched`). The implementer enumerates the exact list before the regen. Everything is triaged by `golden-diff-triager`. No other digest content is expected to move, because no digest golden workspace has an interface.
- **CDO.** `cdo-gate` must pass. Spot-run a digest `--file` on one CDO interface file, master vs branch, and report the implementer roots added and the time.

## Discoveries

(filled at the end)

## Timeline

- Worktree created at master `a744cd61`.
- Investigation (opus, read-only; subagents 1): `.agent/issue-81/investigation.md`.
- Classified bounded. The design above is the spec.
- Spec panel (sol + opus, the operator roster). Register hash = sha256(ledger.md)[:16] at the round:
  - r1 (spec_rounds 1; pi_calls 2; hash 9d754d67e3b1322d): sol REJECTED D2, and opus confirmed with gaps. 6 important and 6 minor findings, merged as S1-S10. The important ones are:
    - a lone candidate was taken without a signature check;
    - `#if` duplicate rows counted as several candidates;
    - visibility was not applied;
    - the diagnostic was invisible in human output;
    - A3 could not discriminate, because of declaration order;
    - the matrix was missing cases.

    Revision 2 adds a normalized interface-match key (D2.0) required even for a single candidate, grouping by stable id, exclusion of `local`, a concrete diagnostic shape, original-case names for display, human rendering of diagnostics, and tests A3b/A3c/A7-A10.
  - r2 (spec_rounds 2; pi_calls 4; hash e2867440d91d6770): both accepted S2-S5 and S7-S10. Both re-raised S1 (A3c only varied case, which the old hash already folds) and S6 (A4/A9 could not discriminate their breaks). New findings:
    - sol N1 (important): `sig_fp::normalize_type_text` collapses whitespace inside quotes, which erases identifier identity;
    - opus N1 (important): the human-output goldens with diagnostics move, which was not predicted;
    - opus N2 (minor): candidate displays do not tell overloads apart.

    Revision 3 changes the key to token-aware normalization over `scope::canonicalize_type_text`, makes A3c a quote/whitespace case with an overload, adds the A3d negative, makes A4/A9 assert an empty diagnostic set with a dependency implementer that lacks `Bar`, fixes the human line format, predicts the human goldens, and uses signature displays for candidates.
  - r3 (spec_rounds 3; pi_calls 6; hash 812e8cd675d51ab7): both accepted all 13 entries. New minors: N4 (sol and opus: the human field order was ambiguous, and the example did not match the sorted canonical JSON) and N5 (opus: the no-entries branch). Revision 4 sorts the keys, fixes the example, and renders diagnostics in the no-entries branch, tested in A6.
  - r4 (spec_rounds 4; pi_calls 8; hash ad44143fef2ce317): both accepted all 15 entries. Sol found nothing new. Opus raised N6 (minor): the parameter separator in the signature display was undefined, and the `,` array join was ambiguous. Revision 5: parameters are separated by `; `, arrays are joined with ` | `, and A3b gains a two-parameter overload with the exact text asserted.
  - r5 (spec_rounds 5, the cap; pi_calls 10; hash 630added785fde5b):
    - opus accepted all 16 entries;
    - sol accepted 15 and re-raised S6: A3b now has two candidates, so break (3), "skip the key check for a single candidate", no longer has a test;
    - opus raised two minors: the signature display does not say whether `var` is shown, and N3's evidence plus this timeline's order were stale.

    These were folded in after the cap, as revision 6. They are recorded here, and the pre-ratified attempt's final panel re-marks every entry:
    - a new unconditional CLI case, A3e (ImplD has only `Baz(X: Text)`), with break (3) now pointing at it;
    - the display is `[var ]<raw type text>` per parameter;
    - N3's evidence is updated, and the timeline is reordered.

    NOT converged at the cap. The handoff rule is met: no `open` entry; every mark is `accepted` or `re-raised`; every entry is accepted by at least one reviewer (S6 by opus).
- Handoff (spec-panel cap): the ledger and register are committed on the branch as a pre-ratified design for attempt 2.
