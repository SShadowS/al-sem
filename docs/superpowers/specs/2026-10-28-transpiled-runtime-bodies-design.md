# Transpiled bodies for runtime packages without AL source

Status: draft, 2026-10-28, at base `d47a7099` (branch `continai-gaps`). Asked for by the
CONtinAI verifier session (G24 follow-up). Not yet reviewed by a panel.

**Decided by the owner (2026-10-28): route A (§3, §8), and building it in this public
repo is fine (Q2: yes, with hand-written fixtures only).**

## 1. Problem

A BC **runtime package** (`.NEA`, read since #366) holds the app as BC's transpiled C#
(`bin/*.cs`) plus `SymbolReference.json`. A publisher may also ship the AL `src/` in it
(Continia's `allowDownloadingSource`), and then al-sem reads the app as source. When it
does not, al-sem knows the app from its symbols only: every routine is **bodyless**.
A cone that enters one is `opaque-dependency`, the app is a `symbol-only-boundary`
analysis gap, and `prove` cannot answer `no`.

On Continia DO 29.0 one package is like that: **Core** (585 entries, 230 `bin/*.cs`,
0 `.al`). It is behind most of DO's undecided `prove` claims (measured at `d47a7099`,
408 claims, with and without `--closed-world`: 318 proven, 88 unknown, 2 contradicted):

| Blocker in the 88 unknown claims | Claims |
|---|---|
| `coverage-incomplete` (reasons include `opaque-dependency`) | 76 |
| `analysis-gap` — the gap is Core's `symbol-only-boundary` | 43 |
| unresolved calls into Core (`CSC XML Node`, `CSC RegEx Management`, `Core.*`, `ContiniaCore.*`) | ~35 |

The C# is not obfuscated and is regular. The effects are in it.

## 2. Facts (probes, all on Core 29.0.0.221304)

- **P1 Routine identity is exact.** Each C# method has `[MethodId(n)]`; it equals the
  `Id` of that method in `SymbolReference.json`. `CSC XML Node`: 39 of 39 methods match
  by id and name.
- **P2 Calls name their target exactly.** A codeunit variable is
  `x = new NavCodeunitHandle(scope, <codeunit id>)`; a call is
  `x.Target.InvokeAsync(<method id>, new object[] { args })`. Target app is open
  (6225934 is in Continia System Application, which has AL source).
- **P3 Record variables name table and temporariness.**
  `new NavRecordHandle(scope, <table id>, <temporary: bool>, SecurityFiltering.…)`.
- **P4 Record operations are runtime calls with AL names.** `ALInsertAsync`,
  `ALModifyAsync`, `ALDeleteAllAsync`, `ALGetAsync`, `ALFindSetAsync`, … each with
  `DataError.ThrowError` (statement position: the error propagates) or
  `DataError.TrapError` (the Boolean is consumed).
- **P5 Other effects are static runtime calls.** `ALDatabase.ALCommit` (58),
  `NavDialog.ALError` (260), `ALMessage` (31), `ALConfirm` (28),
  `ALSystemOperatingSystem.ALHyperlink` (20).
- **P6 Events.** 113 `[NavEvent(…)]` publishers (Integration 94, Internal 18, Business 1),
  91 `[NavEventSubscriber(…)]` subscribers.
- **P7 Control flow is kept** (`if`, loops, `try`/`catch` around `ALMethodScope`, the
  `CStmtHit(n)` statement counters).
- **P8 The `NavRuntimeHelpers.CompilationError` calls (194)** are the `default:` arm of
  each codeunit's method-id dispatch switch, not missing code.
- **P9 An app is either source or symbol-only** in the model: ABI rows
  (`program::model::abi_rows`) are built for symbol-only dependencies; a source app's
  objects all come from its parsed source.

## 3. Two ways in

### A. Synthesize AL source for the whole app

`snapshot::embedded::extract_embedded_source` is the one place package source enters.
For a package with no `.al` but with `bin/*.cs`, write AL text from the symbols (every
object: tables with fields and keys, pages, enums with values and implementations,
interfaces, …) and the C# bodies, and let the existing parse → model → resolve → cone
pipeline run.

- Everything downstream is reused, events and temp-state included.
- But by P9 the synthesized source must reproduce **every** object faithfully, or the
  app's tables, fields and enums silently change shape. Symbols already describe them;
  re-deriving them as text is a second source of truth.
- A call loses its exact target (P2): AL text is re-resolved by name and argument types,
  so an overloaded target can become ambiguous where the C# was exact.

### B. Keep the symbols, attach transpiled body summaries (recommended)

The app stays symbol-only (ABI rows are the objects). A new reader turns each C# method
into a **body summary** of the routine with that `MethodId` (P1):

- the capability facts the AL extractors would emit, in the same vocabulary
  (`insert`/`modify`/`delete`/`rename`/`commit`/`ui-*`/`error-throw`/`error-implicit`/
  `open` (hyperlink), with table id and the variable's temporariness from P3/P4);
- the call edges, **already resolved** by `(codeunit id, method id)` (P2), so no
  re-resolution and no ambiguity;
- events published and subscribed (P6);
- every shape it does not translate as an explicit **opaque** fact (§5), never silence.

The model marks such a routine `body: transpiled`. The cone reads its facts and follows
its edges like a source body's. `opaque-dependency` and the `symbol-only-boundary` gap
stay only for what is left.

Why B: it keeps one source of truth for objects (the symbols), uses the exact call
targets the C# gives, and confines the change to a new body kind plus one reader.
Cost: the fact extraction for C# is new code, which must stay aligned with the AL
extractors (shared op vocabulary and one test table that runs the same AL behaviour
through both).

## 4. Conditionality

The MAY questions (`may-commit`, `writes-table`, `reaches-ui`, `throws-error`) need only
"the effect can happen". The MUST question (`commits-on-success-path`) needs "on every
success path". Phase 1 records every transpiled fact as **conditional**, which is sound
for both: a MAY question sees it, and the MUST question never treats it as
unconditional. Reading P7's control flow for unconditional facts is a later phase.

## 5. Soundness rules

1. A routine is `transpiled` only when its C# method is found by `MethodId` and read
   without error; otherwise it stays bodyless (today's behaviour).
2. Every runtime call or construct the reader does not classify is an **opaque** fact
   naming the shape. It blocks a `no` exactly as an unresolved call does now.
3. Dynamic dispatch the C# cannot resolve (an interface handle, `Codeunit.Run` on a
   variable id, `RecordRef`) stays unresolved, as in AL.
4. Provenance is visible: evidence anchors point at the C# file and line
   (`dep:<guid>:bin/COD6192821.cs:<line>`), with `sourceKind: "transpiled"`.

## 6. Phases (route A)

1. **Method ids + the resolver rule** (§8.1 P11, §8.3), testable with hand-written AL:
   the id function pinned by the alc probe ids; `__alsem_mid_<n>` binding on source and
   ABI targets.
2. **C# reader + census:** read every C# file of a package into per-method statements;
   a census test on a hand-written fixture lists every runtime-call shape and fails on a
   new unclassified one.
3. **Synthesizer** (§8.2): declarations from P10 + symbols, bodies from phase 2.
4. **Hook + measure:** `extract_embedded_source`, cache version, the `transpiled`
   provenance on anchors; measure DO's claims.
5. Later: conditionality from control flow (unconditional facts for the MUST question).

## 7. Questions for the owner (answered)

- Q1: route B (recommended) or A? **A.**
- Q2: Is reading a publisher's transpiled code acceptable in this public repo? **Yes**,
  with hand-written fixtures in the transpiler's shape, never a publisher's code.

## 8. Route A design

### 8.1 More facts

- **P10 Per-object metadata.** `bin/<KIND><id>.xml` (286 in Core: 135 `CodeUnit`, 64
  `MetaTable`, 59 pages, 17 enums, 2 queries, 2 interfaces, permission sets) holds each
  object's declaration: a table's fields (id, name, `Datatype`, length, `FieldClass`, …),
  keys and properties. With `SymbolReference.json` (routine signatures, ids, attributes)
  it is enough to declare every object without its code.
- **P11 The method id is computable.** alc's `MethodSymbol.CalculateMethodId` (runtime ≥
  Spring 2021; read from alc 18.0 `Microsoft.Dynamics.Nav.CodeAnalysis`, reimplemented,
  not copied): FNV-1a over the UTF-16LE upper-cased name, then `Combine` with the return
  type's `NavTypeKind`, then per parameter `Combine(i, isVar, kind)` (kind with the
  `_ReturnValue` bit cleared for the kinds `ReturnValuesAddedForRuntimeVersion7` lists;
  an array is `Array`), plus each parameter's subtype hash (object id, or 1) when the
  method can be overloaded (not an event, subscriber or handler) and a parameter is an
  object type. `Combine(a, b) = ((a << 5) + a + (a >> 27)) ^ b` on `i32`.
  Checked: 14 of 14 probe methods compiled with alc 18.0.43; 1536 of 1568 of Core's
  methods. The 32 misses all take a generic (`List of [...]`, `Dictionary`) or
  `ModuleInfo` parameter; not yet understood, so such a target stays unresolved.

### 8.2 Shape

1. **Where:** `snapshot::embedded::extract_embedded_source`. A package with no `.al` but
   with `bin/*.cs` gets synthesized `SourceFile`s, one per object, at virtual paths
   `transpiled/<KIND><id>.al`. The cache key carries a synthesizer version.
2. **Declarations** from P10 + the symbols: every object, field, key, enum value,
   implementation and routine signature (with its attributes: events, subscribers,
   `[TryFunction]`, access).
3. **Bodies** from the C#, per routine found by `MethodId` (P1). In source order, each
   statement that has an effect or calls code becomes one AL statement, inside
   `if <branch> then` (§4) unless proven unconditional later:
   - record operations on the record variable, declared `Record <table id>` with
     `temporary` from P3; `TrapError` keeps the Boolean (`if X.Insert() then;`),
     `ThrowError` is a statement (§2 P4);
   - `Commit()`, `Error('')`, `Message('')`, `Confirm('')`, `Hyperlink('')`, page and
     report runs, `Codeunit.Run`;
   - calls into the same object by name; calls on a codeunit variable as
     `Cu."__alsem_mid_<method id>"(args)` (§8.3);
   - anything else the census (§6 phase 1) classifies as effectful but the writer cannot
     express: a call to the undeclared `"__alsem_untranslated"('<shape>')`, so it is an
     unresolved call with its shape in the detail, never silence.
4. **Arguments:** a record variable passed is passed (temp-state flows through); any
   other argument is a local of the parameter's type.

### 8.3 Calls by method id

The synthesized AL names a cross-object target only by method id (the C# has no name,
and the target may be a source-only app with no symbols). The resolver gets one new
rule: a member call whose name is `__alsem_mid_<n>` binds to the routine of the receiver's
object whose method id (P11, from its declared signature) is `n`. Exactly one match
binds; zero or more stays unresolved. ABI routines use their symbol `Id` directly.
