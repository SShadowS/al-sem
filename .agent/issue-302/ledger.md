# Issue #302 — ABI ingestion should read page Editable / ModifyAllowed from a dependency's symbols

## Claim

Run `20261025-211402-5cd486`, attempt 1. Branch `issue/302-agent-discovery-abi-ingestion-pa-a1`,
worktree `/work/al-sem-issue-302-a1`, base `08c58d63` (v2.1.1).
Issue body hash: `b93301341daae1b5b3e54c09f66c56f6efdb07fc4f2eaa4fccd49391ae5d7212`.

## Classification

**Bounded.** One parse step in `src/engine/deps/symbol_reference.rs` (the Page branch already
reads `PageType` / `SourceTable` there), one forwarded field in `projection.rs`, five field
assignments in `abi_rows.rs`, and doc fixes. No serialized schema (`AbiObject`,
`ProjectedObject` and `ModelObject` are not `Serialize`; the ABI cache is in-process,
`abi_ingest.rs:120-128`; the pack codec stores `ObjectNode`, not `AbiObject`), so no cache
version bump. No committed fixture carries these properties, so no golden or ABI vector can
move. The design stays here, in `## Design`.

## Assumption probes

Scratch data: `/tmp/claude-1000/-work-alsem/e2bf58b3-5d1b-467f-8c3b-1c59a8d42f0c/scratchpad/302/`
(below: `SP/`). Binaries: `SP/alsem-base` (base `08c58d63`, release-fast) and
`SP/alsem-proto` (base + the prototype, `SP/proto.diff`). CDO = `<CDO_WS>`
at `bc3ccb1`, clean.

| probe | result | evidence |
|---|---|---|
| Do real symbols carry page `Editable` / `ModifyAllowed`? | **Yes, in object-level `Properties`, as `{"Name":"Editable","Value":"0"}`.** Values are always the strings `"0"` / `"1"`, never `true` / `false`. Base App 28.1 (2,615 pages): Editable `0` 924, `1` 50; ModifyAllowed `0` 275, `1` 21; InsertAllowed `0` 726, `1` 5; DeleteAllowed `0` 632, `1` 16; SourceTableTemporary `0` 2, `1` 312. System App 28.1 (224 pages): Editable `0` 104 / `1` 9; ModifyAllowed `0` 79 / `1` 10. Continia Core 30 (55 pages): Editable `0` 18, ModifyAllowed `0` 6. Continia Delivery Network 30 (86): Editable `0` 26, ModifyAllowed `0` 23. Business Foundation (11): Editable `1` 1, ModifyAllowed `1` 1. No other value form appears in any of the 11 packages. Same Base App package in both `ws-baseapp` and CDO `.alpackages` | `SP/sym/*.json` (extracted `SymbolReference.json`), jq counts (re-counted by the Opus reviewer) |
| Example: the issue's page | `Assembly Orders` (Page 902): `Editable = "0"`, `SourceTable = "900"` | `SP/sym/ba.json` |
| **Trap: the value form** | The native path's rule (`workspace.rs:1109`, `:1116-1117`: `v == "true"`, any other present value → `Some(false)`) would read `"1"` as `false`. The ABI read needs its own rule: `"1"`/`"true"` → `Some(true)`, `"0"`/`"false"` → `Some(false)` (trimmed, any case), anything else or absent → `None`. The two rules differ on purpose (symbols spell booleans as digits); the code doc must say so (design item 1) | `workspace.rs:1105-1130` |
| `SourceTable` form in symbols | Always the table **number** (`"900"`; 2,448 / 2,448 Base App pages; Continia too). `record_types::resolve_table_ref_to_id` already resolves a number, so a pageext's `Rec` over a dependency page gets its table | `record_types.rs:119-126`, `:225-229` |
| How ABI ingestion reads page properties today | `abi_from_sections` Page branch reads only `PageType`, `SourceTable` and `Controls`; `apply_access_properties` reads `Scope` / `InherentPermissions` / `InherentEntitlements` for every object. `AbiObject` → `ProjectedObject` (`project_abi_to_index`) → `ModelObject` (`abi_rows::dep_object_to_l3`), which hard-codes `editable`, `insert_allowed`, `modify_allowed`, `delete_allowed`, `source_table_temporary` to `None` | `symbol_reference.rs:1309-1322`, `projection.rs:357-379`, `abi_rows.rs:42,56-63` |
| **When is a dependency page symbol-only?** | When its `.app` has no embedded `.al` source. `alsem analyze` builds the snapshot in **Embedded mode** (`DependencySource::default()`, production site `resolved.rs:239` → `full.rs:2236-2248` `build_program_with_coverage_profiled`; default defined at `snapshot/snapshot.rs:85-98`); Embedded mode falls back to symbols for an app with no source (`snapshot/provider.rs:123-139`). `append_dependency_rows` builds a source-bearing dependency's objects (every object, demanded or not, `workspace.rs:1943-1968`) from its IR, the native path that already sets the booleans; only a `source.is_none()` unit goes through the ABI rows (`workspace.rs:1861`). **Every required page-bearing CDO dependency has embedded source** (Base App 8,073 `.al`, System App 1,319, System 361, Continia apps 12-542); the one exception, `Microsoft_Application`, has no source and declares no pages. So on pinned CDO the ABI page row is never built, and the issue's `Assembly Orders` reproducer holds only when Base App is symbol-only. (The `alsem` binary has no dependency-source flag; the LSP's `--dependency-source symbols` runs no L5 detector.) | `resolved.rs:231-242`, `full.rs:2236-2248`, `workspace.rs:1838-1968`, zip listing |
| Readers of the page booleans | (1) `page_may_modify` (d39: `Editable` + `ModifyAllowed`). (2) **d64** (opt-in: all four + `source_table_temporary`, over every `Page` in `ws.objects`, dependency rows included). (3) `record_types.rs:449` reads the declaring object's `source_table_temporary` for every routine, ABI dependency routines included (`workspace.rs:2242-2245`). (3) is NOT a no-op for every ABI routine (round 2 correction): an ABI routine has no body, so no record operations and no call sites, but each RECORD parameter becomes a record-variable row (`abi_rows.rs:134-163`, `is_parameter: true`, temp state from the param's `temporary` marker / `var` / by-value), and `record_types.rs:449-464` sets `Known(true)` on every row named `rec`/`xrec` without checking `is_parameter`. So after #302 a procedure of a symbol-only page with `SourceTableTemporary "1"` whose record parameter is named `Rec` or `xRec` gets that parameter marked temporary; before, the flag was `None` and nothing changed. The same rule already applies to source pages (pre-existing, not new behaviour). Measured: no finding moved (`SP/cdo-sym` and `SP/bsym` fingerprint diffs empty, CDO payload identical). d64 is why the design reads all four plus `SourceTableTemporary`, not only the two the issue names (measured below) | `mod.rs:566-583`, `d64.rs:94-157`, `record_types.rs:439-466` |
| Reproducer, symbol-only base page | Fixture `SP/wsym` (generator `SP/scripts/mkfixture.py`): a symbol-only dependency `Dep302` with pages `Dep RO List` (`Editable "0"`), `Dep NoMod Card` (`ModifyAllowed "0"`), `Dep Card` (none), `Dep Editable Card` (`"1"`/`"1"`), `Dep RO Temp List` (`Editable "0"`, `SourceTableTemporary "1"`); the workspace has one pageext per page whose OnAction calls `H.Dirty(Rec)` (a `var` helper that only `Validate`s), plus a workspace `Editable = false` page doing the same (`W RO List`) and a pageext over a workspace read-only temp page (`W RO Temp WsExt`). **Base, symbol-only: d39 reports only `W RO List` and `W RO Temp WsExt`; the pageexts over `Dep RO List` / `Dep NoMod Card` are exempt. Reproduced.** Twin `SP/wemb` (same dependency, `.al` embedded): base reports `W RO Ext`, `W NoMod Ext`, `W RO Temp Ext` as well — the source path already works. Output channels: the symbol-only run lists `Dep302` in `payload.summary.opaqueApps` (`["Dep302"]`) and prints the stderr warning `analysis coverage degraded — 1 symbol-only dependency app(s): Dep302` (`preflight.rs:78`); the embedded twin has `opaqueApps: []`. The top-level envelope `diagnostics` (a sibling of `payload`, not inside it) is `[]` in both | `SP/base-wsym-*.json`, `SP/base-wemb-*.json`, `SP/base-wsym-primary.err` |
| Prototype on the reproducer | `SP/wsym` with `alsem-proto`: the 5 rows of `SP/wemb`, **same fingerprints** (`979cef87…` W RO Ext, `0b9bf686…` W NoMod Ext, `a2422ae9…` W RO Temp Ext), at both scopes. `W Card Ext` and `W Editable Ext` stay silent | `SP/proto-wsym-*.json` |
| Pre-existing FP shape seen on the way | A pageext over a read-only `SourceTableTemporary = true` base page reports d39 (`W RO Temp WsExt` over a WORKSPACE page, base binary; `W RO Temp Ext` over an embedded dependency page, base binary). `record_types`'s SourceTableTemporary override reads the DECLARING object, i.e. the pageext, never its base page (base-page lookup happens separately, `record_types.rs:225-229`), so the pageext's `Rec` is not temp and d39's G-13 skip (`d39.rs:159-165`) misses it. Not caused by #302; the fix extends it to symbol-only pages for parity. A discovery to file (see `## Owed / discoveries`), not fixed here | `record_types.rs:439-466`, fixture rows |

## Population (prototype measure)

All runs: release-fast, default cross-app mode, one analyze at a time, `heavy-ok` checked
before each. "Identical" = the sorted fingerprint lists match; for CDO `--scope all` the
whole `payload` (jq -S) is byte-identical too.

| workspace | scope | base | proto | moved |
|---|---|---|---|---|
| CDO (embedded deps, as pinned) | primary | 2,217 | 2,217 | 0 |
| CDO | all | 30,006 | 30,006 | 0 (payload identical) |
| CDO, every `.app` stripped to symbols (`SP/cdo-sym`) | primary | 2,238 | 2,238 | 0 |
| `SP/cdo-sym` | all | 2,436 | 2,436 | 0 |
| Base App, deps stripped to symbols (`SP/bsym`) | primary | 72,404 | 72,404 | 0 |
| opt-in d64, `SP/cdo-sym` | all | 8 | **0** | −8 |
| opt-in d64, `SP/bsym` | all | 0 | 0 | 0 |
| opt-in d64, CDO embedded (reference) | all | 0 | — | — |

`alsem fingerprint` on CDO (`--format json`): identical except `generatedAt`
(`SP/fp-cdo-{base,proto}.json`, 129,890,105 bytes each, 4 differing bytes, all in the
timestamp line).

Why d39 does not move anywhere real:
- CDO as pinned: every required page-bearing dependency has embedded source, so the ABI page
  row is never built (probe above).
- CDO with symbol-only deps: 41 of CDO's 93 pageexts extend a read-only dependency page
  (`SP/cdo-ro-ext-bases.txt`). Their 38 `Rec` forwards are 36 × `DocOutput.HasRecipients(Rec, …)`
  (a by-value `Variant` reader, `CDOEMailHandler.Codeunit.al:28`) and 2 × `Page.Run(…, Rec)`.
  None dirties `Rec`, so there is no d39 candidate (consistent with #272: "CDO has 0
  implicit-rec d39 candidates even with the exemption off").
- Base App with symbol-only deps: 9 bases of Base App's 156 pageexts are read-only System App
  pages (`SP/ba-sysro-bases.txt`); the one `Rec` forward
  (`BaseAppSecurityGroups.PageExt.al:45`, `SendLicenseConfigurationNotificationOnFirstRecord(Rec)`)
  only reads it (`Copy(…, true)`, no `Validate`).
- Base App single-app: not relevant (its pages are source; `--single-app` loads no dependency).

**d64 −8, every removal checked against the symbols.** Source: the eight pages' object-level `Properties`, extracted from `SP/sym/ba.json` into `SP/d64-eight-pages.json` (2.5 KB, readable) with `jq -c '[.. | objects | .Pages? // empty | .[] | select(.Id|tostring|IN("5395","5448","5460","5461","5462","9166","9970","9971")) | {Id, Name, Properties: ([.Properties[]? | select(.Name|IN("Editable","ModifyAllowed","InsertAllowed","DeleteAllowed","SourceTableTemporary","PageType","SourceTable")) | {(.Name): .Value}] | add)}] | sort_by(.Id)'`; every row below matches that file (re-checked).
Base findings `SP/d64-cdo-sym-base.json`, all "info: API page write surface not
declared"; proto `SP/d64-cdo-sym-proto.json` stats: 8 considered, 0 emitted, skipped
`declaredClosed` 6 + `sourceTableTemporary` 2:

| page | symbols declare (page level) | proto verdict (native rule `d64.rs:110-157`) | base finding |
|---|---|---|---|
| 5395 Dataverse Entity Changes API | DeleteAllowed 0, ModifyAllowed 0, InsertAllowed 1 | declared, `Editable` not false → `declaredClosed` skip | FP |
| 5448 Automation Extension Depl. | Editable 0, Insert 0, Modify 0, Delete 0 | closed → `declaredClosed` | FP |
| 5460 Webhook Supported Resources | Editable 0, SourceTableTemporary 1 | temp → `sourceTableTemporary` skip | FP |
| 5461 API Webhook Logs | Editable 0, Insert 0, Modify 0, Delete 0 | closed → `declaredClosed` | FP |
| 5462 API Routes | Editable 0, SourceTableTemporary 1 | temp → `sourceTableTemporary` skip | FP |
| 9166 Support Contact Info. Entity | DeleteAllowed 0, InsertAllowed 0 | declared, `Editable` not false → `declaredClosed` | FP |
| 9970 Posted Sales Invoice API | Editable 0, Insert 0, Delete 0, Modify 0 | closed → `declaredClosed` | FP |
| 9971 Posted Purchase Invoice API | Editable 0, Insert 0, Delete 0, Modify 0 | closed → `declaredClosed` | FP |

So 8/8 were FPs of the missing read ("nothing declared" while each page declares). The
proto result (0) equals the embedded-source result on pinned CDO (0). The two temp pages are
why `SourceTableTemporary` is read: without it, 5460 and 5462 would turn into "low:
Editable = false but InsertAllowed/ModifyAllowed/DeleteAllowed not disabled". Reading only
Editable + ModifyAllowed would also have kept 9166 as an FP and turned 9970-like pages with
only Editable/Modify into wrong "low" findings.

## Triage

- **d39, CDO primary: 0 new findings. CDO all: 0 new.** Nothing to triage; the 30% rule
  has no population. The same holds on the symbol-only CDO and Base App variants.
- The only new d39 rows anywhere are the fixture's (`W RO Ext`, `W NoMod Ext`, and the
  pre-existing-shape `W RO Temp Ext`), each identical in rule and fingerprint to its
  embedded-source twin.
- d64 (opt-in): 8 removals, 8/8 FP by the symbols (table above). No additions.

## Design

(Revision 3, after spec rounds 1-2: S1-S12, OP1-OP10; register `findings.json`. Ratified.)

**Goal.** A symbol-only dependency page row carries the same write-surface facts as a source
row, so d39's read-only rule (and d64) judge it the same way.

**Field mapping** (page objects only; every other object keeps the default `None`):

| SymbolReference `Properties` name (case-insensitive) | value (trimmed, any case) → | `ModelObject` field |
|---|---|---|
| `Editable` | `"1"`/`"true"` → `Some(true)`, `"0"`/`"false"` → `Some(false)`, any other value or absent → `None` | `editable` |
| `InsertAllowed` | same | `insert_allowed` |
| `ModifyAllowed` | same | `modify_allowed` |
| `DeleteAllowed` | same | `delete_allowed` |
| `SourceTableTemporary` | same | `source_table_temporary` |

**Where it lives** (the prototype, `SP/proto.diff`, +43/−8, plus the doc items):
1. `src/engine/deps/symbol_reference.rs`: `AbiPageProps` (five `Option<bool>`, `Copy`,
   `Default`) as `AbiObject.page_props`; `abi_bool(properties, name)` next to
   `raw_object_property`; filled in the existing `if object_type == "Page"` block. The
   `AbiPageProps` / `abi_bool` doc states that the value rule differs from the native
   `== "true"` rule on purpose (symbols spell `"0"`/`"1"`; unknown → `None`, not `Some(false)`),
   so nobody "unifies" them later (OP3).
2. `src/engine/deps/projection.rs`: `ProjectedObject.page_props`, copied in
   `project_abi_to_index`.
3. `src/program/model/abi_rows.rs`: `dep_object_to_l3` maps the five fields; the "does not
   carry" comment shrinks to `SingleInstance` + decl anchor.
4. `src/engine/l5/detectors/mod.rs`: delete the "Main limit" bullet of
   `implicit_rec_persisted_by_owner` and the "dependency page" clause of `page_may_modify`'s
   doc. Keep "A page that is not in the model counts as modifiable" (still true, and the
   remaining FN path, OP7). Add the temp-base-page FP limit.
5. `src/engine/l5/detectors/d39.rs:36-38` module doc: drop "and a pageext over a read-only
   dependency page (the model has no page properties for dependency pages)" from the known
   false negatives (S4).
6. `src/program/model/workspace.rs` `ModelObject` field docs (S5): `page_type` says dep
   objects use `None`, but `abi_rows.rs:36` forwards it; `source_table_temporary` says it is
   derived from `TableType = Temporary` / "populated by later tasks", but it is the page's
   declared `SourceTableTemporary` property (`workspace.rs:1103-1110`), now also forwarded from
   the symbols. Correct both. Also state on the four write-surface fields that dependency rows
   carry them from the symbols.
7. `CHANGELOG.md` (Fixed): dependency pages known only from symbols now carry
   Editable/InsertAllowed/ModifyAllowed/DeleteAllowed/SourceTableTemporary; d39 reports a
   pageext over a read-only one; opt-in d64 stops reporting their declared write surface.

**ABI vectors and goldens.** None change. The r2-5a vectors (`tests/r2-5a-vectors/`) pin
routine signatures, attributes and stable ids only; no vector holds object properties. None of
the 29 test `.app` files committed before #302 carries any of the five names (scanned by me and,
independently, by the Opus reviewer), and no inline symbol JSON in `tests`/`src`/`benches`/
`crates` did either (#302's own new fixtures do; direct asserts, no golden). Adding a vector family
would be a new schema for one Page branch, so the parse is pinned by a unit test (test 1). Gate:
`scripts/check-goldens` passes with **no** regen (green, see Implementation). The PR body answers
Acceptance "ABI vectors and goldens updated only for the new fields" with: none needed an update,
because no golden or ABI vector that existed before #302 carries the properties (OP6).

**Tests** (each with a hand-stated precondition):
1. Unit, `symbol_reference.rs` `#[cfg(test)]`, **table-driven over the whole value contract**
   (S1). One literal JSON, one page per row, each carrying `Editable` with the value under test
   (and one page with no `Editable`), parsed by `parse_symbol_reference`:

   | value | expected `editable` |
   |---|---|
   | `"1"` | `Some(true)` |
   | `"0"` | `Some(false)` |
   | `"true"` | `Some(true)` |
   | `"false"` | `Some(false)` |
   | `"TRUE"`, `" False "` (case + trim) | `Some(true)`, `Some(false)` |
   | `"maybe"`, `""` (junk) | `None` |
   | absent | `None` |

   Plus, field wiring with PAIRWISE-DISTINCT values (S11): three pages, values (Editable, ModifyAllowed, InsertAllowed, DeleteAllowed, SourceTableTemporary) = `Wire A` (`"0"`,`"0"`,`"0"`,`"1"`,`"1"`), `Wire B` (`"0"`,`"1"`,`"1"`,`"0"`,`"0"`), `Wire C` (`"1"`,`"0"`,`"1"`,`"0"`,`"1"`).
   Read down the pages, each property gets its own pattern (E 001, M 010, I 011, D 100, S 101), so ANY field fed from any other property differs on at least one page (two complementary pages are not enough: Editable and InsertAllowed would match on both). The test asserts all five `page_props` fields of all three pages; a precondition asserts the five patterns are pairwise distinct; property NAMES in mixed case
   (`"modifyallowed"`) are read; a Codeunit carrying `Editable "0"` keeps the default
   `AbiPageProps`. Precondition: each literal value string is asserted present in the JSON
   before parsing.
2. CLI, new `tests/cli/cli_d39_dependency_page.rs` (member of `--test cli`), through
   `run_analyze_with_exit` with `AnalyzeArgs` (the same path as
   `cli_implicit_rec_bindings.rs:36-61`), default cross-app mode, `--scope primary`. A temp
   workspace written with `symbol_app::write_symbol_app`: dependency pages `Dep RO List`
   (`Editable "0"`), `Dep NoMod Card` (`ModifyAllowed "0"`), `Dep Card` (none),
   `Dep Editable Card` (`"1"`/`"1"`); one pageext per page whose OnAction hands `Rec` to a `var`
   helper that only `Validate`s; a control workspace page `W RO List` (`Editable = false`).
   Expected d39 rows, exactly: `W NoMod Ext | OnAction`, `W RO Ext | OnAction`,
   `W RO List | OnAction`. The fixture deliberately leaves out a temp base page, so the test
   does not pin the inherited FP (said in the test doc).
   Hand-stated preconditions: the dependency `.app` has no `.al` entry (read the zip back); the
   symbol JSON contains each property literal exactly once
   (`assert_eq!(symbols.matches(r#"{"Name":"Editable","Value":"0"}"#).count(), 1)`); the run
   really took the ABI path: `payload.summary.opaqueApps == ["Dep302"]` in the stdout JSON, and
   the stderr warning (`AnalyzeOutput.2`) contains `symbol-only dependency app(s): Dep302`
   (S7, OP5).
   A second case writes the SAME dependency with its `.al` embedded (`write_source_app`) and
   asserts `opaqueApps == []`, no stderr symbol-only warning, and the same three rows with the
   same fingerprints as the symbol-only run: parity is the acceptance criterion.
3. CLI, same file, opt-in d64 at `--scope all` over a symbol-only dependency with **three** API
   pages (S7): `Dep API Closed` (Editable/Insert/Modify/Delete all `"0"`) → no finding;
   `Dep API Temp` (`Editable "0"`, `SourceTableTemporary "1"`) → no finding;
   `Dep API Open` (nothing declared) → exactly one `info` finding (proves d64 reaches
   dependency pages at all). Same `opaqueApps` precondition as test 2.

**Planned breaks** (S3: these are planned mutations with predicted outcomes, not results yet.
Each is made with Edit, run, reverted with Edit; both outcomes are recorded when executed.)

| break | mutation | predicted failure |
|---|---|---|
| B1 | `abi_rows.rs` `editable: None` | test 2 loses `W RO Ext` |
| B2 | `abi_rows.rs` `modify_allowed: None` | test 2 loses `W NoMod Ext` |
| B3a | `abi_bool`: drop the `"1"` arm | test 1 row `"1"` → `None` |
| B3b | `abi_bool`: drop the `"0"` arm | test 1 row `"0"`; test 2 loses both dependency rows |
| B3c | `abi_bool`: drop the `"true"` arm | test 1 rows `"true"`, `"TRUE"` |
| B3d | `abi_bool`: drop the `"false"` arm | test 1 rows `"false"`, `" False "` |
| B3e | `abi_bool`: remove `trim` + lower-casing | test 1 rows `"TRUE"`, `" False "` |
| B3f | `abi_bool`: unknown value → `Some(false)` (the native rule's direction) | test 1 rows `"maybe"`, `""` |
| B3g | `abi_bool`: absent property → `Some(false)` | test 1 absent row; test 3 `Dep API Open` loses its finding; test 2 gains a `W Card Ext \| OnAction` d39 row (`Dep Card` declares nothing, so its absent `Editable` becomes `Some(false)` and the page reads as read-only) (OP10) |
| B4a | `abi_rows.rs` `insert_allowed: None` | test 3 reports `Dep API Closed` (low) |
| B4b | `abi_rows.rs` `delete_allowed: None` | test 3 reports `Dep API Closed` (low) |
| B5 | `abi_rows.rs` `source_table_temporary: None` | test 3 reports `Dep API Temp` (low) |
| B6 | `page_may_modify` returns `true` always | test 2 loses all three rows (the control row proves the CLI path reaches the rule) |
| B7 | fill `page_props` for every object, outside the `if object_type == "Page"` block (OP4, S8) | test 1 codeunit assertion |
| B8 | swap one mapping: `insert_allowed: abi_bool(p, "DeleteAllowed")` (S11) | test 1 wiring rows: `insert_allowed` reads `Some(true)` on `Wire A` (expected `Some(false)`) and `Some(false)` on `Wire B` and `Wire C` (expected `Some(true)`) |

**Expected moves (exact fingerprint gate; expected, to be executed at the measure step):**
- CDO pinned, `--scope primary`: 2,217 → 2,217, 0 fingerprints moved.
- CDO pinned, `--scope all`: 30,006 → 30,006, payload byte-identical.
- `alsem fingerprint` on CDO: identical except `generatedAt` (already measured on the
  prototype, see Population).
- Every golden family: 0 changes (`scripts/check-goldens` green without regen).
- Off-gate evidence (scratch workspaces, not committed): symbol-only CDO and Base App d39
  unchanged; opt-in d64 on symbol-only CDO −8, all FP.

**False-positive shapes.**
- *Inherited, pre-existing:* a pageext over a read-only base page with
  `SourceTableTemporary = true` (`record_types` reads the pageext's own flag, not the base
  page's). Today it fires for workspace and embedded-source base pages; after #302 also for
  symbol-only ones (parity). Real population on CDO / Base App: 0 (no candidate). Base App
  has 170 read-only temp pages, System App 36, so it can occur elsewhere. Discovery D1 below.
- *Name collision:* the base page is looked up by name only, last wins
  (last-wins index `symbol_table.rs:44-60`; lookup `object_by_type_name`, `:104`). With two pages of one name in
  different apps, a pageext can be judged against a LESS editable page (FP: reported though its
  real base is modifiable) or a MORE editable one (FN, below). Pre-existing (S9).
- *Unverified:* a page with `Editable = false` that is made editable at run time. Whether
  `CurrPage.Editable` can lift a page-level `Editable = false` for the OnAction write has not
  been checked in AL or on BC; it is listed only as a possibility, not as an established shape
  (S9 / review item 6).

**False-negative shapes.**
- A property value other than `0`/`1`/`true`/`false` → `None`. The page counts as
  modifiable only if the OTHER flag is not `Some(false)` either (`page_may_modify` needs both
  `editable != Some(false)` and `modify_allowed != Some(false)`, `mod.rs:582`). None seen in 11
  real packages (S9).
- Name collision picking a more editable page (above).
- A base page not in the model at all counts as modifiable (`page_may_modify`'s doc, kept).
- View-mode and non-persisting-caller FNs of #272 stay.

**Fallbacks, stated in advance.**
- (a) Any golden moves: triage line by line before any regen; none is expected.
- (b) Any pinned-CDO fingerprint moves: stop and root-cause (the ABI page row is not built on
  pinned CDO, so a move means the change reached something else).
- (c) A new value form appears in other symbols (e.g. `"Yes"`): it maps to `None`, so that
  field keeps today's behaviour; extend `abi_bool` only with a real sample.
- (d) If the panel rules the extra properties (InsertAllowed, DeleteAllowed,
  SourceTableTemporary) out of scope: the intended ruling is to keep all **five**, because
  `ModelObject` has existing consumers (d64, `record_types`) that need one coherent row, and
  reading two of five makes d64 wrong on real pages (5460, 5462, 9166 above). If the panel
  still rules against it, stop and revise the design with the panel; do not ship a partial
  read silently (S10).
- (e) 30% rule: 0 new d39 findings on every real workspace measured, so nothing to gate; if a
  later measure shows new d39 findings, triage them and apply the rule.

## Owed / discoveries

(Status after final round 1. The items this section listed as owed at spec time, the tests,
the breaks and the gates, are done: see `## Implementation`. D1, "pageext `Rec` over a
`SourceTableTemporary = true` base page is not temporary", was fixed in #302 itself by CR1
(`3110c02a`), so it is no longer a discovery; the Design's "Inherited, pre-existing" FP shape
is superseded by it.)

- **D2 (discovery, to file; CR2, deferred):** the base page of a PageExtension is looked up by
  NAME, last-wins (`symbol_table.rs:44-60`, `object_by_type_name`), in `page_may_modify` and in
  `record_types` (pass 3 and the CR1 temp override). Two pages of one name in different apps can
  judge a pageext against the wrong page. Already a stated limit (Design, S9); a cross-app,
  id-based base-page lookup is out of scope for #302.
- **D3 (discovery, to file; CR5, refuted for #302):** ABI ingestion does not read a codeunit's
  `SingleInstance` (`abi_rows.rs` keeps `single_instance: None`), so d57 cannot see it on a
  symbol-only dependency codeunit. Out of scope: #302 is the page booleans.

## Implementation

Rebased onto `origin/master` `21cd0f1d` (grammar v4.5.0) before any work; the prototype
applied cleanly. Plan: `docs/superpowers/plans/2026-10-25-issue-302-abi-page-props.md`.

| task | commit | gates |
|---|---|---|
| 1. parse (`symbol_reference.rs`, `projection.rs`) + test 1 | `ede528bb` | task-affected 4477/4477 pass; check-goldens 1305 pass, no regen, 0 golden moved; check-diff `reasons: []` |
| 2. `abi_rows.rs` mapping + tests 2/3 + fixture + docs + CHANGELOG | `2d31b3b6` | task-affected 4479/4479 pass; check-goldens 1307 pass, no regen, 0 golden moved; check-diff `reasons: []` |

Deviation from the Design, small: the `FIXTURE_ROOTS` tripwire in
`capability_cone.rs` (`("fixtures", 44)` → `46`) failed task-affected because the
fixture adds two workspace dirs (`ws`, `ws-api`) under `tests/fixtures/`. Its own doc says a
new fixture bumps it; #272 did the same. Both oracles (`derived_only_matches_raw_fold_on_fixtures`,
`relation_equals_the_legacy_base_per_field`) pass over the new dirs.

Test 3 uses its own symbol file (`dep-api/`) and workspace (`ws-api/`), same app
identity `Dep302`, so test 2's "each literal once" precondition holds on `dep/`.

Tests: 1 = `symbol_reference::tests::page_props_value_contract` +
`page_props_field_wiring`; 2 = `cli_d39_dependency_page::d39_reads_a_symbol_only_dependency_page`;
3 = `cli_d39_dependency_page::d64_reads_symbol_only_api_page_properties`.
Logs: `SP/proof-<break>.log`.

**Proof table.** "Red before" = the test run before the production code it pins existed
(test 1: `AbiPageProps` present but unfilled → every field `None`; tests 2/3: `abi_rows`
still `None`). Every break made with Edit, grep-checked to apply exactly once, reverted
with Edit; the after-revert runs are `SP/proof-after-revert-t1{,b}.log` and
`SP/proof-after-revert-t2.log` (all green).

| test | red before | break | under break | after revert |
|---|---|---|---|---|
| 1 | red (all `None`) | B3a drop `"1"` arm | red: `V one` → `None` (wiring also red) | green |
| 1 / 2 / 3 | red | B3b drop `"0"` arm | test 1 red: `V zero` → `None`; test 2 red: only `W RO List` (both dep rows lost, as predicted); test 3 red: `Dep API Closed` info | green |
| 1 | red | B3c drop `"true"` arm | red: `V true`, `V TRUE` → `None` | green |
| 1 | red | B3d drop `"false"` arm | red: `V false`, `V spaced False` → `None` | green |
| 1 | red | B3e no trim / lower-case | red: `V TRUE`, `V spaced False` → `None` | green |
| 1 | red | B3f unknown → `Some(false)` | red: `V maybe`, `V empty` → `Some(false)` | green |
| 1 / 2 / 3 | red | B3g absent → `Some(false)` | test 1 red: `V absent` → `Some(false)`; test 2 red: gains `W Card Ext \| OnAction`; test 3 red: `Dep API Open` finding lost (all as predicted) | green |
| 2 | red (only `W RO List`) | B1 `editable: None` | red: loses `W RO Ext` | green |
| 2 | red | B2 `modify_allowed: None` | red: loses `W NoMod Ext` (test 3 also red: `Dep API Closed` low) | green |
| 3 | red (all three API pages info) | B4a `insert_allowed: None` | red: `Dep API Closed` low | green |
| 3 | red | B4b `delete_allowed: None` | red: `Dep API Closed` low | green |
| 3 | red | B5 `source_table_temporary: None` | red: `Dep API Temp` low | green |
| 2 | red | B6 `page_may_modify` always `true` | red: all three rows lost | green |
| 1 | red | B7 fill `page_props` for every object | red: codeunit `Cu` gets `editable: Some(false)` | green |
| 1 | red | B8 `insert_allowed: abi_bool(p, "DeleteAllowed")` | red: `Wire A` `insert_allowed` `Some(true)` (expected `Some(false)`), as predicted | green |

Log locators (F4): the B3b and B3g rows have two logs each. `SP/proof-B3b.log` and
`SP/proof-B3g.log` hold test 1 (unit); `SP/proof-B3b-cli.log` and `SP/proof-B3g-cli.log` hold
tests 2 and 3. B8 (OF4): the wiring assertion stops at the first failing page, so the log
shows `Wire A` only; the predicted `Wire B` / `Wire C` failures are not reached, and do not
need to be for the break to be proven.

### Final round 1 (`3110c02a`, "#302 final r1: CR1, CR4, CR6, OF2, OF3")

- **CR1** `record_types.rs:450-460`: the SourceTableTemporary override reads a PageExtension's
  BASE page flag (`object_by_type_name("Page", extends_target_name)`, as pass 3 and
  `page_may_modify` do). Fixture: `Dep RO Temp List` (`Editable "0"`, `SourceTableTemporary
  "1"`, symbols and `.al` twin) + `W RO Temp Ext`; test 2's expected rows are unchanged, so
  the pageext is silent in both runs. The `mod.rs` limit describing the old FP is removed. D1
  is closed (see Owed).
- **CR3 refuted, no code change.** `ObjectNode.source_table_temporary` is NOT the page's
  `SourceTableTemporary` property: it is the trailing marker of `SourceTable = X temporary`
  (`node_extract.rs:140-143`: "Always `false` when `source_table` is `None`"; set at `:822-835`
  from `singular_property_value(obj, "sourcetable")`; test
  `source_table_trailing_temporary_marker_stripped`). A source page with
  `SourceTableTemporary = true` gets `false` there too. The ABI node has `source_table: None`
  (`abi_ingest.rs:529`), so `false` is the value the native rule gives; feeding it from the
  page property would make the ABI node DISAGREE with the native node, break the stated
  invariant, and change the `def_surface` hash input (`def_surface.rs:228`).
- **CR4** `symbol_reference.rs:559-595`: `abi_bool(value)` + `abi_page_props`, one reversed
  pass over `Properties` (first of a repeated name wins, as in `raw_object_property`),
  `eq_ignore_ascii_case`, no clone.
- **CR6** `cli_d39_dependency_page.rs:72-117`: `symbols()` + `assert_pages` parse the fixture
  with serde_json and compare every page's write-surface properties exactly.
- **OF2** `mod.rs:560`, **OF3** `symbol_reference.rs:295`: wording.
- **CR2 deferred** (D2), **CR5 refuted** (D3): see Owed.

Gates on `3110c02a`: task-affected 4479/4479 pass; check-goldens 1307 pass, no regen, 0 golden
moved; check-diff `reasons: []` (base `21cd0f1d`).

Breaks re-proved on the CR4 code (its shape changed what B3e, B3g, B7, B8 edit), plus B9 for
CR1. Each made with Edit, grep-checked once, reverted with Edit:

| test | break | under break | after revert |
|---|---|---|---|
| 1 | B3e `abi_bool` without `trim` and case-insensitive compare | red: `V TRUE`, `V spaced False` → `None` (`SP/proof-r1-B3e.log`) | green |
| 1 / 2 / 3 | B3g every field starts at `Some(false)` | test 1 red: `V absent` → `Some(false)`; test 2 gains `W Card Ext`; test 3 loses `Dep API Open` (`SP/proof-r1-B3g{,-cli}.log`) | green |
| 1 | B7 `abi_page_props` called for every object | red: codeunit `Cu` `editable: Some(false)` (`SP/proof-r1-B7.log`) | green |
| 1 | B8 the `InsertAllowed` arm matches `"DeleteAllowed"` | red: `Wire A` `insert_allowed: Some(true)`, `delete_allowed: None` (`SP/proof-r1-B8.log`) | green |
| 2 | B9 (CR1) the PageExtension arm never matches (own flag read) | red: symbol-only run gains `W RO Temp Ext \| OnAction` (`SP/proof-B9-cr1.log`; the test stops there, before the embedded twin) | green |

After-revert logs: `SP/proof-r1-after-revert-t1.log`, `SP/proof-r1-after-revert-t2.log`.
Not re-run: B3a-B3d, B3f (they delete one value arm of `abi_bool`, the same edit on the new
code); B1, B2, B4a, B4b, B5, B6 (`abi_rows.rs` / `mod.rs` code unchanged).

**Final-head fingerprint gate (F1/OF1): 0 moves.** Base `21cd0f1d`, head `3110c02a`, both
release-fast (`SP/gate/{before,after}`), `cdo-measure` `measure.py run` one side at a time,
`heavy-ok` checked before each build and run. CDO `<CDO_WS>` at
`bc3ccb18`, 0 dirty paths, on both sides.

| run | base | head | moved |
|---|---|---|---|
| `analyze --scope primary` | 2,217 | 2,217 | 0 (0 added, 0 removed, 2,210 distinct kept); payload (`jq -S .payload`) byte-identical |
| `analyze --scope all` | 30,006 | 30,006 | 0 (0 added, 0 removed, 29,978 distinct kept); payload byte-identical |
| `fingerprint` | — | — | identical except `generatedAt` |

No CR1 row moved on CDO, so there is nothing to triage. `comp` output: `SP/gate/comp.txt`.

### Final round 2 (`729b3d19`, "#302 final r2: F5, F6, OF5, OF6, OF7")

- **F5** `record_types.rs:460-488`: the page `SourceTableTemporary` override now touches only
  the IMPLICIT `Rec` / `xRec`, for pages and pageextensions alike. The variable half marks only
  the row with `is_implicit_rec`; the op half marks a `rec` / `xrec` op only when no declared
  (non-implicit) row of that name exists. The marker is new, as no existing field told a page's
  seeded `Rec` from a declared local `Rec`: they share the name, the id (`<routine>/rv/rec`)
  and `scope: None`, and `is_receiver` is Table-only and read by d37/d11.
  `ModelRecordVariable.is_implicit_rec` is set at L3 assembly (`workspace.rs`: a `Rec` row that
  no param or local of the IR routine declares; a shadowing global suppresses the seed and is
  promoted with `false`). It lives on the L3 row only: adding it to `PRecordVariable` would have
  moved `l2_features.snapshot` (it digests `Debug`). Argument bindings were already
  shadow-aware (`ir_walk.rs:1890-1892`, `source_kind == "implicit-rec"`).
- Fixture: `W RO Temp Local` (a workspace temporary page) and `W RO Temp LocalExt` (a pageext
  over `Dep RO Temp List`), each with an OnAction that declares a physical `Rec` (d39 reports)
  and a `ReadLocal` procedure with a declared `Rec`: `FindSet` + `Name` read (d3 reports; d3
  skips a temporary record, so it reads that variable's own temp state). `Dep Cust` gained a third
  field, `City` (symbols and `.al`), so d3 has an unread field. Test 2 runs `d39,d3` and checks
  both row sets in the symbol-only run and their parity in the embedded twin.
- **OF7**: the workspace temporary page `W RO Temp Base` and its pageext `W RO Temp WsExt`, both
  silent (the workspace base-page case of CR1).
- First fixture attempt, recorded because a green break is evidence about the test: with the
  local `Rec` only in the d39 OnAction, B10 came back GREEN (`SP/proof-B10-f5.log`). d39's G-13
  reads the binding's `source_temp_state`, which this override never touches for a declared
  local, so d39 cannot see F5. The d3 `ReadLocal` rows were added because of that. Base binary on
  the scratch copy: d3 reports `W RO Temp LocalExt | ReadLocal` only (the page arm already had
  the name-only flaw); `3110c02a`: neither; `729b3d19`: both.
- Unit test `record_types::tests::page_temp_override_spares_a_declared_rec` (hand-stated rows:
  the same `Rec` row with and without `is_implicit_rec`, ops on `Rec` and `xRec`).
- **OF5 / F6**: the CHANGELOG has a separate Model entry, which names the readers that move
  and corrects the closing justification.

Gates on `729b3d19`: task-affected 4480/4480 pass; check-goldens 1307 pass, no regen, 0 golden
moved; check-diff `reasons: []`.

| test | break | under break | after revert |
|---|---|---|---|
| 2 | B10: the op guard (`declared` always false) and the variable half (name-only) both broken | red: d3 rows `[]`, expected `W RO Temp Local \| ReadLocal`, `W RO Temp LocalExt \| ReadLocal` (`SP/proof-B10-f5b.log`) | green |
| unit | B10b: op guard only | red: `is_implicit_rec = false` gives `(false, true, true)`, expected `(false, false, true)` (`SP/proof-B10b-unit.log`). Test 2 stays GREEN under B10b (`SP/proof-B10b-ops.log`): no CLI reader here reads op temp state, so the unit test is what pins the op half | green |
| unit / 2 | B10c: variable half only (name-only) | unit red: `(true, false, true)`; test 2 red: d3 rows `[]` (`SP/proof-B10c-{unit,cli}.log`) | green |
| 2 | B11: L3 marker ignores `declares_rec` | red: d3 rows `[]` (`SP/proof-B11-marker.log`) | green |
| 1 | OF6, B3a: drop `v == "1"` | red: `V one` → `None` (`SP/proof-r2-B3a.log`) | green |
| 1 | OF6, B3b: drop `v == "0"` | red: `V zero` → `None` (`SP/proof-r2-B3b.log`) | green |
| 1 | OF6, B3c: drop `eq_ignore_ascii_case("true")` | red: `V true`, `V TRUE` → `None` (`SP/proof-r2-B3c.log`) | green |
| 1 | OF6, B3d: drop `eq_ignore_ascii_case("false")` | red: `V false`, `V spaced False` → `None` (`SP/proof-r2-B3d.log`) | green |
| 1 | OF6, B3f: unknown → `Some(false)` | red: `V maybe`, `V empty` → `Some(false)` (`SP/proof-r2-B3f.log`) | green |

After revert: `SP/proof-r2-after-revert-t1.log`; tests 2/3 green in the task-affected run.
With OF6, every break from B1 to B11 has now run against the final code of the part it targets.

**CDO fingerprint gate on `729b3d19`: 0 moves.** Base `21cd0f1d` (the same binaries as round
1, `SP/gate/before`), head release-fast `SP/gate/after2`, `heavy-ok` checked before the build
and the run, CDO `bc3ccb18`, 0 dirty paths. Primary 2,217 → 2,217 and all 30,006 → 30,006;
fingerprint lists identical (0 lines differ) and `jq -S .payload` byte-identical at both scopes
(`c7d235c9…` / `199d0c5a…`, the same hashes as round 1); `fingerprint` output identical except
`generatedAt`. Nothing to triage; the CHANGELOG numbers stand.

### Final round 3 (`61d5286d`, "#302 final r3: OF8 (CHANGELOG)")

- **OF8** (CHANGELOG only): the pinned-CDO measurement note is now the closing paragraph of the
  second #302 Fixed entry, not a third top-level bullet. No gate: the change is CHANGELOG-only.
- **OF10, refuted: alc 18.0.41.62505 compiles the F5 fixture shape.** The fixture's own `.al` files
  were compiled unchanged with `/opt/al-tool/al compile` (the tool #299's `alcshadow` probe used),
  in `SP/alc/`. First the dependency (`dep/src/Dep.al`, app `Dep302`, platform 28.0, runtime 17.0,
  System 28.0 symbols), then the workspace (`ws/src/Ws.al`) against the resulting `Dep302.app`.
  Both compile with exit 0 and no diagnostics (`SP/alc/dep-compile.log`, `SP/alc/ws-compile.log`).
  So a page (`W RO Temp Local`) and a pageextension (`W RO Temp LocalExt`) may declare a trigger
  local and a procedure local named `Rec: Record "Dep Cust"`. Negative control: the same
  workspace with the pageext's local `Rec` typed to `Record "No Such Table"` fails exactly there,
  `Ws.al(128,26): error AL0185: Table 'No Such Table' is missing`, exit 1
  (`SP/alc/wsbad-compile.log`). That shows the compiler checks that declaration and accepts it
  when the table exists. alc is Microsoft's compiler, so this is a compile-time fact about AL, not
  a BC runtime claim.
- **OF9, deferred → D4 (discovery, to file):** `workspace.rs`'s `declares_rec` (L3, for
  `is_implicit_rec`) repeats the param/local half of `ir_walk::shadows_implicit_rec` (L2, which
  decides whether the implicit `Rec` is seeded). If the L2 rule changes (for example a named return
  value `Rec`, which the receiver code already treats specially), the seed and the flag can
  disagree with nothing to catch it. Fix: one shared predicate both layers call. No code change
  in #302.

## Timeline

- 2026-10-25 21:14 claim; 21:30 probes, prototype and measure done; design written.
- 2026-10-25 spec round 1: Opus APPROVE (OP1-OP7 minor), sol DO NOT APPROVE (S1, S2
  important; S3-S10 minor). Design revision 2; all 8 d64 removals read against the symbols from
  existing outputs (no new heavy run). Register `findings.json`.
- 2026-10-25 spec round 2: Opus APPROVE (OP8-OP10 minor), sol DO NOT APPROVE (S2, OP2
  re-raised; S11 important, S12 minor). Revision 3: S2 table cites `SP/d64-eight-pages.json`;
  OP2 reader corrected; distinct-value wiring test + B8; envelope `diagnostics`; citations;
  B3g prediction.
- 2026-10-25 implementation: `ede528bb` (task 1), `2d31b3b6` (task 2).
- 2026-10-25 final round 1: sol fix-first (F1 important, F2-F4 minor), Opus ship (OF1-OF4
  minor), code-review CR1 important, CR2-CR6 minor. `3110c02a` (CR1, CR4, CR6, OF2, OF3),
  `40999e33` (CHANGELOG: F1, F2, OF1). CR2 deferred (D2), CR3 refuted, CR5 refuted (D3).
  Final-head CDO gate on `3110c02a`: 0 moves at both scopes. OP6 marks reset (its Owed anchor
  changed).
- 2026-10-25 final round 2: Opus ship (OF5-OF7 minor), sol fix-first (F5 important, F6
  minor); both accepted all 15 open ids. `729b3d19` (F5, F6, OF5, OF6, OF7). CDO gate on
  `729b3d19`: 0 moves at both scopes. Marks reset: F1, F2, OF1 (CHANGELOG restructured);
  CR6 re-anchored (L72-L117 → L76-L121, same hash, marks kept).
- 2026-10-25 final round 3: both ship; sol accepted all 8 ids; Opus OF8-OF10 (minor).
  `61d5286d` (OF8, CHANGELOG only). OF9 deferred (D4), OF10 refuted (alc compiles the fixture
  shape). Marks reset: F1, OF1, F2, F6 (their CHANGELOG lines moved).
- 2026-10-25 final round 4: both ship; F1, OF1, F2, F6, OF9, OF10 accepted by both; OF8 closed
  raiser-only. Register: 44 entries, all accepted.
- Rebased onto 6f1e2718 (#287; CHANGELOG conflict, kept both entries), then onto 26d1428c
  (#340, no conflict). The code commits are unchanged; OF8's fix_commit is now the rebased
  9beb3faf. Gates on H = 9beb3faf (B = 26d1428c): check-goldens-coverage 0, ci-steps-all 0,
  check-goldens 0, cdo-gate 0.
