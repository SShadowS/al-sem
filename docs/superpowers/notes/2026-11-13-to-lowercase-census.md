# `str::to_lowercase()` in the engine: census (TLS-42)

The OUTSTANDING item "`str::to_lowercase()` census in the advisory engine" asked whether the
engine's `to_lowercase()` sites should move to the project's identifier fold,
`fold_identifier` / `eq_fold_identifier` (`crates/al-syntax/src/casing.rs`). This note
classifies every site, measures where the folds differ and on which inputs, and sizes the
work. It is read-only: nothing was migrated.

**Result:**
- `to_lowercase()` against `fold_identifier`: they differ on two inputs. No input the engine
  reads today contains either, so migrating these sites changes nothing today.
- **The real gap is a third fold.** The L2 builder (`src/program/body/ir_walk.rs`) folds
  identifiers with `to_ascii_lowercase()`, and about 30 sites in L5, the model and the impact
  graph compare those values with `to_lowercase()` ones. These two differ on every non-ASCII
  capital letter (`Æ`, `Ø`, `Å`, `Ä`, `Ö`, `É` and so on). AL allows such identifiers, and
  Nordic BC code uses them. CDO happens to have none, so the population is empty here but not
  in general.

## Where the folds differ (measured)

A probe compared `str::to_lowercase()` with `fold_identifier`'s non-ASCII path
(`c.to_lowercase().next()` per char) on every Unicode scalar value, alone, at a word's end
(`A{c}`) and mid-word (`A{c}B`). Exactly two inputs differ:

| input | `to_lowercase()` | `fold_identifier` |
|---|---|---|
| `İ` (U+0130) anywhere | `i` + U+0307 (two chars) | `i` |
| `Σ` (U+03A3) at a word's end | `ς` (final sigma) | `σ` |

`casing.rs`'s module doc names only `İ`. Rust's `str::to_lowercase` maps a word-final capital
sigma to `ς` (context-sensitive), while `char::to_lowercase` gives `σ`. That doc needs one
line for it (done in this change).

`to_ascii_lowercase()` differs from both on every non-ASCII capital: it leaves `Æ` as `Æ`,
while the other two give `æ`.

## Does a differing input exist today? (measured)

| input | what was scanned | `İ` | `Σ` | any non-ASCII capital in an identifier |
|---|---|---:|---:|---:|
| CDO source | all 741 `.al` files under `/work/src/cdo-baseline` (Cloud, Test, FR, OnPrem, DemoApp, TestFR) | 0 files | 0 files | 0 (outside strings and comments, the only non-ASCII characters are 98 byte-order marks and 3 em dashes) |
| dependency symbols | `SymbolReference.json` of all 11 packages (all UTF-8 with a BOM) | 0 | 0 | 0. The two non-ASCII names are `Export Opsætning` (Continia Delivery Network, lowercase `æ`) and `I don’t wish to answer` (Base Application, a typographic apostrophe); all three folds treat both identically |
| repo fixtures | every `.al` under `tests/`, `crates/`, `src/` | 0 | 0 | 0 |
| the grammar's own test corpus | `/work/src/tree-sitter-al` | 0 | 0 | 4 (`Æble`, `Ørn` and two quoted names): AL allows such identifiers |

Inside the packages, `İ` and `Σ` occur only in binary assets (png, gif, docx, xlsx, xltm,
nested zip) and in Microsoft_System's `.lcl` translation files. The engine does not read either
as identifiers.

So on every input we hold, no two of the three folds give different results. The census cannot
show a live miss; it shows where one would occur.

## The 329 production sites

`src` holds 376 `to_lowercase()` calls: 329 in production code and 47 in tests. The backlog's
~364 predates L3's deletion; `src/engine/l2` and `l3` now hold none. Three read-only agents
read every production site in context and assigned one category each. Their table is
`docs/superpowers/notes/to-lowercase-census/sites.tsv` (`path:line`, category, note). I
checked their claims behind the findings below in the source (`summary_runner.rs:778/912`,
`ids.rs:240-246`, `d19.rs:90` against `ir_walk.rs:1275`, `detectors/mod.rs:828`,
`workspace.rs:2181-2205` against `ir_walk.rs:2879-2922`).

| category | sites | what it is | a fold change would… |
|---|---:|---|---|
| `ident-self` | 141 | an identifier lowercased with `to_lowercase` on every side it meets | change nothing on its own (both sides move together) |
| `type-or-keyword-text` | 51 | type text, property values, attribute names | change nothing on real input (keywords are ASCII) |
| `identity` | 37 | goes into an id, fingerprint, key or golden-visible text (`engine/ids.rs`, `impact/keys.rs`, d18/d57/d60/d61/d65 root-cause keys, record-variable ids, `encode_event_id`, `telemetry/hash.rs`) | move ids and fingerprints for names holding `İ` or a final `Σ`. There are none today, but a migration must be read as an id-schema change |
| `ident-vs-literal` | 37 | an identifier compared with an ASCII literal (`"session"`, `"logmessage"`) | change nothing on real input |
| `ident-vs-fold` | 31 | an identifier compared with a value built by ANOTHER fold. Almost all meet `ir_walk.rs`'s `to_ascii_lowercase`; a few meet `fold_identifier` (`detectors/mod.rs:998` into `eq_fold_identifier`, `entries.rs:125/168`, `cone_probe.rs:201`, `workspace.rs:1097` and `extension_fields.rs:104` against key members from `lower/mod.rs:849`) | **fix a real class of misses** (below) |
| `path-or-file` | 17 | file names, extensions, CLI paths | nothing for identifiers; a path fold is its own question |
| `user-text` | 10 | selectors and config names a user types | follow the identifier fold they are matched against |
| `other` | 5 | comment hits and one value with no reader (`event_flow.rs:929`) | nothing |

## The real gap: the L2 builder's ASCII fold

`ir_walk.rs` has 63 production `to_ascii_lowercase()` calls. Several turn identifiers into the
features later layers match on:
- `PCallArgumentBinding.source_variable_name` (2879-2922);
- `PConditionReference.identifier` (784, 797, 2420);
- `identifier_references` (`identifier_ref_set`, 1275, 1522, 1620);
- `PVariableSymbol.name` (3630);
- `var_assignments` `lhs_name` and `rhs_identifier`;
- the loop's `exhausting_advance` variable (906).

The other side of each comparison uses `to_lowercase()`. For a name with a non-ASCII capital
the two never match. Examples (from the agents' table, the first three checked in the source):
- **d19** (`d19.rs:90`): a parameter's `to_lowercase` name is probed in the ASCII-folded
  `identifier_references`. A used parameter named `Ændring` would be reported unreferenced, a
  false positive.
- **The shared suppressions** `record_loaded_by_call_before` / `record_filtered_by_call_before`
  (`detectors/mod.rs:828`, `:1104`, used by d11, d21, d33, d36 and others) compare
  `to_lowercase` names with `source_variable_name`. The by-var-argument suppression fails, so
  findings appear that should be suppressed.
- **Global record-variable backfill** (`workspace.rs:2181-2207`): `global_rv_by_lc` is keyed by
  `to_lowercase`, but probed with the ASCII-folded `source_variable_name`. A global record
  passed as an argument stays `local`.
- **d56** compares `to_lowercase` record names with the ASCII-folded `var_assignments` (six
  sites). **d5/d60** compare the loop driver with `exhausting_advance`.
  `control_context.rs:104/112/525` (the IsHandled guard eligibility) and `literal_values.rs:237`
  / `cross_rules.rs:786` do the same.

None of this fires on CDO (no non-ASCII identifiers). It would fire on code with Danish,
Norwegian, Swedish, German or French identifiers.

## Recommendation and size

1. **Fix the third fold at its source (recommended, ~half a day).** Make `ir_walk.rs`'s
   identifier folds `fold_identifier` (the identifier subset of the 63 sites, not type text),
   so the L2 features use the project fold. The L5/model/impact sides that are `to_lowercase`
   then agree on everything except `İ` and a final `Σ`, which no input holds. Move the sites
   that compare with fold-built keys (`detectors/mod.rs:998`, `entries.rs`, `cone_probe.rs`,
   `workspace.rs:1097`, `extension_fields.rs:104`) to `fold_identifier` / `eq_fold_identifier`
   in the same change. Witness: an r0-corpus fixture with Nordic identifiers (a used parameter
   `Ændring` for d19, a record variable `Ørder` passed by var for the shared suppression, a
   global `Ålcust` for the backfill). Each has a discrimination proof: the old ASCII fold makes
   its assertion fail. Goldens move only for that fixture (the corpus is ASCII today), and
   `ws-unicode-fold` should be checked.
2. **Leave the `identity` sites (37) alone unless an id-schema change is due.** Moving them
   changes ids only for `İ` and a final `Σ` (none today). If they ever move, do it in one
   licensed change with `id-shape-auditor`, like any id change.
3. **Do not migrate the rest for consistency alone.** `ident-self`, `ident-vs-literal`,
   type text and paths (256 sites) gain nothing measurable. If item 1 lands, a later sweep can
   take them file by file, but this census shows no reason to.
4. **`casing.rs` doc:** name the final-`Σ` case beside `İ`. Done here, as a doc-only change.

## Method and limits

- Classification is by three agents (sonnet, read-only) reading each site in context, with
  the categories fixed in advance. Their claims for the findings above were checked in the
  source. The other rows are their reading and may hold a few misbuckets between `ident-self`
  and `type-or-keyword-text`. That does not change the recommendation, which rests on the
  checked rows.
- `to_ascii_lowercase()` (364 calls in `src`) was not classified; only the ones that meet
  `to_lowercase` sites are named above.
- "Today" means the pinned CDO baseline, its 11 packages and this repo. No other customer code
  was available on this host.
