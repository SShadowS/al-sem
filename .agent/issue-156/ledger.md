# Issue #156 ledger

alsem diff: a changed local procedure is a critical ABI procedure-removed + procedure-added; table-field-added has no field name

## Claim

- run: `20261023-074515-40276c`, attempt 1
- body_hash: `23af28df64c060952cb60c234402db976febf5c6aff3ea56fbacacb938a46910`
- base: `7afa91a3`
- worktree: `/work/al-sem-issue-156-a1`, branch `issue/156-alsem-diff-a-changed-local-proc-a1`
- reviewers: gpt-6.1-sol (astra slot) and Opus (flash slot): `--substitute astra=sol --substitute flash=opus`
- operator note: for the CentralGauge demo on 2026-10-27; a 1.7.x release with this fix is needed by 2026-10-24. CentralGauge runs `alsem diff <start> <final> --format json --deterministic` per agent solution and counts `abi` findings as `abi_changes`. Keep the fix tight and correct.

## Timeline

- 2026-10-23 design phase: classified bounded; probes P1-P13 run on base `7afa91a3`; `## Design` written. No code yet.
- 2026-10-23 spec panel round 1 (sol + opus): both REJECT S2 (pairing). Conductor decisions: no finding for local/internal; internal is non-ABI with the internalsVisibleTo limit stated; no schema bump; triggers (I3) IN scope. Findings checked against the code (P14-P18), `## Design` rewritten as design-r1, register R1-01..R1-13 written.
- 2026-10-23 spec panel round 2: opus accepted all 13 and raised N1 and N2 (minor); sol accepted 10 and re-raised R1-02, R1-07 and R1-12. Checked against the code: passes.rs:726-732 (a third raw-old-id site), renames.rs:116-123 with indexes.rs:246-260 (auto endpoints are always in `stableIds`), passes.rs:458-464 (`EventPublisherRemoved` is Medium). Design-r2 fixes them; register updated (R2-01 = N1, kept as its own entry and cross-referenced from R1-12; R2-02 = N2).
- 2026-10-23 step 6 (acceptance tests): 14 `issue156_*` tests and 2 ws fixtures written. On base: 12 fail for the stated reasons, 2 guards pass (a1e, a2e). See `### Acceptance tests`. Not committed.
- 2026-10-23 implementation: commit `c554b98a`. 14/14 pass; 17 proof rows; goldens regenerated (the moved set matches the receipt); task-affected, test and release clippy green; check-diff clean. See `## Implementation`.

## Classification

**bounded.** All three parts live in one module (`src/engine/gate/diff/`: `passes.rs`, `indexes.rs`, `renames.rs`) plus two additive keys in the snapshot composer (`src/engine/l5/snapshot_full.rs` `derive_schema` → field `name`, `derive_contracts` → routine `routineKind` (added at design-r1), and their key lists in `snapshot_full/key_classes.rs`). Every other fact the fix needs is already in the snapshot (P4, P6, P8, P16). No new engine, no id change, no schema-version change. The golden moves are known in advance (see Design).

## Probes

| probe | result | evidence |
|-------|--------|----------|
| P1 Repro on master | Reproduced. Same 5 findings as the issue: `procedure-removed` critical `AddLine` (old `#062bfad3…`, same hash as the issue), `procedure-added` info, `capability-gained-write` on the new id, `capability-lost-under-coverage` on the old id, `table-field-added` with `displayName` = `…:Table:50000#4`. The new hash here is `#9912495e…`, not the issue's `#5a6f67be…`: the issue does not give its new file verbatim, so the signature text differs; the shape is the same. | `/work/bench/156/{old,new}`, output `/work/bench/156/master.json`; debug build of the worktree at base |
| P2 Where the code is | `diff_abi` (`src/engine/gate/diff/passes.rs:102`): removal = old subject missing in new, addition = new subject missing in old, keyed by `stableId`. `diff_capabilities` (`:922`) and permissions key by the same subject id. `table-field-added` at `:410`; display name comes from `DiffIndexes::display_for` (`indexes.rs:296`), which reads only `identities.displayNames` and falls back to the id. | read |
| P3 How routines are identified | A routine's `stableId` is `<object stable id>#<signatureFingerprint>`, and `signatureFingerprint` = sha256 of `canonical_routine_signature(name, params(type, isVar), returnType)` (`snapshot_full.rs:681-701`). Any parameter change gives a new id. So the existing `ProcedureSignatureChanged` branch in `diff_abi` (`passes.rs:186-205`) can never fire for a routine (same id means same hash). Today it fires only for objects via a user rename overlay (`tests/cli-b-goldens/diff/rename.renamed.human.txt`). | `old.fp.json` / `new.fp.json` in `/work/bench/156/` |
| P4 Is `local`/`internal` visible to the diff | Yes. Each routine contract fact has `visibility` ∈ `local`/`internal`/`protected`/`public` (`map_visibility`, `snapshot_full.rs:714`; missing modifier → `public`). Probe: `internal procedure Inner` → `"internal"`, `local procedure AddLine` → `"local"`. | `/work/bench/156/probe.fp.json` |
| P5 Object-level `Access = Internal` | NOT visible. Object contract facts are hard-coded `"visibility": "public"` (`snapshot_full.rs:655-656`, "Objects don't carry accessModifier in the model"). A public procedure in an `Access = Internal` codeunit looks public. | probe codeunit 50001 has `Access = Internal`; its fact says `public` |
| P6 Local event publishers | A `[IntegrationEvent] local procedure OnBeforeX` is contract-fact `kind: "routine"`, `visibility: "local"`, `attributes: [{name: "IntegrationEvent", argsHash}]`. Other apps subscribe to it, so it IS part of the app's contract. A filter on `visibility` alone would wrongly drop it. `[InternalEvent] local` carries `name: "InternalEvent"`. | `/work/bench/156/probe.fp.json` |
| P7 Overloads | Two `procedure Over(...)` get two ids with the same display name `Over` in `identities`. The events pass already matches publishers by object + event name (`publisher_identity_key`, `passes.rs:492`), which is the precedent for name matching. | probe.fp.json; `passes.rs:545-563` |
| P8 Field names in the snapshot | Not present. Field schema facts are `{kind, stableId, shapeFingerprint}` only (`derive_schema`, `snapshot_full.rs:817-821`), and fields are not in `identities`. `tbl.fields[*]` has the name in the model. | new.fp.json `schemaFacts` |
| P9 Which tests/goldens cover diff | `tests/cli/cli_b_diff_differential.rs` (`--test cli`) over `tests/cli-b-goldens/diff/`: 6 vendored snapshot pairs (fixed INPUTS, per `PROVENANCE.md`), one ws-mode rename pair (`tests/fixtures/ws-diff-rename` + `rename-overlay.json`), one ws-mode removed-field pair (`tests/fixtures/ws-diff-removed-field`), and synthetic oracles (`snapshot`/`obj`/`engine`/`find_kind` helpers, ~line 470+). No existing oracle builds a `kind: "routine"` contract fact. The vendored `abi-signature-changed` pair is exactly this bug's shape: public `CreateInvoice` `#3a947e8a…` → `#941a097a…`, today 7 findings (removed, added, field-added, permission-target added/removed, capability gained/lost-under-coverage). | `tests/cli-b-goldens/diff/abi-signature-changed.human.txt`; `jq` over its snapshots |
| P10 Capability/permission matching after pairing | Capability facts key on `op|resourceKind|resourceId` per subject (`cap_fact_key`, `passes.rs:903`); permission facts key on `subject`. If old and new ids map to one subject, unchanged facts produce no finding. In both P1 and P9, the old and new facts are identical, so the 2 (P1) and 4 (P9) effect findings disappear and a real change still shows, on the new id. | `passes.rs:922-999`, P9 jq output |
| P11 "renamed from" note | The human renderer prints ` (renamed from X)` whenever `subject.oldOriginalStableId` differs from the normalized id (`format.rs:100-109`). `oldOriginalStableId` is set only when the rename table changed the id (`indexes.rs:96-108`). | read |
| P12 Snapshot schema version / preflight | `SNAPSHOT_SCHEMA_VERSION = 4` (`snapshot_full.rs:56`); preflight treats a version mismatch as fatal. The diff reads fields by key and falls back to the id for display, so an older v4 snapshot without a field `name` still diffs. `NON_REFERENCE_FIELDS` lists `schemaFacts` data keys (`key_classes.rs:232`); the closure test requires every emitted key to be classified. | read |
| P13 Other vendored diff pairs | Every routine fact in the 12 vendored snapshots is `public`; in `capability-gained-http`, `coverage-strict`, `event-added`, `permission-rights-expanded` every old routine id is still in new (no old-only routine, so nothing to pair). Only `abi-signature-changed` changes. | `jq` over `tests/cli-b-goldens/diff/snapshots/*.snap.json` |
| P14 Member-trigger ids (R1-01) | A member trigger's id is `object#sha256(sigHash, member)` (`ids.rs:324-336`), so its id suffix is NOT its `signatureFingerprint`. Two `OnAction` triggers on one page share name and `signatureFingerprint` but have different ids (`tests/cli-b-goldens/snapshot/ws-sibling-member-triggers.raw.json`). The member string is hashed and cannot be recovered from the id, so it cannot go into a pairing key. Confirmed. | `src/engine/ids.rs:300-337` |
| P15 Raw old ids in the events pass (R1-02) | `writes_of` (`passes.rs:791-818`) looks up a subscriber decl's raw `routine` in `old_capability_facts_by_subject`, which is keyed by the NORMALIZED id (`indexes.rs:213-222`). Event maps normalize only the key (`indexes.rs:195-204`). `EventPublisherRemoved` also uses the raw old `routine` as subject (`passes.rs:570-582`). Confirmed. | read |
| P16 Routine kind in the model (R1-04) | `ModelRoutine.kind` is one of `procedure`/`trigger`/`event-publisher`/`event-subscriber` (`src/program/model/workspace.rs:445-447`). It is set by `classify_kind_ir` (`src/engine/snapshot.rs:91-110`) from the lowercased attributes: `integrationevent`/`businessevent` give a publisher; `InternalEvent` and `ExternalBusinessEvent` leave it `procedure`. Triggers have no access modifier, so their contract fact says `public` (`snapshot_full.rs:714-723`). | read |
| P17 Attribute name case (R1-05) | Snapshot `attributes[].name` is written as it appears in source (`snapshot_full.rs:728-731`, `program/attributes.rs:47`), so `[integrationevent]` stays lowercase. Confirmed. | read |
| P18 Fingerprint root goldens (R1-09) | `cli_b_fingerprint_differential.rs:165-200` serializes the FULL snapshot to `tests/cli-b-goldens/fingerprint/<fixture>.{cbor,cbor.gz}`. All 21 `.cbor.gz` there contain `contractFacts`, and 11 also contain field facts. `tests/r0-goldens` also contain `signatureFingerprint`, but they come from the separate R0 struct (`src/engine/snapshot.rs:33-45`), not `derive_contracts`, so they do not move. | `zcat … \| grep -a` |

## Design

Revision **design-r2** (after spec panel rounds 1 and 2; register ids in brackets). Three changes in `alsem diff`, plus two additive snapshot keys (field `name`, routine `routineKind`).

**Part 2 first (it shapes Part 1): pair a changed routine across old and new.** Before the passes run, add an automatic pairing step next to the user rename overlay (`renames.rs`, a new `pair_signature_changes(old_snap, new_snap, &user_table)`).

- **Candidates.** `kind: "routine"` contract facts that exist only in old (after user-overlay normalization) or only in new, and that meet both of these:
  - **Member-less**: `stableId == <object id> + "#" + signatureFingerprint`. Member triggers (`OnAction`, field `OnValidate`) have an id that hashes the member in (P14). The member cannot be recovered, so it cannot join the key. These routines are never paired [R1-01].
  - **A non-empty display name** in that side's `identities` [R1-08].
- **Exclusions.** An old id that is a key of the user table, and a new id that is a target of the user table, are never candidates.
- **Key.** (object stable id = the id before `#`, display name lowercased, because AL names are case-insensitive).
- **Pair rule.** A group with **exactly one** old-only and **exactly one** new-only routine is a pair `old → new`, and only when old `signatureFingerprint` ≠ new `signatureFingerprint` [R1-01]. The guard is redundant with the member-less filter for well-formed snapshots (distinct member-less ids imply distinct hashes). It is kept as a cheap second wall; its discrimination limit is stated below.
- **Where pairs go.** Into the rename table, with a new flag `auto: true` on `RenameEntry` (user entries are `auto: false`).
- **Effect.** The old side is normalized onto the new id, so every pass sees ONE subject:
  - `diff_abi` reaches its existing signature branch. It emits one `procedure-signature-changed` (critical), with details `oldSignatureHash`/`newSignatureHash`.
  - The capability, permission, coverage (both policies) and event passes compare the routine with itself. They report only real changes, on the new id.
- **Details.**
  - (a) `build_diff_indexes` does not set `oldOriginalStableId` for an `auto` entry, so the human output never says "renamed from" for a signature change. **A paired routine's old id therefore disappears from the JSON** [R1-11]. For an `abi` finding it can be rebuilt exactly, because pairs are member-less: old id = `<object id>#<oldSignatureHash>`. For a capability or event finding on a paired non-ABI routine, the old id is not in the output. This is accepted: the subject is the same routine.
  - (b) `summary.renamesApplied` counts user entries only. The count is kept in its own variable, taken from the user table before the auto pairs are merged, and passed to `compute_summary` (today `mod.rs:179` reads `rename_table.len()` at the end) [R1-10].
  - (c) (design-r2, R1-07) The stale-overlay check (`validate_overlay_against_snapshots`) stays as it is and runs over the merged table. No filtering code is added. An auto pair can never be stale: both ends are picked from `identities.stableIds` (a candidate needs its display name from there), and that is the same id set the check tests (`renames.rs:116-123`, `indexes.rs:246-260`). Filtering out auto entries would therefore change no output, and no test could tell the difference. Design-r1 claimed a discrimination proof for this; that claim is retracted.
  - (d) Every read of a raw OLD id from a declaration body in the events pass goes through a new `DiffIndexes::normalize_old(id)`, which uses the merged table kept on `DiffIndexes` [R1-02]. Three sites (design-r2 adds the third):
    1. `writes_of` for the old side, whose capability map is keyed by normalized ids (P15). Without this, a paired `local [EventSubscriber]` with an unchanged table write produces a false `event-subscriber-induced-capability-gained`.
    2. The subject of `EventPublisherRemoved` (`passes.rs:570-582`).
    3. The subject of the subscriber-induced findings when the publisher is gone from new and the subject falls back to the old publisher's `routine` (`passes.rs:726-732`).

**Overloads.** An overload set is matched by the 1:1 rule above. Unchanged overloads keep their id, so they are never in the old-only/new-only lists; changing ONE overload of a set is therefore a clean 1:1 pair. Adding a new overload is a lone new-only routine, so it stays `procedure-added`. **Pre-stated fallback for ambiguity:** when a group has more than one old-only or more than one new-only routine (two overloads changed at once, or one overload changed and one added), nothing in that group is paired and the diff reports removed + added exactly as today. That is honest (we cannot know which became which) and never worse than master. No parameter-similarity guessing.

**Part 1: `local`/`internal` routines and triggers are not ABI.** (Conductor decisions: no finding, not a new low-severity kind; `internal` is non-ABI; triggers are in scope.)

- `derive_contracts` adds `"routineKind": r.kind` to every routine contract fact. The value is `procedure`/`trigger`/`event-publisher`/`event-subscriber` (P16). `NON_REFERENCE_FIELDS` gets `routineKind` for `contractFacts`. Same no-bump rule as the field `name` [R1-04].
- `is_abi_surface(fact)`, checked in this order:
  1. Not `kind: "routine"` (an object) → ABI.
  2. `routineKind == "trigger"` → NOT ABI. Object, field and action triggers cannot be called from another app.
  3. Any attribute whose name equals `IntegrationEvent`, `BusinessEvent` or `ExternalBusinessEvent`, compared with `eq_ignore_ascii_case` → ABI. Other apps (or Power Automate/Dataverse, for `ExternalBusinessEvent`) subscribe to these even when they are `local` [R1-05, R1-06].
     - The check reads attributes, not `routineKind == "event-publisher"`, for two reasons: the model's classifier does not mark `ExternalBusinessEvent` (P16), and older snapshots lack `routineKind`.
     - `InternalEvent` gets no exception: it is in-app only. On code, these three are the only external publisher attributes. `EventSubscriber` is a subscriber, and `InternalEvent` is in-app.
  4. `visibility` `local` or `internal` → NOT ABI.
  5. Otherwise (`public`, `protected`, or a missing `visibility`) → ABI.
  - A fact without `routineKind` (older v4 snapshots, the vendored inputs) is treated as not a trigger. That is today's behaviour.
- Rules per subject:
  - Only in old → `procedure-removed` only if the old fact is ABI.
  - Only in new → `procedure-added` only if the new fact is ABI.
  - In both, old fact is ABI → the existing narrowing, obsoletion and signature checks run unchanged. So `public` → `local` is still `object-accessibility-narrowed` (critical).
  - In both, old not ABI and new ABI (e.g. `local` → `public`) → `procedure-added` (info). Today this emits nothing.
  - In both, neither ABI → nothing.
- **Stated rule [R1-12]:** when an old fact is ABI only because of its event attribute and the new fact drops the attribute (same id, still `local`), the "old ABI" branch runs. Its narrowing, obsoletion and signature checks see no change, so there is **no `abi` finding**. This is intended: the events pass reports the loss as `event-publisher-removed` (**medium**: `event_severity`, `passes.rs:458-464`; design-r2 corrects the earlier "critical", R1-12/R2-01), because the publisher declaration is gone from `eventDeclarations`. Pinned by an oracle (A1e).
- **Result:** with Part 2, the repro's `AddLine` change produces no `abi` and no `capabilities` finding at all. A real behaviour change of the helper still shows in `capabilities`.

**Part 3: field name in `table-field-added`.** `derive_schema` adds `"name": fld.name` to each `kind: "field"` schema fact; `NON_REFERENCE_FIELDS` gets `"name"` for `schemaFacts`. `build_diff_indexes` copies a schema fact's `name` into that side's display map with `entry(id).or_insert(name)` (identities still win). On the old side the key is the NORMALIZED id (`normalize(sid, table)`), because `display_for` looks up normalized ids [R1-13]. So `table-field-added` (and `table-field-removed`, type-narrowed, data-classification) show `Branch`. No schema-version bump (conductor decision; both reviewers confirm that deserialization keeps extra keys and preflight checks only version, universe and roles). Both new keys are additive, and a snapshot without them (older v4, the vendored inputs) behaves exactly as today.

**Non-goals and stated limits.**
- Object-level `Access = Internal` (P5): not in the model, so a public procedure in an internal codeunit stays ABI. Candidate discovery.
- `internalsVisibleTo` friend apps: an `internal` procedure is callable from them. We follow the issue and treat `internal` as in-app, so an `internal` change gets no `abi` finding even when a friend app calls it.
- Pairing across a user-renamed object whose routines the overlay does not map.
- Renamed routines (name changed) are not paired, by design.
- Member triggers are never paired. Their signatures are fixed by the platform, so nothing is lost, and with Part 1 they are non-ABI anyway.
- **Mixed-version limit [R2-02].** Diffing an old snapshot without `routineKind` (made by a pre-fix v4 build) against a new one: a trigger that exists only in the OLD snapshot still gives `procedure-removed` (critical), because the old fact counts as ABI there. A trigger on both sides, or only in new, behaves correctly. Workspace-mode diffs (CentralGauge's use) rebuild both sides and are not affected. Fix: re-snapshot the old side with the fixed build.

### Acceptance

| # | Issue item | Proof |
|---|-----------|-------|
| A1 | `local` routine changed/removed/added → no `abi` finding | synthetic oracle in `cli_b_diff_differential.rs`: hand-stated `kind: "routine"` facts with `visibility: "local"`, one pair `X#h1`→`X#h2`, one removed, one added → zero `abi` findings |
| A1b | same for `internal` | same oracle, `internal` |
| A1c | an event publisher stays ABI even when `local`: a `local` publisher removed stays `procedure-removed` critical, with four cases: `IntegrationEvent`, `integrationevent` (lowercase, R1-05), `BusinessEvent` (R1-03), `ExternalBusinessEvent` (R1-06); a `local` `InternalEvent` removed → no `abi` finding | oracle |
| A1d | `public` → `local` on the same id still `object-accessibility-narrowed` critical; `local` → `public` is `procedure-added` | oracle |
| A1e | a same-id `local` routine drops `IntegrationEvent`, in two cases: attribute removed, and attribute replaced by `InternalEvent` → no `abi` finding and one `event-publisher-removed` with severity **Medium** in each case (R1-12, R2-01) | oracle (contract facts + `eventDeclarations`) |
| A1f | triggers are not ABI: a `routineKind: "trigger"` fact added, removed, or on both sides → no `abi` finding (R1-04) | oracle; plus E2E-2 below for the production `routineKind` emission |
| A2 | public routine signature change = ONE `procedure-signature-changed` (critical), subject = new id, details carry both hashes, no "renamed from" note in human output, `oldOriginalStableId` absent | oracle + the vendored `abi-signature-changed` golden (moves, see below) |
| A2b | capability findings follow the routine: identical facts → none; a gained op → one `capability-gained-*` on the new id, no lost-on-old. Under `CoveragePolicy::Strict` with old coverage `complete` keyed by the OLD id: the gained finding carries `coverage_state` (`complete`, `complete`) and no coverage diagnostic fires. This would fail if coverage were not normalized (R1-03) | oracle, both policies |
| A2c | overloads: one of two changed → one `procedure-signature-changed`; two changed → removed×2 + added×2 (fallback pinned); new overload → `procedure-added` | oracle |
| A2d | user overlay wins: a STALE user entry `old#X → <id missing from new>` plus a same-name new-only routine `#Y` → no auto pair for `#X` (the result keeps `#X`'s user mapping and reports `#Y` as added, not paired). `renamesApplied` counts only user entries (R1-07, R1-10) | oracle + unchanged `rename.renamed.*` goldens + `abi-signature-changed.json`'s `renamesApplied: 0` |
| A2e | same-name sibling member triggers are never paired: two `OnAction` facts with EQUAL `signatureFingerprint` and member-discriminated ids (id suffix ≠ hash), one old-only and one new-only, give the same findings as master (both kept as separate subjects; their capability facts are not compared with each other) (R1-01) | oracle |
| A2f | paired `local [EventSubscriber]` whose signature changes, with the same table write on both sides → zero `events` findings, zero `abi` findings (R1-02) | oracle (contract + event decl + capability facts) |
| A2g | (design-r2, R1-02 third site) a paired routine whose old fact is a publisher with a subscriber that writes a table; in new, the publisher declaration is gone (same name, signature changed, no event attribute) and so is the subscriber. The resulting `event-subscriber-induced-capability-lost` has `subject.normalizedStableId` = the NEW (paired) id, not the raw old id | oracle |
| A3 | `table-field-added` carries the field name in `subject.displayName` | E2E-1, plus `ws-mode-schema-field-removed.stdout.txt` moving to `Address` |
| E2E-1 | the issue's repro, through the real pipeline (pins the USE: `derive_schema` → snapshot → indexes → passes) | new fixture `tests/fixtures/ws-diff-local-sig/{a,b}` (the issue's two workspaces), diffed in ws-mode via the existing `compose_ws` helper; asserts exactly one finding: `schema/table-field-added`, `displayName` `Branch` |
| E2E-2 | production emission of `routineKind` and attributes (R1-03): a second fixture `tests/fixtures/ws-diff-abi-surface/{a,b}`. Changes from a to b: a page action trigger is added, a table `OnInsert` trigger is added, a `local [IntegrationEvent]` publisher gains a parameter, a `local [EventSubscriber]` gains `var IsHandled` (writes the same table) | asserts exactly: one `abi/procedure-signature-changed` for the publisher, plus the events-pass finding(s) for that publisher's signature change; no `abi` finding for any trigger or the subscriber; no subscriber-induced capability finding |

Discrimination proofs (Edit, run, revert, run, recording both outcomes):
- Drop the local/internal rule → A1, E2E-1 fail.
- Make the attribute check case-sensitive → A1c's lowercase case fails.
- Drop `ExternalBusinessEvent` → A1c fails.
- Drop the trigger rule → A1f, E2E-2 fail.
- Drop the `routineKind` insert in `derive_contracts` → E2E-2 fails.
- Drop the old-non-ABI/new-ABI branch → A1d fails (R1-13).
- Drop the pairing call → A2, A2b, E2E-1, and the `abi-signature-changed` golden fail.
- Let auto entries set `oldOriginalStableId` → A2 fails.
- Drop the 1:1 guard (pair first-of-group) → A2c's two-changed case fails.
- Drop the user-table exclusion → A2d fails.
- Count `renamesApplied` from the merged table → A2d and the `abi-signature-changed.json` golden fail.
- Drop `normalize_old` in `writes_of` → A2f, E2E-2 fail.
- Drop `normalize_old` at the subscriber-induced subject fallback → A2g fails.
- Drop the `name` insert in `derive_schema` → E2E-1 fails.
- Member-trigger guard (A2e): drop BOTH the member-less filter and the hash-differs guard → A2e fails. **Stated limit:** dropping either one alone stays green, because each guard alone excludes the A2e shape. That is a real property of the code (two redundant walls), not a test defect. I will record both single-drop outcomes.

### Expected golden moves

1. `tests/cli-b-goldens/diff/abi-signature-changed.{json,human.txt,sarif}`: 7 findings → 2: `procedure-signature-changed` critical `CreateInvoice`, and `table-field-added` info (still shown by id: the vendored input has no field names). The permission-target and capability pairs disappear (P9/P10: identical facts). This is the bug being fixed.
2. `tests/cli-b-goldens/diff/ws-mode-schema-field-removed.stdout.txt`: `table-field-removed: …:Table:50100#3` → `table-field-removed: Address`.
3. Every full-snapshot golden. Each routine contract fact gains `"routineKind"`, and each field schema fact gains `"name"`:
   - `tests/cli-b-goldens/snapshot/*`: all fixtures; json/raw/envelope/cbor/cbor.gz.
   - `tests/cli-b-goldens/snapshot/shards/ws-d8-commit-in-tx/{all,primary}`.
   - `tests/cli-b-goldens/fingerprint/shards/ws-d8-commit-in-tx`.
   - **`tests/cli-b-goldens/fingerprint/<fixture>.{cbor,cbor.gz}`** for all 21 fingerprint fixtures (P18, R1-09).
   - The `fingerprint/*.json` and `*.human.txt` projections do not carry these facts and should not move.

Nothing else should move:
- The other diff goldens (`capability-gained-http`, `coverage-strict`, `event-added`, `permission-rights-expanded`, `schema-field-removed`) have no paired routine, no local routine and no `routineKind` (vendored, P13).
- `rename.renamed` (ws-mode) has one `procedure Post()` per side, mapped by the user overlay.
- `tests/r0-goldens` come from a separate struct (P18).

Any other move is investigated before regen, via `scripts/check-goldens --regen` then `scripts/check-goldens`.

CHANGELOG: one `Fixed` entry (the three parts, including triggers) and one `Changed` line (snapshot field facts carry `name`, routine contract facts carry `routineKind`).

### Acceptance tests

Written at step 6 (design-r2). Not committed: they go in with the code. Files:
- 14 tests appended to `tests/cli/cli_b_diff_differential.rs` (`--test cli`, member `cli_b_diff_differential`), named `issue156_*`. They use hand-stated routine facts (`routine_fact`, `snap156` helpers).
- Fixtures `tests/fixtures/ws-diff-local-sig/{a,b}` (the issue's repro; `b` also assigns `Buf.Branch`) and `tests/fixtures/ws-diff-abi-surface/{a,b}` (a table `OnInsert` trigger added, a second page action added, a `local [IntegrationEvent] OnBeforeCalc` publisher and its `local [EventSubscriber]` each gain `Qty: Decimal`).

Run on base `7afa91a3` (no fix): `cargo test --test cli issue156`. **12 failed, 2 passed**. Log: `/work/bench/156/acc-master.log`.

| test | on master | reason (each the stated one) |
|------|-----------|------------------------------|
| a1 local/internal | FAIL | 4 abi findings (`procedure-added` ×2, `procedure-removed` ×2) for local routines; expected none |
| a1c publishers | FAIL | the 4 publisher cases pass on master (removal is reported for everything); the `InternalEvent` case fails: `procedure-removed` for `#i1`, expected none |
| a1d visibility | FAIL | `local` → `public` gives no finding on master; expected `procedure-added` (the narrowing half passes) |
| a1e attribute dropped | **pass** | master already gives no abi finding plus `event-publisher-removed` (Medium); the test pins the stated rule so the new ABI filter cannot change it |
| a1f triggers | FAIL | `procedure-added #t3` + `procedure-removed #t1`; expected none |
| a2 signature change | FAIL | removed `#c1` + added `#c2`; expected one `procedure-signature-changed #c2` |
| a2b capabilities | FAIL | gained-write ×2 on `#c2` + lost-under-coverage on `#c1`; expected one gained-write (modify) on `#c2` |
| a2c overloads | FAIL | the one-changed case gives removed `#ob` + added `#oc`; expected signature-changed `#oc` |
| a2d overlay wins | FAIL | the user mapping (`#gone`, `#uy`) already behaves; fails only because `V` is removed+added instead of signature-changed. The overlay-wins and `renamesApplied == 1` assertions discriminate only after the fix (proofs listed) |
| a2e member triggers | **pass** | master never pairs; the guard pins that the new pairing keeps it so |
| a2f paired subscriber | FAIL | removed/added + gained/lost-under-coverage for the subscriber; expected no finding |
| a2g induced subject | FAIL | subject `…Codeunit:9#p1` (raw old id); expected `#p2` |
| e2e local-sig repro | FAIL | the issue's 5 findings, with the field shown by its id; expected only `table-field-added` named `Branch` |
| e2e abi-surface | FAIL | abi: `procedure-added` for `HandleOnBeforeCalc`, `OnAction`, `OnBeforeCalc`, `OnInsert` and `procedure-removed` for `HandleOnBeforeCalc`, `OnBeforeCalc`; expected one `procedure-signature-changed OnBeforeCalc` (the events assertion is not reached on master) |

The two that pass on master (a1e, a2e) are guards on behaviour master already has. They can fail only once the fix exists; their discrimination proofs are in the list above.

## Implementation

Commit `c554b98a` on base `7afa91a3` (main checkout `master` has since moved to `70fc2e9b`; rebase before merge). All 14 `issue156_*` tests pass.

- E2E-2 is now an exact assertion over every finding: one `abi/procedure-signature-changed OnBeforeCalc` and one `events/event-contract-changed-with-affected-subscribers OnBeforeCalc`, and nothing in capabilities or permissions. Reason: the paired publisher and subscriber keep their capability and permission facts, so the passes compare like with like and report nothing. The added `OnInsert` and `OnAction` triggers have empty bodies, so they add no capability to the subscriber's `Log.Insert()` cone or anywhere else.
- Fixture tripwires: `exec_relation::relation_equals_the_legacy_base_per_field` and `capability_cone::FIXTURE_ROOTS` count the app dirs under `tests/fixtures/` on purpose. Both were bumped 29 → 33 for the two new ws fixtures (4 app dirs). Both tests then pass, including the per-dir parity checks on the new dirs. P9's claim "nobody walks `tests/fixtures`" was wrong; these two tests found it.

### Golden moves (receipt `.agent/golden-triage.md`; regen log `/work/bench/156/regen.log`)

The moved set equals the receipt's list, except the 3 shard `manifest.json` files, which did not move. Nothing moved outside the list.
- `tests/cli-b-goldens/diff/abi-signature-changed.{json,human.txt,sarif}`: 7 → 2 findings. Now `procedure-signature-changed` critical `CreateInvoice` (details `oldSignatureHash 3a947e8a…`, `newSignatureHash 941a097a…`, no `oldOriginalStableId`) plus `table-field-added` (still shown by id: the vendored input has no `name`). The permission-target and capability pairs are gone. `renamesApplied` stays 0.
- `tests/cli-b-goldens/diff/ws-mode-schema-field-removed.stdout.txt`: `…Table:50100#3` → `Address`.
- 133 snapshot/fingerprint goldens: `snapshot/*.{raw,envelope}.json`, `.cbor`, `.cbor.gz` (22 fixtures), `snapshot/shards/ws-d8-commit-in-tx/{all,primary}/primary.json`, `fingerprint/shards/ws-d8-commit-in-tx/primary.json`, `fingerprint/*.{cbor,cbor.gz}` (21 fixtures). Every changed line in the JSON goldens is an ADDED `"routineKind": …` (107 lines: procedure 81, event-subscriber 11, trigger 10, event-publisher 5) or `"name": …` (73 lines, on field facts only). No line was removed or changed. The CBOR files encode the same tree.
- `scripts/check-goldens` after the regen: 965 run, 965 passed.

### Discrimination proofs (Edit break → `cargo test --test cli issue156` → Edit revert; final run 14/14 green)

| # | break | result |
|---|-------|--------|
| 1 | `is_abi_surface` visibility match `Some("local" \| "internal")` → `Some("__proof__")` | FAIL a1, a1c, a1d, a2f, e2e-local-sig, e2e-abi-surface |
| 2 | attribute match `eq_ignore_ascii_case` → `==` | FAIL a1c (`[integrationevent] local publisher removal is an ABI break`) |
| 3 | `ExternalBusinessEvent` removed from `EXTERNAL_EVENT_ATTRIBUTES` | FAIL a1c (`[ExternalBusinessEvent] …`) |
| 4 | trigger rule (`routineKind == "trigger"`) disabled | FAIL a1f, e2e-abi-surface (`procedure-added OnAction`, `OnInsert`) |
| 5 | `derive_contracts` emits `routineKindProof` instead of `routineKind` | FAIL e2e-abi-surface (same two trigger findings) |
| 6 | old-non-ABI/new-ABI branch disabled (`&& false`) | FAIL a1d (`local -> public is new ABI surface`) |
| 7 | auto pairs not inserted into the rename table | FAIL a2, a2b, a2c, a2d, a2f, a2g, both e2e; `diff_pairs_human_json_sarif_byte_match` fails (`abi-signature-changed.human.txt` byte mismatch) |
| 8 | `rename_applied = rename_table.get(id).is_some()` (auto counts as rename) | FAIL a2 (`oldOriginalStableId` `Some(#c1)`, want `None`) |
| 9 | pair rule `([o], [n])` → `([o, ..], [n, ..])` | FAIL a2c (`two changed overloads are ambiguous: no pairing`) |
| 10 | user-key exclusion `!user.contains_key(&r.id)` dropped | FAIL a2d (`#uy` becomes signature-changed) |
| 11 | `renamesApplied` from the merged table | FAIL a2d (`left: 2, right: 1`); `abi-signature-changed.json` byte mismatch |
| 12 | `writes_of` old side without `normalize_old` | FAIL a2f (false `event-subscriber-induced-capability-gained`), e2e-abi-surface (same) |
| 13 | induced-subject fallback without `normalize_old` | FAIL a2g (`#p1`, want `#p2`) |
| 14 | `derive_schema` emits `nameProof` instead of `name` | FAIL e2e-local-sig (field shown by id) |
| 15a | member-less filter alone dropped | green (stated limit: the hash-differs guard alone excludes the A2e shape) |
| 15b | hash-differs guard alone dropped | green (stated limit: the member-less filter alone excludes it) |
| 15c | both member-trigger guards dropped | FAIL a2e (sibling `OnAction`s paired: no abi findings) |

Not proven by a test (stated): the new-side exclusion `!user_targets.contains(..)` is redundant whenever the user target exists in new (it is then not new-only); a stale target is never in new. The `EventPublisherRemoved` subject normalization (site 2) has no assertion of its own.

### Gates (worktree, on `c554b98a`'s tree)
- `scripts/ci-steps task-affected`: exit 0 (fmt, clippy-dev, gen-syntax, whole suite: 3726 passed, 8 skipped). The first run failed only on the two fixture tripwires above.
- `scripts/ci-steps test`: exit 0 (3726 passed, 8 skipped).
- `scripts/ci-steps clippy` (release, `-D warnings`): exit 0.
- `agentflow check-diff --base 7afa91a3 --head HEAD --issue 156 --cwd <worktree>`: `reasons: []`.

## Final panel round 1 fix-forward (commit `dccac289` on `41c191ee`)

Brief: `/work/alsem/.agent/runs/20261023-074515-40276c/final-r1-fix-brief.md`. Register: FQ-01..FQ-08, CR-3/5/6/8 (round 4).
- FQ-01: `routine_rows` collapses rows by stable id (last wins, like `build_diff_indexes`).
- FQ-02: the pairing key carries `is_abi_surface`, so a routine whose ABI status changes with its signature is not paired.
- FQ-03: refuted. AppSourceCop AS0025 says dependent extensions can subscribe to local AND internal events. The code already treats internal publishers as ABI; a1c now pins it.
- FQ-04: A2g asserts the `event-publisher-removed` subject.
- FQ-05: new oracle for the user-target exclusion. Stated limit (final round 2, sol re-raise): rename normalization is one hop. With user `X→T`, an old-only routine whose own id is `T`, and a new-only same-name `U`, the auto pair `T→U` is allowed (`renames.rs:172-179` checks old ids only against user keys). `X`'s facts land on `T` and are not followed to `U`. CHANGELOG states it in commit `05dc86e1`.
- FQ-06: the dead `o.sig != n.sig` guard is removed.
- FQ-07: exec_relation reads `FIXTURE_ROOTS`.
- FQ-08: CHANGELOG reworded.

Design-r2 is superseded on two points: the pairing key now includes ABI status, and the hash-differs guard is gone (proofs 15a/15b no longer apply; P-FQ06 replaces them).

| # | break | result |
|---|-------|--------|
| P-FQ01 | `git stash` of `renames.rs` (back to 41c191ee: Vec rows, no ABI key) | FAIL fq01 (removed #d1 + added #d2) and fq02 (narrowed + signature-changed); stash popped |
| P-FQ02 | `key()` → `true \|\| self.abi` | FAIL fq02 (`object-accessibility-narrowed`, `procedure-signature-changed #f2`; want `procedure-removed #f1`) |
| P-FQ03 | `is_abi_surface` → `visibility != internal && (...)` | FAIL a1c (`[IntegrationEvent] internal publisher removal is an ABI break`) |
| P-FQ04 | passes.rs site 2 (`EventPublisherRemoved`) without `normalize_old` | FAIL a2g (`the removed publisher's subject is the paired new id`, left #p1) |
| P-FQ05 | new-side `!user_targets.contains(..)` dropped | FAIL fq05 (signature-changed #t; want removed #tp + added #t) |
| P-FQ06 | member-less filter `suffix == sig` → `(suffix == sig \|\| true)` (now the only guard) | FAIL a2e (sibling OnActions paired) |

All reverted with Edit (or stash pop). The final run: 17/17 `issue156_*` green.

Gates on `dccac289`'s tree:
- `ci-steps task-affected`: exit 0.
- `check-goldens`: exit 0 (968 passed). No golden moved.
- `ci-steps test`: exit 0 (3792 passed, 8 skipped).
- `ci-steps clippy` (release): exit 0.
- `check-diff --base origin/master --head HEAD --issue 156 --cwd <worktree>`: `reasons: []`.

## Base/Head SHAs

- B (base, `origin/master`): `70fc2e9b` (#221's merge; the branch was rebased from `7afa91a3`).
- H (gated head): `05dc86e1` (dccac289's code + the FQ-05 CHANGELOG wording). The full gate run on `dccac289`
  recorded no head for `ci-steps-all`/`cdo-gate` because `05dc86e1` was committed mid-run; both were re-run on H.
- After H: the evidence commit (ledger + register) only.

## Final panel

Rounds: 3 (final_rounds). Round 1: sol (1 important: duplicate rows), Opus (sound; 4 minors), code-review at high (9
candidates). Consolidated FQ-01..FQ-08 fixed in `dccac289`; FQ-03 (internal event publishers) refuted with Microsoft's
AppSourceCop AS0025; CR-3/5/6 refuted by design, CR-8 deferred (minor). Round 2: Opus accepted all 12, sol re-raised
FQ-05's register wording (one-hop chain) → `05dc86e1`. Round 3: both accepted FQ-05.

## Findings register summary

27 entries: spec panel R1-01..R1-13, R2-01, R2-02 (15, 3 rounds) and final panel FQ-01..FQ-08, CR-3, CR-5, CR-6, CR-8
(12, 3 rounds). 0 open; refuted: FQ-03, CR-3, CR-5, CR-6; deferred (minor): CR-8. All accepted by both reviewers.

## Acceptance matrix

Every acceptance item in `### Acceptance` is delivered by its `issue156_*` test (14 + 3 final-panel tests, all green)
and the two ws fixtures (E2E-1: the issue's repro → one `table-field-added Branch`; E2E-2: triggers, local
publisher, subscriber → exactly one `procedure-signature-changed` + one `event-contract-changed-with-affected-subscribers`).

## Gate results

On H `05dc86e1` (recorded by `agentflow run`, clean before/after): `ci-steps-all` 0, `cdo-gate` 0 (both re-run on H),
`check-goldens-coverage` 0, `check-goldens` 0.

## Discoveries

```json
[
  {"subsystem": "gate/diff (event publisher signature changes)", "locator": "src/engine/gate/diff/passes.rs is_abi_surface / diff_abi; renames.rs pair_signature_changes", "symptom": "Adding a parameter to a local or internal event publisher is reported as a critical procedure-signature-changed, but per Microsoft's AppSourceCop AS0025 it does not break dependent extensions (subscribers need not use every parameter); only removing/retyping parameters or changing the event kind breaks them. Found by #156's final panel (FQ-03 evidence).", "kind": "enhancement", "origin_issue": 156, "pre_existing": true, "reproducer": "tests/fixtures/ws-diff-abi-surface (E2E-2): a local [IntegrationEvent] publisher OnBeforeCalc gains a parameter → critical procedure-signature-changed.", "capability": "classify publisher signature changes: parameter appended → info/low (non-breaking); removed/retyped/reordered → critical", "acceptance": "E2E-2's added-parameter case is not critical; a removed-parameter case stays critical"},
  {"subsystem": "gate/diff (rename overlays)", "locator": "src/engine/gate/diff/indexes.rs normalize (one-hop); renames.rs:172-179", "symptom": "Rename normalization is one hop: a user overlay X→T combined with an auto pair T→U leaves X's facts on T instead of U (stated limit of #156, FQ-05).", "kind": "enhancement", "origin_issue": 156, "pre_existing": false, "reproducer": "user overlay X→T, old-only T, new-only same-name U.", "capability": "transitive normalization through user + auto entries, or refuse auto pairs whose old id is a user target", "acceptance": "X's facts land on U in that shape"}
]
```

## Not delivered

Nothing in the acceptance list. Stated limits (CHANGELOG): `internalsVisibleTo` friend apps, object-level
`Access = Internal`, a pre-fix snapshot without `routineKind`, a local/internal `ExternalBusinessEvent` publisher that
drops its attribute, one-hop rename normalization.
