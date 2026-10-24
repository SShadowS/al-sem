# Issue #250 — every `ObjKey::Name` holds the folded LOGICAL name

## Goal

An AL quoted identifier writes a literal `"` as `""`. Notation in this spec: the AL
spelling first, then the logical name, e.g. `interface "A""""B"` (logical `A""B`). The
IR keeps the RAW inner text (`A""""B`: outer quotes stripped, inner `""` kept), and the
engine unescapes it once (`scope::unescape_al_identifier`, #240).

The graph keys a numberless object two ways today, by where the object comes from:

- a SOURCE object is keyed `ObjKey::Name(fold(raw))` (`node_extract.rs:668` and seven
  more sites, D1), so the inner `""` stays in the key;
- an ABI (symbol-only dependency) object is keyed `ObjKey::Name(fold(logical))`
  (`abi_ingest.rs:420`, `ObjKey::from_abi`);
- `ObjKey::model_key` (`node.rs:104-116`) unescapes every name key. That is right for
  a source key and wrong for an ABI key that holds `""`.

After this change, every `ObjKey::Name` holds the folded LOGICAL name, on source and
ABI alike, and `model_key` encodes it as it is. So the graph id of a symbol-only
dependency object equals its projected model id (P3 == P8), which closes the #83 Limit.

## Non-goals

- **Resolving dependency objects whose name holds `""`.** Probe P2 shows two misses.
  They come from the name indexes, which key on `ObjectNode.name`, not from `ObjKey`.
  There is no such object on CDO or in any fixture (P3). This issue does not change
  them; they are discovery D-A.
- **`ObjectNode.name` and every other graph name field** (`extends_target`, `implements`,
  `ObjectRef`s, page controls, dataitems, query columns). They stay raw on the source
  side, and so do the name indexes built from them (`graph.rs:28`, `index.rs:561-608`).
- **Member identifiers** (discovery D-B) and **the LSP event-reference lookup**
  (discovery D-C).
- **Test fixtures** that hand-build `ObjKey::Name(name.to_ascii_lowercase())` with names
  that hold no `"` (`graph.rs:229, 318`, `index.rs:1307`, `receiver.rs:3759, 3885`,
  `applicability.rs:389`, `semantic_golden.rs:1574, 1855`, `node.rs:340, 345`,
  `impact_graph/project.rs:1973`). Their bytes do not change, so they stay as they are.

## Background (evidence: the Assumption probes in the ledger)

- **P1.** A symbol-only dependency declares `interface "A""""B"` (logical `A""B`; JSON
  `"A\"\"B"`, no Id). Its graph key is `Name("a\"\"b")`, so `model_object_id` (P3) gives
  `…/Interface/@a%22b`. The projected `ModelObject` (P8) gives `…/Interface/@a%22%22b`.
- **P2.** Source references to that dependency's `""` objects miss:
  - `X: Interface "A""""B"` gives `routes=[]`;
  - `C: Codeunit "C""""D"` (logical `C""D`, numbered) gives
    `Unknown(ObjectNotInGraph)`.

  The same calls resolve source to source (S2), and against a dependency that ships its
  source (S3), because there both sides are raw. This is out of scope; see D-A.
- **P3.** No object name holds `"` anywhere real:
  - the CDO baseline's 741 `.al` files;
  - the `Name` values of its 11 `.app` symbol files: the 498 of 1,250,923 that hold `"`
    are control `Name`s that are source expressions;
  - `tests/**/*.al`;
  - every golden directory.

  The only `""` object names are in inline test workspaces: `cli_numberless_object_ids.rs`
  (T3), `cli_escaped_object_names.rs`, `cli_object_display.rs`, and the unit test at
  `graphify_export.rs:1565-1579`.

## Design

**D1. One source key helper.** In `src/program/node.rs`, next to `ObjKey::from_abi`:

```rust
/// The key of a SOURCE object: its number, or its folded LOGICAL name. `raw` is the
/// IR's outer-stripped name (inner `""` kept), unescaped here exactly once.
pub fn from_source(number: Option<i64>, raw: &str) -> ObjKey
```

It returns `Id(n)`, or `Name(fold(unescape_al_identifier(raw)))`. The fold commutes with
the unescape, because the fold leaves `"` unchanged (`casing.rs:53-59`). It replaces the
`match obj.id { Some(n) => Id(n), None => Name(fold(raw)) }` at these sites:

| site |
|---|
| `src/program/node_extract.rs:666-669` |
| `src/lsp/snapshot.rs:1168-1171` |
| `src/program/impact_graph/facts.rs:81-84` |
| `src/program/resolve/decl_surface.rs:121-124` |
| `src/program/resolve/stub.rs:50-53` |
| `src/program/resolve/semantic_golden.rs:991-994` |
| `src/program/model/workspace.rs:1741-1744` (`dep_object_node_id`) |
| `src/program/resolve/full.rs:937-940` |

D1 also covers three comparison sites, which the self-consistency check below found:

- `src/program/resolve/differential.rs:466-469` compares
  `ObjKey::from_source(obj.id, &obj.name) == sub_rid.object.key`.
- `src/program/resolve/applicability.rs:286-296` and
  `src/program/resolve/semantic_golden.rs:1145-1152` read the table's name from its
  graph node, whatever the key arm. Today the `ObjKey::Name(s)` arm returns the key text,
  and the caller looks that text up in the table-extension index, which is keyed by the
  RAW `extends_target` (`index.rs:566-574`). After D1 the key text is logical, so the
  lookup would miss for a numberless source table whose name holds `""`. The `Id` arm
  already does the right lookup (`graph.objects…find(|o| o.id == …).map(|n|
  n.name.fold_identifier())`), so both arms use it. A numberless table exists only after
  parse recovery. The change is two lines, and it stops the code from depending on key
  text.

**D2. `model_key` stops unescaping.**
- `node.rs:104-116`: the arm becomes `ObjKey::Name(n) => object_key(None, n)`. The doc
  says every name key is the folded logical name.
- The `unescape_al_identifier` doc (`scope.rs:83-106`) drops `model_object_id` from its
  caller list and gains `ObjKey::from_source`. It keeps the note that the graph's names
  stay raw, now pointing at D-A instead of #250.
- The graphify test at `graphify_export.rs:1565-1579` is restated: its key is built with
  `ObjKey::from_source(None, "I\"\"Foo")` (`interface "I""Foo"`, logical `I"Foo`), and
  its expected id stays `@i%22foo`. A second vector shows that the ABI keys `a"b` and
  `a""b` give different ids.
- Its doc (`:1565-1567`) and the `obj_key_str` doc (`graphify_export.rs:717-721`) say
  the key is already logical, and so does the `model_object_id` doc (`model/mod.rs:34-35`),
  which today says a source name key is unescaped when the segment is produced.

## Self-consistency of D1 alone

After D1, a source object has a logical `ObjKey::Name` and a raw `ObjectNode.name`. The
table lists every place that reads `ObjKey::Name` text, or builds a key that must equal
a graph key. Searched with
`grep -rn 'ObjKey::Name\b\|from_abi(\|raw_text()\|model_key()' src`, and each hit was
read.

| reader | site | after D1 |
|---|---|---|
| graph node keys vs the ids other code builds for the SAME source declaration | the 8 D1 sites | consistent: all 8 use one helper on the same `obj.name` |
| subscriber key vs a source declaration | `differential.rs:466-469` | included in D1 |
| table key text used as a table-extension index key | `applicability.rs:286-288`, `semantic_golden.rs:1145-1147` | would break for a numberless `""` table, so it is included in D1 (reads the node name) |
| name indexes | `graph.rs:28`, `index.rs:561-608` | not affected: they key on `ObjectNode.name`, not on the key |
| `resolve_object` / `resolve_object_ref` queries | `graph.rs:96-136`, `index.rs:771+` | not affected: raw query text against raw names, as today |
| ABI keys | `abi_ingest.rs:420`, `abi_check.rs:103-106`, `edge.rs:72` | not affected: `from_abi`, unchanged |
| key text rebuilt into an `AbiRoutineKey` | `abi_check.rs:373-376`, `resolver.rs:157-160` (inside `obj_tier == SymbolOnly`), `resolver.rs:2174-2177` (`opaque_entry_route`, symbol-only targets) | not affected: symbol-only nodes only, whose keys come from `from_abi` |
| LSP ABI item display | `lsp/handlers.rs:365` (`object_node_key().raw_text()` of an `AbiRoutineKey`) | not affected: ABI only |
| model ids, event ids, external keys | `model/mod.rs:38-42` → `model_key` | source: same bytes (D1 adds the unescape, D2 removes it). ABI `""`: changes, which is the fix |
| graphify object ids | `graphify_export.rs:723-725` → `model_key` | same as the model ids |
| model-to-graph joins by id (`program_calls.rs:1659`, the #83 P2 = P3 join; events at `events.rs:384-385`) | via `model_object_id` | source bytes unchanged |
| `alsem graph` node-id hash | `impact_graph/mod.rs:320-323` (`$` + key text) | changes for numberless source objects whose name holds `""` |
| `alsem graph` `object_name` fallback | `impact_graph/mod.rs:408-415` (key text only when the node is absent) | display only, and a source node is always present |
| `impact_graph` object id | `impact_graph/mod.rs:405-407`, `project.rs:1184` (`Name(_) => None`) | not affected |
| differential harness key text | `differential.rs:166-169` (`obj_key_lc`) | changes for numberless source `""` names; no golden holds one (P3) |
| LSP def-surface hash | `lsp/def_surface.rs:319-330` | changes for those names; compared within one process only |
| LSP `CallHierarchyItem.data` | `lsp/handlers.rs:415-421` serializes the `RoutineNodeId` | changes for those names. The client sends it back in the same session, to the same binary. Call-hierarchy `detail` reads the graph name (`handlers.rs:400-401`), which is unchanged |
| string pooling, ordering | `node.rs:247-253`; `graph.objects` is sorted by `ObjectNodeId` | text changes only for `""` names, and every index is built from the same sorted vector |
| persistence | `program/pack` | nothing persists a graph key: `pack::encode`/`decode` have no caller outside `pack/`, and `DepCache` is in memory |
| the census | `census.rs:215-296` | not affected: it compares names, not keys, and names are unchanged |

**Result.** D1 is self-consistent once it includes the three comparison sites
(differential, applicability, semantic_golden). Nothing builds an `ObjKey::Name` from
reference text: every production `ObjKey::Name` comes from a declaration (D1) or from
`from_abi`.

**What moves.** Only numberless objects whose name holds `""` are affected:
- the model and graphify ids of symbol-only objects (corrected);
- for source objects: the `alsem graph` node-id hash, the differential harness key
  text, the LSP def-surface hash, and `item.data`.

P3 counts zero such objects on CDO and in every fixture and golden. The #83 T3 source
literal (`@%C3%B8%20%22q%22%20%2F%3A%23%25%40%20if`) does not move.

## Acceptance matrix

| acceptance (issue #250) | proof |
|---|---|
| source `ObjKey::Name` is built from the unescaped name at every construction and lookup site, through one helper | D1. Unit vectors in `node.rs`: `from_source(None, "A\"\"\"\"B") == Name("a\"\"b")`, `from_source(Some(7), "x") == Id(7)`, `from_source(None, "IFoo") == Name("ifoo")`. Site check: `rg -n 'ObjKey::Name\(' src`, outside `#[cfg(test)]`, shows only the two helpers in `node.rs` and match arms that read a key. The self-consistency table is the evidence for the lookup sites |
| `model_object_id` stops unescaping | D2. Unit vector `ObjKey::Name("a\"\"b").model_key() == "@a%22%22b"`, plus the restated graphify test |
| a dependency name holding `""` pins P3 == P8 | T1 in `tests/cli/cli_numberless_object_ids.rs`: #83's T5b shape with a symbol-only `interface "A""""B"` (logical `A""B`). It asserts that the graph key is `Name("a\"\"b")`, that `model_object_id(graph, node) == model_object.id == "{DEP}/Interface/@a%22%22b"`, and that `object_key` is the id's last segment. Discrimination: restore the unescape in `model_key`, and P3 becomes `@a%22b`, so T1 fails. Revert that, and T1 passes |
| source ids keep their bytes, and resolution stays as it is today | T2, the control: probe S2 (both sides source) and S3 (the dependency ships its source, tier `EmbeddedSource`), with `interface "A""""B"`, `codeunit 50103 "C""""D"` and `Impl implements "A""""B"`. It asserts literals: `model_object_id` of the source interface is `…/Interface/@a%22%22b` (unchanged from today); `X.Go()` routes to `Impl.Go`; `C.Run2()` routes to `C""D.Run2`. Discrimination: build the key with `fold(raw)` while keeping D2 (`model_key` with no unescape); the literal becomes `@a%22%22%22%22b` and T2 fails |
| the table-extension lookup reads the node name (D1's applicability row) | T3, a unit test in `applicability.rs`: hand-state a numberless table node with key `Name(fold(logical))` and name `T""""1` (logical `T""1`), plus a table extension whose raw `extends_target` is `T""""1`. Assert that the extension's trigger applies. The tested trigger must belong to the EXTENSION (not to the table itself), so the `target.object == ctx.table` shortcut (`applicability.rs:281-306`) cannot pass it without the index lookup; the existing tests at `applicability.rs:699-720` show the setup. Discrimination: return the key text in the `Name` arm, and the test fails |
| the same lookup in the semantic-golden harness (`semantic_golden.rs:1145-1152`) | T3b, a unit test beside it with the same hand-stated vector (numberless table `T""""1`, extension with raw `extends_target` `T""""1`), asserting the extension is found. Discrimination: return the key text in its `Name` arm, and T3b fails |
| resolution on CDO unchanged (`cdo-gate`) | Measurement plan, steps 1 and 3 |
| #83's tests stay green | `source_interfaces_get_distinct_ids_from_every_producer` and T5b pass unchanged |

## False-positive and failure shapes

- **A missed construction site.** If code still builds a source `ObjKey::Name` with
  `fold(raw)`, that source object gets two keys. One of them misses in a
  `binary_search` against `graph.objects`, and the lookup fails silently, only for `""`
  names. The `rg` site check guards every construction site; T2 exercises only
  node_extract and the model join (its callers are numbered codeunits, so it does not
  reach the `full.rs:937`, `stub.rs:50`, `decl_surface.rs:121`, `facts.rs:81` or
  `lsp/snapshot.rs:1168` sites with a numberless `""` object).
- **Double unescape.** `from_source` applied to a value that is already logical (an ABI
  name, a `from_abi` key) turns logical `A""B` into `A"B`. T1 and T2 use names of that
  form on purpose.
- **Reading key text as a name.** After D1, any reader that treats `ObjKey::Name` text as
  the object's name and compares it with raw graph names is wrong. The self-consistency
  table lists every current reader. A new reader must read `ObjectNode.name` instead.

## Pre-stated fallbacks

- **A golden moves.** P3 found no golden that holds a `""` name, so the move is
  unexplained. Do not bless it. Run `golden-diff-triager`, find the site, and fix it.
- **A CDO number moves.** Same rule: the population is zero, so it is a bug, not a
  re-mint.
- **T2 fails before the change** (a resolution control does not hold on master). The
  control premise is then wrong. Record it, drop that one assertion, and keep the id
  literal.
- **Review asks for the resolution half.** It is discovery D-A, with its list of points
  to address. Do not fold it back in without that design.

## Discoveries

```json
[
  {
    "subsystem": "program graph / resolver (object-name lookup)",
    "locator": "src/program/node_extract.rs:759-762, 509-520 (graph object names and ObjectRefs stay raw); src/program/graph.rs:28, src/program/resolve/index.rs:561-608 (name indexes on ObjectNode.name); source reference producers via receiver::unquote_identifier (receiver.rs:3500)",
    "symptom": "A source reference to a symbol-only dependency object whose name holds an escaped quote does not resolve: the source text is raw (A\"\"\"\"B), the ABI node name is logical (A\"\"B), and the name indexes compare the two.",
    "kind": "bug",
    "origin_issue": 250,
    "reproducer": "A symbol-only dependency declares interface A\"\"B (JSON Name \"A\\\"\\\"B\", with Go), codeunit 50200 C\"\"D (with Run2), and codeunit 50201 Impl with ImplementedInterfaces [\"A\\\"\\\"B\",\"IPlain\"]. The workspace calls X.Go() on X: Interface \"A\"\"\"\"B\" (routes=[]) and C.Run2() on C: Codeunit \"C\"\"\"\"D\" (Unknown(ObjectNotInGraph)). The control P.Go() on IPlain reaches Impl.Go. Probe crate: /work/bench/250/probe, output /work/bench/250/p1p2-v2.out (S1).",
    "pre_existing": true,
    "capability": "every object name the graph stores and every object name a source reference looks up is the folded LOGICAL name, so source references bind symbol-only dependency objects whose name holds \"\"",
    "acceptance": "P2's two calls resolve, and S2 and S3 keep resolving. The design must address each of these points. (a) The two-shapes rule: still-quoted text goes through one helper (strip, then unescape); outer-stripped IR text goes through unescape_al_identifier directly; each site names its shape, and R11's four arms in event.rs:131-142 are each handled separately. (b) ObjectRef::Name.raw holds the logical name, so workspace.rs:820-822 stops unescaping SourceTable, and full.rs:1174-1183 written and program_calls.rs:1568-1572, 1600-1603, 1968-1970 compare logical to logical. (c) The census (census.rs:226, 252, 268) stops unescaping the graph side, and the model's implements_interfaces (workspace.rs:831-833) becomes logical; otherwise census.rs:279-296 reports mismatches. (d) al_name (full.rs:1156-1163) and the graphify object_label (graphify_export.rs:756-761) re-escape quotes. (e) The bare enum-type receiver (receiver.rs:1208-1217) is unreachable for escaped names, because is_atomic_receiver_token (receiver.rs:3479-3492, 1458-1477) rejects inner quotes; either the predicate changes or the row is dropped, and a widened predicate comes with malformed-token negatives and member/variable-shadowing negatives. (f) impact_graph property_object_ref (facts.rs:264-266) receives both shapes (the XmlPort dataitem table via facts.rs:183-196 is ident_text): the XmlPort caller moves to the outer-stripped helper, with an XmlPort regression whose table name is ALL quotes (interior-only names pass through the wrong helper unharmed, so they cannot catch it); and a codeunit RunObject fixture bypasses it (facts.rs:155-196, 292-307), so the fixture uses a CardPageId / LookupPageId / DrillDownPageId naming an escaped dependency page and asserts the exact graph edge to that target. (g) Ns.\"E\".FromInteger needs a namespace path of at least 2 segments (receiver.rs:1850-1859, 2418-2437). (h) ABI type text is not always free of escaped quotes: AlreadyQuoted parameter and field types pass through (symbol_reference.rs:899-920), and return types without a Subtype pass their outer text through (820-830), so classify_type_text's normalization must be stated and tested for this source-shaped ABI text. (i) node_extract's ident_text ObjectRef producers (node_extract.rs:711, 750, 775) must keep the temporary stripping that parse_object_ref_value does today. (j) Every test must observe the object-name resolution itself: Record FindFirst emits trigger routes (full.rs:789-864, resolver.rs:2308-2318); V.AsInteger ignores the enum name (resolver.rs:2993-3007, receiver.rs:3209-3210), so an Enum-typed variable assertion cannot isolate the Enum branch of classify_type_text; DataitemNode.source_table (node_extract.rs:745-750) needs a direct assertion. (k) One revert-one-site discrimination per site. (l) LSP call-hierarchy detail (handlers.rs:400-410, 530-538) and item.data (handlers.rs:415-421) changes are listed as intended. CDO resolution is unchanged (cdo-gate), and no golden moves (population 0, P3)."
  },
  {
    "subsystem": "program graph / resolver (member identifiers)",
    "locator": "src/program/node_extract.rs:716-747 (FieldNode.name_lc), RoutineNodeId.name_lc from RoutineDecl.name; member queries at receiver.rs:1007, 1810, 2466, 2486 and arg_dispatch.rs:784, 1101, 1120 (unquote_identifier keeps the inner \"\")",
    "symptom": "A source member reference whose name holds an escaped quote (field Rec.\"A\"\"\"\"B\", call Dep.\"Do\"\"\"\"It\"()) does not bind the symbol-only dependency member, whose ABI name is logical (A\"\"B / Do\"\"It). Source-to-source references bind, because both sides are raw.",
    "kind": "bug",
    "origin_issue": 250,
    "reproducer": "A symbol-only dependency declares table 50200 T with a field named A\"\"B (JSON Name) and a codeunit procedure named Do\"\"It. The workspace uses R.\"A\"\"\"\"B\" and Cu.\"Do\"\"\"\"It\"(); the members do not bind. The same code with names AB and DoIt binds.",
    "pre_existing": true,
    "capability": "every member name the graph stores and every member name a source reference looks up is the folded LOGICAL name, on source and ABI alike",
    "acceptance": "Graph member names (fields, routines, events, enum values, dataitems, controls) are unescaped once at node_extract, and member queries go through one helper, under the same two-shapes rule as D-A. One test per member shape passes against a symbol-only dependency member whose name holds \"\", with a discrimination proof per site. CDO resolution is unchanged (cdo-gate). Population: 0 in the 11 CDO .app files and in the CDO source."
  },
  {
    "subsystem": "LSP custom requests (event reference lookup)",
    "locator": "src/lsp/custom.rs:529-533, with the source-side name from src/lsp/custom.rs:638-666 (find_event_subscriber_display_at)",
    "symptom": "eventReferenceAtPosition compares the symbol-only publisher's ABI name (logical, C\"\"D) with the subscriber's publisher name taken from source text (raw inner text, C\"\"\"\"D) via eq_fold_identifier, so the publisher is not found.",
    "kind": "bug",
    "origin_issue": 250,
    "reproducer": "A symbol-only dependency declares codeunit 50200 C\"\"D with an event OnX. A workspace subscriber has the attribute [EventSubscriber(ObjectType::Codeunit, Codeunit::\"C\"\"\"\"D\", 'OnX', '', false, false)]. al-call-hierarchy/eventReferenceAtPosition on it does not find the publisher; with a codeunit named CD it does.",
    "pre_existing": true,
    "capability": "the LSP event-reference lookup compares logical names on both sides",
    "acceptance": "Only the demonstrated source-text producer (the publisher name from find_event_subscriber_display_at) is made logical, with the helper that matches its shape; its current trim_matches('\"') (custom.rs:660) strips EVERY leading and trailing quote, so it is replaced by strip-one-outer-pair-then-unescape, not wrapped with an extra unescape. find_external_object (custom.rs:374-378) is NOT changed across the board: dependencyDocumentSymbol's names come from explicit arguments or decoded al-preview URIs (custom.rs:282-301), and numberless ABI preview names come from the already-logical AbiRoutineKey.object_name_lc (handlers.rs:510-526), so unescaping them would corrupt them. An LSP test with a \"\" publisher finds it, and a preview-URI round trip of a logical \"\" name still finds its object."
  }
]
```

## Measurement plan

1. **Before** (master `678d6540`) and **after**, on `/work/src/cdo-baseline/Cloud`, run
   `target/cdo/aldump --program-call-graph-stats` and `--dependency-bodies-stats`, and
   save both outputs in `/work/bench/250/`. Expected: byte-identical JSON.
2. `scripts/check-goldens`. Expected: green, and no golden file changes.
3. `scripts/ci-steps task`, then the branch gate: `scripts/ci-steps all` and
   `bash scripts/cdo-gate` (no argument).
4. Run the probe crate `/work/bench/250/probe` (`cargo run` against the branch).
   Expected: S1's P3 is `@a%22%22b`, S1's two calls still miss (D-A), and S2 and S3 are
   unchanged.

## Changelog

Add under `Fixed`:
- A numberless object of a symbol-only dependency whose logical name holds `""` now has
  the same id in the program graph and in the model.
- `alsem graph` node ids of numberless source objects whose name holds `""` change once.
