# Issue #80 — an attribute separated from its routine by a comment, pragma or `#if` line is dropped

## Claim

```json
{"run_id":"20261015-101711-938ef5","issue":80,"attempt":2,"session":"claude-code-session:cc1781f3-fe4e-414a-aea6-4919dc8d314c","branch":"issue/80-agent-discovery-al-syntax-lower-a2","worktree":"U:\\Git\\al-sem-issue-80-a2","body_hash":"baf400e0aa27a05e900b1c8001d9f4fe9c8d19e8b6e341f9dc986d126fcab0a4"}
```

Issue body hash `baf400e0…a0a4` is the same as attempt 1's, so the issue has not changed.

**Attempt 2 carries attempt 1's work forward.** Attempt 1 (run `20261014-024629-4a5a99`, worktree
`U:/Git/al-sem-issue-80-a1`) stopped on the 4 h wall-clock cap before spec round 2, with no
source change and no commit. Its claim did not hand off, so every step runs again here. Its probes,
design and 10-entry register are the starting point. Each was re-checked against the new base, as
recorded below.

Worktree created from `master` @ `1b6f1c3886f1eecbf70791b3d32c7e5e4c117c0a`. Attempt 1's base was
`66b622887a3216d813655af7a382a8ddc016981d`. Between the two, the lowerer changed in one commit
(`049b166c`, #71): `AttributeIr` gained an `origin` field (`lower/mod.rs:1456`), and two tests
were added at the end of the `tests` module. No line cited below `:1453` moved. Grammar:
`U:/Git/al-call-hierarchy/tree-sitter-al` at `8682cb5f` (the committed `node-types.sha256`, same as
attempt 1), used through `TREE_SITTER_AL_PATH` for every cargo command. tree-sitter CLI 0.27.0.
The grammar did not move, so the CST probes (P2-P16, P21b) still hold.

`$SCRATCH` below means
`C:/Users/SShadowS/AppData/Local/Temp/claude/U--Git-al-call-hierarchy/cc1781f3-fe4e-414a-aea6-4919dc8d314c/scratchpad/80`
(a copy of attempt 1's scratch directory). Attempt 1's measurement outputs moved to `$SCRATCH/a1/`.
The files without that prefix were re-measured on the new base.

**Artifact manifest** (every file in `$SCRATCH` this ledger relies on; R1-09):

| file | what it is |
|---|---|
| `s01_line_comment.al` … `s16_r3_shapes.al` | probe inputs (16 files: s01-s16; added in attempt 2: s14 (A18), s15 (A19 `P`/`H`, A20 `H2`, A21 `P3`/`Q3`), s16 (A22 `P`/`Q`, A23 `P2`)) |
| `s01_line_comment.cst.txt` … `s16_r3_shapes.cst.txt` | full `tree-sitter parse --no-ranges` output of each input, all exit 0, 0 `ERROR`/`MISSING` |
| `census.py` | the line-heuristic census script (P19) |
| `census.txt`, `census-r1.txt` | census output before / after adding `#define`/`#undef` as separators (identical site lists) |
| `repro-with/`, `repro-without/`, `repro-arms/` | reproducer workspaces (`app.json` + `src/Pub.Codeunit.al`) |
| `digest-with.json`, `digest-without.json` | `alsem digest --deterministic` outputs, re-measured on `1b6f1c38` (P20c) |
| `a1/digest-arms-FireA.json`, `a1/digest-arms-FireB.json`, `a1/digest-arms-FireC.json` | attempt 1's `alsem digest` outputs for `repro-arms` (P9b, P17), measured on `66b62288`; also `a1/digest-with.json`, `a1/digest-without.json` |
| `cdo-stats-base.json`, `cdo-depbodies-base.json` | base `aldump --program-call-graph-stats` / `--dependency-bodies-stats` on CDO (`1b6f1c38`; `a1/` holds attempt 1's, byte-identical) |
| `analyze-base-primary.json`, `analyze-base-all.json` | base `alsem analyze --format json --deterministic` on CDO, `--scope primary` / `--scope all` (cross-app default), on `1b6f1c38`; `a1/` holds attempt 1's (P20c says what moved) |
| `build-alsem.log`, `build-aldump.log` | the worktree builds (release-fast `alsem`, `cdo` `aldump`); `a1/` holds attempt 1's |
| `base.sh`, `base.out` | attempt 2's base build-and-measure script and its exit codes |
| `a1/spec-briefing-r1.md`, `a1/spec-briefing-r2.md`, `a1/spec-r1-astra.md`, `a1/spec-r1-flash.md` | attempt 1's panel briefings and round-1 reviews |

## Classification

**BOUNDED, one module.** Every attribute-to-routine pairing in the lowerer is made in
`collect_routines` (`crates/al-syntax/src/lower/mod.rs:804-1010`); the only other read of
`attribute_item` is the in-node filter for split headers (`:1394-1399`), which is not
order-sensitive (P15). The fix touches four places in that one file: `collect_routines`
(pairing), `preproc_branches` (`:1156-1197`, refactored so the chain logic has one source),
the per-arm loop in `lower_object` (`:229-247`), and attribute-argument lowering in
`lower_routine` (`:1409-1457`). No IR type changes (`AttributeIr`, `RoutineDecl` unchanged), no
cache stamp (P18), no consumer changes. It does change what consumers see on real code (44
routines on CDO regain attributes by the P19 estimate), so CDO numbers may move; they are
measured before and after, not assumed.

## Assumption probes

| probe | result | evidence |
|---|---|---|
| P1 — the reproducer reproduces on `master` | **HOLDS** | release-fast `alsem` built in the worktree from `66b62288`. `$SCRATCH/repro-with` (`[EventSubscriber]` line, then `// note`, then `local procedure Handler()` calling `Commit()`) and `$SCRATCH/repro-without` (same, no comment line). `alsem digest <ws> --routine Fire --deterministic`: **with** comment → `Fire ['EVENT_PUBLISH']`; **without** → `Fire ['COMMIT', 'EVENT_PUBLISH']` |
| P2 — line comment: CST sibling sequence | `attribute_item, comment, procedure` | `s01_line_comment.cst.txt`. All are direct children of `declaration_body`. Today the `comment` hits the `_` arm (`:984-986`) → `pending.clear()` |
| P3 — block comment, one-line and multi-line | `attribute_item, multiline_comment, procedure` (both) | `s02_block_comment.cst.txt`: `/* note */` and a 4-line `/* … */` are each ONE `multiline_comment` node |
| P4 — `#pragma`, `#region`/`#endregion`, blank line | `attribute_item, pragma, procedure`; `attribute_item, preproc_region, procedure, preproc_endregion`; blank line → **no node** | `s03_pragma_region_blank.cst.txt`. A blank line is whitespace, so it is not a bug today |
| P5 — two attributes split by a comment | `attribute_item, comment, attribute_item, procedure` | `s03` (`TwoAttrs`). Today the first attribute is lost, the second kept |
| P6 — `#if` around attributes only | `preproc_conditional{preproc_if, attribute_item, preproc_endif}, procedure` — the procedure is a SIBLING after the wrapper | `s04_if_attrs_only.cst.txt` (`OnlyAttrs`; `MixedBefore` = `attribute_item, preproc_conditional{…}, procedure`). Today the wrapper arm (`:966-967`) clears `pending`, and the recursion into the arm collects the attribute into its own local `pending`, discarded at the end of the arm |
| P7 — `#if/#else` (and `#elif`) with a different attribute per arm | `preproc_conditional{preproc_if, attribute_item, preproc_else, attribute_item, preproc_endif}, procedure`; `#elif` adds `preproc_elif, attribute_item` | `s04` (`PerArm`), `s06_more_preproc.cst.txt` (`Elif`) |
| P8 — nested `#if`; trivia inside an arm; `#if` holding only pragmas; empty `#if` | nested: `preproc_conditional{preproc_if, preproc_conditional{preproc_if, attribute_item, preproc_endif}, preproc_endif}`; trivia in arm: `preproc_conditional{preproc_if, comment, attribute_item, multiline_comment, preproc_endif}`; pragma-only: `attribute_item, preproc_conditional{preproc_if, pragma, preproc_endif}, procedure` (a plain `preproc_conditional`, NOT `preproc_pragma_only`); with `#else`: `preproc_conditional{preproc_if, pragma, preproc_else, pragma, preproc_endif}`; empty: `attribute_item, preproc_conditional{preproc_if, preproc_endif}, procedure` | `s06` (`Nested`, `TriviaInArm`, `PragmaInIf`, `EmptyIf`), `s11_test_shapes.cst.txt` (`PragmaElse`). `preproc_pragma_only` (`grammar.js:5040`) occurs only at `source_file` level (`:1003`), between a field header and its body (`:3011`) and in a procedure tail (`:4429`), never as a body sibling |
| P9 — `#if` wrapping attribute AND procedure | `preproc_conditional{preproc_if, attribute_item, procedure, preproc_endif}` | `s04` (`Wrapped`). Already paired today. Not a bug |
| P9b — attribute BEFORE an `#if` that wraps the procedure | `attribute_item, preproc_conditional{preproc_if, procedure, preproc_else, procedure, preproc_endif}` | `s06` (`AttrThenIfProc`). **Same bug** (the issue title's "#if line"): `$SCRATCH/repro-arms`, `FireC` → `['EVENT_PUBLISH']` only (`a1/digest-arms-FireC.json`) |
| P9c — two `#if X` wrappers in a row after an attribute (R1-02) | `attribute_item, preproc_conditional{preproc_if, procedure P, preproc_endif}, preproc_conditional{preproc_if, procedure R, preproc_endif}, procedure Q` | `s12_r1_shapes.cst.txt`. In an `X` build `A` belongs to `P`; in a not-`X` build to `Q`; `R` never has it. A context-free join (round-1 design) attached `A` to `R` |
| P9d — an attribute with an `#if` inside its arguments, itself inside an `#if not X` arm, before the routine (R1-01) | `preproc_conditional{preproc_if, attribute_item{… attribute_argument_list{…, preproc_conditional_attribute_args{preproc_if, string_literal 'OnA', preproc_else, string_literal 'OnB', preproc_endif}, string_literal, …}}, preproc_endif}, procedure H` | `s12_r1_shapes.cst.txt`. The only build where `H` has the attribute is not-`X`, where the event is `'OnB'`. Read with `H`'s own context (`[]`, `X` undecided) `conditional_list_arm` (`:1215-1218`) takes the first live arm, `'OnA'` — a subscription no build has |
| P9e — an attribute-only `#if X` followed by a split header `#if X P(I) #else P(T) #endif begin…end` | `preproc_conditional{preproc_if, attribute_item, preproc_endif}, preproc_split_procedure{preproc_if, …, preproc_else, …, preproc_endif, …}` | `s13_split_after_if.cst.txt`. The split header lowers to one routine per arm (`header_arms`, `:1079`); the attribute belongs to the `X` arm only |
| P10 — attribute, comment, TRIGGER; `///`; same-line trailing comment; `#define`/`#undef` | `attribute_item, comment, trigger_declaration`; `///` is a plain `comment`; `[Scope('OnPrem')] // trailing` → `attribute_item, comment, procedure`; `attribute_item, preproc_define, preproc_undef, procedure` | `s05_trigger_dangling_multiline.cst.txt`, `s06` (`DocComment`), `s10_define_and_misc.cst.txt`. `trigger_declaration` is in the pairing arm (`:825`) |
| P11 — attribute at the end of a body with no routine after it | `procedure, attribute_item` closes `declaration_body`; a following comment sits OUTSIDE it, under `codeunit_declaration` | `s05`, `s09_dangling.cst.txt`. In a table, `fields{fields_body{field_declaration, attribute_item}}, procedure TP` (`s12`): a dangling attribute at the end of a nested section, followed by a routine in the SAME object |
| P12 — multi-line attribute | ONE `attribute_item` across 3 lines | `s05` (`Multi`). The bug hits it exactly as a one-line attribute when a comment/pragma follows |
| P13 — every node kind that can sit between an attribute and its routine | the grammar's `extras`: `comment, multiline_comment, pragma, preproc_region, preproc_endregion, preproc_define, preproc_undef` (`grammar.js:539-547`); every probe produced only those, `attribute_item`, or a `preproc_conditional` wrapper | `kind_policy.rs`: `Comment`/`MultilineComment` are `Class::Trivia`; `Pragma` (`:404`), `PreprocDefine` (`:433`), `PreprocEndregion` (`:437`), `PreprocRegion` (`:446`), `PreprocUndef` are `Class::Structural`. So "keep on `Class::Trivia`" would NOT fix the pragma case — the keep-set is named explicitly |
| P14 — other object kinds use the same path | page: `preproc_conditional{…attribute_item…}, procedure` and `attribute_item, comment, procedure`; interface: `attribute_item, comment, interface_procedure`; tableextension: `attribute_item, comment, procedure`; report: `attribute_item, pragma, procedure` | `s07_other_objects.cst.txt`. All reached by the one `collect_routines` recursion |
| P15 — where the grammar admits `attribute_item`, and which of those the lowerer pairs (corrected in R1-05) | **grammar**: `_body_element`, `_action_element`, `_procedure_header`, `fields_body`, `_field_branch_items`, `keys_body`, `_key_branch_items`, `dataset_body`, `preproc_conditional_dataset`, `parameter_list` (a walk of `src/grammar.json`; `grammar.js:2954`, `:2973`, `:3026`, `:3065`, `:3775`, `:4006`, `:4617-4619`). **Lowerer**: only `collect_routines` (`:823`) pairs, and the split-header filter (`:1398`) reads in-node attributes. | Fields, keys and dataitems are not routines: an attribute in those bodies is followed by a `field_declaration`/`key_declaration`/dataitem (→ `_` arm → clear) or ends the body (P11). Parameter attributes sit inside `parameter_list`, which `collect_routines` never enters (it stops at a routine) and `lower_param` does not read; unchanged. Action triggers (`_action_element`) go through `collect_routines` like every trigger. Variable attributes are a different node, `var_attribute_item` (`s08_var_attr.cst.txt`), never lowered; nothing in `src/` reads them. `src/language.rs`'s `attribute_item` queries are dead code |
| P16 — the wrappers that can hold an `attribute_item` are ONE `#if` chain (corrected in R1-05) | **HOLDS for all 10** | A walk of `src/grammar.json` (following hidden `_` rules) finds exactly 10 `preproc_conditional*` rules that can hold an `attribute_item`: plain, `_actions`, `_controladdin`, `_dataset`, `_fields`, `_keys`, `_layout_mixed`, `_query`, `_report`, `_xmlport`. Each has its own `grammar.js` rule with exactly one `preproc_if` and one `preproc_endif` (e.g. `preproc_conditional`, `grammar.js:4985-4997`). The 12 aliased types (`_property_value`, `_property_value_sequence`, the ten B7b list groups; `grammar.js:257-384`, `:581-601`) cannot hold one. The design still checks the single-chain shape at run time and falls back to today's `clear()` otherwise |
| P17 — an `#if/#else` whose arms hold the WHOLE procedure (same signature) with a different `[EventSubscriber]` per arm | **pre-existing, different code path** | `$SCRATCH/repro-arms`: `FireA` → `['COMMIT','EVENT_PUBLISH']`, `FireB` → `['EVENT_PUBLISH']` (`a1/digest-arms-FireA.json`, `a1/digest-arms-FireB.json`). `dedup_routines_preserving_genuine_overloads` (`src/program/build.rs:709`) keeps the first copy's `event_subscribers` only → Discovery D1 |
| P18 — no on-disk cache holds lowered attributes | **HOLDS** | `src/program/dep_cache.rs` is process-level. The version tuple in `src/engine/gate/cache_prune.rs:193-202` is read only by the prune command; no writer of `artifactKey` exists outside it |
| P19 — real-world population (CDO), an ESTIMATE | **~48 attributes on ~44 routines are lost today; 4 are `[EventSubscriber]`** | `$SCRATCH/census.py` is a line heuristic, NOT the parser: an attribute line, then any run of blank / comment / pragma / region / `#define`/`#undef` / `#if`/`#elif`/`#else`/`#endif` lines and further attributes, ending in a `procedure`/`trigger` header; an attribute counts as lost when a non-blank, non-attribute line follows it in the run; a same-line `//` after `]` counts. Over `$CDO_WS` @ `bc3ccb18` (clean): 582 workspace `.al` + the `.al` entries in all 11 dependency `.app` packages. Of 32,373 routines with attributes: lost by kind — Obsolete 15, NonDebuggable 14, **IntegrationEvent 12**, **EventSubscriber 4**, TryFunction 2, InherentPermissions 1. By separator — comment 19, `#if` line 19, pragma 16, region 1, same-line comment 1, `#define`/`#undef` 0 (`census-r1.txt`, site list identical to `census.txt`). By source (routines) — workspace 4 (all NonDebuggable, `CDOeSealSetup.Table.al`), Base Application 27, **System Application 4** (corrected in R1-06: `SharePointClient.Codeunit.al:522,533`, `SystemInitializationImpl.Codeunit.al:30`, `GuidedExperienceImpl.Codeunit.al:1113`), Continia Core 3, Continia Delivery Network 6. The 4 subscribers: Continia Core `CloudMigrationSubscr.Codeunit.al:584,619` (inside `#if BCONLINE … #endif`), System Application `SystemInitializationImpl.Codeunit.al:30` (`#pragma` after), `GuidedExperienceImpl.Codeunit.al:1113` (same-line comment). Spot-checked against real text: `JobQueueStartReport.Codeunit.al:223`, `DetailTrialBalance.Report.al:367`, `BOMTreeNodeDictionary.Codeunit.al:26`. A heuristic misses attributes on lines that also hold other code; it is the population estimate only |
| P20 — current CDO call-graph numbers (base) | recorded | `cargo build --profile cdo --bin aldump` in the worktree (exit 0), `CDO_WS` NOT set. `--program-call-graph-stats` → sha256 `2009fab1ca0849ab5c065759b873806e585b429e5b45c3cbdee178302e343c25`: **primaryScoped** total 20916, resolvedSource 9459, resolvedCatalog 6156, honestEmpty 5221, honestDynamic 57, conditionalResolved 0, ambiguousResolved 23, unknown 0; **wholeProgram** total 46317, resolvedSource 11566, resolvedCatalog 6156, honestEmpty 28249, honestDynamic 57, conditionalResolved 266, ambiguousResolved 23, unknown 0; recoveredFiles 0; coverage holds. `--dependency-bodies-stats` → sha256 `7eacefbb48668b6520c4b9768f44804d5fe647679f81e883068c76d1685261e8`: total 431321, resolvedSource 189341, resolvedCatalog 116276, honestEmpty 124186, honestDynamic 1059, ambiguousResolved 459, unknown 0 |
| P20b — current CDO detector findings (base, R1-03) | recorded | release-fast `alsem analyze $CDO_WS --format json --deterministic` (cross-app default), exit 0. `--scope primary`: 2292 findings, sha256 `d48386e838469e93c07e32b0bc14cacb435df3620898fa4d9b10efeb6c0ca478`, 20 s. `--scope all`: 27106 findings, sha256 `4fce6ba3118b44b7bc1004389936d318cbaa680f450bbce4ddee0fb225b7b96e`, 18 s. Per-detector counts by full id are in the two files; attribute-reading detectors present include `d12-dead-integration-event` (23/23), `d14-dead-routine` (81/17512), `d35-commit-in-event-subscriber` (10/77), `d38-subscriber-to-obsolete-event` (1/1), `d43-event-ishandled-skip` (22/979), `d44-event-multi-subscriber-overlap` (20/248), `d54-publish-in-tryfunction-cone` (6/6), `d55-event-publish-in-loop` (31/31), `d59-integrationevent-var-boolean-guard` (0/72) |
| P20c — attempt 2 re-measure on `1b6f1c38` (THE baseline for this attempt) | P1 HOLDS; call-graph stats UNCHANGED; detector findings MOVED (by master, not this issue) | `$SCRATCH/base.sh`, `base.out` (every step exit 0). `digest-with.json` → `['EVENT_PUBLISH']`, `digest-without.json` → `['COMMIT','EVENT_PUBLISH']`. `cdo-stats-base.json` sha256 `2009fab1…3c25` and `cdo-depbodies-base.json` `7eacefbb…61e8`: byte-identical to attempt 1. `analyze-base-primary.json` sha256 `204f740c…f3b3`: 2293 findings (attempt 1: 2292; d43 22→23). `analyze-base-all.json` sha256 `c51ce36e…b651`: 26954 findings (attempt 1: 27106; d43 979→841, d44 248→234). alsem version 1.6.1→1.7.1. The commits between the two bases (#69, #71, #72) moved these numbers, before any change for this issue |
| P24 — population (2): positional-list `#if` groups whose reading the running context can change (A2-R3-03) | **0 on CDO** | `$SCRATCH/popscan/result.md` (sonnet subagent, read-only). It scanned 582 workspace `.al` files and 10,881 `.al` entries in the 11 dependency `.app` packages. Text pre-filter (`prefilter.py`): a directive line inside an open `[…]` attribute or `key(…)` gives **0 candidate files**, so 0 `preproc_conditional_attribute_args` and 0 `preproc_conditional_field_list_items` groups, and 0 nested / sibling-shared / contradictory. Validated: synthetic `key(PK; A, #if X B, #endif C)` and `[EventSubscriber(…, #if X 1, #endif 2)]` are flagged, a plain `#if` around a procedure is not. An independent looser check (`loosecheck.py`: a directive after a line ending in `,` or `(`) finds 75 hits, all permission-set `Permissions` lists, none an attribute or key list. So on CDO every lowered-fact change can only come from attribute pairing (P19) |
| P21 — fixtures/goldens containing an affected shape | **no golden fixture. ONE unit test asserts the bug** (corrected, A2-R1-06): `attribute_separated_by_comment_is_not_attached_today` (`lower/mod.rs:5633`, added by #71 on purpose). The original audit missed it, because it greps real lines and that AL text sits in a Rust string with `\n` escapes. It is updated as part of A1. **A second one, found by the pre-commit hook in Task 2:** `tests/cli/cli_digest_changed_spans.rs` `declaration_start_line_attribute_limits` (#71, "a #80 fix moves this assertion on purpose"), updated in `563a26e6`. A `grep -rn "#80"` over `src/`, `crates/`, `tests/` and `CHANGELOG.md` now finds no other test that asserts the bug. CHANGELOG `:124`/`:163` state #80 as a #71 limit; Task 4 updates them | `census.py` over `tests/`: 179 routines with attributes, 0 lost. `grep -A1` for an attribute-only line followed by a separator line over every `.rs`/`.al` in `crates/`, `src/`, `tests/`: 0 hits (the same grep finds 27 in `$SCRATCH/s0*.al`). The only attribute directly inside an `#if` arm in the repo is in two lowerer tests (`lower/mod.rs:4243-4245`, `:5507-5518`) where the routine is in the same arm — the P9 shape. In `:5507-5518` the attribute's guard equals the routine's context (both inside `#if not X`), so its args are read exactly as today |
| P21b — the planned test inputs parse as the design assumes | **HOLDS** | `s11_test_shapes.cst.txt` (A9, A7, A8), `s12_r1_shapes.cst.txt` (A13, A14, A11-table), `s13_split_after_if.cst.txt` (A15): exit 0, no `ERROR`/`MISSING` |
| P22 — no test pins an exact CDO number | **HOLDS** | `grep -E '\b(46317|28249|11566|20916|9459|431321|124186|189341)\b' tests src scripts` → 0 hits. The CDO tests are ratchets/ceilings and `cdo-compiler-anon.json` (compiler-oracle pairs, audited by `cdo_compiler_audit_explains_every_disagreement`) |
| P23 — primary scope is decided by the CALLER only (R1-03) | **HOLDS** | `is_primary_scope` (`src/program/resolve/full.rs:311-314`) tests `edge.edge.from.object.app`. A workspace caller of a dependency publisher that regains `[IntegrationEvent]`, or of an event a dependency subscriber now binds to, is a primary-scope edge, so `primaryScoped` can move even though no workspace routine regains a call-relevant attribute |

## Design

### Data

`pending` becomes a list of **entries** `(attribute node, guard)`. The guard is the full build
context under which that attribute is still waiting for its routine: the symbol values
(`Vec<(String, bool)>`, the same form as `ctx` and `RoutineDecl::preproc_context`) of every
enclosing `#if` branch plus every `#if` path the attribute has travelled since it was written.
An UNPROJECTED guard always contains the `ctx` of the level the entry is at. A projected guard
(see rule 4) keeps only the literals later code reads, so it need not (A2-R4-01). The tuple `collect_routines`
pushes to `out` carries entries instead of bare nodes.

### The chain logic, one source (R1-08)

`preproc_branches` (`:1156`) becomes a thin wrapper over a new `preproc_chain(node, source,
ctx)` that returns the same branch list PLUS `falls_through: Option<Vec<(String, bool)>>`: the
symbols of the implicit "no arm taken" path (every `#if`/`#elif` condition assumed false via
`assume_preproc`), `Some` only when the chain has no `#else` and no arm is decided TRUE under
`ctx` — i.e. the existing `taken` flag is false at `#endif`. `preproc_branches`'s callers keep
their exact output; only `collect_routines` reads `falls_through`.

**Contradictory symbol sets (A2-R1-02).** A symbol set that gives one symbol both `true` and `false`
describes no build. `assume_preproc` accumulates literals with no consistency check, so one can arise
from the chain itself: in `#if X … #elif not X … #endif`, the fall-through path is `{X:false, X:true}`,
and in `#if X … #elif X … #endif` the second arm is `{X:false, X:true}`. One helper,
`consistent(a, b) -> Option<Vec<(String, bool)>>`, joins two symbol sets and returns `None` on a
conflict. Every guard join below goes through it, and a `None` join kills that path for attributes.
Routine collection does not use it and is unchanged: the arm walk still uses `ctx ∪ arm_syms`, exactly
as today.

### The rule (`collect_routines`)

`collect_routines` takes `pending: Vec<Entry>` and returns the entries still pending at its
end. For each child, in document order:

1. `attribute_item` → push `(child, ctx.to_vec())`.
2. **Keep-set** — the grammar's `extras`: `comment`, `multiline_comment`, `pragma`,
   `preproc_region`, `preproc_endregion`, `preproc_define`, `preproc_undef` → `pending`
   untouched, not descended (none can contain a routine). One named arm citing
   `grammar.js:539-547`.
3. A routine kind (`procedure`, `trigger_declaration`, `interface_procedure`,
   `preproc_split_procedure`, `preproc_split_procedure_preamble`) → takes every entry, as today.
4. A preproc wrapper (`is_preproc_wrapper`) with exactly one `preproc_if` and one
   `preproc_endif` among its named children (P16) → **guarded path flow**:
   - Arms are walked ONCE each, exactly as today: `preproc_chain(child, source, ctx)`, a live
     arm is walked with `ctx ∪ arm_syms`; a dead arm is not walked. Routine collection is
     unchanged.
   - **Admission.** For each incoming entry `(n, g)`, compute `preproc_chain(child, source, g)`.
     The entry enters arm `k` iff arm `k` is live under `g` AND `consistent(g, arm_syms_k)` is
     `Some`. Inside the arm its guard is that join. **Why an admitted arm is always walked
     (A2-R4-01).** Projection (below) means `g` need not contain `ctx`. But liveness of this arm
     reads only the symbols its own condition (and the earlier conditions of the chain) mention.
     Projection keeps every literal on those symbols, because the conditions start after any
     earlier wrapper's end. So the arm is live under projected `g` exactly when it is live under
     the unprojected guard. The unprojected guard does contain `ctx`, so the arm is live under
     `ctx` too, and the walk visits it. Routine collection keeps its own unprojected `ctx`. The arm's own attributes enter with
     guard `consistent(ctx, arm_syms_k)`. When that is `None`, no build reaches the arm, and the
     arm's own attributes enter nothing.
   - **Fall-through.** The entry also leaves the wrapper unconsumed, with guard
     `consistent(g, falls_through)`, iff `falls_through` is `Some` and the join is `Some`.
   - **After the wrapper (bounded and exact, A2-R1-04, A2-R2-01, A2-R3-01).** `pending` = every
     arm's returned leftovers, then the fall-through entries. Each entry's guard is then
     **projected** onto the symbols that can still matter. A literal `(s, v)` is kept only if `s`
     is mentioned by (a) a preprocessor condition (`condition` of any `preproc_if` /
     `preproc_elif` node, at any depth, split headers and argument groups included) that STARTS at
     or after the wrapper's end byte in the same object, or (b) a condition inside the entry's
     own attribute node (its argument groups). After that, exact duplicates (same node, same
     projected guard) are removed. Entries carry no other state, so (node, guard) IS the entry's
     whole identity.
     **Why projection is exact.** After this point, an entry's guard is read in only three ways:
     liveness of a later arm (`decide_preproc` on a later condition), `consistent()` with a later
     arm's or header arm's symbols (taken from later conditions), and the argument reading
     (the attribute's own conditions). Each reads only the symbols those conditions mention. A
     literal on any other symbol cannot change any of them, so dropping it changes no attachment
     and no reading. In particular, N pragma-only wrappers over unrelated symbols `Xi`, each
     mentioned nowhere later, leave ONE entry (A17), and every path keeps its own exact reading
     (A19).
     **Hard bound, loud.** If one attribute node still has more than `PENDING_PATH_CAP = 64`
     distinct projected guards (the threshold is that count, nothing else; guards are partial
     assignments, so as few as 5 symbols can produce more than 64, A2-R4-03), its entries are then
     DROPPED, and the object records a `SyntaxIssue` at the
     attribute's origin: "attribute reaches its routine through more than 64 distinct `#if`
     paths; not attached". There is no collapse, no widening and no pinning, so no wrong reading
     is ever produced silently. The drop is the only under-attachment in the design. It is
     declared, diagnosed and tested (A22). The cap trigger was not measured on real code; the
     P19 census counts separator kinds, not wrapper depth.
   - **Order (A2-R2-04).** A routine takes its entries sorted by the attribute node's start
     byte (stable). Lowering therefore always sees source order, whatever order the arms and the
     fall-through returned. Example: `[Scope] #if X procedure P … [NonDebuggable] #endif procedure Q`
     gives `Q` `["scope","nondebuggable"]` (A21).
   - Content before `#if` and after `#endif` is not walked. It is empty for every single-chain
     wrapper that can hold an attribute (P16), so this is no loss (A2-R1-07).
5. Any other node (a non-single-chain wrapper, sections, dataitems, `modify`, `var_section`,
   fields, keys, `ERROR`, …) → `pending.clear()`, recurse with an empty `pending`, ignore the
   result — exactly today's behaviour. A dangling attribute therefore never leaves the body it
   is in (P11).

The doc comment of `collect_routines` (`:783-802`, "resetting on any other node") is rewritten
to state the keep-set and the guarded path flow (R1-08).

Why the guard fixes P9c: after `#if X P #endif`, `A` survives only by fall-through, with guard
`{X: false}`. At the second `#if X`, arm `X` is decided false under that guard, so `A` does not
enter it and `R` gets nothing; `A` falls through again and reaches `Q`.

### Attaching and lowering (R1-01)

- **Split headers** (`lower_object`, `:229-247`): `collect_routines` hands a routine ALL its
  entries, every exact guard included (no deduplication there). For each header arm `a`,
  `lower_object` keeps an entry `(n, g)` only if `consistent(g, arm_symbols(r, a, source))` is
  `Some` (`:1364`; `source` is in scope there), computed in the per-arm loop before the arm's
  attribute list is built (A2-R1-01). For P9e, the `X`-guarded attribute reaches `P(I)` and not
  `P(T)`. Non-split routines have one arm, with empty arm symbols, and keep every entry.
- **Argument context** (`lower_routine`, `:1413-1449`): an outer entry's `#if` argument groups
  are read by `conditional_list_arm` under `g ∪ arm_symbols` (the arm symbols are empty for a
  non-split routine), instead of the routine's `preproc_context`. An in-node split-header
  attribute keeps `preproc_context`, as today. For every attribute written directly before its
  routine, `g` equals the routine's `object_ctx`, so its reading is unchanged; only attributes
  that crossed an `#if` read differently. P9d then reads `'OnB'`.
- **Nested argument groups (A2-R1-03, a pre-existing defect that this fix newly exposes).**
  `conditional_list_arm` (`:1207-1226`) picks the first live arm. It then recurses into a group
  nested in that arm with the ORIGINAL `ctx` (`:1221`), which drops the chosen arm's symbols. So
  `#if Y (#if not Y 'Never' #else 'OnA' #endif) #else 'OnB' #endif` reads `'Never'`, a value no
  build has. **Fix (A2-R1-03, A2-R2-02): one running context per positional list.**
  `conditional_list_arm` takes the context as `&mut Vec<(String, bool)>`. It picks the first arm
  that is live under the running context AND whose symbols are `consistent` with it. A
  contradictory arm (e.g. `#if Y and not Y`, which `preproc_branches` reports live under an
  empty `ctx`) is skipped, and there is no fallback. It then EXTENDS the running context with the
  chosen arm's symbols before reading the arm's items, including nested groups. Each caller
  starts the running context from its own context once per list and passes the same vector to
  every group in that list. So sibling groups see each other's choices: a publisher group that
  chose `Codeunit::PubA` under `X` makes a later event-name group choose its `X`-compatible arm.
  No group can pair choices that no build makes. When no arm is consistent, the group adds
  nothing (as today for an `#if` with no live arm). There are exactly two callers:
  attribute arguments (`lower_routine`, `:1441`, one running context per argument list, started
  from the entry's guard as below) and `key_members` (`:512`, one running context per key, started
  from `[]` as today). A reading changes only when a
  later or nested group's condition depends on an earlier choice's symbols, or when an arm is
  self-contradictory.
- **Deduplication, per header arm (A2-R3-02).** One node can reach a routine by several exact
  paths. AFTER the header-arm filter above, and separately for each lowered header arm, every
  kept entry's argument selection is read under `consistent(g, arm_symbols)`. One entry per
  (node, selection) is kept, the first in source order. Deduplicating before the arm filter
  would be wrong. For `[Scope]`, a pragma-only `#if X/#else/#endif`, then the split header
  `#if X P(I) #else P(T) #endif`, the routine receives `{X:true}` and `{X:false}` with the same
  selection. Deduplicating first would keep only `{X:true}`, and the filter would then take
  `Scope` from `P(T)` (A23). The comparison is on selected argument NODES before `lower_expr`,
  so there are no dead `ExprId`s and no duplicate `SyntaxIssue`s. Two paths that select DIFFERENT arms give two `AttributeIr`s.
  Each is the attribute of some build (union-read; several `[EventSubscriber]`s on one procedure
  are already modelled, `src/program/resolve/event.rs:511-512`). The same-node, same-selection case
  (A9) yields exactly one.
- **Declaration span (#71).** `AttributeIr.origin` (`:1456`, added by #71) is the attribute node's
  own span, so a regained attribute moves its routine's declaration span (the digest's
  changed-root matching) up to that attribute's first line. That span is the correct one: it is
  what #71 built for an attribute written directly above its routine.

### What each shape gets

| shape | result |
|---|---|
| attribute, `//`/`///`/same-line `//`, routine | routine has the attribute |
| attribute, `/* … */` (one or many lines), routine | has it |
| attribute, `#pragma` / `#region` / `#endregion` / `#define` / `#undef`, routine | has it |
| attribute, comment, attribute, routine | both, in source order |
| multi-line attribute, comment, routine | has it (one node) |
| `#if X [A] #endif` routine | `A`, unless `ctx` decides `X` false |
| `#if X [A] #elif Y [B] #else [C] #endif` routine | `A, B, C` (one per build; union-read) |
| nested `#if X #if Y [A] #endif #endif` routine | `A` |
| `[A] #if X #pragma … #else #pragma … #endif` routine | `A` once |
| `[A] #if X procedure P #else procedure P #endif procedure Q` | `A` on both `P`, not on `Q` |
| `[A] #if X procedure P #endif procedure Q` | `A` on `P` and on `Q`; inside an enclosing `#if X` (X decided true): only `P`; inside `#if not X`: only `Q` |
| `[A] #if X P #endif #if X R #endif Q` | `A` on `P` and `Q`, not `R` |
| `#if not X [ES(…, #if X 'OnA' #else 'OnB' #endif …)] #endif H` | `H` subscribes to `OnB` only |
| `#if X [A] #endif` + split `#if X P(I) #else P(T) #endif` | `A` on `P(I)` only |
| `[S] #if X [A] #else [B] #endif` routine | `S, A, B` |
| `[A] #if X procedure P #elif not X procedure R #endif procedure Q` | `A` on `P` and `R`, not on `Q` (the fall-through `{X:false, X:true}` is contradictory) |
| `#if X [A] #elif X [B] #endif` routine | `A` only (arm 2 is `{X:false, X:true}`) |
| `[A]` then 40 × `#if Xi #pragma #else #pragma #endif`, routine | `A` once; one entry throughout (each `Xi` is projected out, because nothing later reads it) |
| `[A]` then 7 × `#if Xi #pragma #else #pragma #endif`, then `#if X1 and … and X7 procedure P #endif procedure Q` | `A` dropped from `P` and `Q`, plus one `SyntaxIssue` (128 relevant paths > 64) |
| `[Scope] #if X #pragma #else #pragma #endif #if X P(I) #else P(T) #endif begin … end` | both `P(I)` and `P(T)` have `scope` |
| `[ES(…, #if X and Y 'OnA' #else 'OnB' #endif …)] #if X #if Y procedure P #endif #endif procedure H` | `P` reads `OnA`; `H` reads `OnB` only (both exact paths, `{X:true,Y:false}` and `{X:false}`, select `OnB`) |
| `[ES(#if X Codeunit::PubA, #else Codeunit::PubB, #endif #if not X 'OnB', #else 'OnA', #endif …)] procedure H` | `PubA` + `OnA` (the running context makes the siblings agree) |
| `[Scope] #if X procedure P [NonDebuggable] #endif procedure Q` | `Q` `["scope","nondebuggable"]` (source order) |
| attribute arg `#if Y (#if not Y 'Never' #else 'OnA' #endif) #else 'OnB' #endif` | `'OnA'` (nested group read under the chosen arm's symbols) |
| attribute, comment, trigger / interface procedure | has it |
| attribute at the end of any body | attaches to nothing (as today) |
| attribute, `var` section / field / key / `ERROR`, routine | dropped (as today) |
| `#if X [A] procedure P #endif` | `A` on `P` (as today) |

## Base/Head SHAs

- Attempt 1 base: `66b622887a3216d813655af7a382a8ddc016981d`
- Base: `1b6f1c3886f1eecbf70791b3d32c7e5e4c117c0a` (`master`; = `origin/master` at step 11, no rebase)
- Head (approved diff): `a92c1cc1cb5228d4f0741a57a7c36605ee0e895c`. Commits: `37e2be1f`, `563a26e6`,
  `cd03e77a`, `b8fef204`, `8350785a`, `a92c1cc1`.

## Timeline

- 2026-10-14T00:47Z — worktree created, claim recorded.
- 2026-10-14T00:59Z — probes P1-P22 run; ledger steps 1-4 written. No source change.
- Spec panel round 1 (astra = gpt-6.1-sol, flash = Opus 5.5): both "not ready"; 10 distinct
  findings (one a confirmation), all accepted (`findings.json`). Revision: guarded path flow, per-attribute argument
  context, split-header arm filtering, one chain helper, probes P9c-P9e/P15/P16/P19/P20b/P23,
  tests A7b/A13-A15, CDO detector measurement, artifact manifest. No source change, no commit.
- Round-1 revision interrupted: the conductor session's login expired mid-revision and stayed
  expired for hours; the agent resumed after `/login`. Ledger sha256 after revision `35eb1f47…c188`.
- Spec panel round 2 refused: `charge` replied `{"exhausted": "wall-clock"}` (the 4 h cap;
  507 min since claim, most of it the login outage). Outcome `blocked wall-clock`. The design,
  the probes and the 10-entry register (all fixed, unreviewed in round 2) are retained here; a next
  attempt can start at round 2 with `spec-briefing-r2.md` in run dir `20261014-024629-4a5a99`.
- 2026-10-15T08:18Z — **attempt 2** claimed (run `20261015-101711-938ef5`). The worktree was created
  from `1b6f1c38`. Attempt 1's ledger, register and scratch files were carried forward. Base builds
  and re-measurements (P1, P20, P20b) were started.
- Attempt 2 numbers its spec rounds from 1 again. This attempt's round 1 reviews the round-1-revised
  design, with the roster this skill names (`gpt-6-astra`, `gemini-3.8-flash`). Attempt 1's round 1
  used different reviewers. Those round-1 findings are carried as R1-01…R1-10, and both new
  reviewers mark every one of them.
- Attempt 2, spec round 1 (`spec_rounds` remaining 4, `pi_calls` 18). flash: all 10 accepted, one
  minor (A2-R1-01), "ready". astra: 9 accepted, R1-09 re-raised (manifest paths after the `a1/`
  move), 3 important + 3 minor new (A2-R1-02…07), "not ready". All were source-verified and fixed:
  `consistent()` guard joins, the per-node merge (bounded `pending`), the nested-arm context
  in `conditional_list_arm`, A6 `MixedBeforeElse`, the A1 flip of `:5633`, no boundary walk,
  tests A16-A18, and `s14_nested_args` parsed. Reviews: `.agent/runs/20261015-101711-938ef5/spec-r1-{astra,flash}.md`.
- Attempt 2, spec round 2 (`spec_rounds` remaining 3, `pi_calls` 15, including one flash retry
  after pi refused the continuation thread: "Project servers blocked"). flash: all 17 accepted,
  "ready". astra: 16 accepted; A2-R1-03 re-raised (live does not imply consistent:
  `#if Y and not Y`); new A2-R2-01 (the per-node intersection can change a realizable
  reading), 02 (sibling groups chosen independently), 03 (A7b/A13 mutations masked by
  `consistent`), 04 (merge order); "not ready". All were source-verified (`:512`/`:1441` are the
  only callers; `:1430-1444` gives each group the same unchanged context). Fixed: exact
  per-path entries with `PENDING_PATH_CAP` collapsing by argument selection, a running context
  per positional list with consistent-arm selection and no fallback, dedup per (node, selection)
  at attachment, sort by start byte, precise A7b/A9/A13/A17 mutations, tests A19-A21 plus an A12
  merge variant, `s15_r2_shapes` parsed. Reviews: `spec-r2-{astra,flash}.md`.
- Attempt 2, spec round 3 (`spec_rounds` remaining 2, `pi_calls` 12, including a second flash
  retry for the same pi error). flash: all 21 accepted, "ready". astra: 20 accepted; A2-R2-01
  re-raised (a pinned selection under-attaches after later arms); new A2-R3-01 (pinned
  entries compare equal and are lost), 02 (dedup before the header-arm filter drops a split
  arm; verified, `:882-889` consumes pending before `:229-247`), 03 (the change population
  misses sibling and contradictory positional readings); "not ready". Fixed by removing the
  collapse and pinning: guards are projected onto the symbols later conditions and the
  attribute's own arguments read (exact, so A17 stays at one entry), a loud drop + `SyntaxIssue`
  above 64 relevant paths (A22), dedup per header arm after the filter (A23), and a widened,
  pre-measured change population. `s16_r3_shapes` parsed. Reviews: `spec-r3-{astra,flash}.md`.
- Attempt 2, spec round 4 (`spec_rounds` remaining 1, `pi_calls` 10). flash: all 24 accepted,
  "ready". astra: all 24 accepted, 3 minor new (A2-R4-01 the `g ⊇ ctx` invariant no longer holds
  after projection; 02 A9's standalone shape is masked by projection; 03 the "7 symbols" claim is
  wrong, since partial guards reach 64 with 5), **"ready"**. All three were fixed in text: the
  admission proof is restated over retained symbols, A9 gets a later `#if X` routine, and the
  cap is described as a guard count only. Round 5 is for marks only. Reviews: `spec-r4-{astra,flash}.md`.
- Attempt 2, spec round 5 (`spec_rounds` remaining 0, `pi_calls` 8): both reviewers accepted all 27
  entries against ledger sha256 `18ffd360…8b4f`, with no new findings, "ready". **SPEC CONVERGED.**
  Reviews: `spec-r5-{astra,flash}.md`.
- Step 6: acceptance-test subagent (opus) dispatched (`subagents` 59 left). RED on `1b6f1c38`:
  25 tests (24 in `lower::tests`, A12 in `--test cli`); **24 FAIL, 1 PASS (A11)**. Every failure is
  the stated reason: the attribute is missing, the wrong `args[2]` (A18 `'Never'`, A20 `'OnB'`), or no
  path-cap issue (A22). A7/A7b, the controls, fail on their POSITIVE halves (an attribute crossing an
  `#if`), and their negative halves pass. `cargo test -p al-syntax --lib lower::tests` exit 101
  (70 passed, 23 failed); `cargo test --test cli cli_attribute_trivia::` exit 101. Table, commands,
  inputs: `$SCRATCH/red.md`, logs `$SCRATCH/red-lower.log`, `red-cli.log`, probe inputs `$SCRATCH/t_*.al`
  (all parse, 0 `ERROR`). Uncommitted. `tests/cli/cli_attribute_trivia.rs` stays uncommitted
  until it is green, because `--test cli` is a `check-goldens` target and the pre-commit hook would
  reject it.
- Step 8, Task 1 (opus implementer; `task_attempts` T1 = 1): A18 and A20 green, 72/21 in
  `lower::tests`, `l2_ir` 73 passed, `program_resolve_harness` 200 passed, clippy `-D warnings` clean.
  Deviation: `preproc_chain`/`falls_through` moved to Task 2, because adding it unread would trip
  `dead_code`. The implementer's commit was refused by the pre-commit hook, which tests the WORKING
  tree, including the untracked red A12 file. The conductor parked `tests/cli/cli_attribute_trivia.rs`
  and its `mod` line in `$SCRATCH/parked/`, committed **`37e2be1f`** (hook: goldens OK), and restored
  both. `check-diff` `{"reasons": []}`. Report `$SCRATCH/t1.md`. Sonnet review: **approve**, with 3
  minor notes (`$SCRATCH/t1-review.md`): the `conditional_list_arm` doc lead sentence, a comment on how
  the trailing post-`#endif` entry makes "no consistent arm" read nothing, and `type Syms` placement.
  They are deferred to the review-fix pass after Task 2, because Task 2 is editing the same file.
- Step 8, Task 2 (opus implementer) dispatched in parallel with the T1 review (read-only).
  Result (`$SCRATCH/t2.md`): `al-syntax` lib 113 passed / 3 failed (A14, A15, A19, which are Task 3's);
  `l2_ir` 73, `program_resolve_harness` 200, `r4` 57, clippy clean. Deviations: a temporary
  dedup-by-node in `lower_object` (A6 `MixedBefore` reaches the routine by two exact paths; Task 3
  replaces it), and the own-attribute guard is `consistent(&[], ctx)`. The first commit attempt was
  refused by the hook: `--test cli` `cli_digest_changed_spans::declaration_start_line_attribute_limits`,
  a SECOND #71 test that asserts the bug on purpose ("a #80 fix moves this assertion"), with
  `left: (2, 4)` / `right: (4, 4)`. P21's audit missed it (AL inside a Rust string). The conductor
  updated it to `(2, 4)`, renamed it `declaration_start_line_claims_separated_and_multi_line_attributes`,
  and committed **`563a26e6`** (hook: goldens OK). `check-diff` `{"reasons": []}`. **Discovery-
  adjacent note:** CHANGELOG lines 124 and 163 (#71's entries) state #80 as a limit; Task 4 updates
  them.
- Step 8, T2 review (sonnet) and Task 3 (opus implementer, which also applies the 3 T1 minor
  notes) dispatched in parallel. T2 review: **changes** (`$SCRATCH/t2-review.md`). 1 important,
  source-verified at `563a26e6:lower/mod.rs:1214`: `preproc_condition_symbols`'s
  `contains("#if")` pre-check misses spaced `# if` / upper-case `#IF` (the scanner accepts both),
  so projection would drop every literal and over-attach. 5 minor: linear projection scans,
  O(n²) dedup, the cap is per scope rather than per node (doc and message), the interim dedup,
  and the rule-1 guard form. The review-fix was routed to the RUNNING Task 3 implementer, the
  only editor of that file (`subagents` charged), with a new test
  `projection_sees_spaced_and_upper_case_directives` and its discrimination proof.
- Step 8, Task 3 (`$SCRATCH/t3.md`): `al-syntax` lib 117/117 plus the new test, `l2_ir` 73,
  `program_resolve_harness` 200, `r4` 57, `cli` 328 with A12 green, clippy clean. Committed by the
  conductor as **`cd03e77a`** (hook: goldens OK), including `tests/cli/cli_attribute_trivia.rs`.
  `check-diff` `{"reasons": []}`. Deviations: the existing "`#if` inside attribute arguments" issue
  text is unchanged (byte-identical output), and the per-routine `seen` is a `Vec` (tiny). T3 review
  (sonnet) dispatched. The CDO after-measurement runs in the background (`$SCRATCH/after.sh`).
  T3 review: **approve** (`$SCRATCH/t3-review.md`), 5 minors. (1) `preproc_condition_symbols` now
  walks every object's CST (a constant cost): judged against the CDO before/after timings below.
  (2)/(3) `Vec` dedup and selection computed for discarded duplicates: tiny, no action.
  (4) An outer entry's argument context no longer includes `object_ctx`. Answered by the projection
  argument: before projection an entry's guard contains `ctx`, and projection keeps every symbol the
  attribute's OWN argument conditions mention. So a symbol the arguments read is never lost, and
  an attribute that crosses no wrapper has guard = `ctx` exactly. No defect. (5) A12 checks outcome
  (COMMIT), not the event name: accepted per the row.
- Step 4 (Task 4): population scan (P24 = 0), CDO before/after measured and every edge traced
  (Gate results). CHANGELOG committed **`b8fef204`**. The first gate run was started, then STOPPED
  by the conductor (TaskStop, plus 3 orphaned `rustc` processes of this worktree killed). Final-
  panel fixes were coming, so that run could not have been the attested tree. No gate verdict was
  taken from it.
- Step 10, final panel round 1 (`final_rounds` 4 left, `pi_calls` 6). Built-in `code-review high`
  on the branch (forked skill): 7 candidates. flash: all 27 accepted, "ready". astra: A2-R1-02
  re-raised (`consistent` did not validate its first operand), 2 important + 5 minor, "not ready".
  Each was source-checked. A review-fix (sonnet, `task_attempts` RF1) fixed all of them except
  two minors, which are deferred with rationale (F1-02, which became discovery D2, and F1-09 in
  Not delivered). Commit **`8350785a`** (hook: goldens OK). Register F1-01…F1-10. Report
  `$SCRATCH/rf1.md`. The gates were rerun on `8350785a`.
- Final panel round 2 (`final_rounds` 3 left, `pi_calls` 4). flash: all 37 accepted, "ready".
  astra: F1-05 and F1-07 re-raised, and new F2-03 (important, `consistent`'s first-operand check is
  quadratic) and F2-04 (minor, `arm_live` per entry), "not ready". All are cost-only and
  source-verified. The gate run on `8350785a` was stopped again (its tree was about to change).
  RF2 (opus, `task_attempts` RF2) committed **`a92c1cc1`**: `chain_parts` once per wrapper, a
  shared directive slice, hashed `consistent`, a `Run` index with `decide_preproc_by`, and
  `split_conditions` once per routine. The new `attribute_flow_scales_linearly` (20000 attributes
  × 20000 procedures; a 16000-symbol guard × 300 wrappers) passes in 4.8 s and TIMES OUT at 120 s
  with either the H1 or the H2 fix reverted. `--lib` 123 passed, clippy clean, hook goldens OK.
  `$SCRATCH/rf2.md`. The gates were rerun on `a92c1cc1`.
- Final panel round 3 (`final_rounds` 2 left, `pi_calls` 2): astra accepted all 41 entries,
  "ready"; flash accepted all 41, "ready". **FINAL PANEL CONVERGED**, against ledger sha256
  `e5c692d6…5099`. Two minors stay `deferred` with rationale, each accepted by both reviewers
  (F1-02, which became D2, and F1-09).
- Step 11: `git fetch origin`. `origin/master` is still `1b6f1c38`, so there was no rebase and
  `rebase_regate` was not charged. B = `1b6f1c3886f1eecbf70791b3d32c7e5e4c117c0a`, H =
  `a92c1cc1cb5228d4f0741a57a7c36605ee0e895c`.
- Step 7: plan written, `docs/superpowers/plans/2026-10-15-issue-80-attribute-trivia.md`, 4 tasks
  (`plan_tasks` 8 left): T1 running-context `conditional_list_arm`; T2 guarded entries in
  `collect_routines`; T3 per-header-arm filter, argument context and dedup; T4 the A12 digest test,
  population scan, CDO measurement and CHANGELOG.

## Acceptance matrix

Lowerer tests are unit tests in `crates/al-syntax/src/lower/mod.rs`'s `tests` module, using
the existing `parse(src)` helper and reading `af.objects[i].routines[j].attributes` /
`attributes_parsed`. Each asserts the EXACT attribute list (names, in order), and every test
with an `[EventSubscriber]` also asserts its event-name argument (`args[2]`), so a missing,
duplicated or wrongly-read attribute fails. A1-A6, A8-A10 and A12-A15 contain a shape that
loses (or misreads) an attribute on `66b62288`; A7, A7b and A11 are negative controls that may
pass on base and guard against over-reach. The implementer records the base outcome of each.

| # | acceptance item | test | mutation that makes it fail |
|---|---|---|---|
| A1 | across a line comment | `attribute_survives_a_line_comment` — `[EventSubscriber(…'OnFire'…)]`, `// note`, `local procedure Handler()`; also `///` lines and `[Scope('OnPrem')] // trailing`; asserts `args[2]` is `'OnFire'`. **Plus the existing test `attribute_separated_by_comment_is_not_attached_today` (`:5633`, #71's stated limit, which asserts the bug).** It is renamed to `attribute_separated_by_comment_is_attached` and now asserts `["IntegrationEvent"]` with origin row 2 (A2-R1-06; its own doc says "A fix for #80 changes this test on purpose") | delete the keep-set arm |
| A2 | across a block comment | `attribute_survives_a_block_comment` — `/* note */` and a 4-line `/* … */` | delete `RawKind::MultilineComment` from the keep-set |
| A3 | across a pragma | `attribute_survives_pragma_and_region_lines` — `#pragma`, `#region`/`#endregion`, `#define`/`#undef`, one routine each | delete `RawKind::Pragma` (and, separately, `PreprocRegion`) from the keep-set |
| A4 | multi-line attribute | `multi_line_attribute_survives_a_comment` — a 3-line `[EventSubscriber(…)]`, `// c`, procedure; asserts the attribute and its 6 args | delete the keep-set arm |
| A5 | two attributes split by a comment, in order | `attributes_split_by_a_comment_are_both_kept_in_order` → `["scope","nondebuggable"]` | delete the keep-set arm |
| A6 | across an `#if` wrapping only attributes, per arm | `if_wrapping_only_attributes_gives_every_live_arm` — `OnlyAttrs`, `MixedBefore`, `MixedBeforeElse` (`[Scope] #if X [A] #else [B] #endif procedure`, → `["scope","a","b"]`), `PerArm`, `Elif`, `Nested`, `TriviaInArm`, `PragmaInIf`, `EmptyIf` (s04/s06) | (a) the wrapper returns no arm leftovers; (b) incoming entries are not admitted to arms. Only `MixedBeforeElse` loses `Scope` under (b): an exhaustive `#else` leaves no fall-through to rescue it. `MixedBefore`, `PragmaInIf` and `EmptyIf` keep it through fall-through (A2-R1-05, which corrects the earlier claim) |
| A7 | a dead arm contributes nothing (known-false) | `if_attribute_in_a_dead_arm_is_not_attached` — outer `#if X`, inner `#if not X [A] #endif procedure P` → `P` has `[]`; and inside an outer `#if not X`, `[A] #if X procedure P #endif procedure Q` → `P` absent, `Q` has `A` | walk dead arms (ignore `live`) |
| A7b | a decided-true arm stops fall-through (known-true, R1-04) | `fall_through_is_dead_when_an_arm_is_decided_true` — inside an outer `#if X`, `[A] #if X procedure P #endif procedure Q` → `P` has `A`, `Q` has `[]` | `preproc_chain` returns `falls_through = Some(vec![])` (no literals) whenever the chain has no `#else`, ignoring `taken` → `Q` gets `a`. (Keeping the `{X:false}` literals would be masked by `consistent`; A2-R2-03) |
| A8 | attribute before an `#if` that wraps the routine | `attribute_before_if_reaches_each_arm_and_does_not_leak` — `[A] #if X procedure P(I: Integer) #else procedure P(T: Text) #endif procedure Q` → both `P` have `A`, `Q` has `[]`; `[A] #if X procedure P #endif procedure Q` → `P` and `Q` have `A` | (a) no admission into arms; (b) fall-through always (`Q` gets `A` in case 1); (c) fall-through never (`Q` loses `A` in case 2) |
| A9 | no duplicate when one attribute reaches a routine by several paths | `[A] #if X #pragma … #else #pragma … #endif procedure P` (s11) → `["a"]` exactly once. The input ALSO has a later `#if X procedure Z() … #endif` after `P` (A2-R4-02). That keeps `X` relevant, so the two paths `{X:true}` and `{X:false}` survive projection as two distinct entries and reach `P` | skip the per-header-arm (node, selection) deduplication → `["a","a"]`. (Without the later `X` reference, projection would merge the two entries first and mask this mutation) |
| A10 | trigger and interface procedure | `attribute_survives_a_comment_before_a_trigger_and_an_interface_procedure` | delete the keep-set arm |
| A11 | what is still dropped (negative controls) | `attribute_is_still_dropped_by_a_var_section_and_at_body_end` — `[Scope] var … procedure AfterVar` → `[]`; **table** `fields { field(1; A; Integer) { } [Scope('OnPrem')] } procedure TP()` (s12) → `TP` has `[]` (R1-07: the same-object case is the one that discriminates); two codeunits, dangling attribute at the end of the first → the second's first procedure has `[]` (regression check only — it cannot fail under any `collect_routines` mutation, each object is a separate top-level call, `lower/mod.rs:84`, `:219-228`) | (a) add `RawKind::VarSection` to the keep-set → `AfterVar` fails; (b) feed the `_`-arm recursion's leftover back into the parent's `pending` → `TP` fails |
| A12 | the subscriber binds in the reproducer (end to end) | new `tests/cli/cli_attribute_trivia.rs` (listed in `tests/cli/main.rs`), `digest_sees_a_subscriber_separated_by_trivia`: the reproducer as source in a tempdir (like `cli_s6_consumers.rs`), one `[IntegrationEvent]` `OnN` + `FireN` per separator kind (line comment, block comment, `#pragma`, same-line comment, `#if` around the attribute only, attribute before an `#if` wrapping the handler) and a no-trivia control; each handler calls `Commit()`. `run_digest_pipeline(ws, None, Some(["FireN"]), None, …, true, None, false)` → every `FireN` has `COMMIT`. **Target variant (R1-04)**: events `OnA`, `OnB`, handler `#if not X [EventSubscriber(…, #if X 'OnA', #else 'OnB', #endif …)] #endif` → `FireB` has `COMMIT`, `FireA` does NOT. **Merge variant (A2-R2-01)**: the A19 shape with events `OnA`/`OnB` and a separate handler `P` (no `Commit`) → `FireB` has `COMMIT` (from `H`), `FireA` does NOT | delete the keep-set arm; no admission into arms; read args with the routine's context (`FireA` gains `COMMIT`, `FireB` loses it) |
| A13 | correlated sequential wrappers (R1-02) | `fall_through_guard_blocks_a_later_arm_of_the_same_condition` — `[A] #if X P #endif #if X R #endif Q` (s12) → `P` `["a"]`, `R` `[]`, `Q` `["a"]` | admission ignores the entry's guard: liveness from `ctx` AND the inside guard `consistent(ctx, arm_syms)` → `R` gets `a`. (Changing liveness alone is masked by `consistent`; A2-R2-03) |
| A14 | a carried attribute's arguments are read under its own arm (R1-01) | `carried_attribute_args_read_under_their_arm` — s12 `H` → one `EventSubscriber`, `args[2]` is `'OnB'` | pass the routine's `preproc_context` to `conditional_list_arm` → `'OnA'` |
| A15 | a guarded attribute reaches only the matching split-header arm | `guarded_attribute_reaches_only_its_split_header_arm` — s13 → `P(I: Integer)` `["scope"]`, `P(T: Text)` `[]` | skip the guard/arm conflict filter in `lower_object` → `P(T)` gets `scope` |
| A16 | contradictory paths attach nothing (A2-R1-02) | `contradictory_guard_paths_attach_nothing` — `[A] #if X procedure P #elif not X procedure R #endif procedure Q` → `P` `["a"]`, `R` `["a"]`, `Q` `[]`; `#if X [A] #elif X [B] #endif procedure H` → `H` `["a"]` | make `consistent` never return `None` (plain concatenation) → `Q` gets `a`, `H` gets `b` |
| A17 | the pending list stays bounded (A2-R1-04) | `many_pragma_only_wrappers_stay_bounded` — `[A]`, then 40 × `#if Xi #pragma #else #pragma #endif` (distinct `Xi`), then `procedure P` → `["a"]` | drop the projection (keep every literal) → 2^40 distinct guards, the cap fires, and `P` gets `[]` plus a `SyntaxIssue` (fails fast; the cap stops the blow-up at 65 entries) |
| A22 | the hard bound is loud (A2-R3-01) | `too_many_relevant_paths_drops_with_a_syntax_issue` — the 7-wrapper shape in the table → `P` `[]`, `Q` `[]`, exactly one `SyntaxIssue` whose message names the 64-path limit and whose origin is the attribute | (a) never drop (no cap) → `P` and `Q` get `a`, no issue; (b) drop silently → no issue |
| A23 | deduplication runs after the header-arm filter (A2-R3-02) | `dedup_after_split_header_arm_filter` — `[Scope('OnPrem')]`, `#if X #pragma warning disable AA0001 #else #pragma warning disable AA0002 #endif`, then `#if X procedure P(I: Integer) #else procedure P(T: Text) #endif begin end;` → both `P(I)` and `P(T)` `["scope"]` | deduplicate per (node, selection) in `collect_routines`, before `lower_object`'s arm filter → `P(T)` `[]` |
| A19 | exact paths keep their own argument reading (A2-R2-01) | `exact_paths_keep_their_argument_reading` — `[EventSubscriber(…, #if X and Y 'OnA', #else 'OnB', #endif …)] #if X #if Y procedure P #endif #endif procedure H` → `P` `args[2]` `'OnA'`; `H` exactly one `EventSubscriber`, `args[2]` `'OnB'` | merge every node's entries by guard intersection at each wrapper (the round-2 design) → `H` reads `'OnA'` |
| A20 | sibling argument groups agree (A2-R2-02) | `sibling_attribute_arg_groups_share_one_context` — `[EventSubscriber(ObjectType::Codeunit, #if X Codeunit::PubA, #else Codeunit::PubB, #endif #if not X 'OnB', #else 'OnA', #endif '', false, false)] procedure H` → `args[1]` is `Codeunit::PubA` AND `args[2]` is `'OnA'` (the full pair) | give each group a fresh copy of the context (no running context) → `PubA` + `'OnB'` |
| A21 | source order survives consumed arms (A2-R2-04) | `attributes_keep_source_order_across_a_consuming_arm` — `[Scope('OnPrem')] #if X procedure P … [NonDebuggable] #endif procedure Q` → `Q` `["scope","nondebuggable"]` | drop the sort by start byte → `["nondebuggable","scope"]` |
| A18 | a nested argument group is read under the chosen arm (A2-R1-03) | `nested_attribute_arg_group_reads_under_the_chosen_arm` (`conditional_list_arm`) — `$SCRATCH/s14_nested_args.al`: `[EventSubscriber(ObjectType::Codeunit, 1, #if Y #if not Y 'Never', #else 'OnA', #endif #else 'OnB', #endif '', false, false)] local procedure H` (one directive per line). It parses as `preproc_conditional_attribute_args` nested inside the first arm of another, with exit 0 and 0 `ERROR`/`MISSING` (`s14_nested_args.cst.txt`). Expected: `args[2]` is `'OnA'` | recurse with the unchanged `ctx` (today's `:1221`) → `'Never'` |

Each discrimination proof is recorded below as the test lands (break with Edit, run, revert with
Edit, run), and each scripted break is shown to have applied.

### Golden license

**No committed golden file may change.** P21 found no fixture with an affected shape, and the
one in-repo attribute-in-arm test (`:5507-5518`) keeps its reading (P21). The one unit test that
asserts the bug (`:5633`) changes on purpose (A1). That is a test expectation, not a golden. Every one of the 23
golden directories and the 8 `check-goldens` targets must stay byte-identical; a move is a
finding to triage before any regen, not a license.

**CDO** (not committed goldens; `CDO_WS` unset for the measurements, `scripts/cdo-gate
$CDO_WS` once as the branch gate): before/after of
`--program-call-graph-stats`, `--dependency-bodies-stats` (P20) and `alsem analyze` at
`--scope primary` and `--scope all` (P20b). Hard requirements: `unknown` 0 in every scope,
`recoveredFiles` 0, coverage holds, `cdo-gate` green. Everything else is a HYPOTHESIS to test
(R1-03), not a prediction to defend. The only lowered facts that change are (1) attribute
pairing, for the P19 routines (~44, an estimate), and (2) every positional-list reading that
`conditional_list_arm` makes differently, in routine attributes AND object keys (A2-R3-03):
a group nested in another (A18), sibling groups whose conditions share a symbol (A20), and a
self-contradictory arm. Population (2) is measured BEFORE the change: a scratch scan of every
`preproc_conditional_attribute_args` and `preproc_conditional_field_list_items` in CDO and its
`.alpackages` sources that is nested, has a sibling group in the same list sharing a symbol, or
has a contradictory arm. Those routines and keys are added to the trace list; every moved edge or finding must be traced to one of them —
as caller, target, publisher, subscriber, or a routine whose effect cone passes through one —
including callers and findings outside that list (P23: a workspace caller of a dependency
publisher is primary-scope). Any move that cannot be traced blocks. The before/after numbers
and the per-detector deltas go into Gate results and CHANGELOG.

## Discrimination proofs

| test | mutation | fail output | pass output | commit |
|---|---|---|---|---|
| A18 `nested_attribute_arg_group_reads_under_the_chosen_arm` | in `conditional_list_arm`, recurse into a nested group with `&mut start.clone()` (a copy of the context from before the join) | exit 101, `left: ["'Never'"]` / `right: ["'OnA'"]` | exit 0, `... ok` | `37e2be1f` |
| A20 `sibling_attribute_arg_groups_share_one_context` | in `lower_routine`, `&mut run` → `&mut run.clone()` (applied once, `grep -c` = 1) | exit 101, `left: ["'OnB'"]` / `right: ["'OnA'"]` | exit 0, 72 passed / 21 failed as before | `37e2be1f` |
| A1 (both), A2, A3, A4, A5, A10 | delete the whole keep-set arm | all 7 fail, e.g. A1 `left: []` / `right: ["IntegrationEvent"]`, A5 `left: ["NonDebuggable"]` / `right: ["Scope","NonDebuggable"]` | 7 ok | `563a26e6` |
| A2 | remove `MultilineComment` from the keep-set | `left: []` / `right: ["EventSubscriber"]` | ok | `563a26e6` |
| A3 | remove `Pragma`; separately remove `PreprocRegion` | `left: []` / `right: ["EventSubscriber"]`; `left: []` / `right: ["IntegrationEvent"]` | ok | `563a26e6` |
| A6 | (a) `pending.extend(left)` → `drop(left)`; (b) admission `if false` | (a) `left: []` / `right: ["Obsolete"]`; (b) only `MixedBeforeElse` fails, `left: ["NonDebuggable","Obsolete"]` / `right: ["Scope","NonDebuggable","Obsolete"]` | ok | `563a26e6` |
| A7 | walk dead arms (`if !live && false`) | `P is in a dead arm` | ok | `563a26e6` |
| A7b | `falls_through = (!has_else).then(Vec::new)` | Q `left: ["Scope"]` / `right: []` | ok | `563a26e6` |
| A8 | (a) no admission; (b) fall-through always; (c) never | (a) `[("I",[]),("T",[])]`; (b) Q `left: ["Scope"]` / `right: []`; (c) Q2 `left: []` / `right: ["Scope"]` | ok | `563a26e6` |
| A11 | (a) `VarSection` in the keep-set; (b) feed `_`-arm leftovers back | (a) AfterVar `left: ["Scope"]`; (b) TP `left: ["Scope"]` / `right: []` | ok | `563a26e6` |
| A13 | admission ignores the entry's guard (liveness and guard from `ctx`) | R `left: ["Scope"]` / `right: []` | ok | `563a26e6` |
| A16 | `consistent` = plain concatenation | Q `left: ["Scope"]` / `right: []` | ok | `563a26e6` |
| A17 | drop the projection | `left: []` / `right: ["Scope"]` in 0.00 s (the cap stops it) | ok | `563a26e6` |
| A21 | comment out the sort by start byte | `left: ["NonDebuggable","Scope"]` | ok | `563a26e6` |
| A22 | (a) no cap; (b) `issues.push` → `Vec::new().push` | (a) P `left: ["Scope"]` / `right: []`; (b) `left: 0` / `right: 1` | ok | `563a26e6` |
| `cli_digest_changed_spans::declaration_start_line_claims_separated_and_multi_line_attributes` (#71 test, assertion moved on purpose) | the pre-fix lowerer (base `1b6f1c38`): that test was green on master asserting `(4, 4)` | on Task 2's code the old assertion failed `left: (2, 4)` / `right: (4, 4)` (the pre-commit hook log `$SCRATCH/t2-commit.log`, first try) | `(2, 4)` passes (hook: goldens OK) | `563a26e6` |

| A9 | skip the per-arm (node, selection) dedup | `left: ["Scope","Scope"]` / `right: ["Scope"]` | ok | `cd03e77a` |
| A14 | read args from the routine's `preproc_context` instead of the entry's guard | `left: ["'OnA'"]` / `right: ["'OnB'"]` | ok | `cd03e77a` |
| A15 | skip the per-arm filter (`Some(e.guard.clone())`) | `left: [("I",["Scope"]),("T",["Scope"])]` / `right: [… ("T",[])]` | ok | `cd03e77a` |
| A19 | merge a node's entries by guard intersection (the plan's mutation). A "keep the first entry per node" break stays GREEN, because both of H's paths select `'OnB'`, so that break does not discriminate. Recorded, not counted | `left: ["'OnA'"]` / `right: ["'OnB'"]` | ok | `cd03e77a` |
| A23 | dedup per (node, selection) before the per-arm filter (in `lower_object`) | `left: [("I",["Scope"]),("T",[])]` / `right: [… ("T",["Scope"])]` | ok | `cd03e77a` |
| A12 | (1) delete the keep-set arm; (2) read args with the routine's context | (1) `Fire1..Fire4: false`; (2) `FireA: true, FireB: false` | ok | `cd03e77a` |
| `projection_sees_spaced_and_upper_case_directives` (new, from the T2 review) | restore the `contains("#if")` pre-check | `left: ["Scope"]` / `right: []` (R gets `Scope`) | ok | `cd03e77a` |

| `guarded_attribute_skips_an_unreachable_split_header_arm` (F1-01) | `.filter(arm_live)` → `.filter(\|_\| true)` | `left: [("a",["EventSubscriber"]),("b",["EventSubscriber"])]` | ok | `8350785a` |
| `attribute_arg_group_issue_is_reported_once` (F1-03) | `reported.insert(..) \|\| true` | `left: 2` / `right: 1` | ok | `8350785a` |
| `consistent_rejects_a_contradictory_first_operand`, `contradictory_context_in_node_attribute_attaches_nothing` (A2-R1-02) | (a) `consistent` copies `a` unchecked: both fail; (b) call-site guard `Some(preproc_context.clone())`: the lowerer test fails | (a) `Some([X,true],[X,false])` vs `None`; (b) `("a",["Scope"])` vs `("a",[])` | ok | `8350785a` |
| `sibling_groups_under_a_disjunction_are_not_correlated` (F1-08, a pinned LIMIT) | a true `X or Y` records both literals | `left: ["'OnB'"]` / `right: ["'OnA'"]` | ok | `8350785a` |
| `attribute_flow_scales_linearly` (F2-01, F2-03) | (1) per-entry full child walk restored; (2) nested-scan `consistent` restored | (1) TIMEOUT rc=124 at 120 s (part A); (2) part A 1.39 s, then TIMEOUT rc=124 in part B | 4.83 s, `--lib` 123/123 | `a92c1cc1` |

Task 2's per-proof logs: `$SCRATCH/t2dp/<label>.log`. RF1/RF2 proofs: `$SCRATCH/rf1.md`, `rf2.md`. Task 3's: `$SCRATCH/t3dp/<label>.log` (one revert
was first refused because the file was stale. Its red run was discarded and the revert redone). Every break was checked to have applied with
`grep -c`. One break (`p03a`) did not apply at first; its green run was discarded and the break redone.

## Findings register summary

Round 1: 10 distinct findings from astra's 9 bullets and flash's 6 (merged: R1-01 = astra #1 +
#7 + flash arg-context; R1-03 = astra #3 + flash detector measurement + flash primaryScoped;
R1-07 = astra A11 + flash A11). 5 important, 5 minor (a merged entry takes the higher of its
sources' severities); 10 fixed, 0 refuted. R1-10 is astra's keep-set confirmation, which needed
no change. See `findings.json`.

**Final state (attempt 2): 41 entries, all `accepted` by BOTH reviewers** (`gpt-6-astra`,
`gemini-3.8-flash`): spec R1-01…R1-10 (attempt 1's round 1), A2-R1…A2-R4 (attempt 2 spec rounds
1-4; converged in round 5), F1-01…F1-10 (final round 1: code-review high + astra), F2-01…F2-04
(final round 2). Severity: 17 important, 24 minor, 0 critical. Disposition: 39 `fixed`, 2 `deferred`
(F1-02, F1-09, both minor, so no blocking entry is deferred). No entry is `open`.

**Acceptance status:** every row of the matrix (A1-A23 plus the A12 end-to-end) is green at
`a92c1cc1`, and so are the issue's acceptance items. `collect_routines` keeps pending attributes
across comment/pragma trivia (A1-A5, A10) and across an `#if` wrapping only attributes, per arm (A6).
There are lowerer tests for a line comment (A1), a block comment (A2), a pragma (A3) and a
multi-line attribute (A4), and the subscriber binds in the reproducer (A12 and the CDO reproducer
`digest-with-after.json` → `['COMMIT','EVENT_PUBLISH']`).

## Gate results

### Repo gates (step 9), at H = `a92c1cc1`, every one through the supervisor (`"supervised": true`)

| gate (`--name`) | exit | seconds | log |
|---|---|---|---|
| `ci-steps-all` | 0 | 527.8 | `.agent/runs/20261015-101711-938ef5/logs/ci-steps-all/20261015T123220-1b2e4955.log` |
| `check-goldens-coverage` | 0 | 1.2 | `.agent/runs/20261015-101711-938ef5/logs/check-goldens-coverage/20261015T124108-1ffce253.log` |
| `check-goldens` | 0 | 55.5 | `.agent/runs/20261015-101711-938ef5/logs/check-goldens/20261015T124109-35207533.log` |
| `cdo-gate` | 0 | 230.0 | `.agent/runs/20261015-101711-938ef5/logs/cdo-gate/20261015T124205-0f26b75e.log` |

After the gates, `git status --porcelain -- . ':!.agent'` in the worktree is empty. Toolchain: rustc
1.96.0 (x86_64-pc-windows-msvc). Grammar: tree-sitter-al `main` @ `8682cb5f`. CDO_WS identity hash:
sha256(`<CDO_WS path>@bc3ccb18`) = `1e512deb11c28d513ade51d45a7e4cbd2f8a3fb6f145ba94387ec8e5b288bce7` (the
pinned baseline worktree, detached at `bc3ccb18`, clean). `cdo-gate` was run with `CDO_WS` passed to the
executor process only (never globally). Two earlier gate runs (on `b8fef204`, then `8350785a`) were
STOPPED before any verdict, because final-panel fixes were about to change the tree. No result was
taken from them.

At H, `aldump --program-call-graph-stats` and `--dependency-bodies-stats` (built by `cdo-gate`, `cdo`
profile) are byte-identical to the `cd03e77a` measurement below (`$SCRATCH/cdo-stats-final.json`
`c032c8db…94ef`, `cdo-depbodies-final.json` `7eacefbb…61e8`). The later commits (`8350785a`,
`a92c1cc1`) moved no CDO number. `alsem analyze` was not re-run at H: the stats are byte-identical
and `cdo-gate`'s ratchets are green.

### CDO before/after (Task 4; `CDO_WS` unset for the measurement, `$SCRATCH/after.sh`, `after.out`, all exit 0)

Branch build at `cd03e77a`, workspace `$CDO_WS` @ `bc3ccb18` (clean).

| measure | base (`1b6f1c38`, P20c) | after (`cd03e77a`) |
|---|---|---|
| reproducer `alsem digest --routine Fire`, with comment | `['EVENT_PUBLISH']` | **`['COMMIT','EVENT_PUBLISH']`** |
| reproducer, without comment | `['COMMIT','EVENT_PUBLISH']` | same |
| `primaryScoped` | total 20916, unknown 0 | **identical** |
| `wholeProgram` total / resolvedSource / honestEmpty / unknown | 46317 / 11566 / 28249 / 0 | 46328 / 11568 / 28258 / **0** |
| coverage `parsedObligations` = `classifiedEdges` | 46317 | 46328 (holds) |
| `--dependency-bodies-stats` sha256 | `7eacefbb…61e8` | **identical** |
| `alsem analyze --scope primary` findings | 2293 | 2293, `payload.findings` byte-equal |
| `alsem analyze --scope all` findings | 26954 | 26954, `payload.findings` byte-equal |

**Every moved edge is traced** (`aldump --graphify-export`, base built in the main checkout at
`1b6f1c38`, whose stats sha matched `2009fab1…3c25`; `$SCRATCH/gx-base.json` vs `gx-after.json`).
Routine kind changes: exactly 11 `procedure → event_publisher` and 4 `procedure → event_subscriber`,
with no other kind change and no lost publisher. The 11 publishers are the P19 census's
`[IntegrationEvent]` sites minus `AccScheduleOverview.OnBeforeUpdateColumnCaptions` (already a
publisher on base): JobQueueStartReport ×2, DetailTrialBalance ×2 (Base App); CDN PEPPOL, OIOUBL,
CII ×2, CreateeBillingResp (Continia Delivery Network); SharePointClient ×2 (System App). Each adds one
`raises_event` edge to an empty target (+11 `honestEmpty`). The 4 subscribers are exactly the census's 4
(`GuidedExperienceImpl.OpenRoleBasedSetupExperience`, `SystemInitializationImpl.Init`, CSC Cloud
Migration ×2). Two of them give an empty publisher its first subscriber (System
`OnCompanyOpen` → `Init`, and the `OpenRoleBasedSetupExperience` event): 2 edges move from
`honestEmpty` to `resolvedSource`. Net: `honestEmpty` +11 −2 = **+9**, `resolvedSource` **+2**,
total **+11**, which equals the stats delta exactly. The export also gains 11 and loses 2
`«dynamic»` placeholder nodes, matching those edges. Population (2) is 0 (P24), so no
positional-list reading moved. The trace script is `$SCRATCH/trace_pubs.py`.

## Discoveries

```json
[
  {
    "subsystem": "program engine / routine dedup (src/program/build.rs)",
    "locator": "src/program/build.rs:709 dedup_routines_preserving_genuine_overloads; src/program/node_extract.rs:770-780 event_subscribers",
    "symptom": "A procedure written twice under #if/#else with the SAME signature but a different [EventSubscriber] in each arm keeps only the first arm's subscription: the two RoutineDecls share one RoutineNodeId and dedup keeps the first copy's event_subscribers (and attributes) only, so the other arm's event never reaches the handler.",
    "kind": "bug",
    "origin_issue": 80,
    "reproducer": "codeunit with FireA()->OnA(), FireB()->OnB() (both [IntegrationEvent]) and '#if XSYM [EventSubscriber(...,'OnA',...)] local procedure H() begin Commit(); end; #else [EventSubscriber(...,'OnB',...)] local procedure H() begin Commit(); end; #endif': alsem digest --routine FireA -> ['COMMIT','EVENT_PUBLISH'], --routine FireB -> ['EVENT_PUBLISH'] (expected COMMIT too).",
    "pre_existing": true,
    "capability": "a routine duplicated across #if arms keeps the union of every arm's subscriptions and publisher/attribute facts",
    "acceptance": "the collapsed survivor's event_subscribers is the union of every collapsed copy's (deduplicated); a program-build test with the reproducer shows FireB reaches COMMIT; same-signature collapse otherwise unchanged."
  },
  {
    "subsystem": "program engine / event attribute consumers (src/program/resolve/event.rs, src/program/node_extract.rs)",
    "locator": "src/program/resolve/event.rs:302 publisher_include_sender (.find()); src/program/node_extract.rs:772 event_subscribers",
    "symptom": "A routine can carry several readings of ONE attribute (one AttributeIr per distinct #if argument reading reaching it, the lowerer's union-read since #80). Consumers take the first matching publisher attribute, so a publisher's IncludeSender comes from the first reading only, and two [EventSubscriber] readings that differ only in a later flag argument give the handler two subscriptions to the same event, which can double-count it.",
    "kind": "bug",
    "origin_issue": 80,
    "reproducer": "codeunit: [IntegrationEvent(#if X true, #else false, #endif false)] procedure OnFoo() reached by two #if paths (attribute, then a pragma-only #if X #pragma #else #pragma #endif, then the procedure): the routine gets two IntegrationEvent AttributeIrs, and publisher_include_sender returns the first one's arg[0] only.",
    "pre_existing": false,
    "capability": "consumers of publisher/subscriber attributes handle several per-build readings of one attribute on one routine (union of the IncludeSender values; one subscription per distinct event)",
    "acceptance": "a program-build test with the reproducer: a subscriber with the extra Sender parameter binds in either reading; two readings of one EventSubscriber that name the same event give ONE subscription edge."
  }
]
```

## Not delivered

- **Union across builds for a routine that exists in several builds.** A routine outside any
  `#if` that receives an attribute from one path carries it unconditionally: in
  `[A] #if X procedure P #endif procedure Q`, `Q` has `A` also in the `X` build, where it does
  not. `#if CLEAN25 [ES(…OnA…)] #else [ES(…OnB…)] #endif procedure H` subscribes `H` to both
  events. Each attribute's ARGUMENTS are read under its own path (A14), so no attribute carries
  a value no build has; what is not recorded is WHICH build each attribute belongs to.
  `AttributeIr` has no build-context field, and adding one would change the IR every consumer
  reads. This matches the union-read the lowerer already applies to undecided `#if`s.
- **More than 64 distinct relevant paths for ONE attribute: not attached, with a diagnostic
  (A2-R1-04, A2-R2-01, A2-R3-01).** The guards are exact, projected onto the symbols later code
  reads. The cap is a count of distinct projected guards, not of symbols (A2-R4-03). The attribute
  is then dropped, and a `SyntaxIssue` names it.
  That is an under-attachment, but a loud one, chosen over any silent approximation. How often
  real code reaches the cap was NOT measured.
- **Guards are symbol literals only.** An entry's guard records what `assume_preproc` can
  derive (`A and B` true → both; `A or B` true → nothing; `not (A and B)` true → nothing). An
  undecidable combination keeps the entry live (it over-attaches, never under-attaches). This is
  the same limit every `#if` decision in the lowerer has (`decide_preproc`, `:1257`).
- **Multi-chain wrappers** still clear `pending`; none can hold an `attribute_item` (P16).
- **An attribute followed by a non-routine declaration** (`var` section, field, key, dataitem,
  `ERROR`) is still dropped, deliberately.
- **Same-signature procedures duplicated across `#if` arms** with different attributes: the
  program build still keeps only the first copy's (Discovery D1, P17).
- **`#define`/`#undef` inside an object body do not decide later `#if`s** (F1-09). The lowerer has
  never applied them to any `#if` decision: routine collection also walks an `#if not X` arm after
  a `#define X` as live. An attribute that crosses one follows the same union-read. Before this
  change, such an attribute was dropped entirely.
- **Several readings of one attribute on one routine** (F1-02). When an attribute reaches a
  routine by paths that read its `#if` arguments differently, the routine gets one `AttributeIr`
  per reading. Consumers that take the first matching attribute (`publisher_include_sender`,
  `event.rs:302`) see only the first reading. Discovery D2.
- **Literals only in the running context** (F1-08). A group chosen under a compound condition
  such as `X or Y` records no literal, so it does not constrain later groups in the same list
  (`sibling_groups_under_a_disjunction_are_not_correlated` pins this).
- **Variable and parameter attributes** (`var_attribute_item`; `attribute_item` in
  `parameter_list`) are not lowered, before or after; nothing reads them (P15).
- The P19 census is a line heuristic and an estimate; the CDO before/after delta is the
  measured effect.
