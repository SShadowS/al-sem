# Issue #83 — a unique object id for objects without a number

## Goal

Every object gets a unique, stable object id, whether or not it has a number. Today every
Interface and ControlAddIn in an app gets `<app guid>/Interface/0` (or `/ControlAddIn/0`). The
collision reaches every id built on it: the model object id, the R0 and R1a stable object ids, and
the dependency ABI object ids and routine ids. For example, `tests/r0-goldens/ws-interface-dispatch…`
has two rows `…:Interface:0`, for `IEmpty` and `IProcessor`.

## Non-goals

- **Rename tracking.** An object id that carries a name changes when the object is renamed. This is
  the same as a numbered object's id changing when it is renumbered: a rename is a remove plus an
  add.
- **`ObjKey::Name` normalisation in the program engine** (see D3 and Limits). The source side keys
  on the folded RAW name; the ABI side keys on the folded logical name. Not changed here.
- **`object_signature_fingerprint`** (`ids.rs:331`). It already hashes the name, so it is already
  distinct. Its `number` input stays `0` for a numberless object, so no golden moves for it.
- **Table ids** (`encode_table_id`, `workspace.rs:957`). A table whose number did not parse still
  collides there (see Limits).

## Background (evidence: `$SCRATCH/83/map.md`)

- **One encoder.** `engine::ids::encode_object_id(guid, type, number: i64)` (`ids.rs:130`). Every
  producer passes `id.unwrap_or(0)`:
  - P1 `engine/snapshot.rs:154-161` (R0 identity);
  - P2 `program/model/workspace.rs:778-780` (the model);
  - P3 `program/model/mod.rs:32-40` `model_object_id` (`ObjKey::Name(_) => 0`);
  - P5/P6 `program/body/l2_workspace.rs:280-284, 459-461` (R1a);
  - P8 `engine/deps/projection.rs:341` (every dependency ABI object), plus its `source_hash` at
    `:350-354`.
- **The ABI has no "absent" number.** `AbiObject.object_number: i64` (`symbol_reference.rs:267-268`)
  is `o.id.unwrap_or(0)` (`:1291, 1350, 1368, 1394, 1454`). The program engine already reads `0` as
  "no number": `abi_ingest.rs:421-425` makes it `ObjKey::Name`, and so do `program_calls.rs:1530-1534,
  1580-1584` for external keys.
- **Routine ids.**
  - The internal routine id hashes `[guid, type, number.to_string(), kind, name_lc, normhash,
    (member)]` (`ids.rs:250-268`).
  - The stable routine id is `{stable object id}#{hash}` (`ids.rs:315-328`).
  - Source Interface and ControlAddIn have no `ModelRoutine`s, but dependency ABI interface methods
    do (`deps/projection.rs:201-236, 364-373`). So two dependency interfaces with a method of the
    same signature collide on BOTH routine ids today. A numbered kind whose number did not parse
    has routines too, and they collide in the same way.
  - `CanonicalRoutineKey` is built at `scope.rs:131` (all `compute_routine_id` callers:
    `l2_workspace.rs:136, 229`, `workspace.rs:1021, 2800`, `tests/l2_ir/ir_robustness.rs:235, 273`),
    `deps/projection.rs:202`, `workspace.rs:2497` (test re-mint), `ids.rs:451` (test) and
    `tests/l2_ir/encoder_vectors.rs:33`.
- **#71's declaration-only ids** (`workspace.rs:899-931`) avoid the collision by folding the object
  name in as the enclosing member.
- **Collision damage.**
  - Maps keyed by `ModelObject.id` keep only the last colliding object today: fingerprint, detector
    context, d17, d50, root classification, effect query, policy, HTML, the identity table and the
    contracts (`map.md` §3 lists the sites).
  - `l5/fingerprint.rs:150-152` builds its object part from `"{type}/{number}"`, so it collides too.
  - `tests/differential.rs:168-199` keys on `(stableObjectId, name)` to work around the collision.
- **Event ids embed the publisher's model object id** (`events.rs:384-385`). It goes through P3 for
  every bound publisher, including platform publishers. A publisher keyed `ObjKey::Name` therefore
  gets `…/<Kind>/0/event/…` today.
- **Goldens holding a numberless id.** I searched every golden directory with the regex
  `[:/]0([#"/]|::)`. The hits outside `tests/r0-goldens` (6 lines) and `tests/r1a-goldens` (6 lines)
  are not object ids:
  - arity suffixes (`h:name:ping/0` in `cli-query-goldens`);
  - finding-index suffixes (`…#<hex>/0` in `r4-goldens` d39–d42, d59);
  - an object-index column (`::0::` in `l2_features.snapshot`).

## Design

**D1. The object key.** One new function in `engine/ids.rs` decides the segment; nothing else does.

```rust
/// The object segment of an object id: the decimal number when the object has one; else `@`
/// followed by the percent-encoded, folded logical name.
pub fn object_key(number: Option<i64>, logical_name: &str) -> String
```

- `Some(n)` gives `n.to_string()`. This is byte-identical to today, so **no numbered object's id,
  internal routine id, stable routine id or fingerprint moves**.
- `None` gives `@` + `pct(fold_identifier(logical_name))`.
  - `pct` keeps only the bytes `a-z` and `0-9`. It writes every other UTF-8 byte as `%XX`
    (uppercase hex), including `.`, `_` and `-` (R1-09).
  - Examples: `IFoo` → `@ifoo`; `"Sales Line"` → `@sales%20line`; `"My/If:#x"` →
    `@my%2Fif%3A%23x`; `"123"` → `@123`; `"Ø ""Q"" If"` (logical `Ø "Q" If`) →
    `@%C3%B8%20%22q%22%20if`.
- **Why `@`:**
  - It is not a digit, so the segment never parses as a number. `abi_rows.rs:110-116` parses it, and
    a quoted `"123"` interface must not read as object 123.
  - It is not `/`, `:` or `#`, which consumers split on (`map.md` §3), and `pct` never emits it.
- **Why percent-encoding and not a hash:**
  - The id stays readable (`…:Interface:@iprocessor`).
  - The encoding is reversible and cannot collide.
  - An id never holds `/`, `:`, `#`, space or `"`.
- **Sort order (R1-09).** The orders are not changed, and each output keeps its existing contract:
  - The identity table (`l5/snapshot.rs:1141-1165`, a `BTreeMap`) stays **ordinal**. Its "ordinal ==
    ICU" claim (M8, `:25-29, 1136-1138`) was an al-sem parity argument, and al-sem is retired. The
    comment is rewritten to state the contract as ordinal.
  - The contracts (`snapshot_full.rs`) stay `locale_compare`.
  - Keeping only `a-z 0-9` unencoded makes the two orders agree on every key except where `@` or
    `%XX` meets another byte in the same position. Today that only happens for a numbered kind whose
    number did not parse, next to a numbered object of the same kind.
  - T7 pins the mixed case for both outputs.
- **Folding.** `al_syntax::fold_identifier`, the program engine's object-name fold, makes the key
  case-insensitive, like AL names.

**D2. Every producer uses the key.**

- `encode_object_id(app_guid, object_type, object_key: &str)`.
- `CanonicalRoutineKey.object_number: i64` becomes `object_key: String`. The routine hash uses it
  verbatim in place of `number.to_string()`, which is the same bytes for a numbered object.
- `ModelObject` gains `object_key: String` (R1-12). The test re-mint (`workspace.rs:2497`) reads
  it, and so does `l5/fingerprint.rs:150-152`, whose object part becomes `"{type}/{object_key}"`.
  That is the same bytes for a numbered object (R1-10).
- `ProjectedObject` (`deps/projection.rs:342`) gains `object_key: String`, set by P8 to
  `object_key(abi_object_number(n), name)`. `abi_rows.rs:270` (`dep_object_to_l3`) copies it into
  the dependency `ModelObject`, and nothing re-parses it from the id (R2-02).
- `ModelObject.object_number` and `ModelRoutine.object_number` stay `i64` (`0` when absent) for
  callers that need a number.
- Every site listed under Background "Routine ids" and every producer P1–P8 calls `object_key`. The
  dependency `source_hash` (`projection.rs:350-354`) hashes the key in place of the number.

**D2a. The ABI zero rule (R1-01).** One helper, `ids::abi_object_number(n: i64) -> Option<i64>`
returning `(n != 0).then_some(n)`, is the single statement that "an ABI object numbered 0 has no
number". It is used by:
- P8;
- `abi_ingest.rs:421-425`;
- `program_calls.rs:1530-1534, 1580-1584`.

AL object numbers start at 1, and SymbolReference.json has no `Id` for Interface, ControlAddIn or
DotNetPackage. So this rule matches the program engine's existing convention: P3 and P8 agree on the
object SEGMENT for every dependency object (the KIND segment has existing mismatches, see Limits).

**D3. The logical name.** Each producer passes the object's LOGICAL name (outer quotes stripped,
inner `""` unescaped):
- source IR (P1, P2, P5, P6): `unescape_al_identifier(&o.name)` (`scope.rs:97`);
- ABI (P8): the ABI name, which is already logical;
- P3, from `ObjKey::Name(n)`: `unescape_al_identifier(n)`.
  - On a source object, `n` is the folded RAW name, and unescaping gives the folded logical name.
    Folding does not change `"`, so `fold` then `unescape` equals `unescape` then `fold`.
  - On an ABI object, `n` is the folded logical name, and unescaping changes it only if the logical
    name itself contains `""` (see Limits).

So P2 and P3 give the same string for every source object, and P8 and P3 for every dependency object
whose logical name has no `""` and whose kind both map the same way (Limits).

**D4. #71 declaration-only ids are re-derived.** With a unique object id, folding the name in is
redundant. The id becomes `to_stable_routine_id_from_parts(&stable_object_id, &hash, None)`, i.e.
`{guid}:Interface:@ifoo#{normhash}`. This is the issue's "re-derived from it". It is also the same
shape the dependency side gives the same interface's method. Every declaration-only id changes once.

The rest of D4 (R1-11):
- Its consumers (`digest_cli.rs:312-362, 843-869`) key on the same field, so they move with it.
- `cli_digest_changed_spans.rs`'s `decl_only_id` doc and mutation note (`:1765-1783`) are rewritten
  for the new derivation.
- The CHANGELOG states that digest `stableId` values for declaration-only roots change, while the
  schema version does not.

**D5. The differential harness compares objects one-to-one (R1-03).** `tests/differential.rs:168-199`:
- Before building its maps, it asserts that neither snapshot has two object rows with the same
  `stableObjectId`. A duplicate is a hard failure naming the id.
- It then keys by `stableObjectId` alone.
- It now compares `name`, which until now was skipped because it was part of the key.
- The comment explaining the old workaround is replaced by one explaining this check.

## Acceptance matrix

| Acceptance (issue #83 + findings) | proof |
|---|---|
| unnumbered objects get a unique object id, keyed by name | T1 unit vectors for `object_key`: numbered is `n.to_string()`; `IFoo`/`ifoo` equal; `/ : # % @ . _ -`, space and `"` are all encoded; an all-digit name; non-ASCII case pairs (`Ø`/`ø`); a parse round-trip check that no id holds `/ : #`. T2: the R0/R1a goldens regenerate with distinct ids |
| two interfaces get distinct `ModelObject` ids; a test pins both object ids | T3: a temp workspace with interfaces `IEmpty` and `"Ø ""Q"" /:#%@ If"`. For each interface, the expected literal id is asserted from P1 (R0 snapshot), P2 (the model), P3 (`model_object_id` of its graph node) and P5 (R1a), so all four producers agree (R1-02, R1-08) |
| a numbered kind whose number did not parse | T3b: two codeunits whose number overflows `i64` (`codeunit 99999999999999999999 "CU A"` / `"CU B"`), each with `procedure Run()`, get distinct object ids, internal routine ids and stable routine ids. One of them implements an interface, and the interface's dispatch does not list it as not-found: it exercises the P2 = P3 join at `program_calls.rs:1659` (R1-08). The test itself asserts the precondition: `al_syntax::parse(...)` gives each object `kind == Codeunit` and `id == None`. So it fails loudly if the lowering ever saturates or wraps the number (R2-03) |
| #71's declaration-only id is re-derived from it | T4: `decl_only_id` uses the new object id and no member. Its shape assertion pins `…:Interface:@<name>#` + 64 hex. Two interfaces declaring the same `Bar()` still get distinct ids |
| dependency interfaces no longer collide | T5: the REAL SymbolReference parser (`symbol_reference.rs`) reads a JSON with two `Interfaces` without `Id` that declare the same method. The projected object ids contain `@<name>` and differ, and their internal and stable routine ids differ (R1-01) |
| P3 = P8 for a dependency interface | T5b: in a cross-app model of a workspace whose dependency declares interfaces without `Id`, the test finds the dependency Interface graph node (`ObjKey::Name`) and the projected dependency `ModelObject`, asserts that both exist, and asserts `model_object_id(graph, &node.id) == model_object.id`, and that `model_object.object_key` is the id's last segment (R1-08, R2-01). It uses an existing fixture if one has a dependency interface; otherwise it builds the symbol package in the test, the way the `r25_abi` vectors do |
| ordering of mixed keys | T7: an identity table holding `<guid>:Codeunit:1` and a recovered `<guid>:Codeunit:@a` lists `…:1` first (ordinal), and the contracts list `…:@a` first (`locale_compare`). Each output keeps its own existing order contract (R1-09) |
| colliding objects survive end to end | T6: the full-snapshot path (identity table + contracts) on T3's workspace has both interfaces, with distinct `stableId`s and their own names (R1-04) |
| the harness can no longer hide a collision | D5, proven by discrimination: an injected duplicate row fails, and a changed name fails (R1-03) |
| numbered ids do not move | T1 vector `object_key(Some(50811), "x") == "50811"`; `encoder_vectors` unchanged; no golden for a numbered object moves (triage) |

## Limits (stated, not fixed)

- An ABI object whose LOGICAL name contains `""` (a source name written `"A""""B"`) gives P3 ≠ P8
  (D3). Making P3 unescape only for nodes from source apps would close it. That needs the graph to
  record each node's origin, which is out of scope. No real object name is known to have this shape.
  `aldump --graphify-export`'s object ids share this limit (ABI logical names `a"b` and `a""b` give
  one id), because they are the model's ids (`ObjKey::model_key`).
- **Existing object-KIND mismatches between the ABI and the graph (R1-08).** These exist today and
  are not changed. They give P3 ≠ P8 with or without a number:
  - `abi_ingest.rs:259-278` maps an ABI `DotNetPackage` (and any unknown type) to `Codeunit`, while
    P8 keeps `DotNetPackage`;
  - `model_object_id` names `PermissionSetExtension`, `Profile` and `Entitlement` `Unknown`
    (`model/mod.rs:34`, `ir_walk.rs:2653`).

  No call or event targets these kinds (a DotNetPackage holds no AL routines), so no join reads them.
  The claim in D2a/D3 that "P3 and P8 agree" is narrowed to the kinds both map the same way:
  Interface, ControlAddIn and every numbered routine-bearing kind.
- An object with an empty or missing name (parse recovery) gets the key `@` and collides with
  another such object (R1-13).
- A table whose number did not parse still collides through `encode_table_id` (R1-13).
- Two same-kind objects with the same name in one app cannot compile. If they did reach the engine,
  their ids would collide as before.

## Upgrade note (CHANGELOG)

A stored baseline or gate snapshot that holds a numberless object id (`…:Interface:0`,
`…:ControlAddIn:0`), a dependency interface routine id, or a #71 declaration-only id does not match
the new ids. It reports those objects as removed and re-added once. Numbered ids are unchanged.

## Measurement plan

- **Goldens.** `scripts/check-goldens --regen`, then `golden-diff-triager`. Expected: the 6 + 6
  numberless-id lines in `tests/r0-goldens` and `tests/r1a-goldens`. Triage separates id
  substitutions from ADDED rows: a map that dropped colliding objects now keeps them, so row counts,
  counters and downstream results can change (R1-04). Each added row must be a previously collapsed
  object. Anything else is unexplained and is not blessed.
- **Events (R1-06).** Before and after, on CDO and DO, count the bound publishers whose graph node
  is `ObjKey::Name`, by kind and app (platform app included), and the event ids that change. Expected
  0 (ControlAddIn events are not subscribable). A non-zero count is triaged: each changed event id
  must change the same way on the publisher side (P2/P8) and the subscriber side (P3) (R2-04).
- **CDO and DO, before and after (R1-05):**
  - numberless `ModelObject` rows, counted two ways: distinct logical identities (app, kind, folded
    name) and distinct ids. Expected: ids < identities before, ids = identities after. Repeated
    physical declaration rows of one logical object are counted once (`program/physical.rs:1-10`);
  - `alsem analyze` findings count, and fingerprint churn (fingerprints that change).

  The probe used is recorded in the ledger.
- **`cdo-gate`** at the branch gate. A CDO-pinned golden that moves is triaged and re-minted through
  the repo's minting path; it is never hand-edited.
