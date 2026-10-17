# Issue #96: implicit-trigger edges in the typed graph

Status: revision 3b (spec rounds 1-4: sol + opus, plus the T0/T1 corrections marked below). The evidence lives in the issue's committed ledger `.agent/issue-96/ledger.md`: its `## Assumption probes` hold the measurements this spec cites, on master `6b47f58d`, including the argument-less Modify/Insert result. The register ids cited here (S*, N*, R3-*) are entries of the committed register `.agent/issue-96/findings.json`. The investigation's full file:line map was a working note and is not committed; every claim the spec relies on is restated in the ledger.

## Goal

Writes, reads and other capability facts made by implicit table and field triggers reach their callers, wherever callers are judged through the typed edge graph. The places that use it:
- the capability cone (so fingerprint `writes`/permissions, `ConeDerivedStore`, `capability_query`, prove, and the cone-based detectors);
- the snapshot;
- digest effects and `viaPaths`.

They do this with a **temporary receiver staying non-physical**.

Today L4's db-effect store already follows these edges (`alsem query effects` shows `[Insert] on Log Row via implicit-trigger`), but every typed-graph consumer does not. So `prove ValidateIt "writes-table:Log Row"` answers an unsound **no**, and digest reports no effect for `ValidateIt`.

## Non-goals

- Param guards across a trigger edge. `carry_reqs` keeps dropping them: a sound over-approximation, unchanged (OUTSTANDING:123).
- R3a-4 dependency symbol artifacts (`src/engine/deps/dep_artifact_l4.rs:473-477`), which also admit no trigger edge. Filed as a discovery.
- Platform trigger EVENTS (`OnAfterValidateEvent` …). Unchanged (`publisher_routine_id: None`, `src/program/model/events.rs:257-262`).
- Which record operations fire a trigger. The program resolver's `TriggerSiteRule` decides which edges exist; this change adds or removes no PROGRAM-ENGINE edge. One model-layer gap found in T0 IS in scope:
  - **The gap:** the adapter's `can_fire_trigger` (`src/program/model/program_calls.rs:340-342`) omits `modifyall`/`deleteall`, so it drops the program engine's existing `ModifyAll`/`DeleteAll` → `OnModify`/`OnDelete` edges.
  - **The effect:** the model, and so L4 too, misses bulk trigger effects.
  - **The fix:** widen it to the ops the resolver already emits for. This is the same defect class as #96, and the program-engine stats stay byte-identical. (A stale OUTSTANDING note, which says argument-less `Modify()`/`Insert()` fire, is corrected in prose: measured on master, they do not.)
- `xRec`, codeunit `OnRun`, page and report triggers. Only the implicit `Rec` of **table and table-extension triggers** changes, because only those triggers are entered through an implicit-trigger edge whose record operation carries the receiver.

## Acceptance matrix

| Acceptance item (issue #96, plus soundness) | Proof |
|---|---|
| A digest of a routine that calls `Validate` on a field whose `OnValidate` writes table X reports a `DB_*` effect on X, with a `viaPath` through the trigger | **A1** (cli test, inline tempdir workspace, the `temp_state_probe::write_workspace` pattern of `tests/cli/cli_digest_temp_state.rs`). It drives `run_digest_pipeline` on the issue's shape: table "Trig Row", field Amount, `OnValidate` inserts "Log Row", `"Trig Caller".ValidateIt` does `T.Validate(Amount, 1)`. Asserts on `ValidateIt`'s entry:<br>- a `DB_INSERT` on "Log Row";<br>- a `viaPaths` entry whose first hop is `kind: "implicit-trigger"` to `OnValidate`, with `anchor` at the `Validate` call's line and column (M4);<br>- the hop is unconditional (Validate always fires its field's `OnValidate`).<br>The same workspace also checks `fires` (N2):<br>- `T.Insert(true)` (OnInsert writes) gives an unconditional trigger hop;<br>- `T.Insert(B)` (non-literal) and `T.ModifyAll(Amount, 0, true)` (bulk) give a conditional one;<br>- a `T.Validate` inside an `if` gives a conditional one through the op's control context. |
| The capability cone's physical write set for that routine contains X | **A2** (same workspace): `ConeDerivedStore::writes_physical_tables_of(ValidateIt)` contains "Log Row"'s table id, and `prove ValidateIt "writes-table:Log Row"` = yes. |
| A test drives `run_digest_pipeline` on that shape | A1. |
| A temporary receiver does not become a physical write, in L4 AND in the cone (R2) | **A3** (same pattern). `TempT.Validate(Amount, 1)` on a `temporary` "Trig Row" whose `OnValidate` does `Rec.Modify()`, and `TempT.Insert(true)` with `OnInsert` doing `Rec.Modify()`:<br>- no physical `DB_MODIFY` on "Trig Row" in digest or the cone. prove's `writes-table` is temp-blind by design today (`prove.rs:388-393` matches any `DB_*` write by resource name), so for prove the test asserts that the DB_MODIFY effects it reports on "Trig Row" are all `known(true)` (T0 finding 2; prove's temp-blindness is recorded as a discovery, not changed here);<br>- AND L4 `query effects` shows that Modify as `temp known(true)` (I4), so the L4 binding is pinned too.<br>A non-temporary twin still yields the physical write. |
| The bindings and keys hold per operation, not per (caller, trigger) pair | **A4** (same pattern). ONE routine with `T.Validate(Amount, 1)` (real) and `TempT.Validate(Amount, 2)` (temporary), both reaching the same `OnValidate`, plus a second real `T.Validate` on the same field:<br>- the real sites give the physical write, and the temporary site gives none;<br>- the fact has exactly ONE `viaPath`, anchored at a REAL site. That is the op the cone selected: the minimum distance, then the lowest first-hop `TypedOutEdge` sort key, whose op ids compare as strings, so `…/op10` sorts before `…/op2`. For the PD form, `pd_solve` takes its first shortest link in the same order. It is NEVER the temporary op (N1, R3-1); `rep_key` does not distinguish the sites, because they share one callee entry. Digest's witness contract is a representative path per fact, the same as for two calls to one callee; that contract is unchanged here;<br>- the cone's typed graph and the snapshot's PD arena hold a separate edge per op;<br>- no `PdEdgeKey` duplicate (C2) and no incoherence diagnostic. |
| A forwarded receiver is bound through a procedure parameter | **A5**: `procedure V(var R: Record "Trig Row") begin R.Validate(Amount, 1); end`, called once with a temporary and once with a real record. The temp state follows the argument: PD through the parameter, then through the trigger. |
| Dependency trigger bodies reach workspace callers (cross-app) | **A6** (I5): a workspace calling `Validate` on a field of a table from a **source-bearing dependency**, whose `OnValidate` writes a dependency table. Cross-app `run_digest_pipeline` reports the write with the trigger hop. It reuses the existing cross-app fixture mechanism (the `r4_cross_app_program` / `assemble_and_resolve_cross_app_program` test pattern). If no mechanism can build a source-bearing dependency in a test, this row is recorded under `## Not delivered` with the reason, never silently dropped. |
| Each part is independently pinned | **Discrimination**: each break must turn at least one of A1-A6 red:<br>(a) reinstate the `build_typed_edges` skip;<br>(b) remove the L4 op-receiver binding;<br>(c) remove the cone `bound_site` arm for `implicit-trigger`;<br>(d) drop `operation_id` from `TypedOutEdge`'s key;<br>(e) drop `operation_id` from `PdEdgeKey`;<br>(f) make digest Case C ignore `witness_first_hop_operation_id`. |
| No unintended change elsewhere | Golden movement matches the per-fixture prediction in the Measurement plan, triaged line by line. CDO `--program-call-graph-stats` is byte-identical: no program-engine edge changes, though `ir_walk.rs` (the model build) changes (I2). CDO/DO detector deltas are triaged. |

## Design

### D1. A typed `implicit-trigger` edge (`src/engine/l4/combined_graph.rs`)

`build_typed_edges` replaces the skip (`:524-526`) with a branch placed BEFORE the `call_site_by_id` lookup. Trigger edges carry `callsite_id = op id` (`"{routine}/opN"`, `program_calls.rs:1781-1792`), never a `PCallSite` id, so the lookup would drop them anyway. For each `ImplicitTrigger` resolved edge with a `to`, push:

```
TypedEdge { kind: "implicit-trigger", from, to, callsite_id: None,
            operation_id: Some(op id), trigger_kind: Some(<trigger name of `to`>),
            fires: Always | Maybe, .. }
```

**`fires` is derived from the record operation, not from the adapter's `resolution` (I1).**
- `Validate` and `Rename` always fire.
- `Insert`, `Modify` and `Delete` fire always when the op's RunTrigger argument is the literal `true`, and maybe when it is not a literal.
- `ModifyAll` and `DeleteAll` are **always `Maybe`** (N2): they run the trigger once per matching row, and the row set may be empty.
- A literal `false` or no argument gives no edge at all; the resolver already omits it.

"Always" means the trigger fires whenever the op executes. The op's own control context still applies (D5), so a `Validate` inside a branch stays conditional.

This needs the op's RunTrigger argument for `Insert` as well. Today `ir_walk.rs:1010-1016` captures it only for Modify/Delete/the bulk ops, so capture it for `Insert` too. `PRecordOperation.run_trigger` is `#[serde(skip)]`, so this moves no golden (N3). The adapter's existing `resolution` field is unchanged, so L4 is unaffected. Tests: A1 covers `Validate`; the literal-true `Insert(true)` and the non-literal `Insert(B)` shapes in A1 check `fires`; a `ModifyAll(…, true)` site checks bulk `Maybe`.

`callsite_id` stays `None` (snapshot third pass, `snapshot.rs:1905-1930`; M1). Cross-app dependency trigger edges come along through the existing `build_combined_graph(ws, &dep_calls, …)` path (`capability_cone.rs:3415-3423`).

### D2. Operation identity through the cone (C3, C2)

- **`TypedOutEdge` gains `operation_id: Option<String>`** (`capability_cone.rs:1081-1140`). It enters the edge's sort key, so two trigger edges from one caller to one trigger through different ops stay distinct. `build_typed_edge_graph` (`:1170-1193`) copies it. D5's earlier "traversal unchanged" was false: traversal semantics are unchanged, but the edge identity is not.
- **`PdEdgeKey` gains `operation_id`** (`snapshot.rs:~1426`, `:1447-1497`) on both sides: the cone's PD link and the snapshot's edge map. Then two Validate ops reaching the same trigger never collide.

### D3. Trigger `Rec` temp state is bound by the caller (R2)

Today a table or table-extension trigger's implicit `Rec` is `Known(false)` (`ir_walk.rs:2083-2094`). Measured: `TempT.Insert(true)` → `OnInsert` `Rec.Modify()` gives an L4 physical `[Modify] … temp known(false)`. D1 would carry this into the cone, prove and digest.

1. **Model** (`ir_walk.rs`): a table/table-extension TRIGGER's implicit `Rec` becomes `ParameterDependent(RECEIVER_PARAM_INDEX)`, as for table procedures (`features.rs:219-237`). Triggers of other object kinds are unchanged.
2. **L4** (`summary_runner.rs` `substitute_pd_temp_state`, `:836-843`): for an `implicit-trigger` edge, `RECEIVER_PARAM_INDEX` binds to the matched record operation's own effective temp state (`ModelRecordOperation`, `workspace.rs:259-273`), with Unknown as the fallback. Today the lookup goes by `callsite_id` and finds nothing.
3. **Cone** (`pd_solve.rs` `bound_site`/`cross_edge`): for `implicit-trigger`, the bound site is the op found through `TypedOutEdge.operation_id` (D2), and the receiver binds to that op's effective temp state. `carry_reqs` keeps dropping guards, now explicitly.
4. **A trigger with no caller edge** (platform-fired) stays a root whose `Rec` is PD, handled exactly as a table-procedure root is today: PD is not known-true, so its direct facts stay physical (`cone_derived.rs:167-193`). Their serialized temp state changes from `known(false)` to the PD form, so goldens move (see the Measurement plan).

### D4. Snapshot model (`src/engine/l5/snapshot.rs`)

- New `SnapshotGraphEdge::ImplicitTrigger { from, to, operation_id, trigger_kind, fires, anchor (the op's source anchor), edge_id }`.
  - Its `callsite_id()` returns `None` (M1).
  - Every exhaustive match fails to compile until it is handled.
- `derive_typed_edges` (`:1260-1420`) maps the new kind instead of `_ => continue`.
- `compute_edge_id` fills its existing `trigger_kind` field (`:927-950`).
- `derive_operation_evidence` (`:1571-1600`) adds typed-edge `operation_id`s to the referenced set, so the op's anchor exists.
- **A trigger edge whose op anchor is missing is emitted with no anchor, never dropped (M2)**, so the snapshot never lacks an edge the cone follows. The PD arena's `debug_assert` (`:1497-1499`) therefore stays valid, and A6 exercises the cross-app case.
- The stale comments (`:1301-1303`, `:1411-1412`) are rewritten.

### D5. Digest (`src/engine/l5/digest.rs`)

- **First-hop witness (C1).** An inherited fact whose first hop is an `implicit-trigger` edge carries `witness_first_hop_operation_id` (a new field). It is NOT carried in `witness_callsite_id`, which keeps its meaning.
  - The cone sets it in `retag` **explicitly for every edge**: `witness_first_hop_operation_id: edge.operation_id.clone()`. So a non-trigger edge writes `None`, and a stale op id can never leak through `..rep.clone()` (N4).
  - It is serialized with `skip_serializing_if = "Option::is_none"`, so only trigger-first facts gain it in the r3a3 and snapshot output (predicted below). Digest's memo identity (`effect_fact_loop_identity`) includes it, so two facts that differ only by it never share a witness contribution.
  - `reconstruct_witness_paths` Case C (`:1122-1152`) and `enumerate_proofs` (`:~828`, and the PD first-hop check at `:886`) select the first edge by `operation_id` when the fact has the op id, and by callsite otherwise.
  - Neither returns `first-hop-not-found` for a trigger-first fact.
- `WitnessHop::ImplicitTrigger`, with `edge_has_hop`/`edge_to_hop` (`:596-720`). Internally it holds the `operation_id`, which the conditionality arm uses to look up `op_ctx`, even though the JSON hop does not serialize it (N5). The JSON hop shape already exists (`:2278-2286`, `:3503-3508`); its `anchor` is the op's. A `HumanHop` variant (`:4003`) is added.
- **Conditionality.** An explicit `ImplicitTrigger` arm replaces the wildcard (`:2632-2666`). The hop's condition is the triggering operation's own control context (as for a call site), combined with `fires`: `Always` adds nothing, and `Maybe` adds the conditional floor.
- prove and fingerprint read the same snapshot and cone.

### D6. Ordering and policy

- **Ordering:** `ordering_inter` already maps the kind (`:365-377`, `:127`); an edge without a callsite is never a dominating caller, which is conservative.
- **Ordering, call chains (T8, final-panel N-F1):** `reconstruct_call_chains` used to skip every hop with no call site, so a trigger hop made no `CallLink`. A trigger-first effect then looked root-local, and `inter_hb` compared its order id (from the trigger's frame) with an order id from the root's frame: a false happens-before, even `MustAllPaths`. The trigger hop now makes a link keyed by its firing operation (`QueryWitnessHop.operation_id`, internal), with the operation's order and `unordered-broadcast` sequencing. `build_canonical_key` uses the same operation in the hop's site slot.
- **Policy:** the `via` value `"implicit-trigger"` is already legal (`predicate_fields.rs:133-140`).

### D7. Prose and oracles

- **Stale prose rewritten:** `pd_solve.rs:117-125`, `ir_walk.rs:2085-2087` and `summary_runner.rs:784-786` (M3), plus the snapshot comments in D4. The CHANGELOG's #69 non-goal note is superseded by a new entry, not rewritten.
- **Parity oracle:** `tests/r3/r3_temp_class_parity.rs` (the A5 matched graph) twins trigger edges by `operation_id`, and its probe gains the A3/A4 shapes (I4).

## False-positive and failure shapes

- **Double counting:** low. The caller's own record-op fact and the trigger body's facts have different keys, and equal keys collapse in the cone's min-distance dedup (`inherited_fact_key`, `capability_cone.rs:1246`). No other mechanism inlines trigger effects.
- **Temporary receivers:** D3, pinned by A3-A5 in L4 and the cone separately.
- **Per-op collisions:** D2, pinned by A4 and breaks (d)/(e).
- **`Maybe` triggers:** followed, as L4 already does, and marked conditional in digest. `Validate`, `Rename` and a literal-`true` RunTrigger are not marked conditional (I1).
- **Detector movement:** every cone-based detector (d43/d44/d45, d8, d50, d2 witness sets, policy) can gain findings, especially cross-app through Base Application `OnValidate`/`OnInsert` bodies. Each moved finding is triaged as a true trigger path or a defect; no wording presumes either.
- **Cost:** the cone gains trigger fan-out, and after D3 every table-trigger `Rec` op is a PD producer, which grows the PD solve (`pd_solve.rs:246-343`). Digest witness enumeration grows with the new hops. All three are measured.

## Measurement plan

1. **Goldens: a per-fixture prediction (I3).** Every moved line must be one of these shapes; anything else is a defect.
   - **D1 typed edges.** `tests/r3a1-goldens/` gains typed edges: ws-implicit-trigger +1, ws-member-call-codeunit +2, ws-triggers +2, plus `manifest.json`. The hand vector `tests/r3a1-vectors/r3a1-vectors.json` `edge_kind_enumeration` is updated by hand, with a reason.
   - **D1/D3 inherited facts.** `tests/r3a3-goldens/ws-implicit-trigger` (`ValidateQuantity`/`ProcessLines` gain the FindSet read via `implicit-trigger`); any other r3a3 routine whose trigger is reached by an edge.
   - **D3 trigger `Rec`.** Only `Rec` of table/table-extension TRIGGERS changes, from `known(false)` to the PD form, in:
     - `tests/ir-l2-goldens/l2_features.snapshot` (`recordVariables`, `tempState` on `PRecordOperation`);
     - the r1a/r2a differential goldens that serialize them (e.g. `tests/r1a-goldens/ws-triggers.l2.golden.json:420-435`);
     - r3a2/r3a3 direct facts' `tempState` for trigger-`Rec` ops (e.g. `ws-r2a-record-types` `Tables.al:21-26`);
     - possibly `tests/l4-summary-baseline/`.
   - **D1 `fires` capture for Insert.** No SERDE golden moves, because `run_trigger` is `#[serde(skip)]` (N3). BUT `tests/ir-l2-goldens/l2_features.snapshot` prints `PFeatures` with `Debug`, so the capture changes the hash of every routine with an `Insert` op: 31 routines, measured in T1 by reverting only the capture (43 → 12 changed) (T1 correction).
   - **D3.1 call-site bindings.** A trigger that passes its `Rec` on (e.g. ws-d40 `OnAfterInsert` → `TouchHelper(Rec)`) gets `sourceTempState` known(false) → PD(receiver) in its call-site argument binding.
   - **Synthetic L4 fixture.** `tests/l4-summary-baseline/pd_substituted_via_collision{,_reversed}` gives an `implicit-trigger` edge a call site with an argument binding at param 0. Real trigger edges bind only the receiver through their op, so under D3.2 that edge gives Unknown and the intended rank-3 via collision disappears. The fixture is REWRITTEN to the real shape: `b`'s op is PD(receiver); the trigger edge's `callsite_id` is an op of `a` whose temp is PD(0); the direct edge binds the receiver with PD(0). That keeps its stated purpose; it is not just regenerated.
   - **D5 witness field.** `witnessFirstHopOperationId` appears ONLY on trigger-first inherited facts in r3a3 `PCapabilityFact` output and in snapshot capability facts (N4).
   - **r4/r4f:** findings move only where a fixture's detector reads a trigger path. Each moved r4 finding is triaged.
2. **CDO** `aldump --program-call-graph-stats`: byte-identical (no program-engine edge changes).
3. **CDO and DO detectors:** `alsem analyze` before/after, at `--scope primary` and `--scope all`, cross-app (the default). Every new or vanished finding is classified (`triage-findings`); one explained by neither a real trigger path nor D3 is a defect.
4. **Cost, on CDO and DO (I6)**, before and after, in the `release-fast` profile on the same machine:
   - the `ALSEM_CONES_CENSUS` counters: cone size, `PD_PRODUCERS`, `PD_STATES`, `PD_LINKS`, `PD_SOLVE_BYTES`, `PD_SNAPSHOT_BYTES`;
   - `alsem analyze`, digest and fingerprint wall time and peak;
   - the digest witness truncation and unmapped-edge counters.

   Recorded in the ledger. A regression above 10% is reported, not hidden.
