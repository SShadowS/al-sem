# Issue #69 — digest temp-state coherence (design, rev 5b)

Status: rev 5b = rev 5 plus the round-5 finding R5-F01 (the insert branch shares the projected dedup-and-cap). Before that: revised after spec panel round 4. Re-raised in round 4: R1-A09, R2-A05 and R3-F03. New in round 4: R4-A01, R4-A02 and R4-F01..F03. Earlier rounds: R1 had 23 findings, R2 had 16 and R3 had 5. Base: master `049b166c`.

**Rev 5 changes (no new mechanism):**
- The witness keeps today's first-hop filter (`digest.rs:930-956`), and the depth-0 proofs obey it too.
- `MixedCarrier`, `ForwardMix` and `IncRoot` are restructured under one root first hop.
- A path's identity includes its terminal, and an equal projected merge keeps the least restrictive conditionality.
- An empty-path-set diagnostic is decided over the combined set and is not folded into truncation.
- The cost budget has a defined denominator.
- Three mutation lines are corrected.

**Rev 4 in one paragraph.** The producer solve is now TRANSITIVE. A state no longer stops when it resolves to a known temp state. It keeps propagating to every caller through the full guard step, so a guard introduced farther up still applies (R1-F12/R2-A03). As a result every routine holds its OWN states for every producer it reaches, and PD-origin facts never travel through the shared cone. The digest witness for a PD-origin fact is therefore always the root's own proofs (depth 0). Below-root arena carriers no longer exist, so the wrong-frame witnesses are gone (R3-A01/R3-F02). A5 compares two transitive populations (R2-A04/R2-F03/R3-F01). Enumeration incompleteness is folded into the truncation signal that conditionality and ordering already read (R3-A02).
Findings register: `.agent/issue-69/findings.json`. Probe evidence: `.agent/runs/20261014-220818-bd516e/probes.md` (main checkout).

Terms:
- **Temp state**: `known(true)`, `known(false)`, `unknown`, or `parameter-dependent(i)` (PD), which means "temporary if the caller passes a temporary record for parameter i".
- **kt / nt**: the binary temp class (`fact_is_known_temp`, `cone_derived.rs:188`). kt is exactly `known(true)`. nt is everything else. A write is "physical" when it is nt.
- **Frame**: the routine whose parameters a PD index or a guard requirement refers to.
- **Guard requirement (`reqs`)**: "this happens only if Boolean parameter j has value v" (`param_guard`), in some routine's frame.
- **Producer**: one raw DIRECT table fact whose temp state is PD, identified by its owner routine and `operation_id`.
- **Cone**: the capability cone (`l4/capability_cone.rs`). It feeds the snapshot (digest, prove, fingerprint, the ordering facts of d47/d49/d51) and `cone_derived` (the physical sets of d43/d44/d45).
- **L4**: the db-effect store (`SummaryBundle::db_effects`), read by `alsem query effects` and by digest's `detail.tempState`.

## Goal

Inside one digest document, every statement about a DB write's temp state agrees, and each is true in the frame of the routine that reports it:

1. The ordering engine's physical-write filter grades a write the way `detail.tempState` reports it.
2. A write that becomes temporary at a call site keeps its evidence, its conditionality, and every `viaPath` through the fact's selected first hop. That is today's witness contract: one inherited fact = one cone key, and its paths start at its `witness_callsite_id` (`digest.rs:930-956`). This issue does not widen it to other root branches. One routine reaching the same producer with a temporary and a real record gets one effect per class.
3. `Validate`, `Copy` and `TransferFields` stop being reported as database writes, on any record.

The fix is in the cone, which is the source of the values and has every consumer. The digest witness reads what the cone solved; it does not re-derive it.

## Non-goals

- Implicit-trigger edges in the typed graph (discovery 1). `TestField` as a capability read (discovery 2). The Validate relation-check read (discovery 3).
- Guards on ORDINARY (non-PD) facts in recursive SCCs. They are dropped there today (`capability_cone.rs:2193-2207`), which is conservative, and they stay dropped. PD-origin facts get guards on every typed edge, cycles included, because the transitive solve carries them (D1.2). This holds for exactly the guards `param_guard` models (usable Boolean parameters); nothing beyond that is claimed.
- One shared worklist implementation with L4. The per-edge temp transition is shared, and an oracle (A5) pins agreement on a matched graph.
- Digest `--order`. Cross-app snapshot facts (`compose_snapshot` stays source-only).

## What is wrong today (source-verified)

- Ordering reads the cone fact's temp state (`ordering_engine.rs:312-332` ← `digest.rs:3136` ← `digest.rs:2569`). `detail.tempState` reads L4 (`digest_cli.rs:598-640, 820-829`).
- The cone resolves PD only when the caller is a non-recursive singleton (`capability_cone.rs:2402-2412`) AND the callee's entry is frame-bound. Disagreeing raw facts make an entry unbound (`:2083-2091`, `merge_cone :1655-1669`). The rep is rewritten only for `known(true)` (`:1553-1566`).
- Recursive members' own output (`inherited_facts_by_bfs`) emits siblings' direct facts in the sibling's frame (`:1977-1985`) and downstream reps without substitution (`:2012-2026`). Root SCCs build no cone (`:2401`).
- The witness guards need a same-class direct producer (`digest.rs:1111-1113`, `:1208-1211`). After substitution, a kt fact's only producer is PD (nt), so the evidence is lost. The BFS never revisits a routine on one path (`:1248-1276`).
- `dedupe_key` (`digest.rs:2199`) and `build_canonical_key` (`:2300-2337`) carry no class. `for_effect` unions L4 rows without filtering by class.
- `Validate`/`Copy`/`TransferFields` → `modify` in two maps (`capability_cone.rs:166`; L2 `table_commit.rs:21`, reached via `capability/mod.rs:374` → `l2_workspace.rs:337-338`).

## Design

### D1. Producers leave shared propagation; a product-state solve resolves them

**Invariant: every cone entry's rep has its key's class, and no entry is ever re-classed through its rep.** The frame-bound PD machinery is removed: `ConeFactEntry.pd`, the PD half of `substitute_entry`, and the PD fields of `PdAnchor`. The singleton GUARD machinery for ordinary facts stays as it is.

**D1.1 Split.** A routine's raw direct facts split into ordinary facts and producers. Ordinary facts enter the cone exactly as today. Producers never enter a shared cone and never share a key with ordinary facts. They stay in `direct_raw` / `capability_facts_direct` (they ARE the owner's direct facts) and in the owner's SELF fold (`:2509-2513`). There, PD reads as nt, which is correct in the owner's own frame.

**D1.2 Solve.** The solve is a product-state graph over the TYPED edges, built per producer, on the pattern of `db_effect_solver::solve_pd_reachability` (`db_effect_solver.rs:170-230`).
- **State** `(p, r, t, reqs)`. In r's frame, p's write has temp state `t` (PD(i), `known(b)` or `unknown`) and happens only under `reqs` (r's frame).
- **Seed**: `(p, owner(p), PD(i0), site reqs of p)`, where the site reqs come from `site_reqs` through the owner's `FrameGuards`. Frames exist for every routine with a usable guard parameter, recursive ones included (`guard_frames`, `:2234-2256`).
- **Transition** over a typed edge `c → r`: `cross_edge(state, edge, c)` is EXACTLY today's per-edge step of `substitute_entry`, factored out (R2-F08):
  - the temp map `pd_temp_state_at_callsite` on `direct-call`/`variable-typed-call`, `event_param_temp_state` on `event-dispatch`, `unknown` on other kinds. It applies to PD only; `known`/`unknown` transfer unchanged;
  - the guard map: `across_edge` on call edges, the by-name re-anchor on event edges (`:1572-1591`), then `conjoin` with the caller's own site reqs for that call (`edge_reqs` = `FrameGuards.by_site[callsite]`, `:2154-2157`);
  - a contradiction drops the transition.
- **No state stops (rev 4).** Every state propagates across every typed caller edge. `t` is mapped only while it is PD; a known or unknown `t` transfers unchanged. `reqs` goes through the guard step every time. So a free state (non-PD `t`, empty `reqs`) at r can become guarded again at a caller whose call site sits under a guard, and a later contradiction drops it. Counterexample covered: `TempHelper` resolves to kt, a recursive `G1`/`G2` calls it under `G1`'s Boolean, and an outside caller passes `false`, so the state is dropped (R1-F12, R2-A03). The solve is transitive in the same sense as L4's Step B/C union (`db_effect_solver.rs:11-18, 392-408`). Every routine reaching a producer holds its own states for it.
- **Links.** Each state keeps ALL its incoming links `(callee-state, edge)`, not just one parent. Links are deduplicated and kept in edge order. The visited set is the observed state tuples. It is finite: `t` is drawn from the producer's observed indices and three constants, and `reqs` from the observed literals (the argument of `db_effect_solver.rs:206-211`). The graph may have cycles, for example `PermUi`'s `(p,PermUi,PD0) ↔ (p,PermUi,PD1)`.
- Mixed outcomes split per producer. `TwoRec(Temp, Real)` gives a kt state for X's producer and an nt state for Y's. In the cycle where A calls B with a temporary buffer, A only ever gets a kt state. Absence is the bottom.

**D1.3 Placing the solution.** There is a separate per-routine map, `resolved_by_routine: HashMap<RoutineId, Vec<ResolvedEntry>>`. It is NOT `direct`, NOT `direct_raw` and NOT `capability_facts_direct` (R2-F01). A `ResolvedEntry` holds `(state id, rep: Arc<CapabilityFact>, dist, first_hop edge, reqs)`. The rep is p's raw fact with `temp_state = t` and `witness_operation_id` = p's op. `dist` is the shortest link-path length to the seed. `first_hop` is the edge of r's shortest link. One entry per state at r, except the seed. The seed IS the owner's direct fact and stays there; the owner's other states, for example `PermUi`'s PD(1), are entries (R2-F04).

Three uses, each named (rev 4: there is no shared-cone use any more):
- (a) **r's own output.** `inherited_facts_for_singleton` and `inherited_facts_by_bfs` add r's entries as candidates, at their `dist` and `first_hop`, under the usual min-dist / edge-sort rule per base key. Because the solve is transitive, these entries are r's COMPLETE PD-origin output. No producer fact reaches r through a callee's cone.
- (b) **r's derived fold** folds them as inherited reps. A guarded entry is folded like a guarded ordinary fact today: it is present in the physical sets, which is the sound direction for a conditional write.
- (c) Producers, and every entry, never enter `direct`, `direct_raw`, `capability_facts_direct` or any SCC cone. `inherited_facts_by_bfs` no longer emits a sibling's producers in the sibling's frame. A member's PD-origin output is its own entries, and root SCCs need no cone for it.

Value correctness follows. ViaHelperReal: `known(false)`. Forward: PD(0). ViaRecursionTemp: `known(true)`.

**D1.4 Cost.** The 493 figure is L4's, not the cone's. A transitive solve holds one state per (producer, ancestor, `t`, `reqs`), so its population is comparable to L4's PD-derived memberships times the guard variants. It is a GATE, measured on DO and CDO with new `cones_census` counters: producers, states, links, entries, and **solve bytes** = arena bytes + `resolved_by_routine` bytes, including the reps. Reps are shared: one `Arc<CapabilityFact>` per `(producer, t)`, not one per state. The budget has three parts, each checked against B0 on the same workspace, same profile (`release-fast`), 3 runs each (R4-F02):
- solve bytes ≤ 5% of B0's measured peak RSS of the same run (`alsem analyze`, and the digest probe in the Measurement plan);
- peak RSS within B0's noise band;
- wall time ≤ B0's median + 10%. If the budget is exceeded, the merge is blocked and the spec returns to the panel. Cone entries get smaller (`pd` is removed), and producer facts no longer ride in every ancestor cone.

### D2. The snapshot carries the solve as an arena; facts carry nothing new

- `CapabilitySnapshot` gets a serde-skipped `pd_solution: PdArena { states: Vec<StateNode>, links: Vec<(StateIx, EdgeIx)>, by_routine: HashMap<RoutineId, Vec<StateIx>> }`. `EdgeIx` indexes `typed_edges` AFTER the snapshot's sort and filter. It is mapped once at snapshot build by the edge identity `(from, to, kind, callsite, event)`. A cone edge with no snapshot edge is a build error (`debug_assert`) and is counted (R2-A07).
- **No fact field changes.** `retag` is untouched, and there is no `pd_origin` on any fact, so nothing can leak to an ancestor's copy (R2-A01, R2-F02). Whether a routine's OWN states can serve as carriers is a property of that routine (the `by_routine` index), never of a fact.
- The serialized snapshot moves only where fact values move (D1).

### D3. Digest witness: carriers include the solve; paths are enumerated from the arena

**First-hop contract (rev 5, R4-A01).** Every witness path for `f`, whether a proof or a BFS path, starts with an edge whose callsite is `f.witness_callsite_id`. This is exactly today's `first_edges` filter (`digest.rs:930-956`), unchanged. The cone already decided `f`'s existence, its guards (guarded and unguarded copies; contradicted edges never produce `f`) and its first hop. The witness explains that decision and nothing wider. The early returns (`first-hop-not-found`) are evaluated after the proofs, over the combined set (see Incompleteness).

**Two carrier kinds, two places (rev 4).** For an inherited fact `f` at root R:
- **(ii) Root states, depth 0 only.** Every non-seed arena state `s` in `by_routine[R]` with `fact_equivalent(rep(s), f)` and `class(t_s) == class(f)` contributes its **proofs** whose first edge has `f`'s first-hop callsite: all such link-paths from `s` down to the seed, enumerated from the arena. A proof may revisit routines but never a STATE. Each proof is a complete path from R, so it is valid in R's frame by construction, including guards and parameter swaps. Arena states at nodes BELOW R are never carriers. With a transitive solve every PD-origin fact at R has its own states at R, so a below-root arena carrier is never needed. It was also unsound: a bound state's class holds only in its own routine's frame (R3-A01, R3-F02, the `PermMix` one-hop and `ForwardMix` counterexamples).
- **(i) Ordinary carriers, through the BFS.** After the proofs, the BFS runs exactly as today: seeded with the first-hop edges, pruned by `valid_nodes`. It finds ordinary direct facts `d` at nodes n ≠ R with `fact_equivalent(d, f)` and `class(d) == class(f)`. Such a node is terminal, as today. Raw producers are not ordinary facts and are never carriers.

**One carrier predicate.** The BFS terminal test and the `valid_nodes` seed call ONE function, `is_bfs_carrier(node, d, f)`, and fingerprint calls the same one. Any change to it changes both sites at once.

**Path identity and merge (R4-A02).** The final path set is the proofs plus the BFS paths. A raw path's identity is its full edge sequence PLUS its terminal evidence kind and id (operation or callsite). Deduplication uses that identity, before any cap counts paths. The tie-break order is length, then edge sort keys, then terminal id. So two producers reached through the same call edges stay two raw paths, each with its own conditionality from its own terminal (`compute_path_conditionality`, `digest.rs:2427-2464`).

Projection drops the terminal, so such paths can project equal. `merge_normalize_via_paths` (`digest.rs:2640-2703`) today keeps the FIRST conditionality of equal projected paths. It now keeps the LEAST RESTRICTIVE one, using the same rank `effect_conditionality` uses. That means an unconditional terminal is never hidden behind a conditional twin.

The INSERT branch (`digest.rs:2979-3010`, an effect's first fact) today stores the raw projected paths and conds with NO projected dedup, and caps them by `projected_paths.len() > MAX_PATHS`. It now goes through the same helper: `merge_normalize_via_paths(&[], &[], false, &projected_paths, &path_conds, outcome.truncated, MAX_PATHS)`. Both branches then share one dedup-and-cap, so projected twins never appear as duplicate `viaPaths` and never count toward `MAX_PATHS`/`had_truncation` (R5-F01).

All of this applies to ordinary facts too. Projected twins are common for PD-origin facts (any helper with two same-table writes to its var record) and rare for ordinary facts under today's first-match terminal test. Any `viaPaths` count or `viaPathsTruncated` move in r4f/cli-b is triaged as this cause. So is a change in `viaPaths` ORDER, and a `canonicalKey`/`factId` move that follows from it: the shared helper re-sorts the insert branch's paths into projected order, which can differ from the raw order of `reconstruct_witness_paths`, and the first viaPath feeds the canonical key (R6-F01).

The carrier tests read only fields in `effect_fact_loop_identity` plus R's arena, so equal identities have equal witnesses (R2-A06).

**Enumeration (R2-A02).** Proofs are enumerated breadth-first by length, with ties broken by the sequence of edge sort keys. The combined path count per `f` is capped at `HARD_PATH_CAP` in global length order, and hitting the cap sets `truncated`. A proof longer than `MAX_DEPTH`, or an enumeration exceeding `MAX_STATES`, sets `incomplete` with today's diagnostics. Each path gets its own conditionality, folded by `effect_conditionality`.

**Incompleteness reaches the consumers (R3-A02).** Today digest drops `outcome.incomplete` (`digest.rs:2954-3025`). It now ORs `incomplete` into the accumulated `had_truncation` at both the insert and the merge sites. That one signal already feeds:
- `effect_conditionality(…, truncated)`: no unconditional path plus truncated gives `unknown` (`conditionality.rs:89-124`);
- `via_paths_truncated` → ordering's `path_enumeration = "capped"` instead of `"complete"` (`ordering_inter.rs:205-220`);
- the serialized `viaPathsTruncated`.

Its meaning becomes "a non-empty path set is not known to be complete". The CHANGELOG states the widened meaning. Exactly two sources are folded: the state limit (`state-limit-exceeded`) and the depth limit (`depth-exceeded`), from either the BFS or the proof enumeration. Today's state-limit and depth-limit cases on ordinary facts move too, which is the correct direction, measured in the goldens.

**Not folded: empty path sets (R4-F01).** `terminal-not-found` and `first-hop-not-found` are decided over the COMBINED set (proofs plus BFS paths). Today the BFS sets them internally (`digest.rs:1312-1316`, `:932-956`). Now the BFS reports "no ordinary path found", and the caller emits `terminal-not-found` only when the proofs are empty too. So a PD-origin fact whose paths are all proofs gets no false diagnostic and no truncation. These two are NOT folded into `had_truncation`: an empty path set already yields `unknown` conditionality and no ordered op, so folding would change only `viaPathsTruncated`, for no information.

**Seed (`valid_nodes`).** The reverse BFS seeds from `is_bfs_carrier` (ordinary carriers only). Proofs do not use the BFS.

**Consumers.** `reconstruct_witness_paths_pub` and `build_fingerprint_indexes_pub` / `pub_idx_as_private` (`digest.rs:3929-4039`) use the same carrier predicate and arena, so `alsem fingerprint`'s witnesses (`fingerprint_query.rs:577, 598`) match digest's (R2-F06).

The stale guard comments (`digest.rs:~1137-1186`) are rewritten to the new precondition: every inherited fact has a same-class ordinary producer reachable below the root, or a same-class non-seed arena state AT the root. `oracle_r3a3_inherited_keys_trace_to_a_direct_producer` drops its PD→kt allowance and accepts the root's own arena states instead.

**Identity.**
- `dedupe_key` gains the class (`|kt`/`|nt`).
- `build_canonical_key` always gains `|kt` for kt DB effects (nt is unchanged), so opposite classes never share an occurrence id.

### D4. `detail.tempState`: per-operation join

```rust
struct TempJoin { states: BTreeSet<TempStateKind>, problems: Vec<(String /*op*/, JoinProblem)> }
enum JoinProblem { NoL4Row, ClassAbsent }
```

Typed states are rendered with `temp_state_str` at the presentation boundary (R2-A08).

- For each originating op: no L4 row → `NoL4Row`. Rows exist but none in the effect's class → `ClassAbsent`, and that op contributes ALL its rows. Otherwise the op contributes only its rows in the effect's class.
- **Precedence (R2-F05):** if any op has `NoL4Row`, the field is absent (never invent a value). Otherwise the field is the sorted, distinct union of every op's contribution.
- Every problem is an `entryDiagnostics` item `temp-state-incoherent`. `EntryDiagnostic` gains optional `factId`, `operationId` and `reason` (`no-l4-row` | `class-absent`), serialized at `digest_cli.rs:1037-1052`. This is an additive minor `schemaVersion` bump for digest and for prove (shared builder, `prove.rs:959-975`).
- The effect's class is the class of its cone fact, which is frame-correct after D1.

### D5. Validate / Copy / TransferFields are not database writes

They are removed from `modify`. `table_commit::map_op` (L2) is the one table, and `capability_cone::map_table_op` calls it. This is settled from documented semantics and the engine's own classification (`summary_runner.rs:308-330`, `op_classification.rs:79-82`):
- `Copy` copies fields, filters and keys in memory. `Copy(From, true)` on temporary records shares the temp table; on a real record it is a runtime error.
- `TransferFields` assigns field values in memory.
- `Validate` assigns the value and runs `OnValidate`. A trigger's writes are separate effects, and a relation check reads the RELATED table. Neither modifies the record's own table.

Result, on temporary and real records alike: no capability fact, no `DB_MODIFY`, no physical write, no fingerprint `M`, no diff `CapabilityGainedWrite`, no policy `modify` from these ops. The CHANGELOG (`Fixed`) names each op and each moved consumer.

## Acceptance matrix

Tests live in `tests/cli/cli_digest_temp_state.rs` and drive `run_digest_pipeline` in both modes (`check_mode`). The exceptions are A4b (a `--lib` test) and A5 (a `tests/r3` oracle). Ordering is observed as `WRITE_PENDING_AT_UI` on a `UI_MESSAGE` in the SAME terminal routine as the write (probes: it does not fire across routines), so every `*Ui` fixture keeps its `Message` next to its write.

Fixture routines (in `PROBE_AL`):
- `HelperUi(var R)` (Insert), `HelperModUi`, `HelperDelUi`, each writing then calling `Message`.
- Callers: `HelperUiTemp`, `HelperUiReal`. `HelperUiBoth` is `HelperUi(Row); HelperUi(Buffer)`, real first. `HelperUiBothDeep` is root → `A`, where `A` is `HelperUi(Row); HelperUi(Buffer)`.
- `ForwardUi(var X)` → `HelperUi(X)`; callers `ViaForwardUiTemp` and `ViaForwardUiReal`.
- `CondTwice(var X; Flag)`: `if Flag then HelperUi(X); HelperUi(X);`. Root `ViaCondTwiceTemp` (Flag unknown at the call, a local) passes a temporary buffer.
- `RecUi1(var R; N)`: `if N > 0 then RecUi2(R, N - 1); R.Insert(); Message`. The producer is in RecUi1. `RecUi2(var R; N)`: `RecUi1(R, N)`. Callers `RecUiTemp` and `RecUiReal` call `RecUi1`.
- `MixedUi(var R)`: `R.Insert(); Local.Insert(); Message`. `MixedUiTemp`.
- `TwoRecUi(var X; var Y)`; `TwoRecMix` (Temp, Real); `TwoRecTemp` (Temp, Temp2).
- `CycA()` (local temp B, calls `CycB(B, 2)`); `CycB(var R; N)`: `R.Insert(); Message; if N > 0 then CycA()`; `CallsCycA`.
- `CycQ1(var R; N)`: `R.Insert(); CycQ2(R, N)`. `CycQ2(var R; N)`: `Real.Insert(); Message; if N > 0 then CycQ1(R, N-1)`. Root `CycQTemp`.
- `RecH(var R; N)`: `HelperUi(TempLocal); if N > 0 then RecH(R, N-1)`. This is a recursive singleton whose own output needs its own solve entry (use (a)).
- `SibP1(var R; N)`: `R.Insert(); Message; if N > 0 then SibP2(N)`. `SibP2(N)`: local temp `T`; `SibP1(T, N-1)`. A two-member cycle; the producer is in the OTHER member.
- `RootCyc1`/`RootCyc2`: a root cycle passing a local temporary buffer into `HelperUi`.
- `PermUi(var A; var B; N)`: `A.Insert(); Message; if N > 0 then PermUi(B, A, N-1)`. Root `PermMix`: `PermUi(Temp, Real, 2)`.
- `SelfRecUi(var X; N)`: `X.Insert(); Message; if N > 0 then SelfRecUi(TempLocal, N-1)`. Root `SelfRecReal`: `SelfRecUi(Real, 1)`.
- `GuardFwd(var R; DoIt: Boolean)`: `if DoIt then HelperUi(R)`. Roots `GuardFwdFalse` (Real, false) and `GuardFwdTrue` (Real, true).
- `GCyc1(var R; En: Boolean; N: Integer)`: `if En then HelperUi(R); if N > 0 then GCyc2(N - 1, R, En)`. `GCyc2(N: Integer; var R; En: Boolean)`: `GCyc1(R, En, N)`. The Boolean parameter index shifts between members. Root `GCycFalse`: `GCyc2(1, Real, false)`. Root `GCycTrue`: `GCyc2(1, Real, true)`.
- `TempHelperUi()`: local temp `T`; `HelperUi(T)`. `GT1(En: Boolean; N: Integer)`: `if En then TempHelperUi(); if N > 0 then GT2(N - 1, En)`. `GT2(N: Integer; En: Boolean)`: `GT1(En, N)`. Roots `GTFalse` (`GT2(1, false)`) and `GTTrue` (`GT2(1, true)`). Here a guard is introduced ABOVE the point where the state resolves to kt (R1-F12/R2-A03).
- `ForwardMix`: `Both(TempBuf)`. `Both(var X)`: `ForwardUi(X); RealWriter()`. `RealWriter`: `Real.Insert()` (same table, no `Message`). Both branches sit below ONE root first hop (`ForwardMix → Both`), so the nt fact's BFS (first hop = the `Both` call) does reach `Both`. Under the correct predicate, `valid_nodes` prunes `ForwardUi`, because it holds no ordinary nt carrier. This settles the round-4 disagreement: astra was right that the rev-4 shape put `ForwardUi` on a different root branch, which the first-hop filter (`digest.rs:944-948`) never enters; flash was right that `valid_nodes` also prunes it. (R3-F02, R4-A01, R4-F03)
- `MixedCarrier`: `Wrap(TempBuf)`. `Wrap(var X)`: `HelperUi(X); TempLocal.Insert()` (same table, kt). Precondition, asserted: both kt contributors share the root first hop `MixedCarrier → Wrap`, whichever one wins the cone's rep selection (`capability_cone.rs:1822-1850`). (R3-F03, R4-A01)
- `HelperTwoUi(var R; Flag: Boolean)`: `if Flag then R.Insert(); R.Insert(); Message`, so producer p1 is conditional and p2 unconditional. Root `HelperTwoTemp`: local temp `Buf` and local `F`; `HelperTwoUi(Buf, F)`. (R4-A02)
- `CondOnlyTemp`: local temp `Buf` and local `Flag`; `if Flag then HelperUi(Buf)`. (R4-F01)
- `CopyTemp`, `CopyShareTemp` (temp only), `CopyReal`, `TransferTemp`, `TransferReal`, `ValidateTemp`, `ValidateReal`, `ValidateRealUi`, `CopyRealUi`.
- Generated in test code: `Deep0 … Deep66` ending in `DeepEnd` (`TempBuf.Insert(); Row.Insert()`, same table), root `DeepRoot`. Also `DeepF0(var X) … DeepF66(var X)` → `HelperUi(X)`, `IncWrap(var X)`: local `Flag`; `if Flag then HelperUi(X); DeepF0(X)`, and root `IncRoot`: local temp `Buf`; `IncWrap(Buf)`. Under ONE root first hop, that gives one short conditional proof and one proof longer than `MAX_DEPTH` (R3-A02).

**Discrimination rule.** Each mutation is made with Edit. The test asserts the break applied (exactly one match). It runs and must fail ON THE NAMED ASSERTION, not on an unrelated failure elsewhere in the suite. Then revert, run and see green. Both outcomes and the failing assertion's message go into the ledger.

| # | Claim | Assertions | Mutation (USE broken) → named red assertion |
|---|---|---|---|
| A1a | Ordering's filter agrees with `detail.tempState` | For every entry with a UI_MESSAGE: `WRITE_PENDING_AT_UI` ⇔ some DB write in that terminal routine has a `tempState` part ≠ known(true). Named: `RecUiTemp` (known(true), no label), `RecUiReal` (label), `ViaForwardUiTemp` (no label), `ViaForwardUiReal` (label). | `temp_state: None` in `OrderingEffectInput` (`digest.rs:3136`) → "RecUiTemp: WRITE_PENDING_AT_UI on a known(true) write". |
| A1b | No incoherence | No `temp-state-incoherent` diagnostic in the whole document. | Put producers back into `fact_cone_for_scc` for recursive SCCs → "RecUiTemp: temp-state-incoherent class-absent". |
| A1c | Cone values frame-correct. Stated limit: asserted on `compose_snapshot`, the production builder that `run_digest_pipeline` calls, and not claimed as a pipeline pin. | ViaHelperReal and ViaRecursionReal inserts are known/false; ViaRecursionTemp is known/true; Forward is PD(0); PermUi's own output has PD(0) and PD(1). | Give solve entries the producer's raw PD value instead of `t` → "ViaHelperReal: PD(0) != known/false". |
| A2a | Temporary call site keeps its witness (Insert/Modify/Delete) | `HelperUiTemp`, the temp callers of `HelperModUi`/`HelperDelUi`, `ViaForwardUiTemp`: source evidence at the producer's excerpt; the exact hop list (`ViaForwardUiTemp` → `ForwardUi` → `HelperUi`); no `ORDERING_UNKNOWN`. | Skip depth-0 proof enumeration → "HelperUiTemp: evidence unavailable". HelperUiTemp has no ordinary carrier, so the proofs are its only path source. (R1-A09, R2-A05, R4-F03) |
| A2b | One effect per class | `HelperUiBoth`: exactly 2 DB_INSERTs with known(true) and known(false), first hops through the Buffer and Row calls; the label is present. | (i) Drop the class from `dedupe_key` → "HelperUiBoth: expected 2 DB_INSERT, got 1". (ii) Drop D4's class filter → "HelperUiBoth: tempState known(false),known(true)". |
| A2c | Depth-2 proof, real call first | Precondition asserted: the kt effect's path has 2 hops (a depth-0 proof through A). kt path = root→A→A's Buffer call→HelperUi; nt path = root→A→A's Row call→HelperUi. Neither contains the other's callsite. | Make the depth-0 class test class-agnostic → "HelperUiBothDeep: kt path contains Row callsite". |
| A2d | All paths and conditionality kept (R2-A02, R4-A02, R4-F01) | `ViaCondTwiceTemp`: the kt effect has 2 viaPaths (both `CondTwice` callsites) and conditionality `unconditional-on-success`. `HelperTwoTemp` (one cone fact, so its effect is built by the INSERT branch alone): the kt effect has exactly ONE viaPath and conditionality `unconditional-on-success` (p1 and p2 are distinct raw paths; their projections are equal). `CondOnlyTemp`: the kt effect has conditionality `conditional` and `viaPathsTruncated == false`. | Keep one link per state (first parent only) → "ViaCondTwiceTemp: expected 2 viaPaths". Deduplicate raw paths by edge sequence only (first in tie order = p1) → "HelperTwoTemp: conditionality conditional". Keep the first conditionality on an equal projected merge → "HelperTwoTemp: conditionality conditional" (red through the insert branch's normalization, which stores the merged conds). Skip the insert-branch normalization → "HelperTwoTemp: expected 1 viaPath, got 2". Fold the BFS's own `terminal-not-found` into `had_truncation` → "CondOnlyTemp: conditionality unknown". |
| A2e | Recursion, cycles, mixes, guards | `MixedUiTemp`: kt and nt, with the label. `TwoRecMix`: kt and nt, with the label. `TwoRecTemp`: known(true) only, no label. `CycA` and `CallsCycA`: known(true) only, no label. `CycQTemp`: kt and nt, with the label. `RecH`: Helper's insert is known(true) with source evidence. `SibP2`: known(true) only, no label. `RootCyc1`: known(true) with source evidence. `PermMix`: kt and nt; EVERY nt path enters PermUi twice, and no nt path has a single PermUi hop (R3-A01). `SelfRecReal`: the kt path is exactly SelfRecReal→SelfRecUi→SelfRecUi, plus an nt effect. `GuardFwdFalse`, `GCycFalse`, `GTFalse`: no DB_INSERT and no label. `GuardFwdTrue`, `GCycTrue`: DB_INSERT with the label. `GTTrue`: a known(true) DB_INSERT. `ForwardMix`: the nt effect has exactly one viaPath (ForwardMix→Both→RealWriter), and the entry has no `WRITE_PENDING_AT_UI`. `MixedCarrier`: precondition (both contributors' first hop is the `Wrap` call); one kt effect with exactly 2 viaPaths, MixedCarrier→Wrap→HelperUi (proof) and MixedCarrier→Wrap (the BFS terminal at `TempLocal.Insert`). | One per claim: re-merge producers into ordinary keys → "MixedUiTemp: label missing". Collapse a producer's states to "any nt ⇒ all nt" → "TwoRecMix: no known(true)". In `cross_edge`, map known(true) to unknown on intra-SCC edges → "CycA: label present". Skip ordinary facts of recursive members → "CycQTemp: label missing". Drop use (a) for the routine's own entries → "RecH: no DB_INSERT". Restore sibling producers in `inherited_facts_by_bfs` → "SibP2: temp-state-incoherent class-absent" (after D3 a restored sibling producer has no witness, so it never gets a label; the shared no-incoherence check is the guard). Skip use (a) for root-SCC members → "RootCyc1: no DB_INSERT". Cap proof enumeration at revisit-free routine paths → "PermMix: nt evidence unavailable". Drop `edge_reqs` in `cross_edge` → "GuardFwdFalse: DB_INSERT present". Drop `reqs` on states inside recursive SCCs → "GCycFalse: DB_INSERT present". Stop propagation at free states and hand them to the shared cone (the rev-3 rule) → "GTFalse: DB_INSERT present". Admit arena states below the root in `is_bfs_carrier`, the ONE predicate that both the terminal test and the `valid_nodes` seed call → "ForwardMix: nt viaPaths != [ForwardMix→Both→RealWriter]" (Both's PD(0) state becomes a carrier on the nt first hop, and the terminal test `continue`s there, `digest.rs:1215`, so the produced path is ForwardMix→Both. The named assertion compares the whole path list, so it is red whatever path the mutation produces). Skip the BFS after the proofs → "MixedCarrier: expected 2 viaPaths". |
| A2g | Enumeration incompleteness reaches the consumers (R3-A02) | `IncRoot`: the kt effect has `viaPathsTruncated == true` and conditionality `unknown` (not `conditional`). | Drop the `incomplete` OR into `had_truncation` → "IncRoot: conditionality conditional". |
| A2f | Degraded witnesses never share an occurrence | `DeepRoot`: kt and nt DB_INSERTs, both with unavailable evidence, with different `factId`s. | Drop the `|kt` canonical suffix → "DeepRoot: factIds equal". |
| A3 | State-only ops are not DB writes | `ValidateTemp`, `ValidateReal`, `CopyTemp`, `CopyShareTemp`, `CopyReal`, `TransferTemp`, `TransferReal`: no DB_* effect. `ValidateRealUi`, `CopyRealUi`: no label. | Put each op back in `modify`, one at a time → "<op routine>: DB_MODIFY present". |
| A4 | Values come from `query effects`' store (authority/subset) | Each part ⊆ the `query effects` states for that routine. | Render "unknown" for every row in `for_effect` → "TempOnly: digest says unknown". |
| A4b | Per-op join (`--lib`, hand-stated rows; stated limit: `NoL4Row` cannot be produced through the pipeline without a cone/L4 split) | Rows: op1 in class, op2 only the other class → states = op1-filtered ∪ op2-all, with a `ClassAbsent(op2)` problem. Plus op3 missing → field absent with `NoL4Row(op3)`. | Union the ops before checking → "ClassAbsent not reported". |
| A5 | Cone/L4 class parity on a matched graph | See A5 below. | Skip transitions across intra-SCC edges in the solve → "Compare 1: RecUi2 ∅ != L4 {nt}". RecUi2 reaches the producer in RecUi1 only through the intra-SCC edge. (R4-F03) |

**A5 (R2-A04, R2-F03, R3-F01).** A `tests/r3` oracle over every `tests/r0-corpus` fixture and the probe workspace. Both sides are TRANSITIVE populations: the solve no longer stops (D1.2), and L4's terminal rows are unioned into every ancestor (`db_effect_solver.rs:11-18, 392-408`).
- **Matched graph.** Build L4 (source-only, `build_db_effect_bundle`) on a combined graph restricted to the edges that have a typed twin. `implicit-trigger` is dropped, plus any other edge with no typed counterpart, listed by kind. The restriction is computed from the graphs, never from the cone result being tested.
- **Supported ops.** The capability write verbs after D5: Insert→insert; Modify and ModifyAll→modify; Delete and DeleteAll→delete. Rename is excluded on both sides. Stated limit: read producers (`Get`) are solve producers too but L4 has no write row for them, so neither compare checks them; Compare 1 only counts them per fixture and asserts the count is non-zero on ws-d21/d40/d42.
- **Compare 1 (unreduced, producers).** For every routine r and every producer p that r reaches: the set of `class(t)` over p's states at r, the seed included when `r = owner(p)`, equals the set of classes of L4's rows for p's `operation_id` at r. Routines are enumerated from L4's side AND the solve's side, so a routine missing from either side is a failure, not a skip.
- **Compare 2 (reduced).** For every routine r and every `(verb, table)`: the set of classes in r's direct AND inherited write facts equals the set of classes of L4's rows (base and inherited) for that `(verb, table)` at r.
- **Fixtures with param guards** use ⊆ (cone ⊆ L4), because L4 has no guards. The list is explicit.

The existing `ValidateTemp` "emits DB_MODIFY" block is replaced by A3. The CHANGELOG states the change of pinned behaviour.

## False-positive and failure shapes

- **Detectors move in both directions.** d43 is guarded minus setter writes (`d43.rs:475-498`). d44 is readers minus writers (`d44.rs:210-219`). Caps can surface findings that were capped before. d47/d49/d51 ordering evidence moves with the witnesses. All of it is triaged.
- **Guards.** PD-origin facts carry `param_guard` requirements on every typed edge, cycles included, including guards introduced above the point where the temp state resolves (D1.2). Ordinary facts in recursive SCCs still drop guards (a non-goal, conservative).
- **Solve population.** The transitive solve's size is bounded by producers × ancestors × the `t`/`reqs` variants observed. It is a measured gate with a budget (D1.4). Exceeding it blocks the merge.
- **Enumeration blow-up.** Proof enumeration is capped globally. Truncation and incompleteness both set `viaPathsTruncated`, which makes conditionality conservative and marks ordering's chains `capped`. The arena is linear in the number of states plus links. Materialized paths are built only for reported witnesses. Arena and materialized bytes are census counters.
- **`viaPathsTruncated` widens** to also mean "incomplete" (the state limit or the depth limit only; empty-path-set diagnostics are not folded, D3). Existing incomplete witnesses on ordinary facts now report truncated and can move from `conditional` to `unknown`. That is the correct direction, and it is triaged in the goldens.
- **Residual cone/L4 drift** shows as `temp-state-incoherent` with an op and a reason, measured on CDO.

## Measurement plan

Run the full suite on B0 and on the branch, and diff the per-test outcomes. Regenerate with `scripts/check-goldens --regen`, and triage every moved file in `.agent/golden-triage.md`.

Expected to move:
- `tests/r1a-goldens/` (D5; 9 fixtures carry Validate/Copy/TransferFields).
- `tests/r3a3-goldens/`, `tests/r3a5-goldens/` (D5; D1 values; producer separation; tie-breaks).
- `tests/r4f-goldens/` (the above, plus D3 witnesses, the dedupe split, the kt `factId` suffix, and `viaPathsTruncated`/conditionality where a witness was incomplete).
- `tests/cli-b-goldens/` (digest).
- `tests/cli-a-goldens/` (prove: D4 values even where witnesses do not move).
- `tests/cli-c-goldens/`, `tests/cli-c-policy-goldens/`, `tests/gate-goldens/`: fingerprint `M` and fingerprint witness paths (D3 is shared), diff, policy.
- `tests/r4-goldens/`: d43/d44/d45/d47, in both directions.

Must NOT move: `tests/l4-summary-baseline/`, `tests/goldens/semantic-edges/`, `tests/r0-goldens/`, `tests/r2*`, `tests/ir-l2-goldens/`.

CDO (`scripts/cdo-gate U:/Git/DO-cdo-baseline/Cloud`, branch gate):
- `aldump --program-call-graph-stats`: byte-identical.
- `alsem analyze` (cross-app default), `--scope primary` and `--scope all`: per-detector added, removed, changed and cap-substituted findings, each triaged against source.
- `temp-state-incoherent` count over every reportable root (a one-off probe through `run_digest_pipeline`), by reason, together with the number of DB_* effects checked. Target 0; every residual is explained.
- Cone and solve cost on DO and CDO (`release-fast`, 3 runs before and after): wall time, peak RSS, and `ALSEM_CONES_CENSUS=1` with the new counters, checked against the D1.4 budget.
