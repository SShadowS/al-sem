# Compiler proof — `resolve_in_object` per-candidate access filter (Object-arm + Interface-impl)

**Status: SPEC-STATED, NOT COMPILER-RUN**, same caveat as
`tests/r0-corpus/ws-visibility-local-protected/COMPILER_PROOF.md`: no AL compiler (`alc`/
`ALC_EXE`) was available in this task's execution environment. Every row below states the AL
access-modifier semantics as documented by Microsoft's AL language reference ("Access Modifiers")
rather than an actual `alc` compile/diagnostic run. The `genuine_wrong == 0` CDO L3 semantic-audit
gate is the REGRESSION BACKSTOP; this document is the intended CORRECTNESS proof artifact.

## What this task closes

`resolve_in_object` (`src/program/resolve/resolver.rs`) is the shared arity-matching routine
lookup 7 callers share. It previously did **zero access filtering** of its own — access filtering
lived entirely in each CALLER's pre-gate (`object_has_visible_member_candidate`), which only 4 of
the 7 callers actually invoked (A `resolve_in_table_scope`, C `resolve_bare` Step 2). The other 3
"self" callers (B `resolve_bare` Step 1, E `SelfObject`) are trivially safe (candidate object ==
caller object, so every access level is visible). That left **D** (`resolve_member`'s
`ReceiverType::Object` general dispatch) and **F/G** (the `Interface` SymbolOnly/Source-impl
fan-out delegates) with NO access filtering at all — a cross-app `internal` or a same-app-but-
different-object `local` member reached through an Object receiver or an interface implementer
could false-resolve to `Source`.

This task makes `resolve_in_object` itself PER-CANDIDATE access-aware (a `routine_candidate_is_
visible` predicate applied to each arity-matched candidate, not an existential "some candidate is
visible" pre-check), and threads `from_object: &ObjectNodeId` through all 7 callers so gaps D/F/G
are closed structurally rather than by yet another caller-side pre-gate.

## AL access-modifier semantics (source: Microsoft Learn, "Access Modifiers")

Identical table to `ws-visibility-local-protected/COMPILER_PROOF.md` — reproduced here for
locality:

| Modifier | Visible from |
|---|---|
| (none) / `public` | Anywhere the object itself is visible (any app that can see the object). |
| `protected` | The declaring object itself, AND any object that `extends` it. NOT visible to unrelated objects, even in the same app. |
| `internal` | Any code within the SAME extension (app/package). NOT visible from a different app (absent the separate, out-of-scope `InternalsVisibleTo` mechanism). |
| `local` | The DECLARING OBJECT ONLY. Not visible to any other object, even in the same app, even to that object's own extensions. |

AL does **not** require an interface-implementing procedure to be `public` — a codeunit may
implement an interface member with `internal` (or, in principle, `local`/`protected`) access; the
member is still a valid implementation, but its normal access-modifier visibility rules still
apply to anyone dispatching through the interface. This is the "AL-valid internal-interface"
scenario the `g1`/`g2` fixtures below exercise.

## Case-by-case matrix

Each row: fixture directory (under this directory) → AL semantics → Expected (compiles /
access-error, from the CALLING object's perspective) → fresh-engine route (pre-fix / post-fix) →
backing Rust test(s) in `src/program/resolve/resolver.rs`.

| Case | Directory | AL rule applied | Expected | Pre-fix fresh route | Post-fix fresh route | Rust test(s) |
|---|---|---|---|---|---|---|
| (D-pos-1) Object receiver, cross-app `public` | `d1_object_cross_app_public/` | `public` visible anywhere the object is visible | Compiles | Source (already correct) | Source | `resolve_member_object_cross_app_public_method_resolves_to_source` |
| (D-pos-2) Object receiver, same-app `internal` | `d2_object_same_app_internal/` | `internal` is app-scoped; same app is always visible | Compiles | Source (already correct) | Source | `resolve_member_object_same_app_internal_method_resolves_to_source` |
| (D-pos-3) Object receiver, direct PageExtension → base `protected` | `d3_object_direct_extension_protected/` | `protected` visible to a DIRECT extension of the declaring object | Compiles | Source (already correct — gap D had no filtering, so this true positive was never at risk) | Source | `resolve_member_object_direct_extension_protected_method_resolves_to_source` |
| (B-pos) bare call to own `local` | `b1_bare_own_local/` | `local` visible to declaring object (self) | Compiles | Source (self-no-op, unaffected) | Source | `bare_own_local_procedure_resolves_to_source` |
| (E-pos) `this.LocalProc()` (SelfObject) | `e1_self_local_this/` | `local` visible to declaring object (self) | Compiles | Source (self-no-op, unaffected) | Source | `resolve_member_self_object_local_procedure_resolves_to_source` |
| (D-neg-1) Object receiver, cross-app `internal` | `d4_object_cross_app_internal_excluded/` | `internal` not visible outside the declaring app | Access error | **Source → `IntNTarget.Secret` (WRONG — the bug)** | Unknown (`InternalNotVisible`) | `resolve_member_object_cross_app_internal_method_excluded` |
| (D-neg-2) Object receiver, same-app DIFFERENT object's `local` | `d5_object_same_app_local_cross_object_excluded/` | `local` is OBJECT-scoped, not app-scoped | Access error | **Source → `LocNTarget.Hidden` (WRONG — the bug)** | Unknown (`LocalNotVisible`) | `resolve_member_object_same_app_local_cross_object_excluded` |
| (D-pos-4, was D-neg-3) mixed-access same-arity overload (`public Foo(Integer)` + `internal Foo(Text)`), called cross-app | `d6_object_mixed_access_overload_no_source/` | AL leaves an inaccessible overload out of overload resolution (S9.0e alc probe: public `P(Integer)` + `local P(Text)`, an outside `A.P('abc')` fails AL0133), so the call binds to the lone visible overload | Compiles | Unresolved (pre-existing `matched.len()>1` collision guard, unaffected by access) | Source → `Foo(Integer)` (S9.0e; was Unresolved `AccessFilteredOverload`) | `resolve_member_object_mixed_access_same_arity_overload_binds_the_visible_one` |
| (D-neg-4) Object receiver, same-app UNRELATED (non-extension) `protected` | `d7_object_same_app_non_extension_protected_excluded/` | `protected` requires self OR extends-relationship | Access error | **Source → `ProtNTarget.P` (WRONG — the bug)** | Unknown (`ProtectedNotVisible`) | `resolve_member_object_same_app_non_extension_protected_excluded` |
| (D-neg-5) Object receiver, cross-app UNRELATED (non-extension) `protected` | `d8_object_cross_app_non_extension_protected_excluded/` | Same as D-neg-4, a fortiori excluded cross-app | Access error | **Source → `ProtXNTarget.P` (WRONG — the bug)** | Unknown (`ProtectedNotVisible`) | `resolve_member_object_cross_app_non_extension_protected_excluded` |
| (D-neg-6) Object receiver, WRONG-KIND extension (`PageExtension extends` a same-named `Page`, not the `Table`) | `d9_object_wrong_kind_extension_protected_excluded/` | `object_extends` is kind-compatible; a `PageExtension` extending a `Page` does NOT extend a same-named `Table` | Access error | **Source → `Shared`(Table).P (WRONG — the bug)** | Unknown (`ProtectedNotVisible`) | `resolve_member_object_wrong_kind_extension_protected_excluded` |
| (G-neg/pos) Interface fan-out, `public` + CROSS-APP `internal` implementer | `g1_interface_cross_app_internal_impl_excluded/` | `public` implementer always resolves; cross-app `internal` implementer excluded | `public` impl compiles + dispatches; `internal` impl compiles but is NOT visible from a different app's caller | **BOTH implementers → Source (WRONG for the internal one — the bug)** | `public` → Source; `internal` → Unknown (`InternalNotVisible`), NOT dropped | `resolve_member_interface_cross_app_internal_impl_excluded_public_impl_still_resolves` |
| (G-pos) Interface fan-out, SAME-app `internal` implementer | `g2_interface_same_app_internal_impl/` | `internal` is app-scoped; same-app caller sees it | Compiles | Source (gap G had no filtering, so this true positive was never at risk) | Source | `resolve_member_interface_same_app_internal_impl_resolves_to_source` |
| (D-neg-7) user-defined member literally named `Run`, arity 2, cross-app `internal` (NOT the `Codeunit.Run(arity<=1)` OnRun-trigger special case) | `d10_run_named_member_cross_app_internal_excluded/` | The OnRun-trigger dispatch is scoped to `arity<=1`; a 2-arg `Run` is ordinary member dispatch | Access error | **Source → `RunNTarget.Run` (WRONG — the bug; also proves "Run" was never a blanket exemption)** | Unknown (`InternalNotVisible`) | `resolve_member_object_user_defined_run_cross_app_internal_excluded_not_run_exempt` |
| (Run-control) `Codeunit.Run()` with NO `OnRun` trigger | `run_control_no_onrun/` | Entry-trigger dispatch bypasses `resolve_in_object` entirely; no trigger declared → boundary unknown, not a resolver bug | N/A (runtime no-op) | Opaque `AbiSymbol` boundary route (unaffected — this path never called `resolve_in_object`) | Opaque `AbiSymbol` boundary route (unchanged) | `resolve_member_codeunit_run_no_onrun_trigger_emits_opaque_not_source` |

## Why the D-neg / G-neg rows are the exact pre-fix wrong routes named in the task brief

Verified empirically during this task's TDD Step 2 by temporarily neutralizing
`routine_candidate_is_visible` (hardcoded to always return `true`, simulating the pre-fix "zero
per-candidate filtering" behavior) and re-running the new tests. 7 of 8 targeted negative tests
failed with `assert_eq!` showing the EXACT wrong `RouteTarget::Routine(..)` predicted above:

- (D-neg-1) → wrongly resolved to Codeunit id `53950` (`IntNTarget`) `Secret`.
- (D-neg-2) → wrongly resolved to Codeunit id `53960` (`LocNTarget`) `Hidden`.
- (D-neg-4) → wrongly resolved to Codeunit id `53980` (`ProtNTarget`) `P`.
- (D-neg-5) → wrongly resolved to Codeunit id `53990` (`ProtXNTarget`) `P`.
- (D-neg-6) → wrongly resolved to Table id `54000` (`Shared`) `P`.
- (D-neg-7) → wrongly resolved to Codeunit id `54030` (`RunNTarget`) `Run` (arity 2).
- (G-neg) → BOTH implementers wrongly resolved to `Source` (Codeunit id `54010` `PubImplX.Bar`
  AND Codeunit id `54011` `IntImplX.Bar`), where only the first should.

The 8th target (D-neg-3, the mixed-access overload guard) did **NOT** fail under this simulation —
see the dedicated note below; it is a regression-guard test, not a "was false-Source" test.

### Why the overload-narrowing guard fixture (D-neg-3) doesn't reproduce a pre-fix false `Source`

`RoutineNodeId` identity for SOURCE-tier routines does not include parameter TYPES (`sig_fp` is
always `0` for source; see `node.rs`), so two textually-distinct same-arity overloads
(`Foo(Integer)` public + `Foo(Text)` internal) collide onto the SAME `RoutineNodeId`. The
PRE-EXISTING `matched.len() > 1` collision guard (present before this task, `resolve_in_object`'s
"genuine SOURCE-overload collision" rule) already forced this case to `Unresolved` regardless of
access — confirmed by temporarily removing ONLY the guard's `pre_filter_count == 1` condition
(leaving `routine_candidate_is_visible` itself correct): the fixture still resolved to
`Unresolved`, because BOTH duplicate-id candidate entries are indistinguishable by identity and
therefore always report the SAME per-candidate visibility verdict (never split 1-visible-of-2).
The overload-narrowing guard is implemented exactly as specified (defense-in-depth, and
forward-compatible with a future source `sig_fp` that could make same-arity overloads
identity-distinct) — this fixture pins the OBSERVABLE INVARIANT ("mixed-access same-arity
overload never resolves to `Source`") rather than proving the guard's specific selection branch is
reachable under the CURRENT identity model. See `.superpowers/sdd/task-1-report.md` for the full
transcript.

The fix (per-candidate `routine_candidate_is_visible` + the `from_object` threading) closes all 7
reproduced false-`Source` routes; re-running the same tests against the fixed code, all pass
(honest `Evidence::Unknown` with the specific `UnknownReason` shown above, or — for G-neg — the
`public` sibling still resolving `Source` while the `internal` one is excluded and NOT dropped).
