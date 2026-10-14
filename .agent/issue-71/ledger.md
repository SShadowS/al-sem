# Issue #71 ledger — digest changed-root resolution matches start lines, not routine spans

## Claim

```json
{"run_id":"20261014-155631-70caa1","issue":71,"attempt":2,"session":"claude-code-local:bb3fcebb-4fcc-42e9-82de-a5b63c6b6cfa","branch":"issue/71-agent-discovery-digest-changed-a2","worktree":"U:\\Git\\al-sem-issue-71-a2","body_hash":"b1dfb24a63ae1e5677e091e2e1568f86fb99783b8679ae64bd19eac8a73c60e9"}
```

Body hash: `b1dfb24a63ae1e5677e091e2e1568f86fb99783b8679ae64bd19eac8a73c60e9`

## Classification

**Bounded.** One subsystem (changed-root resolution, `resolve_changed_roots` in
`src/engine/l5/digest_cli.rs`, shared by `digest` and `prove`, plus the digest entry
anchor that reads the same per-routine data), one root cause (the file → routine index is
built from snapshot evidence start lines instead of routine declaration spans), and the
right data already exists in the model (`ModelRoutine.source_anchor`, the IR routine
node's range). The design reads that one span, adds a small additive plumb for attribute
lines (one IR field, one model routine field), one model object field carrying
Interface/ControlAddIn procedure declarations, and makes the diff parser read hunk
bodies with a line-count-bounded state machine. Existing diagnostic kinds, JSON keys and
exit codes keep their spelling; no diagnostic kind is added; the new entry keys
(`routine.declarationOnly`, `routine.anchors`) are additive and bump the digest schema
1.4.0 → 1.5.0 (1.4.0 shipped in v1.7.0, P21); prove's schema stays 1.3.0; no new consumer.
Golden moves are the `schemaVersion` line of all 21 digest JSON goldens plus field moves
in 6 of them and 3 digest human files, predicted field by field below. Not a spike (the answer is known and
reproduced), not architectural (no new layer).

## Assumption probes

Attempt 2 base is `52e64d1a` (attempt 1 probed `66b62288`). Between the two, #68 and the
disk-hygiene chore changed, of the files this design cites, ONLY `src/engine/l5/digest_cli.rs`
(the envelope-diagnostics helpers: `DEFAULT_DETECTOR_NAMES` and `default_detectors()` removed,
`build_envelope_diagnostics_json*` now call `compute_analyzer_diagnostics` /
`compute_analyzer_diagnostics_cross_app`, `digest_cli.rs:40-64`) and one comment line in
`prove.rs:899` (no line shift). That shifts every later `digest_cli.rs` line by -59 (-58 for
`strip_scheme`/`normalize_input`); every citation below is re-read on `52e64d1a`.
`diff_parser.rs`, `snapshot.rs`, `src/program/**`, `crates/**`, `src/engine/ids.rs`,
`tests/cli/cli_digest_query_dependencies.rs`, `tests/cli-b-goldens/**` and
`tests/ir-l2-goldens/**` are byte-identical (`git diff --stat 66b62288 52e64d1a -- <paths>`
prints nothing). The 16 `ModelRoutine {` and 6 `ModelObject {` literal files (Data plumb 2,
5) are the same lists on `52e64d1a`. The dev profile's line-tables-only debug info changes
no behaviour cited here. bc-dev-mcp's `src/core/risk.ts` moved after attempt 1 (`db13079`,
`5a50a93`, 2026-10-14); its citations are re-read at `715e0b9`.

Binaries: attempt 1 `target/release-fast/alsem.exe` at `66b62288`; attempt 2 the same
profile rebuilt in `U:/Git/al-sem-issue-71-a2` at `52e64d1a` (P18). Scratch:
`C:/Users/SShadowS/AppData/Local/Temp/claude/U--Git-al-call-hierarchy/bb3fcebb-4fcc-42e9-82de-a5b63c6b6cfa/scratchpad/71/`
(`ws` = copy of `tests/r0-corpus/ws-d8-commit-in-tx`; `ws2` = shapes probe; `ws3` = one
op-less procedure; `p*.patch` = hand-written diffs; `sweep.py` = one single-line hunk per
source line). Round-1 panel probes by flash are in that session's `scratchpad/rv71/`.

| probe | result | evidence |
|-------|--------|----------|
| P1. The issue reproducer: a hunk over an op-less line inside a routine body | **Confirmed** (re-run on `52e64d1a`, P18). Exit 2, `hunk-outside-routines`, zero roots. | `p1-assign.patch` (`@@ -9,1 +9,1 @@`, line 9 `Header."Status Posted" := true;`) → `exit=2`, diags `[{"kind":"hunk-outside-routines","file":"src/posting.al","startLine":9,"endLine":9}]`, roots `[]`. |
| P2. How much of a routine body is reachable today | **Worse than the issue says.** 5 of the 14 lines of `PostSalesDoc` (3-16) match: 8, 10, 12, 13, 15. 2 of the 10 lines of `HandlePosted` (3-12, attribute included) match: 10, 11. The header, `var` section, `begin`, `end;`, assignments, `SetRange` (11) and `Init` (subscriber 8) miss. The empty publisher `OnAfterPostSalesDoc` matches on no line. | `sweep.py` over `ws/src/posting.al` and `ws/src/subscriber.al`. The index holds only (a) operations that witness a capability fact (`derive_operation_evidence`, `src/engine/l5/snapshot.rs:1404-1430`, the `referenced` set) and (b) call sites that witness a fact **or anchor a typed edge** (`derive_callsite_evidence`, `snapshot.rs:1515-1542`). It is not "every operation". Built at `build_routine_file_index`, `digest_cli.rs:191-227`. |
| P3. `--file` on a file whose routines have no indexed op/callsite | **Confirmed FileUnmatched** (re-run, P18). | `ws3/src/Pure.al` (`procedure Add(...) begin exit(A + B); end;`): `alsem digest ws3 --file src/Pure.al` → `exit=2`, `[{"file":"src/Pure.al","kind":"file-unmatched"}]`; `--routine Add` → exit 0, root `Add`. `FileUnmatched` is pushed at `digest_cli.rs:253-264`. |
| P4. Routines silently dropped from a matched file | **Confirmed, with real effects lost** (re-run, P18). | `ws2 --file src/Shapes.al` → 9 roots; `OnPub` and `Dup` are missing. In the d8 fixture `OnAfterPostSalesDoc` is not a root of `--file src/posting.al`, yet `--routine OnAfterPostSalesDoc` reports 3 transitive effects (COMMIT, DB_INSERT `Audit Entry`, an event). |
| P5. Where routine declaration spans live | **Model, not snapshot.** `CapabilitySnapshot` (`snapshot.rs:813-828`) has no routine span; its only routine anchors are `root_classifications[].source_anchor` (entry-point routines only), built as `anchor_from_panchor(&r.source_anchor, …)` (`snapshot.rs:2098-2100`). The span is `ModelRoutine.source_anchor: PAnchor` (`src/program/model/workspace.rs:495`), set at `workspace.rs:1257` from `anchor_from_origin(&ir_routine.origin, …)`; `RoutineDecl.origin = origin_of(node)` (`crates/al-syntax/src/lower/mod.rs:1585`). Lines are 0-based. `digest` and `prove` hold the `Model` when they call `resolve_changed_roots` (`digest_cli.rs:1340`, `prove.rs:908`). | Golden anchors from root slots agree: `PostSalesDoc` anchor line 2 = `procedure` row 2; `HandlePosted` line 3 = `procedure` row 3 (its attribute is row 2). |
| P6. Shape coverage of that span | **Every bodied routine shape; attributes excluded.** `tree-sitter parse ws2/src/Shapes.al`: `OnRun` [2,4]-[5,8]; table field trigger [46,12]-[49,16]; table trigger [53,4]-[56,8]; page field trigger [69,16]-[72,20]; action trigger [83,16]-[86,20]; report dataitem trigger [101,12]-[104,16]. `#if` with two whole procedures (`Dup`) → two `procedure` nodes [19..22], [24..27]. `#if`-split header (`Split`) → one `preproc_split_procedure` [30,0]-[37,8]; the lowerer emits one `RoutineDecl` per arm (`header_arms`, `lower/mod.rs:1070-1110`), all with `origin_of(node)`. Attributes are SIBLINGS before the routine node (`attribute_item [7,4]-[7,90]` then `procedure [8,4]-[11,8]`), collected as `pending` (`collect_routines`, `lower/mod.rs:823`) with no origin kept (`AttributeIr { name, raw, args }`, `ir/decl.rs:98`). AL has no nested procedures; member triggers nest in a field/action/dataitem wrapper, never in another routine. | `p4-attr.patch` (line 18 `[IntegrationEvent(...)]`) and `p8-subattr.patch` (subscriber line 3) → exit 2, `hunk-outside-routines` today. |
| P7. File keys: index vs diff | **Compatible, except two edge forms.** Index key `normalize_input(source_unit_id)` = strip `ws:` / `app:<x>:`, `\`→`/`, lowercase (`digest_cli.rs:84-100`). Workspace routines are `ws:<rel_posix>` (`workspace.rs:1646`); dependency source routines are `dep:<guid>:<path>` (`workspace.rs:1517`), never stripped; ABI-only routines have an empty `source_unit_id` (`abi_rows.rs:219`). Diff side: `a/`/`b/` stripped, unquoted, forward-slashed. | `SRC/Posting.AL` diff and `--file src\posting.al` match. Mismatches (pre-existing, Discoveries 1-2): `./src/posting.al`, and a diff made from a repo root above the workspace (`a/Cloud/src/posting.al`). |
| P8. A hunk outside every routine but inside an object | **`hunk-outside-routines` today and under the new rule.** | Sweep lines 1-2 and 22-50 of posting.al; `p6-tablefield.patch` → exit 2. |
| P9. Hunk header ranges include context lines | **Today's matching over-reports through context** (re-run, P18). A whitespace-only change on the blank line between two routines makes `PostSalesDoc` a root. A span-range rule over the header range would also add `OnAfterPostSalesDoc`. | `p11-context.patch` (`@@ -14,7 +14,7 @@`, only line 17 changed) → exit 0, roots `['PostSalesDoc']`. The parser keeps only header numbers (`DiffHunk { new_start, new_count }`, `diff_parser.rs:10-16`; `old_count` is parsed and dropped at `:167-172`). The hunk loop that matches the header window is `digest_cli.rs:335-357`. |
| P10. Pure-deletion hunks | **Never match today.** `new_count == 0` gives an empty window. Git writes `new_start` as the line BEFORE the gap when `new_count == 0` (`@@ -9,1 +8,0 @@` deletes old line 9; the gap is between new lines 8 and 9). | `p7-deletion.patch` → exit 2, `hunk-outside-routines`. |
| P11. Golden families / tests that exercise `--file` / `--diff` / `--routine` | `cli_b_digest_differential.rs` runs 20 `DIGEST_CORPUS` fixtures with every `.al` file as `changed_files` (→ `tests/cli-b-goldens/digest/<f>.json` + `.human.txt`) and `ws-d8-commit-in-tx.changed.diff` (→ `.diff.json`). `cli_b_digest_exit_oracles.rs` uses `--routine` (zero roots) and `--file` only for a bad-format error. `cli_digest_query_dependencies.rs` uses `--file src/Work.al` (`DoWork`, `CallLib`, both indexed today). `cli_digest_temp_state.rs` asserts per routine name. `prove` passes a routine selector only (`prove.rs:902-908`); `fingerprint` has its own selector path and never calls `resolve_changed_roots`. | Golden entries vs routines in `tests/ir-l2-goldens/l2_features.snapshot`: equal for 17 fixtures; differ for 3: `ws-d8-commit-in-tx` 2 of 3 (no `OnAfterPostSalesDoc`), `ws-d34` 5 of 6 (no `DoWork`), `ws-d14-dead-routine` 1 of 3 (no `LiveHelper`, `DeadHelper`). `--routine` on each missing routine: `OnAfterPostSalesDoc` 3 effects, `DoWork`/`LiveHelper`/`DeadHelper` 0 effects. Six goldens carry `[{"kind":"file-unmatched","file":"src/Rec.Table.al"}]` today (`ws-txn-d47-neg-temp`, `d47-pos-file`, `d47-pos-http-commit-after`, `d47-pos-http-nocommit`, `d49-pos-modify-message`, `d49-pos-modify-runmodal`); each `Rec.Table.al` is a table with no trigger and no procedure (`grep -il "trigger\|procedure"` matches none), so under the new rule it still has no routine and the diagnostic stays. All golden facts hold on `52e64d1a` (the goldens are byte-identical to `66b62288`). |
| P12. The model's routine population (round-1 critical) | **Interface and ControlAddIn procedures are NOT model routines.** The lowerer emits a `RoutineDecl` for every `interface_procedure` (`lower/mod.rs:840-855`), but the model skips both object kinds (`if object_type == "Interface" \|\| object_type == "ControlAddIn" { continue; }`, `workspace.rs:881-904`) and routines with an empty name (`workspace.rs:926-929`). So they have no `stable_routine_id`, no identity-table row (`snapshot.rs:1087-1110` reads `ws.routines`), no coverage and no effects. A root for one could only produce `root-not-in-snapshot`. Their OBJECT rows do exist: `ModelObject` is pushed (`workspace.rs:843`) before the skip, with `object_type` and `source_anchor` (file), and `ir_object_type` maps both kinds to `Some` (`src/program/body/ir_walk.rs:2650-2651`). No `DIGEST_CORPUS` fixture holds an interface or controladdin object. | Source lines named; `grep -liE "^\s*(interface\|controladdin)\b"` over the 20 fixtures' `src/*.al` → no match. Consequence: Design → Declaration-only procedures. |
| P13. Distinct ids vs declarations | **Shapes has 12 routine declarations but 11 distinct stable ids.** The two `Dup(): Integer` arms have identical identity inputs (`workspace.rs:1269-1288`), so one id has two spans; the two `Split` arms differ by signature (two ids, one span). Roots are a set of ids (`digest_cli.rs:244`). | flash, re-run in P18: `--routine Dup` → one entry (anchor line 24, the `#else` arm, a root-slot anchor: today's root-slot loop is last-wins, `digest_cli.rs:569-581`); `--routine Split` → `selector-ambiguous ['Split','Split']`. |
| P14. Attribute collection stops at trivia | **An attribute separated from its routine by a comment, pragma or `#if` is lost.** `collect_routines` clears `pending` on any other named child (`_ => pending.clear()`, `lower/mod.rs:984-986`; the preproc arms clear too, e.g. `:936`), and comments are named extras. | flash: a subscriber whose `[EventSubscriber]` is followed by a `// comment` line stops binding (`Fire`'s digest drops COMMIT). Recorded as the "al-syntax / lowerer (routine attributes)" discovery; stated as a limit in the Design. |
| P15. The digest consumer needs the routine anchor | **Routines that #71 newly surfaces get no `routine.anchor` today.** `anchor_by_rid` reads root slots, then the first indexed op, then the first indexed callsite (`digest_cli.rs:568-611`). bc-dev-mcp joins digest entries to changed procedures by `routine.anchor.file` + `line` and treats a missing anchor as file `""`, line 0 (`U:/Git/bc-dev-mcp/src/core/risk.ts:99-110`); its doc says it avoids `--diff` because of exactly #71 (`risk.ts:12-18`). In the goldens, 4 existing entries are anchored INSIDE the body by the op fallback: `ws-d34` `Persist` (line 47, `Commit();`), `ws-d35` `PersistAudit` (31), `ws-txn-d46-neg` `DoSetup` (31), `ws-txn-d48-pos` the second `SendOne` (39, `Sender.SendOne();`). All others are anchored at the declaration by a root slot, whose anchor IS `source_anchor` (P5). The human output never prints the anchor (`format_digest_human`, `digest_cli.rs:1158-1176`). | Script over `tests/cli-b-goldens/digest/*.json` comparing each anchor line's text with `procedure`/`trigger`; flash: `--routine DoWork` → no anchor. |
| P16. Diff parser dispatch order | **Header tests run before anything else, on every line.** `--- ` (`diff_parser.rs:258-270`), `+++ ` (`:272`), `rename from` (`:333`), `diff --git` (`:343`) are tested on each line; every other line is ignored (`:377`). A deleted body line whose text starts `-- ` or an added one starting `++ ` is taken as a file header. Two unit tests (`simple_modified_file`, `rename_with_hunks_is_renamed`, `diff_parser.rs:414-430`, `:467-495`) use bodies shorter than their header counts (`-10,3 +10,4` with one `-` and one `+` line). The parser never records an error today (`let errors: Vec<String> = Vec::new();`, `diff_parser.rs:221`). A hunk header attaches to the CURRENT file (`diff_parser.rs:371-376`). Attempt 1 cited `:257-268`, `:271`, `:371`, `:363-369`, `:216-217` for these; the file is unchanged, those were off by one to eight lines and are corrected here. | Source lines named. |
| P17. Round-2 source checks | (a) Unnumbered objects share one object id: `encode_object_id` is `{app}/{type}/{number}` (`src/engine/ids.rs:130-132`) and the model uses `o.id.unwrap_or(0)` (`workspace.rs:718`), so every Interface/ControlAddIn is `<guid>/Interface/0` / `<guid>/ControlAddIn/0`. (b) Anchor columns are UTF-8 BYTE columns: `Utf16Cols::col` returns `byte_col` verbatim (`src/program/body/node_util.rs:21-37`). (c) The diff splitter yields a final empty element for text ending in `\n` (`normalized.split('\n')`, `diff_parser.rs:217-218`). (d) Prove reads the entry anchor: `run_digest_query_full_from_entries` (`prove.rs:965`) → `routine_anchor` (`prove.rs:694-700, 1002`). (e) The routine-slot anchor loop is last-wins per id (`digest_cli.rs:569-581`). (f) The embedded dependency fixture puts `dep:{guid}:src/Lib.al` routines in the model only when `CallLib` demands them (`cli_digest_query_dependencies.rs:48-64, 89-108, 251-270`; `workspace.rs:1502-1506`). | Source lines named. |
| P18. Attempt-2 re-run on `52e64d1a` (master moved: #68) | **No result moved.** #68 touched only the envelope-diagnostics path, not root resolution, the entry builder or the parser, and every re-run matches attempt 1. | `target/release-fast/alsem.exe` built in the a2 worktree at `52e64d1a` (exit 0). `ws --diff p1-assign.patch` → exit 2, `[hunk-outside-routines 9-9]`, roots `[]` (P1). `ws3 --file src/Pure.al` → exit 2, `[file-unmatched src/Pure.al]` (P3). `ws2 --file src/Shapes.al` → exit 0, 9 roots (`Sub`, `Split`×2, `OnRun`, `OnAction`, `OnValidate`×2, `OnAfterGetRecord`, `OnInsert`), no `OnPub`, no `Dup` (P4). `ws --file src/posting.al` → roots [`PostSalesDoc`] only; `ws --routine OnAfterPostSalesDoc` → exit 0, no anchor, effects `COMMIT`, `DB_INSERT`, `EVENT_PUBLISH` (P4, P15). `ws --diff p11-context.patch` → exit 0, roots [`PostSalesDoc`] (P9). `ws --diff p7-deletion.patch` → exit 2, `hunk-outside-routines` (P10). `ws2 --routine Dup` → one entry, anchor line 24; `--routine Split` → `selector-ambiguous ['Split','Split']` (P13). The goldens behind P11/P15 are byte-identical to `66b62288`. |
| P19. The shared entry builder vs a root the snapshot does not know (round-3 R3-01) | **Such a root is dropped or counted twice today.** `run_digest_query_full_from_entries` (`digest_cli.rs:529`) skips a root with no identity-table display and pushes `RootNotInSnapshot` (`:623-631`), and pushes `NoCoverageRecord` for a root with no coverage record while still building the entry (`:633-641`). Each query diagnostic becomes an envelope diagnostic `DIAG-digest-root-not-in-snapshot` / `DIAG-digest-no-coverage-record` (`:1035`, `:1043`) and adds 1 to `rootsRequested` (`= roots.len() + query_diagnostics.len()`, `:1069-1070`); `rootsResolved = entries.len()` (`:1071`). The identity table holds `ws.routines` only (`snapshot.rs:1087-1110`), so a declaration-only id is never in it. `compute_digest_effects_cli` keeps only `reportable_roots(resolved)` ∩ the requested roots (`src/engine/l5/digest.rs:3568-3586`), so an unknown id yields no base entry and no diagnostic there. | Source lines named. Consequence: Design → Data plumb 6 builds declaration-only entries before those lookups. |
| P20. The bc-dev-mcp reader, re-read at `715e0b9` (round-3 R2-02) | **One entry per id, one anchor per entry, joined by span.** The provider runs `alsem digest <project> --file <f>... --deterministic --out <file>` (`risk.ts:130`), never `--diff` (`risk.ts:12-18`). `parseAlSemDigest` keeps one `DigestEntry` per digest entry, reading only `routine.anchor.file` (`ws:` stripped) and `routine.anchor.line + 1` (`risk.ts:99-110`); `routine.anchors` is ignored. `findRisk` keeps entries with the same object type, object id and display, then those whose anchor line lies in the subject's `[startLine, endLine]` (or `>= startLine` when `endLine` is absent) in the same file, and takes the lowest line (`risk.ts:158-169`; the comment at `:165` says the subject starts at the first attribute and al-sem anchors at the keyword). Subjects normally carry `endLine` (`al-procedures.ts:866, 931`, `coverage-gaps.ts:137`). So for one id with two declarations a singular anchor can join exactly one declaration's subject when `endLine` is set. | Source lines named. Consequence: Design → Entry anchor. |

| P21. Digest/prove schema state on `52e64d1a` (spec round 1 of attempt 2: R1-11, R1-26) | **Digest 1.4.0 is RELEASED; prove is 1.3.0.** `CHANGELOG.md:79` is `## [1.7.0] - 2026-10-14`, and its section records "Digest `schemaVersion` 1.3.0 -> 1.4.0 and prove 1.2.0 -> 1.3.0 (#65) … #66 bumps digest to 1.4.0 as well" (`:115-121`); `[Unreleased]` (`:8`) holds no digest bump. The digest envelope sets `"1.4.0"` at `digest_cli.rs:1143` (comment `:1141-1142`); prove sets `"1.3.0"` at `prove.rs:725`. Prove's routine ref projects only `anchor` (`prove.rs:692-702`). Golden counts: 21 `tests/cli-b-goldens/digest/*.json`, each with exactly one `"schemaVersion": "1.4.0"` line; no digest `.human.txt` mentions a schema version; 19 `tests/cli-b-goldens/prove/*.json`, all `"1.3.0"`. Tests pinning the digest version: `tests/cli/cli_digest_query_dependencies.rs:169`, `tests/cli/cli_digest_temp_state.rs:142`. | `grep -c schemaVersion`, `grep -l`, `sort \| uniq -c` over the golden dirs; `grep -rn 1.4.0 src tests docs`. Consequence: Data plumb 7 bumps digest to 1.5.0. |

No probe falsified the issue's claim. P2, P4, P9 show it is wider than stated. P12 limits
which declarations the fix can reach (see Design → Population).

## Design

**Rule.** A routine is a changed root iff a CHANGED line of the input touches one of its
declaration spans. A span is the 0-based inclusive line range `[decl_start, end]`:
`end = source_anchor.end_line` (the IR routine node's last row), `decl_start =
min(source_anchor.start_line, the start row of every attribute the lowerer attached to
the routine)`. Spans come from ONE place, the model's routines
(`resolved.workspace.routines`), keyed by `stable_routine_id` (the id the snapshot,
identity table and selector index use). The snapshot's `operation_index` /
`callsite_index` are no longer read for root resolution or for the entry anchor.

**Population (exact).** The indexed population is every model routine whose
`source_unit_id` starts with `ws:`: procedures, triggers, object/field/page-field/action/
dataitem triggers, event publishers and subscribers, both arms of an `#if` duplicate and
every arm of an `#if`-split header, bodied or not — PLUS every declaration-only
procedure (below). A routine with an empty name stays out (malformed; the model drops it,
`workspace.rs:926-929`).

**Declaration-only procedures (the acceptance, literally).** Interface and ControlAddIn
procedure declarations are routines with a span, so a hunk inside one makes it a changed
root, and a file holding them is never `FileUnmatched`. The model keeps skipping them as
`ModelRoutine`s (`workspace.rs:880-904`): removing that skip would let detectors such
as unused-parameter fire on signature-only methods, a separate change (the "stale
Interface/ControlAddIn skip" discovery records that the skip's stated reason, parity with
the retired al-sem TS reference, is gone). Instead they ride on the object row
(Data plumb 5), built from the same IR in that loop before the `continue`, and digest
emits a root entry for each touched one:
- identity: a `stableId` unique per declaring object, built with the same hashing
  scheme as `stable_routine_id` (`workspace.rs:1121-1288`) over an object key that
  carries the object NAME:
  `to_stable_routine_id_from_parts(to_stable_object_id(object_id),
  normalized_signature_hash(name, ir_parameter_symbols specs, return_type),
  Some(<unescaped object name>))`. The third argument is the existing discriminator
  fold (`sha256_of_strings([sig_hash, key.to_lowercase()])`, `src/engine/ids.rs:315-328`),
  unused by these procedures otherwise (an interface method has no enclosing member).
  Object names are unique per object kind within an app (lowercased, as AL identifiers
  are case-insensitive), so two interfaces declaring the same `procedure Bar()` get two
  different ids. The object segment stays `<guid>:<Kind>:0` and the result keeps the
  load-bearing `{stableObjectId}#{64 hex}` shape (`ids.rs:290-298`; bc-dev-mcp reads
  the object segment as a number, `risk.ts:99`), which is why the name goes into
  the hash and not into the object segment. `display` = the procedure name;
- `routine.declarationOnly: true` (new key, present only on these entries);
- `effects: []`, `unresolved: []`, `unresolvedTraversal {maxDepth: 64, truncated: false,
  visitedRoutines: 0}`, `coverage {status: "unknown", reasons: ["declaration-only"]}`
  (an existing status; one new reason string);
- `routine.anchor` from its declaration (same rule as Entry anchor below);
- counted ONCE in `rootsRequested` and once in `rootsResolved`, with no query diagnostic
  (Data plumb 6, P19); exit 0 when any root exists.
Interfaces and controladdins have no object number, so every one of them shares the
object id `<guid>/<Kind>/0` (`encode_object_id`, `src/engine/ids.rs:130-132`;
`workspace.rs:718`; P17). With the name-carrying key above, #71's new roots never
inherit that collision: two interfaces declaring the same signature yield two roots with
distinct ids (A2d(c)). The general object-id collision, which other consumers of the
object id still see, is the "unnumbered object id" discovery. These entries are sorted
with the others by `routineId`. The `file-declarations-only` /
`diff-file-declarations-only` diagnostics of an earlier revision are NOT part of the
design: every file that declares any routine now yields roots, so no case needs them.
`--routine Bar` selectors do not reach declaration-only procedures (the selector index
reads the snapshot; unchanged, stated under Known limits). All new keys/strings are
additive and ship under digest schema 1.5.0 (Data plumb 7). Mapping an interface method change to
its implementers stays the Interface/ControlAddIn discovery. Dependency isolation is by the
`ws:` prefix, one stated invariant: dependency source routines are `dep:<guid>:<path>` and
ABI-only ones have an empty unit (P7). No app-guid filter is added.

**Data plumb.**
1. `crates/al-syntax/src/ir/decl.rs`: `AttributeIr` gains `origin: Origin`, set to
   `origin_of(item)` at its one construction site (`lower/mod.rs:1452`).
2. `src/program/model/workspace.rs`: `ModelRoutine` gains `declaration_start_line: u32`,
   set at `:1257` beside `source_anchor` to the minimum of `ir_routine.origin.start.row`
   and every `ir_routine.attributes_parsed[i].origin.start.row`. `source_anchor` is NOT
   changed (census and event joins key on it: `census.rs:95-113`, `events.rs:370`; d19/d29
   report it). Every other `ModelRoutine { … }` literal gets the field (0 or the anchor's
   start): `abi_rows.rs`, `program_calls.rs`, `program_calls_adapter_tests.rs`,
   `gate/format_pr_summary.rs`, `l4/cfg_walker.rs`, `l4/db_effect_solver.rs`,
   `l5/d1_temp.rs`, `l5/detectors/d17.rs`, `l5/fingerprint.rs`, `l5/table_display.rs`,
   `l5/test_support.rs`, `engine/return_summary.rs`, `engine/root_classification.rs`,
   `tests/l4_summary_differential.rs`, `tests/temp_state/temp_state_path.rs`,
   `tests/temp_state/temp_state_shadowing.rs` (re-grepped on `52e64d1a`: same 16 files).
3. `src/engine/l5/digest_cli.rs`: `build_routine_file_index(snap)` (`:191-227`) is replaced
   by `build_routine_span_index(resolved: &Model) -> RoutineSpanIndex` with
   `struct RoutineSpan { routine: String, source_unit_id: String, start: u32, end: u32,
   anchor_line: u32, anchor_column: u32, declaration_only: Option<DeclOnlyInfo> }`
   (`DeclOnlyInfo` carries the procedure's display name). One `RoutineSpan` per
   DECLARATION (never collapsed by id), in document order. It keeps the ORIGINAL
   `source_unit_id` (e.g. `ws:src/posting.al`, case kept) for the anchor; only the LOOKUP
   KEY is normalized: `by_file: HashMap<normalize_input(source_unit_id), Vec<usize>>` and
   `by_routine: HashMap<routine id, Vec<usize>>` index into one `Vec<RoutineSpan>`. Model
   routines and declaration-only procedures (Data plumb 5) enter the same vector.
   `resolve_changed_roots` (`:239`) gains `resolved: &Model` (both callers hold it) and
   returns, besides `roots`, the TOUCHED declaration indices per root (the anchor rule
   below): for `--diff`, the declarations whose span a change touches; for `--file` and a
   rename without hunks, every declaration of that id in that file; for a `--routine`
   selector, every declaration of that id. Touched sets of one id from several inputs are
   unioned.
4. `src/engine/l5/diff_parser.rs`: `DiffHunk` keeps `new_start`/`new_count`, gains
   `old_count` and `changes: Vec<HunkChange>`, `enum HunkChange { Added(i32), DeletedAfter(i32) }`
   (1-based new-file numbers; `DeletedAfter(L)` = a gap between new lines L and L+1).
   **Count-bounded hunk state machine.** After a valid `@@` header the parser is IN the
   hunk body until it has consumed `old_count` old-side and `new_count` new-side lines,
   and header tests (`--- `, `+++ `, `diff --git`, `rename …`) are NOT applied while in
   a body. The input is split on `\n` after CRLF normalization (`diff_parser.rs:217-218`);
   when the text ends with `\n`, the final empty split element is the terminator, not a
   line, and is dropped before parsing, so only a PHYSICAL blank line can be read as
   context. In a body: ` ` or an EMPTY physical line = context (one old, one new; cursor +1);
   `-` = old only (a run of `-` lines records one `DeletedAfter(cursor - 1)`); `+` = new
   only (`Added(cursor)`, cursor +1); `\ …` = ignored, not counted. A line is accepted as
   body only while its side still has count left: `-` needs old-side count, `+` needs
   new-side count, context needs both. The cursor (next new-file line) starts at
   `new_start`, or at `new_start + 1` when `new_count == 0` (git's convention, P10). The
   hunk is COMPLETE when both counts reach zero; the next line is then a top-level line.
   Any line that is not accepted as body before the counts are exhausted, or end of input,
   ends the hunk as INCOMPLETE: a `DiffParseError` ("hunk at +N truncated", N =
   `new_start`) is recorded and the hunk keeps only the changes read so far; the line that
   ended it is then processed as a TOP-LEVEL line (header handling), never dropped. A
   header line that a body still accepts is body: while old-side count remains a line
   starting `-` is a deleted line, and while new-side count remains a line starting `+` is
   an added line, so a bare `--- a/next.al` / `+++ b/next.al` pair after a short hunk in
   plain `diff -u` concatenation (no `diff --git`) is read as body. The pinned outcome for
   that malformed input, with explicit counts, is A1g(c2) (truncated), A1g(c2-overrun) (a `-`
   line after the old count is spent) and A1g(c2-exact) (counts filled exactly). There is no header-range
   fallback: context is never treated as change. The two unit tests with short bodies
   (P16) keep passing on what they assert (file kind, path, header numbers) and will
   now also carry one parse error each; their bodies are corrected to match their
   counts so they test well-formed input.
5. `src/program/model/workspace.rs`: `ModelObject` (not `Serialize`, `workspace.rs:37`)
   gains `declaration_only_routines: Vec<ModelDeclarationOnlyRoutine>` with
   `struct ModelDeclarationOnlyRoutine { name: String, stable_routine_id: String,
   parameters: Vec<ModelParameter>, return_type: Option<String>, source_anchor: PAnchor,
   declaration_start_line: u32 }`. It is filled in the object loop for `Interface` /
   `ControlAddIn` objects only, from `o.routines` (named ones), with the SAME helpers
   the routine loop uses (`ir_parameter_symbols`, `normalized_signature_hash`,
   `to_stable_routine_id_from_parts`, `anchor_from_origin`, the Data plumb 2
   attribute-start rule), and the object row is pushed with it before the `continue` at
   `:903`. Empty for every other object kind. `ir_object_type` maps both kinds to `Some`
   (`ir_walk.rs:2650-2651`), so the rows exist. Other `ModelObject { … }` literals get
   `Vec::new()`: `abi_rows.rs`, `l5/detectors/d17.rs`, `l5/detectors/d50.rs`,
   `l5/fingerprint.rs`, `engine/return_summary.rs`, `engine/root_classification.rs`
   (re-grepped on `52e64d1a`: same 6 files). `build_routine_span_index` reads it from
   `ws:` object rows only.
6. Entry building for BOTH callers. `run_digest_query_full_from_entries`
   (`digest_cli.rs:529`, shared by digest and prove: `prove.rs:965`, anchor projected at
   `prove.rs:694-700, 1002`) stops building `anchor_by_rid` from root slots / ops /
   callsites (`digest_cli.rs:568-611`) and takes `&RoutineSpanIndex` plus the per-root
   touched declaration indices. Digest and prove both build the index from their `Model`;
   prove's roots come from a `--routine` selector, so every declaration of the id is
   touched (Data plumb 3). **Declaration-only roots are built FIRST** (R3-01, P19): for a
   root whose declarations carry `declaration_only: Some(..)`, the entry is built entirely
   from the span index (display, anchor, the fixed empty effects / unresolved /
   `unresolvedTraversal` / `coverage {unknown, [declaration-only]}` of the Declaration-only
   section) and the loop continues, BEFORE the identity-table display lookup
   (`digest_cli.rs:623-631`), the coverage lookup (`:633-641`), the base-entry lookup and
   the unresolved cone. It pushes no `RootNotInSnapshot` and no `NoCoverageRecord`, so it
   adds no `DIAG-digest-*` envelope diagnostic (`:1035`, `:1043`) and is counted exactly
   once in `rootsRequested` (`= roots.len() + query_diagnostics.len()`, `:1069-1070`) and
   once in `rootsResolved` (`= entries.len()`). Model routines take the existing path
   unchanged.
7. Digest schema bump 1.4.0 → 1.5.0 (P21). 1.4.0 SHIPPED in al-sem v1.7.0
   (`CHANGELOG.md:79`, `[1.7.0] - 2026-10-14`, which carries the #65/#66 bump at
   `:115-121`), so the attempt-1 reasoning ("1.4.0 is unreleased, one unreleased minor
   covers it") is stale. `routine.declarationOnly`, `routine.anchors` and the
   `declaration-only` coverage reason are new additive keys/values, so the minor moves:
   `digest_cli.rs:1141-1143` sets `"1.5.0"` and its comment names #71. A minor (not
   major) because every key is additive and existing readers that check the major only
   keep working (bc-dev-mcp checks major 1, `risk.ts:90-93`). The two tests that pin the
   digest version move with it: `tests/cli/cli_digest_query_dependencies.rs:169` and
   `tests/cli/cli_digest_temp_state.rs:142` (`"1.4.0"` → `"1.5.0"`). **Prove stays
   `1.3.0`** (`prove.rs:725`): prove's routine ref projects only `anchor` (`prove.rs:692-702`),
   and #71 adds neither `declarationOnly` (no declaration-only root can reach prove, whose
   roots come from a selector over the snapshot) nor `anchors` to it; prove's anchor
   changes VALUE only (declaration instead of the op/callsite fallback), the same field
   and shape, and no prove golden's value moves (license).

**Matching.**
- `--file F`: every indexed declaration of `F` (model routines and declaration-only
  procedures) is a root. `FileUnmatched` means exactly "`F` declares no routine at all,
  interface methods included" (e.g. a table with no trigger, an empty interface).
- `--diff`, per file: `DiffFileDeleted` unchanged. `DiffFileUnmatched` means "the path
  declares no routine at all". A rename with no hunks takes every declaration of the
  file (unchanged).
- Per hunk: `Added(n)` touches a span when `start+1 <= n <= end+1`. `DeletedAfter(L)`
  touches a span only when the gap is interior: `start+1 <= L` and `L+1 <= end+1`. So
  deleting a whole routine, or blank lines between routines, blames no neighbour, and a
  deletion at the start of the file (`L = 0`) blames nothing. Every touched span's
  routine id is a root.
- `HunkOutsideRoutines` means "this hunk's changes touch no span" (object header, field
  declaration, property, the object's global `var` section, a blank line or comment
  between routines, an `#if`/`#else`/`#endif` line around a duplicated procedure). It is
  still reported with the hunk header's range; a file with one hunk inside and one
  outside gives a root AND the diagnostic, exit 0. Exit 2 still means zero roots.
- Overlap: spans never nest (P6). Arms of an `#if`-split header share one span and each
  arm's id is a root. Two declarations with one id (`Dup`) are two spans; a hunk in
  either yields that id once as a root, and the resolver records WHICH declarations were
  touched (multiplicity kept, see Entry anchor).

**Entry anchor (pulled into #71 from the round-0 anchor discovery).** `routine.anchor` =
`{file: <original source_unit_id>, line: source_anchor.start_line, column:
source_anchor.start_column}` of ONE declaration: **the LAST touched declaration of that id
in document order**, with "touched" as defined in Data plumb 3 (`--diff`: the declarations
whose span a change touches; `--file`, a rename without hunks, `--routine` and prove: every
declaration of the id). For a single-declaration id this is that declaration. When an id has
more than one declaration, the entry also carries `routine.anchors`: every declaration's
anchor in document order (the key is absent for single-declaration ids, so no current
golden changes shape).

Why this rule (R2-02, decided in attempt 2; Option A, deliverable inside al-sem). The
production consumer, bc-dev-mcp, reads ONLY the singular anchor and joins one digest entry
to the changed procedure whose `[startLine, endLine]` contains it (P20). One singular anchor
can therefore serve exactly one declaration of a multi-declaration id, so the anchor must
point at the declaration the change is about:
- `--diff` with one arm changed: the anchor IS the changed arm (first or second), so the
  existing reader joins the changed arm, unchanged. A4(c1), A4(c2).
- `--diff` with both arms changed, and `--file` (no hunk information; every declaration of
  the file counts as touched): the anchor is the LAST declaration. This equals today's
  value for every root-slot routine (the root-slot anchor loop is last-wins,
  `digest_cli.rs:569-581`; `--routine Dup` anchors at line 24 today, P13), so no value a
  consumer sees today moves for a root-slot duplicate. With the reader unchanged, the
  other arm's subject joins only when bc-dev-mcp carries no `endLine` (`risk.ts:166-167`).
  This is a STATED LIMIT, not delivered by #71: a per-declaration join under `--file`
  needs the reader to expand over `routine.anchors` (or bc-dev-mcp to switch to `--diff`,
  which #71 makes usable: `risk.ts:12-18` avoids `--diff` only because of #71). Both are
  bc-dev-mcp changes; see Also in the change and Not delivered.
- Option B (change bc-dev-mcp's reader in #71) is rejected: it is a second repository, and
  #71 cannot deliver or gate it.
For a single-declaration root-slot routine the value equals today's anchor (P5). It
changes: (a) the 4 op-fallback anchors (P15); (b) entries that had no anchor now have one;
(c) a duplicate-id NON-root-slot routine, today anchored by the op/callsite fallback inside
a body, now anchors at a declaration line; no golden holds a duplicate id. Columns are
`source_anchor.start_column`, which is a UTF-8 BYTE column (`Utf16Cols::col` returns the
byte column verbatim, `src/program/body/node_util.rs:21-37`); values are preserved, and
A4(d) pins a declaration preceded by non-ASCII text on its line.

**Known limits (stated, not delivered).**
- Attribute lines are in the span only when the lowerer attached the attribute; an
  attribute separated from its routine by a comment, pragma or `#if` is not (P14, the
  "routine attributes" discovery).
- (Superseded by the final panel, code-review D.) Deleting a routine's first or only
  attribute now DOES make it a root: the parser marks a deleted run that is one or more
  complete attribute groups (each starts with a trimmed `[` and runs, over one or more
  lines, until its brackets balance; final round 2, flash) as `DeletedAttributesAfter`,
  and such a gap right above a span (`l + 1 == first`) touches it. Exactly that narrow:
  other text between or after groups (round 3, astra: after a closing `]` only whitespace,
  another group or a `//` comment may follow on its line), a stray `]`, or a group left
  open makes it a plain
  deletion. Remaining limit: brackets are counted on raw text, so a bracket inside a
  string argument (`'[x'`) can misclassify the run. Pinned by
  `deleting_a_routines_first_attribute_line_makes_it_a_root`,
  `deleting_a_multi_line_attribute_makes_the_routine_a_root` and
  `attribute_only_deletion_runs`; A1i(a) is replaced by it.
- `#if`-split header arms with DIFFERENT signatures share one span, so a change in either
  arm's header line makes both arms' ids roots (safe over-approximation; final panel).
- Declaration-only roots are special-cased in digest (`RoutineSpan.declaration_only_display`,
  the build-first branch) instead of being model routines; the root cause is the model's
  Interface/ControlAddIn skip, issue #82.
- Counts are authoritative inside a hunk body, as in `git apply` and GNU patch (final-panel
  evidence in the proofs section): plain `diff -u` outputs concatenated without
  `diff --git`, after a malformed hunk that promises more lines than its body, have the
  next file's `---`/`+++` pair read as body while counts remain; an exact fill attaches
  the next file's hunks to the previous file with NO error (A1g(c2-exact)), otherwise a
  `diff-parse-error` is reported.
- Changing an `#if`/`#else`/`#endif` directive line around duplicated procedures gives no
  root (directive lines are outside every span). Pinned by A1i(b); part of the
  object-level discovery.
- An Interface/ControlAddIn method change makes the METHOD a declaration-only root, not
  its implementers (the Interface/ControlAddIn discovery). Declaration-only procedures are
  not reachable by `--routine` selectors (selector index unchanged).
- Object-level changes map to no routine (the object-level discovery).
- A multi-declaration id has one singular anchor (Entry anchor). A reader that ignores
  `routine.anchors` joins only the anchored declaration when its subjects carry an
  `endLine`: under `--file`, and under `--diff` when both arms changed, that is the last
  declaration. Pinned by A4(c3)/(c4).

**Golden license (exact).** Regenerate all families together (`scripts/check-goldens
--regen`). No golden carries a declaration-only file: none of the 20 `DIGEST_CORPUS`
fixtures has an `interface` or `controladdin` object (`grep -liE
"^\s*(interface|controladdin)\b"` over their `src/*.al` matches nothing), and none
holds two declarations with one id, so `declarationOnly` and `anchors` appear in no
golden. Allowed to move, and only in these fields:
- **Every** digest JSON golden, exactly 21 files (`tests/cli-b-goldens/digest/*.json`: the
  20 `DIGEST_CORPUS` fixture files plus `ws-d8-commit-in-tx.diff.json`; each holds exactly
  one `"schemaVersion": "1.4.0"` line, P21): that line becomes `"schemaVersion": "1.5.0"`
  (Data plumb 7). For 15 of the 21 this is the ONLY moved byte; the 6 below move in their
  listed fields as well. No `.human.txt` carries the schema version.
- `tests/cli-b-goldens/digest/ws-d8-commit-in-tx.json`: + entry `OnAfterPostSalesDoc`
  (anchor `ws:src/posting.al` line 18 col 4); summary `rootsRequested` 2→3,
  `rootsResolved` 2→3, `totalEffects` 8→11; `totalUnresolved` recomputed.
  `ws-d8-commit-in-tx.human.txt`: + its block.
- `ws-d34.json`: + entry `DoWork` (line 43 col 4, 0 effects); `Persist` anchor line 47
  col → line 45 col 4; summary roots 5→6, `totalEffects` unchanged.
  `ws-d34.human.txt`: + `DoWork` block.
- `ws-d14-dead-routine.json`: + `LiveHelper` (line 7 col 1), + `DeadHelper` (line 9 col
  1), 0 effects each; roots 1→3, `totalEffects` unchanged. `.human.txt`: + 2 blocks.
- `ws-d35.json`: `PersistAudit` anchor line 31 → line 29 col 4. Nothing else.
- `ws-txn-d46-neg.json`: `DoSetup` anchor line 31 → line 27 col 4. Nothing else.
- `ws-txn-d48-pos.json`: the second `SendOne` (codeunit 50001) anchor line 39 col 8 →
  line 35 col 4. Nothing else.
Everything else stays byte-identical: every other entry, `changed`,
`changedRootsDiagnostics` (stays `[]` in 14 fixtures and the six `src/Rec.Table.al`
`file-unmatched` entries stay in the other 6, P11), `dependencies`, envelope
`diagnostics` (as minted on `52e64d1a` after #68; #71 does not touch the analyzer path),
`workspaceFingerprint`, the other 14 digest fixtures apart from their `schemaVersion`
line, the 17 digest `.human.txt` files other than the three listed above, d35/d46-neg/d48-pos
among them (the human form has no anchor and no schema version), `ws-d8-commit-in-tx.diff.json`
apart from its `schemaVersion` line (its only changed lines, new 6-7, are inside
`PostSalesDoc`, whose anchor is a root slot), every one of the 19 `prove/*.json` goldens
INCLUDING its `"schemaVersion": "1.3.0"` (prove's schema does not bump, Data plumb 7;
prove DOES read the same anchor, Data plumb 6, but each of the
18 non-dummy prove goldens' roots is a single-declaration root-slot routine anchored at
its `procedure` line, e.g. `ws-d34/src/Codeunit.al:4`, `ws-d35/src/Codeunit.al:19`,
`ws-d1-multi-caller/src/D1MultiCaller.al:22`, so the value is unchanged; the dummy-doc
has no entry) and every other golden family (`AttributeIr`, `ModelRoutine` and
`ModelObject` are not `Serialize`; no serialized projection reads the new fields).
Columns are UTF-8 byte columns exactly as `source_anchor` stores them today; d14 indents
with a tab (one byte, col 1).

**Also in the change.** CHANGELOG `Changed`: `--file`/`--diff` now resolve by routine
declaration span; new meanings of `file-unmatched`, `diff-file-unmatched`,
`hunk-outside-routines`; Interface/ControlAddIn procedures become declaration-only
roots (`routine.declarationOnly`); entry anchors at the declaration in digest AND prove
`routine.anchor` (the last touched declaration of a multi-declaration id), with
`routine.anchors` for multi-declaration ids; digest `schemaVersion` 1.4.0 → 1.5.0 for
the additive keys (every digest JSON golden moves in that line; prove stays 1.3.0). `Fixed`: hunks are read line by line (context no longer creates
roots, pure deletions resolve). A follow-up note for bc-dev-mcp (a separate repo; not
changed here): `risk.ts:12-18` can drop its "`--diff` is NOT used" workaround once this
ships, and `parseAlSemDigest` (`risk.ts:87-115`) should expand an entry over
`routine.anchors` so both declarations of a multi-declaration id join under `--file`.

## Base/Head SHAs

- **B** = `52e64d1a5d22b47616fbeed7f84789e96056948d` (origin/master at freeze; no rebase needed).
- **H** = `7da1fbe69e6457d320ca3a377b7b5fc8ee01bb78`: all four gates exit 0, supervised, on this exact
  tree (logs `.agent/runs/20261014-155631-70caa1/logs/*/20261014T2116*` to `T2125*`).
- Spec panel converged in round 2 of 5; final panel converged in round 4 of 5 (both reviewers accepted
  all 40 register entries). Reviewers: astra = gpt-6.1-sol (pi), flash = Opus 5.5 (Agent).

## Timeline

Attempt 1 (`U:/Git/al-sem-issue-71-a1`, base `66b62288`, run `20261014-015045-3206fd`):
- 2026-10-14T01:51 local: worktree created from `66b62288`.
- 2026-10-14T00:02Z: steps 1-4 (claim, classification, probes P1-P11, design, acceptance matrix, discoveries) written. No source change, no commit.
- Spec panel round 1 (astra, flash): both "not ready". Register R1-01..R1-16; ledger revised (probes P2, P11-P16; Design rewritten: population, parser state machine, cursor, anchor, limits, exact license; matrix rewritten; two discoveries added (routine attributes, Interface/ControlAddIn), the objectDisplay locator path fixed, the anchor discovery pulled into scope and removed).
- Coordinator: no narrowing of the acceptance; no schema bump (digest 1.4.0 is unreleased). Superseded in attempt 2: 1.4.0 shipped in v1.7.0 (P21, Data plumb 7).
- Spec panel round 2: flash ready, astra not ready. Interface/ControlAddIn procedures became declaration-only roots on `ModelObject.declaration_only_routines`; `*-declarations-only` diagnostics removed; anchors keep the original unit and declaration multiplicity (`routine.anchors`); EOF split element and truncated-hunk terminating line specified; prove anchor plumbing and UTF-8 byte columns stated; register R2-01..R2-06; conductor R2-07 (name-folded declaration-only ids).
- Spec panel round 3 (then the cap of 3; ledger sha256 `e045a239…09db`): flash ready with 2 new minors (R3-01, R3-02); astra re-raised R2-02 (bc-dev-mcp's `--file` reader ignores `routine.anchors`). Outcome `blocked spec-panel-cap`.

Attempt 2 (`U:/Git/al-sem-issue-71-a2`, base `52e64d1a`, run `20261014-155631-70caa1`; the spec cap is now 5):
- 2026-10-14: ledger carried over from attempt 1. Carried unchanged: classification, probes P1-P17 (results), the span rule, population, declaration-only roots, data plumb 1-5 and 7, matching, known limits, golden license, acceptance rows A1a-A1f, A1h, A1i, A2a-A2c, A3, discoveries.
- Master moved (#68, disk-hygiene chore): every cited file:line re-verified; only `digest_cli.rs` lines moved (-59; -58 for 84-100) and `risk.ts` in bc-dev-mcp (re-read at `715e0b9`). P1, P3, P4, P9 and P13 re-run on a `52e64d1a` build (P18). No probe result moved.
- R2-02 resolved with Option A refined: singular anchor = the LAST TOUCHED declaration (P20; Entry anchor; A4(c) rewritten with a consumer-join row; the reader's per-declaration join under `--file` is a stated limit and a bc-dev-mcp follow-up).
- R3-01 resolved: declaration-only entries are built before the identity/coverage lookups with no query diagnostic (P19; Data plumb 6; A2d(a) asserts the summary counts and no `DIAG-digest-*`).
- R3-02 resolved: Data plumb 4 states per-side count acceptance; A1g(c2) has explicit counts (`@@ -3,3 +3,1 @@`) and the full pinned outcome.
- Findings register re-issued: 25 entries, every review reset to unreviewed.
- Spec panel round 1 of attempt 2: flash accepted 25/25. astra accepted 24, re-raised R1-11 and raised R1-26 (important: digest 1.4.0 shipped in v1.7.0, so "additive within the unreleased 1.4.0" is stale) and R1-27 (minor: A1g(c2)'s "accept `-` with no old-side count" mutation cannot fail its fixture). Both verified (P21; the c2 body never offers a `-` line after the old count is spent). Fixed: digest schema 1.4.0 → 1.5.0, prove stays 1.3.0 (Data plumb 7); the license now permits the `schemaVersion` line in all 21 digest JSON goldens; A2d(f) and the two version-pinning tests move to 1.5.0; A1g(c2) split into three executed cases (truncated, per-side overrun, exact fill) with outcomes and the mutation each one fails. The attempt-1 coordinator direction "no schema bump" is superseded.
- Spec panel round 2 of attempt 2: converged (both reviewers accepted all 27 entries).
- Step 6: acceptance tests written (see Acceptance tests (step 6)); 18 red for the stated reasons, 2 guards green, 2 existing tests green. Nothing committed.
- Step 7: plan written, `docs/superpowers/plans/2026-10-14-issue-71-digest-routine-spans.md` (8 tasks).
- Final panel fix pass (HEAD was `fa963cb7`): A (unescape the object name once, `scope::unescape_al_identifier`), B (header ranges checked, invalid range = `diff-parse-error`), C (`+++` opens a file only after a pending `---`; counts stay authoritative, evidence in the proofs section), D (attribute-only deletion run right above a span touches it; `limits_are_pinned` (a) replaced by a positive test), E (declaration-only routines follow the population), F (unused fields removed, `DeclOnlyInfo` → `Option<String>`), G (per-change binary search), H (one `### Changed` header; ControlAddIn events recorded as a discovery). Limits added: split-header arms with different signatures, digest special-casing of declaration-only roots (#82), concatenated `diff -u`.

## Acceptance matrix

Every test drives `run_digest_pipeline` or the `alsem` binary (never
`resolve_changed_roots` directly), states its precondition literally, and names the
mutation that must turn it red. New member `tests/cli/cli_digest_changed_spans.rs` unless
named. Line numbers in diffs are 1-based; anchor lines are 0-based (as the JSON carries them).

| Acceptance item | Test | Precondition hand-stated | Mutation that must make it fail |
|---|---|---|---|
| A1a. A hunk on an op-less body line makes the routine a root | `hunk_on_assignment_line_is_a_changed_root`: copy `ws-d8-commit-in-tx` to a tempdir, diff `@@ -9,1 +9,1 @@`, `run_digest_pipeline`; exit 0, roots = [`PostSalesDoc`], no diagnostic. | Line 9 is literally `Header."Status Posted" := true;`; the snapshot's `operation_index` + `callsite_index` hold nothing on row 8 of that file. | Rebuild the index from evidence start lines (old rule) → exit 2. |
| A1b. Every modelled routine shape, every line of its span | `every_routine_shape_line_maps_to_its_routine`: the Shapes fixture (text in the test) in a tempdir; a table of (line, expected root ids) via the binary's `--diff`: header, `var`, `begin`, `end;`, attribute line, empty publisher `OnPub`, `OnRun`, table field trigger, table trigger, page field trigger, action trigger, dataitem trigger, a line in EACH `Dup` arm (→ the one `Dup` id), the `Split` shared body (→ both `Split` ids). | Each row asserts its line text first. | (i) start at `source_anchor.start_line` → attribute rows fail; (ii) `end` exclusive → `end;` rows fail; (iii) keep one span per id → the first-`Dup`-arm row fails; (iv) first split arm only → `Split` row fails. |
| A1c. Context lines create no roots | `context_lines_do_not_create_roots`: the `p11-context.patch` shape → zero roots, one `hunk-outside-routines` with `startLine` 14 / `endLine` 20, exit 2. | Only line 17 is `-`/`+`. | Match the header range → `PostSalesDoc` root. |
| A1d. Deletions: interior, boundary, file start/end | `deletions_resolve_by_interior_gap`: (a) `-U0` `@@ -9,1 +8,0 @@` → `PostSalesDoc`; (b) `-U0` delete the blank line after `PostSalesDoc`'s `end;` (`@@ -17,1 +16,0 @@`) → zero roots; (c) delete lines 17-21 (blank + `OnAfterPostSalesDoc`) against the post-deletion file → zero roots; (d) delete line 1 (`@@ -1,1 +0,0 @@`) → zero roots; (e) delete the last line → zero roots. | Each diff and post-change file text is in the test. | (i) cursor = `new_start` always → (b) gives `PostSalesDoc`; (ii) gap touches L or L+1 → (c) gives `PostSalesDoc`. |
| A1e. Object-level hunks stay outside | `object_level_hunk_is_outside_routines`: table field line 29 → `hunk-outside-routines`, exit 2. | Line 29 is a `field(...)` line. | Map any hunk in a file to all its routines. |
| A1f. Mixed hunks in one file | `one_inside_one_outside_hunk`: two hunks (line 9 and line 29) → root `PostSalesDoc` AND one `hunk-outside-routines` for line 29, exit 0. | Both hunks in the test. | Drop the per-hunk diagnostic when the file has any root → fails; reset the cursor per file instead of per hunk → the second hunk mis-resolves. |
| A1g. Body parsing is count-bounded | `hunk_body_is_count_bounded`, all on the d8 copy (`PostSalesDoc` = lines 3-16, `HandlePosted` in `src/subscriber.al` includes line 9): (a) a deleted line whose text is `-- x` and an added line `++ y` inside a hunk resolve as changes of the routine, no new file appears; (b) an EMPTY line as context shifts later `+` lines correctly (the `+` lands in the routine after it, not the one before); (c) a truncated hunk `@@ -3,2 +3,1 @@\n-old\n` WITH a final newline and the same text WITHOUT it → one `DiffParseError` in both, roots only from the lines read; (c1) a truncated hunk `@@ -3,2 +3,1 @@\n-old\n` ended by a second hunk header `@@ -20,1 +20,1 @@` (`-x`/`+y`, line 20 inside `OnAfterPostSalesDoc`, 18-21) → `DiffParseError` for the first AND root `OnAfterPostSalesDoc` from the second hunk (the terminating line is re-dispatched; an `@@` terminator, not `diff --git`, because after `diff --git` the next `---`/`+++` open the file anyway and a swallowed line would not show); (c2) see the three explicit-count rows below; (d) CRLF diff over a CRLF source → same roots as LF; (e) `\ No newline at end of file` is not counted. | Diff texts in the test. | (a) header tests before the body state → a phantom file; (b) skip empty lines → wrong root; (c) treat the terminal split element as a blank context line → the with-newline case reports no error; (c1) drop the terminating line (truncation path `continue`s) → the second hunk is lost, `OnAfterPostSalesDoc` missing (to be executed when the test is restored in Task 5); header-range fallback → a context-only root. |
| A1g(c2). A bare `---`/`+++` pair inside a short hunk, explicit counts | `bare_header_pair_inside_short_hunk_is_body`: the diff text, ending in `\n`, is exactly: `diff --git a/src/posting.al b/src/posting.al` / `--- a/src/posting.al` / `+++ b/src/posting.al` / `@@ -3,3 +3,1 @@` / `-old` / `--- a/src/subscriber.al` / `+++ b/src/subscriber.al` / `@@ -9,1 +9,1 @@` / `-x` / `+y`. Pinned outcome: hunk 1 (old 3, new 1) reads `-old` (old 1/3, starts a run: `DeletedAfter(2)`), `--- a/src/subscriber.al` (old 2/3, same run), `+++ b/src/subscriber.al` (new 1/1, `Added(3)`); `@@ -9,1 +9,1 @@` is not body while old count 1 remains → hunk 1 INCOMPLETE: exactly one `DiffParseError` naming the hunk at `+3`; that line is re-dispatched as a top-level hunk header and attaches to the CURRENT file `src/posting.al` (`diff_parser.rs:371-376`); hunk 2 reads `-x`, `+y` → `Added(9)`, complete. Parsed files = exactly one (`src/posting.al`, two hunks). Digest: roots = [`PostSalesDoc`] (`Added(3)`: `2+1 <= 3 <= 15+1`; `DeletedAfter(2)` is not interior, `3 <= 2` false; `Added(9)` in the body), no `HandlePosted`, no `diff-file-unmatched`, no `hunk-outside-routines`, `changedRootsDiagnostics` = the one `diff-parse-error`, exit 0. | The diff text and the 1-based `PostSalesDoc` span 3-16 are in the test. | Apply header tests inside a body → `src/subscriber.al` becomes a second file and `HandlePosted` (its line 9) a root; drop the re-dispatched line → hunk 2 is lost but `PostSalesDoc` stays, so the test also asserts two hunks on the one parsed file through the parser's public `parse_unified_diff`. (The per-side mutation is NOT claimed here: every `-` in this body arrives while old count remains. It is discriminated by the next row.) |
| A1g(c2-overrun). A `-` line after the old-side count is spent ends the hunk | `minus_line_past_old_count_ends_the_hunk`: same layout, diff text exactly: `diff --git a/src/posting.al b/src/posting.al` / `--- a/src/posting.al` / `+++ b/src/posting.al` / `@@ -3,1 +3,2 @@` / `-old` / `+a` / `--- a/src/subscriber.al` / `+++ b/src/subscriber.al` / `@@ -9,1 +9,1 @@` / `-x` / `+y` (final `\n`). Pinned outcome: hunk 1 (old 1, new 2) reads `-old` (old 1/1, `DeletedAfter(2)`), `+a` (new 1/2, `Added(3)`); `--- a/src/subscriber.al` starts `-` but old count is 0 → not body → hunk 1 INCOMPLETE (new count 1 left): exactly one `DiffParseError` for the hunk at `+3`; the `---` line is re-dispatched as a top-level header, `+++ b/src/subscriber.al` opens file 2, and `@@ -9,1 +9,1 @@` / `-x` / `+y` gives `Added(9)` there, complete. Parsed files = exactly two (`src/posting.al` with one hunk, `src/subscriber.al` with one hunk). Digest: roots = exactly {`PostSalesDoc`, `HandlePosted`}, `changedRootsDiagnostics` = the one `diff-parse-error`, exit 0. | Diff text, both spans and the per-side counts are in the test. | Accept a `-` line with no old-side count left (check only that SOME count remains) → `---` is read as a deleted line (`DeletedAfter(3)`), `+++` as `Added(4)` filling new 2/2, the hunk completes, `@@ -9` attaches to `src/posting.al`: one parsed file, zero errors, roots [`PostSalesDoc`] only → fails on the file count, the error count and the missing `HandlePosted`. |
| A1g(c2-exact). Exact fill completes the hunk; no error | `bare_header_pair_exact_fill_is_complete`: diff text exactly: `diff --git a/src/posting.al b/src/posting.al` / `--- a/src/posting.al` / `+++ b/src/posting.al` / `@@ -3,2 +3,1 @@` / `-old` / `--- a/src/subscriber.al` / `+++ b/src/subscriber.al` / `@@ -9,1 +9,1 @@` / `-x` / `+y` (final `\n`). Pinned outcome: hunk 1 (old 2, new 1) reads `-old` (old 1/2, `DeletedAfter(2)`), `--- a/src/subscriber.al` (old 2/2, same run), `+++ b/src/subscriber.al` (new 1/1, `Added(3)`) → both counts zero → COMPLETE, no error; `@@ -9,1 +9,1 @@` is a top-level hunk header and attaches to the current file `src/posting.al` (`diff_parser.rs:371-376`): `Added(9)`. Parsed files = exactly one (`src/posting.al`, two hunks), zero parse errors. Digest: roots = [`PostSalesDoc`], `changedRootsDiagnostics` = [], exit 0. This is the stated, intended reading of malformed `diff -u` concatenation whose counts happen to fit. | Diff text and counts are in the test. | Treat count exhaustion as truncation (report an error unless a non-body line follows a full hunk) → one `diff-parse-error` appears; apply header tests inside a body → file 2 and `HandlePosted` appear. |
| A1i. Stated limits stay pinned | `limits_are_pinned`: (a) delete a whole `[IntegrationEvent(false, false)]` line above `OnAfterPostSalesDoc` (`-U0` `@@ -18,1 +17,0 @@`) → zero roots, `hunk-outside-routines`; (b) the Shapes fixture with `#if CLEAN25` changed to `#if CLEAN26` (the line above the first `Dup`) → zero roots, `hunk-outside-routines`. | Diff and post-change text in the test. | (a) treat a gap whose upper neighbour is `decl_start` as touching → `OnAfterPostSalesDoc` becomes a root; (b) extend spans to the enclosing `preproc_conditional` → `Dup` becomes a root. Either change must update this row deliberately. |
| A1h. Renames | `rename_with_and_without_hunks`: rename with a hunk on line 9 → `PostSalesDoc` only; pure rename → every routine of the file. | Diffs in the test. | Treat a rename-with-hunks as whole-file → extra roots. |
| A2a. A file containing modelled routines is never `FileUnmatched` | `file_with_only_opless_routines_is_matched`: the Pure fixture, `--file src/Pure.al` → exit 0, roots [`Add`]; same via `--diff` → no `diff-file-unmatched`. | The snapshot holds no op/callsite for `Add`. | Old index → `file-unmatched`, exit 2. |
| A2b. `--file` returns every modelled routine of the file | `file_digest_returns_every_routine_of_the_file`: Shapes `--file` → exactly the 11 distinct ids, incl. `OnPub` and `Dup`. | The 11 display names hand-written; the test asserts 12 declarations / 11 ids in the fixture. | Old index → `OnPub`/`Dup` missing. |
| A2c. Dependency routines never become roots | extend `cli_digest_query_dependencies.rs` with the EMBEDDED fixture and `CallLib` enabled (`workspace(dir, true)` + `embedded_dependency(dir)`), BOTH modes (default cross-app and `--single-app`): `--file dep:{DEP_GUID}:src/Lib.al` → `file-unmatched`, no root, exit 2; `--file src/Work.al` → roots exactly `DoWork`, `CallLib` (workspace). | Before the exclusion checks, in default mode: `app_sources` is `[(DEP_GUID, "embedded")]` and `query effects --routine CallLib` has an effect whose `origin.anchor.file` starts with `dep:{DEP_GUID}` (as `:260-270` asserts), proving dependency routines with `dep:` units ARE in the model. | Index every non-empty `source_unit_id` (drop the `ws:` filter) → `dep:{DEP_GUID}:src/lib.al` matches the dependency routines → roots appear, test fails. |
| A2d. A hunk inside an Interface/ControlAddIn procedure makes it a changed root | `declaration_only_procedures_are_roots`, through the binary: fixture `src/IFoo.Interface.al` = `interface IFoo` with `procedure Bar();` (line 3) and `procedure Baz(X: Integer);` (line 4), `src/IQux.Interface.al` = `interface IQux` with `procedure Bar();`, `src/Add.ControlAddIn.al` = a `controladdin` with `procedure Init();`, plus a codeunit. (a) `--diff` changing line 3 of IFoo → exit 0, exactly one entry: `routine.display` `Bar`, `routine.declarationOnly` true, `routine.anchor` `ws:src/IFoo.Interface.al` line 2, `stableId` = `<WS_GUID>:Interface:0#<64 hex>`, `effects` [], `coverage.reasons` [`declaration-only`]; `summary.rootsRequested` == `summary.rootsResolved` == 1; the envelope `diagnostics` hold no entry whose `code` starts with `DIAG-digest-`; no `hunk-outside-routines`; (b) `--file src/IFoo.Interface.al` → two entries `Bar`, `Baz`, `rootsRequested` == `rootsResolved` == 2, no `file-unmatched`; (c) `two_interfaces_same_procedure_distinct_roots`: one `--diff` changing the `Bar` line in BOTH IFoo and IQux → exactly two roots, two entries `Bar` with DIFFERENT stableIds (each equal to the id computed in the test from its own object name) and anchors in their own files; (d) the controladdin `Init` line → a declaration-only root; (e) an interface with NO procedures → `file-unmatched`; (f) the digest `schemaVersion` is `1.5.0`; and in `entry_anchor_is_the_declaration` (A4(e)) the prove document's `schemaVersion` is still `1.3.0`. The existing pins `cli_digest_query_dependencies.rs:169` and `cli_digest_temp_state.rs:142` change to `1.5.0`. | Fixture texts in the test; the test asserts each stableId equals `to_stable_routine_id_from_parts("<WS_GUID>:Interface:0", normalized_signature_hash("Bar", [], None), Some(<object name>))`, computed in the test via the public `engine::ids` helpers. | (i) Leave `declaration_only_routines` out of the span index → (a)-(d) fail (hunk-outside / file-unmatched); (ii) use the shared object id (pass `None` instead of the object name) → (c) yields ONE root id for both `Bar`s and fails; (iii) derive the id from the procedure name alone → (a)'s stableId assertion fails; (iv) route declaration-only roots through the identity/coverage lookups (drop the build-first branch of Data plumb 6) → (a) loses its entry (`root-not-in-snapshot`, `rootsResolved` 0, a `DIAG-digest-root-not-in-snapshot`), or, if the entry is kept, `rootsRequested` 2 and a `DIAG-digest-*` diagnostic. |
| A4. Entry anchors at the declaration | `entry_anchor_is_the_declaration`: (a) d8 `--routine OnAfterPostSalesDoc` → anchor `ws:src/posting.al` line 18 col 4; d34 `--routine Persist` → line 45 col 4 (op fallback today: 47); (b) a fixture file named with MIXED case (`src/MixedCase.Codeunit.al`) → anchor `file` byte-identical `ws:src/MixedCase.Codeunit.al` (original unit, not the lowercased key); (d) a declaration indented 4 spaces and preceded on its line by a non-ASCII comment (`    /*é*/ procedure Y()`) → column = UTF-8 byte offset 11 (UTF-16 would give 10); (e) prove: `run_prove_pipeline(ws-d34, "Persist", …)` → `routine.anchor` line 45 col 4. | Lines, columns and file names hand-stated. | (a)/(e) restore the slot/op/callsite fallback; (b) build the anchor from the normalized key → lowercased/`ws:`-stripped file; (d) convert the column to UTF-16 → 10. |
| A4(c). Multi-declaration anchor = the last TOUCHED declaration (R2-02) | `duplicate_id_anchor_follows_the_changed_arm`, Shapes fixture through the binary (`Dup` arms: 1-based lines 20-23 and 25-28, anchor lines 19 and 24): (c1) `--diff` on line 26 (second arm) → one `Dup` entry, `anchor.line` 24, `anchors` lines [19, 24]; (c2) `--diff` on line 21 (first arm only) → `anchor.line` 19, `anchors` [19, 24]; (c3) one `--diff` with hunks on lines 21 AND 26 → `anchor.line` 24; (c4) `--file src/Shapes.al` → `anchor.line` 24, and `--routine Dup` → `anchor.line` 24 (today's root-slot value, P13). Consumer join, in the same test: a helper that is a literal transcription of bc-dev-mcp's join (`risk.ts:99-110` + `:158-169`: entry kept when display matches, file = `anchor.file` minus `ws:` lowercased equals the subject file, and `anchor.line + 1` lies in the subject's 1-based `[startLine, endLine]`), with subject A = arm 1 (20-23) and subject B = arm 2 (25-28), both with `endLine`: (c1) joins B and not A; (c2) joins A and not B; (c4) `--file` joins B and NOT A (the stated limit, pinned so a change of rule is deliberate). | Fixture text, arm lines and subjects hand-stated; the test asserts the two `Dup` arms share one stableId first. | (i) first declaration regardless of touch → (c1), (c3), (c4) fail; (ii) last declaration regardless of touch → (c2) fails; (iii) first TOUCHED declaration → (c3) and (c4) fail; (iv) drop `routine.anchors` → the `anchors` assertions fail. |
| A3. Goldens moved under the stated license | `scripts/check-goldens --regen`, then a `golden-diff-triager` receipt at `.agent/golden-triage.md`: exactly the 21 digest JSON files moved (15 in the `schemaVersion` line only, 6 also in their licensed fields) plus the 3 licensed human files, and no prove golden moved; then `scripts/check-goldens` green. | — | Any unlicensed moved byte fails the triage. |

## Acceptance tests (step 6)

Written 2026-10-14 on `52e64d1a` (uncommitted). Every matrix row is in the tree and
compiles; none needs a type the implementation adds, because each asserts through the
`alsem` binary's JSON, `run_prove_pipeline`, or the parser's EXISTING public fields
(`DiffParseResult.files[].path/.hunks.len()`, `.errors`). The plan-only tests are the
unit tests of new internals (Plan Tasks 1-4: `HunkChange` sequences, `AttributeIr.origin`,
`ModelRoutine.declaration_start_line`, `ModelObject.declaration_only_routines`); their
exact text is in `docs/superpowers/plans/2026-10-14-issue-71-digest-routine-spans.md`.

Files: new member `tests/cli/cli_digest_changed_spans.rs` (registered in
`tests/cli/main.rs`); `tests/cli/cli_digest_query_dependencies.rs` (+ A2c test
`dependency_routines_never_become_changed_roots`, and the `:169` pin → `1.5.0`);
`tests/cli/cli_digest_temp_state.rs` (`:142` pin → `1.5.0`).

Run: `cargo test -p al-sem --test cli -- cli_digest_changed_spans:: cli_digest_query_dependencies:: cli_digest_temp_state::`
→ 18 failed, 4 passed. Log: scratchpad `71/run-tests.log`.

| test (matrix row) | today | first failing assertion (quoted) | lands green in plan task |
|---|---|---|---|
| `hunk_on_assignment_line_is_a_changed_root` (A1a) | red | `assertion left == right failed: {… "changedRootsDiagnostics":[{"endLine":9,…,"kind":"hunk-outside-routines","startLine":9}] …} left: 2 right: 0` (exit 2). The precondition (no op/call site on row 8) holds. | 5 |
| `every_routine_shape_line_maps_to_its_routine` (A1b) | red | 18 of 19 rows, e.g. `src/posting.al:3 \`    procedure PostSalesDoc()\`: want ["PostSalesDoc"], got [] exit 2`; `src/Shapes.al:21 \`    begin\`: want ["Dup"], got [] exit 2`; `src/Shapes.al:8 \`[EventSubscriber…]\`: want ["Sub"], got []`. Only the `Split` row (line 37, a `Message` call site) matches today. | 5 |
| `context_lines_do_not_create_roots` (A1c) | red | `left: 0 right: 2` (exit 0: the header window made `PostSalesDoc` a root) | 5 |
| `deletions_resolve_by_interior_gap` (A1d) | red | `(a) … left: (2, []) right: (0, ["PostSalesDoc"])` | 5 |
| `object_level_hunk_is_outside_routines` (A1e) | green (guard) | — (the old rule gives the same outcome; its mutation targets the new code) | stays green |
| `one_inside_one_outside_hunk` (A1f) | red | `left: 2 right: 0`; diags `[hunk-outside 9-9, hunk-outside 29-29]` | 5 |
| `hunk_body_is_count_bounded` (A1g) | red | `(a) no phantom file … left: 2 right: 1` (the `+++ y` body line opened file `y`) | parser part 1, digest part 5 |
| `bare_header_pair_inside_short_hunk_is_body` (A1g(c2)) | red | `left: 2 right: 1` on `parsed.files.len()` (`[src/posting.al, src/subscriber.al]`) | parser 1, digest 5 |
| `minus_line_past_old_count_ends_the_hunk` (A1g(c2-overrun)) | red | `assertion left == right failed: [] left: 0 right: 1` on `parsed.errors.len()` | parser 1, digest 5 |
| `bare_header_pair_exact_fill_is_complete` (A1g(c2-exact)) | red | `left: 2 right: 1` on `parsed.files.len()` | parser 1, digest 5 |
| `rename_with_and_without_hunks` (A1h) | red | `left: 2 right: 0`, `hunk-outside-routines` 9-9 | 5 |
| `limits_are_pinned` (A1i) | green (guard) | — (pins the stated limits; mutations target the new code) | stays green |
| `file_with_only_opless_routines_is_matched` (A2a) | red | `left: 2 right: 0`, `[{"file":"src/Pure.al","kind":"file-unmatched"}]`. Precondition (empty op/call-site index) holds. | 5 |
| `file_digest_returns_every_routine_of_the_file` (A2b) | red | `left: ["OnAction", "OnAfterGetRecord", "OnInsert", "OnRun", "OnValidate", "OnValidate", "Split", "Split", "Sub"] right: ["Dup", …, "OnPub", …]` | 5 |
| `dependency_routines_never_become_changed_roots` (A2c) | red | `[]: {… "changed":{"files":["dep:bbbb6666-…:src/Lib.al"]},"changedRootsDiagnostics":[],… "entries":[{… "anchor":{"column":4,"file":"dep…` `left: 0 right: 2`. See note below. | 5 |
| `declaration_only_procedures_are_roots` (A2d) | red | `(a) … "changedRootsDiagnostics":[{"file":"src/IFoo.Interface.al","kind":"diff-file-unmatched"}] … left: 2 right: 0` | 7 (needs the 1.5.0 of task 6) |
| `entry_anchor_is_the_declaration` (A4) | red | `left: ("", -1, -1) right: ("ws:src/posting.al", 18, 4)` (`OnAfterPostSalesDoc` has no anchor today) | 5 |
| `duplicate_id_anchor_follows_the_changed_arm` (A4(c)) | red | `one \`Dup\` entry expected: {… "kind":"hunk-outside-routines","startLine":26 …} left: 0 right: 1` (c1). The precondition (`--routine Dup` → one entry, no diagnostic) holds. | 5 |
| `symbol_only_dependency_table_resolves_and_is_selectable` (A2d(f) pin) | red | `left: String("1.4.0") right: "1.5.0"` | 6 |
| `digest_db_effects_carry_query_effects_temp_state` (A2d(f) pin) | red | `left: String("1.4.0") right: "1.5.0"` | 6 |
| A3 (golden regen + triage receipt) | plan only | — | 5 and 6 (regen points), 8 (final check) |

**Step-6 observation (A2c, corrects P7's reading).** P7 said dependency source routines are
`dep:<guid>:<path>`, "never stripped", and the A2c row assumed `--file dep:{DEP_GUID}:src/Lib.al`
gives `file-unmatched` today. It does NOT: `normalize_input` lowercases the `dep:` key on
both sides, so today the dependency file MATCHES and its routines become roots (exit 0,
`changedRootsDiagnostics` `[]`, entries anchored `dep:…`). The test is therefore red for
a real pre-existing leak, not a fixture error; the design's `ws:` filter (Data plumb 3)
closes it, and the test's stated mutation (drop the `ws:` filter) reproduces exactly
today's output. No design change, no discovery to file (fixed in #71).

**Parked acceptance tests (attempt 2, 2026-10-14, before Task 1).** The step-6 tests are
red at base `52e64d1a` and `scripts/ci-steps task` runs the whole suite, so they live in
`.agent/issue-71/pending-tests/` (gitignored) instead of being committed with `#[ignore]`
(this replaces plan Task 1 Step 6). The tracked edits were saved there and also stashed
(`git stash` entry "issue-71 a2: parked acceptance tests"), so the tracked files are at base.
Each later task copies its file(s) back as its red step.

| parked file | content | restored in plan task |
|---|---|---|
| `pending-tests/cli_digest_changed_spans.rs` (new file) | all A1/A2a/A2b/A2d/A4 tests, plus the green guards `object_level_hunk_is_outside_routines`, `limits_are_pinned` | 5 (restored `9c745429`). `declaration_only_procedures_are_roots` goes green in 7: it was committed `#[ignore]`d in `9c745429`, but agentflow check-diff refuses `#[ignore]` (`forbidden:ignore-attribute`), so `bbd8c5d7` removed it from the tree; Task 7 restores it from this parked file |
| `pending-tests/main.rs` (modified `tests/cli/main.rs`) | the `mod cli_digest_changed_spans;` line | 3 (Task 3 put the line in, with an in-tree `cli_digest_changed_spans.rs` holding only its own test and helpers; the parked file also carries that test, so the Task 5 restore replaces the in-tree file whole) |
| `pending-tests/cli_digest_query_dependencies.rs` (modified) | A2c test `dependency_routines_never_become_changed_roots` (Task 5) AND the `:169` pin `"1.5.0"` (Task 6) | 5 for the A2c test (keep the pin at `"1.4.0"` then), 6 for the pin |
| `pending-tests/cli_digest_temp_state.rs` (modified) | the `:142` pin `"1.5.0"` | 6 |

## Discrimination proofs

All Task 1 rows: run `cargo test -p al-sem --lib diff_parser`; every "pass" = `25 passed; 0 failed`.
Mutations made and reverted with Edit.

| test | mutation | fail output | pass output | commit |
|------|----------|-------------|-------------|--------|
| `header_looking_lines_inside_a_body_are_body`, `exact_fill_completes_the_hunk` | (a) move the `if let Some(h) = open.as_mut()` block below the `--- `/`+++ ` arms | both FAILED (`23 passed; 2 failed`) | 25 passed | `67e33aed` |
| `hunk_changes_follow_the_body` | (b) context arm `None \| Some(b' ')` → `Some(b' ')` (empty line rejected) | FAILED at `:761` (`24 passed; 1 failed`) | 25 passed | `67e33aed` |
| `truncated_hunk_is_an_error_with_or_without_final_newline` | (c) delete `lines.pop()` | FAILED at `:778` (with-newline case) | 25 passed | `67e33aed` |
| `minus_line_past_old_count_ends_the_hunk` | (d) `-` guard → `self.old_left > 0 \|\| self.new_left > 0` | FAILED at `:810` | 25 passed | `67e33aed` |
| `pure_deletion_cursor_starts_after_new_start` | (e) `new_count == 0` cursor `h.new_start + 1` → `h.new_start` | FAILED at `:768` | 25 passed | `67e33aed` |
| `the_line_ending_a_truncated_hunk_is_redispatched` (review fix: terminator is now `@@ -9,1 +9,1 @@`, not `diff --git`) | (f) truncation path `continue`s instead of falling through | FAILED at `:807` (with `minus_line_past_old_count_ends_the_hunk`, `23 passed; 2 failed`): left `[[DeletedAfter(2)]]`, right `[[DeletedAfter(2)], [DeletedAfter(8), Added(9)]]` (the second hunk is lost) | 25 passed | `04694720` |
| `minus_line_past_old_count_ends_the_hunk` (NOT the redispatch test) | (g) truncation path no longer clears `open` (delete `open = None;`) | only `minus_line_past_old_count_ends_the_hunk` FAILED (`24 passed; 1 failed`): paths left `["x.al"]`, right `["x.al", "y.al"]`. The redispatch test stays GREEN under (g): its `@@` terminator opens a new hunk that replaces the stale `open`. Under `67e33aed` (g) "failed" the redispatch test only through the panic at the old `expect`, which the review fix removed | 25 passed | `04694720` |
| `no_newline_marker_is_not_counted` | (h) delete the `Some(b'\\') => Some(None)` arm of `accept` | FAILED at `:835` | 25 passed | `67e33aed` |
| `omitted_counts_bound_the_body` | (i) omitted old count default `1` → `0` | FAILED at `:844` | 25 passed | `67e33aed` |
| `simple_modified_file`, `rename_with_hunks_is_renamed` (P16: well-formed bodies + `errors.is_empty()`) | (j) context arm no longer decrements `new_left` | both FAILED (`:546`, `:614`) with `hunk_changes_follow_the_body` (`22 passed; 3 failed`) | 25 passed | `67e33aed` |
| `attribute_origin_is_the_attribute_item` (Task 2; `cargo test -p al-syntax --lib attribute_origin`) | `origin: origin_of(item)` → `origin_of(node)` (the routine node) | FAILED at `lower/mod.rs:5587`: left `[(4, 4), (4, 4)]`, right `[(2, 4), (3, 4)]` | 1 passed; full `-p al-syntax --lib` 90 passed | `312c5ee8` |
| `split_header_attribute_origins_are_the_attribute_items` (review fix, lowerer-reviewer: shared attribute above `#if`, one attribute inside each header arm; `cargo test -p al-syntax --lib split_header_attribute_origins`) | `origin: origin_of(item)` → `origin_of(node)` (the split node) | FAILED at `lower/mod.rs:5613`: left `[[("Obsolete", 3, 0), ("IntegrationEvent", 3, 0)], [("Obsolete", 3, 0), ("BusinessEvent", 3, 0)]]`, right `[[("Obsolete", 2, 4), ("IntegrationEvent", 4, 4)], [("Obsolete", 2, 4), ("BusinessEvent", 7, 4)]]` | 1 passed; full `-p al-syntax --lib` 91 passed | `04694720` |
| `model_routine_declaration_start_includes_attributes` (Task 3, A1b mutation (i); `cargo test -p al-sem --test cli cli_digest_changed_spans::model_routine`). Red step: compile error `no field declaration_start_line on type &ModelRoutine` | helper `declaration_start_line` returns `r.origin.start.row` (attributes ignored) | FAILED at `cli_digest_changed_spans.rs:67`: left `(18, 18)`, right `(17, 18)` | 1 passed | `ab9ba587` |
| `interface_object_rows_carry_declaration_only_routines` (Task 4, A2d mutation (ii); `cargo test -p al-sem --test cli cli_digest_changed_spans::`). Red step: compile error `no field declaration_only_routines on type &ModelObject` | `to_stable_routine_id_from_parts(&stable_object_id, &hash, Some(&name))` → `None` (the shared object id, name not folded) | FAILED at `cli_digest_changed_spans.rs:156` (the IFoo `Bar` row, the first id assertion; the plan expected the IQux one, which sits after it): left id `…:Interface:0#6a2ce166…`, right `…:Interface:0#b782e8b0…` (`1 passed; 1 failed`) | 2 passed | `76014b08` |
| `attribute_separated_by_comment_is_not_attached_today` (Part A, task 2-3 review gap; `cargo test -p al-syntax --lib attribute_separated`) | add `RawKind::Comment => {}` to `collect_routines` (keep pending attributes across a comment, a partial #80 fix) | FAILED at `lower/mod.rs:5644`: `#80: the comment drops the attribute: ["IntegrationEvent"]` | 1 passed; full `-p al-syntax --lib` 93 passed | Part A |
| `declaration_start_line_attribute_limits` (a) (Part A; `cargo test -p al-sem --test cli cli_digest_changed_spans::`) | same mutation as above | FAILED at `cli_digest_changed_spans.rs:106`: left `(2, 4)`, right `(4, 4)` | 3 passed | Part A |
| `declaration_start_line_attribute_limits` (b) (multi-line attribute) | helper `declaration_start_line` maps `a.origin.end.row` instead of `start.row` | FAILED at `cli_digest_changed_spans.rs:113`: left `(9, 10)`, right `(8, 10)`; `model_routine_declaration_start_includes_attributes` stayed GREEN under it (the gap this test closes) | 3 passed | Part A |
| `multi_line_attribute_origin_starts_at_its_first_line` (Part A; `cargo test -p al-syntax --lib multi_line_attribute`) | `origin: origin_of(item)` → `origin_of(node)` | FAILED at `lower/mod.rs:5662`: left `[(4, 6)]`, right `[(2, 3)]` | 1 passed; full 93 passed | Part A |

Task 5 rows: each run is `cargo test -p al-sem --test cli -- cli_digest_changed_spans:: cli_digest_query_dependencies::` in an isolated `CARGO_TARGET_DIR` (scratchpad `71/mut.sh`, logs `71/mut-<label>.log`); every "pass" = `25 passed; 0 failed; 1 ignored` (the Task 7 test). Mutations made and reverted with Edit; the reverted tree re-ran green (`mut-reverted.log`).

| test(s) that failed | mutation | fail output | pass output | commit |
|------|----------|-------------|-------------|--------|
| A1a `hunk_on_assignment_line_is_a_changed_root` + 10 others (A1b, A1d, A1f, A1g, A1g(c2), (c2-overrun), (c2-exact), A1h, A2a, A4(c)) | span = header line only (`start = end = source_anchor.start_line`; stands in for the plan's op/call-site-start-line index, which `build_routine_span_index` cannot build without the snapshot) | `14 passed; 11 failed` | 25 passed | Task 5 |
| A1b (ii) `every_routine_shape_line_maps_to_its_routine` | span end exclusive: `last = end` (not `end + 1`) in `touched_by`. NOTE: the plan's literal mutation (`Added` range `first..last` only) stayed GREEN: a one-line modification also records `DeletedAfter(n-1)`, interior for the `end;` row, so the deletion arm alone keeps it touched; the end-exclusive mutation on both arms is the real "end exclusive" bug | FAILED `:466`: `src/posting.al:16 \`    end;\`: want ["PostSalesDoc"], got [] exit 2` (+ Shapes 6, 73, 105) | 25 passed | Task 5 |
| A1b (iii) `every_routine_shape_line_maps_to_its_routine`, `duplicate_id_anchor_follows_the_changed_arm`, `mixed_inputs_union_touched_declarations` | dedupe spans by routine id (`retain` first per id) | `src/Shapes.al:26 \`    begin\`: want ["Dup"], got []` (the plan named row 21; the first arm is the one kept, so the second-arm row fails) | 25 passed | Task 5 |
| A1b (iv) `every_routine_shape_line_maps_to_its_routine`, A2b `file_digest_returns_every_routine_of_the_file` | keep only the first span per `(source_unit_id, start)` | `src/Shapes.al:37 … want ["Split", "Split"], got ["Split"]` | 25 passed | Task 5 |
| A1c `context_lines_do_not_create_roots`, `hunk_body_is_count_bounded` | header-window fallback (also touch any span holding a line of `[new_start, new_start+new_count)`) | `:522` left `0` right `2` | 25 passed | Task 5 |
| A1d (ii) `deletions_resolve_by_interior_gap`, A1i `limits_are_pinned` (a), `context_lines_do_not_create_roots`, `hunk_body_is_count_bounded` | `DeletedAfter(l)` touches when `l` OR `l+1` is in the span | `:562` left `0` right `2`; `limits_are_pinned` `:705` left `0` right `2` | 25 passed | Task 5 |
| A2c `dependency_routines_never_become_changed_roots` | drop the `ws:` filter (index every non-empty unit) | `cli_digest_query_dependencies.rs:340` left `0` right `2` | 25 passed | Task 5 |
| A4(c) (i)/(iii) `duplicate_id_anchor_follows_the_changed_arm`, `mixed_inputs_union_touched_declarations` | `.next()` instead of `.next_back()` | `:1371` left `(19, [19, 24])` right `(24, [19, 24])`; mixed `:1461` left `19` right `24` | 25 passed | Task 5 |
| A4(c) (ii) `duplicate_id_anchor_follows_the_changed_arm` | `decls.last()` regardless of touch | `:1357` left `(24, [19, 24])` right `(19, [19, 24])` (c2) | 25 passed | Task 5 |
| A4(c) (iv) `duplicate_id_anchor_follows_the_changed_arm` | drop the `anchors` insert (`if false && …`) | `:1348` left `(24, [])` right `(24, [19, 24])` | 25 passed | Task 5 |
| A4 `entry_anchor_is_the_declaration`, A2c | anchor file from `normalize_input(source_unit_id)` | `:1226` left `("src/posting.al", 18, 4)` right `("ws:src/posting.al", 18, 4)` — fails at (a) first; (b) is not isolated by this run | 25 passed | Task 5 |
| A4 (a)/(e) `entry_anchor_is_the_declaration` | anchor line from `start` (declaration start) instead of `source_anchor.start_line` (run together with the next row; independent failures) | `:1226` left `("ws:src/posting.al", 17, 4)` right `(…, 18, 4)` | 25 passed | Task 5 |
| A1e `object_level_hunk_is_outside_routines` + A1c, A1d, A1f, A1g, A1h, A1i | every hunk touches every declaration of its file | `object_level` `:614` left `0` right `2` (14 failed in the combined run) | 25 passed | Task 5 |
| A2a `file_with_only_opless_routines_is_matched`, A2b, A1b, A1g, A1h, A2c, A4, A4(c), both review-focus tests | old population: drop touched ids with no op/call site in the snapshot | A2a `:966` left `2` right `0`; A2b left misses `Dup`, `OnPub` | 25 passed | Task 5 |
| `hunkless_modified_file_takes_every_declaration`, A1h `rename_with_and_without_hunks` | hunkless file touches only its first declaration | `:1478` left `["PostSalesDoc"]` right `["OnAfterPostSalesDoc", "PostSalesDoc"]` | 25 passed | Task 5 |
| `mixed_inputs_union_touched_declarations`, A4(c) (c4 `--routine`), A4 (a) | a selector touches no declaration (no union) | mixed `:1461` left `19` right `24`; dup `:1381` left `-1` right `24` | 25 passed | Task 5 |
| A4 (d) `entry_anchor_is_the_declaration` UTF-8 byte column (Part A review follow-up; the test already asserted col 11) | `Utf16Cols::col` counts UTF-16 units of the line prefix (`l.get(..byte_col).map_or(byte_col, \|p\| p.encode_utf16().count())`) | FAILED `:1148` `(d)`: left `(…MixedCase.Codeunit.al, 6, 10)` right `(…, 6, 11)`; (a)/(b) passed before it | 2 passed (with `interface_object_rows…`); full `ci-steps task` exit 0 | `1a3eb9b1` |
| A1b (ii) added-only row in `every_routine_shape_line_maps_to_its_routine` (new: `@@ -15,0 +16,1 @@` / `+    end;`, no `-` line) | the plan's literal break: `Added(n) => (first..last)` (end-exclusive `Added` only) | FAILED `:475` `added-only \`end;\``: left `(2, [])` right `(0, ["PostSalesDoc"])`; every table row stayed green (the gap this row closes) | 1 passed | `1a3eb9b1` |
| `interface_object_rows…` new `assert_ne!(IFoo.Bar, IQux.Bar)` | fold `"IFoo"` for object `IQux` (`Some(if name == "IQux" { "IFoo" } else { &name })`) | FAILED `:1415` `left != right` (both `…#b782e8b0…`) | 1 passed | `1a3eb9b1` |
| `interface_object_rows…` IQux eq row | fold `"IQuxZ"` for `IQux` only | FAILED `:1416` (IFoo rows and the `assert_ne` passed first) | 1 passed | `1a3eb9b1` |
| `interface_object_rows…` ControlAddIn row | no name fold for ControlAddIn only (`(object_type != "ControlAddIn").then_some(..)`) | FAILED `:1425` (`…#cdaea713…` vs `…#8491c758…`; IFoo/IQux passed first) | 1 passed | `1a3eb9b1` |
| workspace.rs empty-name filter (declaration-only) | not a test change: kept with a reason. `tree-sitter parse` of `interface IFoo { procedure (); }` gives `interface_procedure` with `name: (identifier [2,13]-[2,13])` (zero-width MISSING), so the lowerer emits an empty-named routine | — | — | `1a3eb9b1` |
| Task 6: pins `cli_digest_query_dependencies.rs:169`, `cli_digest_temp_state.rs:142` (restored from pending-tests, byte-identical) + the 21 regenerated digest JSON goldens | envelope `schemaVersion` back to `"1.4.0"` | red step (pins only): both FAILED `left: String("1.4.0") right: "1.5.0"`; after regen: both pins + `cli_b_digest_differential::{json_matches_goldens, diff_json_matches_golden}` FAILED (`1 passed; 4 failed`; human green, the human form has no schema) | 5 passed; `check-goldens` green; `ci-steps task` exit 0. Regen #2: 21 files, 1 line each, triage receipt in both checkouts | `3daa1e58` |
| Task 7 A2d `declaration_only_procedures_are_roots` (restored from pending-tests, no `#[ignore]`). Red step: FAILED `:1113` (a) `diff-file-unmatched`, left `2` right `0` | (i) declaration-only spans left out of the index (`.filter(\|_\| false)` after the `flat_map`) | FAILED `:1113` (a) `diff-file-unmatched`, left `2` right `0` | 1 passed; acceptance command `27 passed`; `ci-steps task` exit 0 (no golden moved) | `eaa8525b` |
| A2d (ii)/(iii) | `to_stable_routine_id_from_parts(.., None)` in workspace.rs (no name fold) | FAILED `:1130` (a) stableId: left `…#6a2ce166…` right `…#b782e8b0…` | 1 passed | `eaa8525b` |
| A2d (iv) | route through the identity lookup first (`if !display_by_id.contains_key(rid) { push RootNotInSnapshot; continue }` above the build-first branch) | FAILED `:1115` (a) entries left `0` right `1`; doc has `DIAG-digest-root-not-in-snapshot`, `rootsRequested 2`, `rootsResolved 0` | 1 passed | `eaa8525b` |

Final-panel fix pass (2026-10-14). Runs: `cargo test -p al-sem --lib -- diff_parser population_tests` (pass = `30 passed`) and `cargo test -p al-sem --test cli -- cli_digest_changed_spans:: cli_digest_query_dependencies::` (pass = `30 passed`); logs in scratchpad `71/`. Mutations made and reverted with Edit; M1-M4 ran together (each fails a different test, attributed per test), then M5+M6 together; the reverted tree re-ran green (`git diff` scanned for leftover breaks).

| test | mutation | fail output | pass output | commit |
|------|----------|-------------|-------------|--------|
| B `out_of_range_hunk_header_is_an_error_not_a_panic` (unit). Red step before the fix: FAILED `attempt to add with overflow` at `diff_parser.rs:242` | range check disabled (`if false && !(fits…)`) | FAILED `attempt to add with overflow` at `diff_parser.rs:252` (`26 passed; 1 failed`) | 27 passed | `5b956c2d` |
| B `out_of_range_hunk_header_is_a_parse_error` (CLI) | same | FAILED: `alsem` panicked (`attempt to add with overflow`, `diff_parser.rs:252`), no JSON (`23 passed; 1 failed`) | 24 passed | `5b956c2d` |
| C `plus_header_without_a_pending_minus_header_keeps_the_current_file` (unit). Red step before the fix: left `["y.al"]` right `["x.al"]` | `+++` arm guard `pending_old_path.is_some()` → `true \|\| …` (any `+++` opens a file) | FAILED left `["y.al"]` right `["x.al"]` | 27 passed | `5b956c2d` |
| C `orphan_plus_header_keeps_the_current_file` (CLI, astra's repro) | same | FAILED `:976` roots left `["HandlePosted"]` right `["PostSalesDoc"]` (posting's `Added(3)` lost) | 24 passed | `5b956c2d` |
| A `quoted_interface_name_is_unescaped_in_the_id` | M1: fold the raw IR name (`Some(&name)` instead of `Some(&logical_name)`) | FAILED `:1600` left `…:Interface:0#df2e81ea…` right `…:Interface:0#c9f7c5ee…` | 30 passed | `edacd9f1` |
| E `declaration_only_routines_follow_the_population` | M2: iterate `o.routines` instead of the population's `routine_ixs` | FAILED `workspace.rs:2924` left `2` right `1` | 30 passed | `edacd9f1` |
| D (a) `deleting_a_routines_first_attribute_line_makes_it_a_root` | M3: `DeletedAttributesAfter(l)` touches like `DeletedAfter` (interior only: `first <= l`) | FAILED `:723` (a) exit left `2` right `0` | 30 passed | `5b956c2d` |
| D `attribute_only_deletion_runs` (`-[A]`/`-x` case) | M4: drop the downgrade (`else if false && !attribute …`) | FAILED `diff_parser.rs:942` left `[[DeletedAttributesAfter(2)]]` right `[[DeletedAfter(2)]]` | 30 passed | `5b956c2d` |
| D (b) narrowness (deleted blank + attribute lines 17-18 → no root) and `attribute_only_deletion_runs` (`-x`/`-[A]` case) | M5: classify the run by its LAST deleted line | FAILED `:736` (b) exit (a root appeared); unit FAILED `diff_parser.rs:944` left `[[DeletedAttributesAfter(2)]]` right `[[DeletedAfter(2)]]` | 30 passed | `5b956c2d` |
| G sorted sweep (`RoutineSpanIndex::touched_in`), no output change: covered by A1b, A2d | M6: skip predicate off by one (`end + 1 <= n`) | FAILED `every_routine_shape_line_maps_to_its_routine` `:475` (`added-only end;` left `(2, [])`), `declaration_only_procedures_are_roots` `:1187` (one-line spans skipped) | 30 passed | `5b956c2d` |
| F (remove `ModelDeclarationOnlyRoutine.parameters`/`.return_type`, `DeclOnlyInfo` → `declaration_only_display: Option<String>`) | not a behaviour change: no reader (grep); ids unchanged (`signature_hash` still reads the parameters); `interface_object_rows_carry_declaration_only_routines` and A2d stay green on the hand-computed ids | — | 30 passed | `edacd9f1`, `5b956c2d` |
| Round 2 (flash): `deleting_a_multi_line_attribute_makes_the_routine_a_root` (a) multi-line group deleted → `Handler` root; `attribute_only_deletion_runs` multi-line case (`:976`) | M7: per-line rule (every deleted line must start with `[`: `if !text.starts_with('[')`) | CLI FAILED `:775` (a) exit left `2` right `0` (`hunk-outside-routines` 6-6); unit FAILED `diff_parser.rs:976` left `[[DeletedAfter(2)]]` right `[[DeletedAttributesAfter(2)]]` (lib `27 passed; 1 failed`, cli `26 passed; 1 failed`) | lib 28 passed, cli 27 passed | `8c4306ee` |
| Round 2: `deleting_a_multi_line_attribute_makes_the_routine_a_root` (b) mixed (group + `// gone`) → no root; `attribute_only_deletion_runs` (`-[A]`/`-x`, `:969`) | M8: no depth-0 `[` check after the run's first line (`if !self.in_deletion_run && …`) | CLI FAILED `:794` (b) exit left `0` right `2` (a root appeared); unit FAILED `diff_parser.rs:969` left `[[DeletedAttributesAfter(2)]]` right `[[DeletedAfter(2)]]` | lib 28 passed, cli 27 passed | `8c4306ee` |
| Round 3 (astra): `deleting_a_routine_on_its_attribute_closing_line_blames_no_neighbour` (delete `[IntegrationEvent(` / `false, false)] local procedure Gone() begin end;` → `Survives` not a root) and `attribute_only_deletion_runs` closing-line case | M9: bracket balance only (any text at depth 0 accepted: default arm `_ => {}`) | CLI FAILED `:820` exit left `0` right `2` (`Survives` blamed); unit FAILED `diff_parser.rs:989` left `[[DeletedAttributesAfter(2)]]` right `[[DeletedAfter(2)]]` | lib 28 passed, cli 28 passed | `7da1fbe6` |
| Round 3: `attribute_only_deletion_runs` trailing-comment case (`-    [A] // note` → attribute-only) | M10: no `//` allowance (`_ if false && …starts_with("//")`) | unit FAILED `diff_parser.rs:994` left `[[DeletedAfter(2)]]` right `[[DeletedAttributesAfter(2)]]` | lib 28 passed | `7da1fbe6` |

C decision evidence (concatenated `diff -u`, scratchpad `71/cdec`): `exact.patch` = `--- a/x.al`/`+++ b/x.al`/`@@ -3,2 +3,1 @@`/`-old`/`--- a/y.al`/`+++ b/y.al`/`@@ -9,1 +9,1 @@`/`-x`/`+y`. `git apply --numstat exact.patch` → `2	3	x.al` (one file; the `---`/`+++` pair counted as a deleted and an added line). GNU patch 2.7.6 `--dry-run -p1` → one file prompt for `x.al`, "2 out of 2 hunks ignored". `short.patch` (`@@ -3,3 +3,1 @@`, body short): git `error: corrupt patch at short.patch:7`; patch `malformed patch at line 7: @@ -9,1 +9,1 @@`. Both tools treat the header counts as authoritative; al-sem does too (it reports the short case as `diff-parse-error` and continues). Decision: keep counts authoritative, no header-pair detection; stated as a limit (Known limits, CHANGELOG Fixed).

## Findings register summary

Attempt 1: round 1 added R1-01..R1-16 (1 critical, 7 important, 8 minor); round 2 added
R2-01..R2-06 and conductor R2-07; round 3 added R3-01, R3-02 and re-raised R2-02.
Attempt 2: 25 entries carried over. R2-02 (astra re-raise), R3-01 and R3-02 are now
`fixed` with attempt-2 evidence (Entry anchor + A4(c); Data plumb 6 + A2d(a); Data plumb 4 +
A1g(c2)). The other 22 keep their dispositions (all `fixed`), with evidence re-pointed to
the attempt-2 citations. Attempt-2 spec round 1: astra re-raised R1-11 and added R1-26
(important, digest schema already released) and R1-27 (minor, A1g(c2) mutation cannot
fail); all three `fixed` (Data plumb 7, P21, golden license, A2d(f), A3; A1g(c2-overrun),
A1g(c2-exact)). 27 entries. Every `reviews` is reset to `unreviewed` and every `hash` is
this ledger's sha256. See `findings.json`.

## Gate results

None run yet (no source change).

## Discoveries

```json
[
  {
    "subsystem": "l5 / digest (changed-root resolution)",
    "locator": "src/engine/l5/digest_cli.rs:84-100 strip_scheme / normalize_input",
    "symptom": "A --file path written with a leading './' is never matched: './src/posting.al' gives file-unmatched and exit 2 while 'src/posting.al' matches.",
    "kind": "bug",
    "origin_issue": 71,
    "reproducer": "alsem digest tests/r0-corpus/ws-d8-commit-in-tx --file ./src/posting.al --deterministic -> exit 2, changedRootsDiagnostics [{kind: file-unmatched, file: ./src/posting.al}]",
    "pre_existing": true,
    "capability": "digest accepts any spelling of a workspace-relative path that names the same file",
    "acceptance": "'./src/x.al', 'src/./x.al' and 'src\\x.al' resolve to the same routines as 'src/x.al'; a test drives the alsem binary with each form."
  },
  {
    "subsystem": "l5 / digest (changed-root resolution)",
    "locator": "src/engine/l5/digest_cli.rs:312-321 (diff file -> index lookup); src/engine/l5/diff_parser.rs strip_ab_prefix",
    "symptom": "A diff produced by 'git diff' at a repository root above the workspace (e.g. DO's workspace is Cloud/) names files 'Cloud/src/x.al'; digest reports diff-file-unmatched for every file and exits 2.",
    "kind": "enhancement",
    "origin_issue": 71,
    "reproducer": "Copy ws-d8-commit-in-tx to <tmp>/ws; diff with headers '--- a/Cloud/src/posting.al' / '+++ b/Cloud/src/posting.al' and a hunk on line 8 -> exit 2, [{kind: diff-file-unmatched, file: Cloud/src/posting.al}]",
    "pre_existing": true,
    "capability": "digest --diff accepts a diff made from the enclosing git repository root",
    "acceptance": "Diff paths are resolved relative to the workspace by stripping the workspace's path below the git top level (or a --diff-root option); a test with a 'Cloud/' prefixed diff resolves the same roots as the unprefixed one."
  },
  {
    "subsystem": "l5 / digest (document projection)",
    "locator": "src/engine/l5/digest_cli.rs:470-476 split_object_routine; src/engine/l5/snapshot.rs:1087-1110 derive_identity_table",
    "symptom": "Every digest entry's routine.objectDisplay is the empty string: the identity table's display names are bare routine names, so split_object_routine never finds '::'. 41 of 41 entries in tests/cli-b-goldens/digest/*.json carry objectDisplay \"\". 'alsem query effects' fills the same field as '<ObjectType> <ObjectName>' (src/engine/l4/effect_query_cli.rs:497).",
    "kind": "bug",
    "origin_issue": 71,
    "reproducer": "grep -h objectDisplay tests/cli-b-goldens/digest/*.json | sort | uniq -c -> 41 \"objectDisplay\": \"\"",
    "pre_existing": true,
    "capability": "a digest entry names the object that owns the routine",
    "acceptance": "objectDisplay is '<ObjectType> <ObjectName>' as in query effects; digest goldens move only in that field (and the human header, which prints 'object::routine' when objectDisplay is set)."
  },
  {
    "subsystem": "l5 / digest (changed-root resolution)",
    "locator": "src/engine/l5/digest_cli.rs:335-357 (hunks outside every routine)",
    "symptom": "A change outside every routine but inside an object (the object's global var section, e.g. adding 'temporary' to a global record; a table field's TableRelation/CalcFormula; a page's SourceTable; an #if/#else directive line choosing between two procedure versions) changes the behaviour of routines, yet yields no root: hunk-outside-routines, exit 2.",
    "kind": "enhancement",
    "origin_issue": 71,
    "reproducer": "Copy ws-d8-commit-in-tx; diff changing line 29 'field(50; \"Status Posted\"; Boolean) { }' -> exit 2, hunk-outside-routines",
    "pre_existing": true,
    "capability": "digest maps an object-level declaration change to the routines it affects",
    "acceptance": "A changed global variable declaration makes every routine of the object that references it a root; a changed field declaration makes the routines that read or write that field roots; each with a test."
  },
  {
    "subsystem": "al-syntax / lowerer (routine attributes)",
    "locator": "crates/al-syntax/src/lower/mod.rs:820-826 collect_routines pending attributes; :984-986 (`_ => pending.clear()`); preproc arms e.g. :936",
    "symptom": "An attribute separated from its procedure by a comment, pragma or #if line is dropped: `pending` is cleared on any other named child, and comments are named extras. An [EventSubscriber] followed by a '// note' line stops binding, so the subscriber's effects vanish from every publisher's cone (lost event edges).",
    "kind": "bug",
    "origin_issue": 71,
    "reproducer": "A codeunit with a publisher Fire() and a subscriber whose [EventSubscriber(...)] line is followed by '// note' before 'local procedure Handler()' that calls Commit(): alsem digest --routine Fire reports ['EVENT_PUBLISH'] only; remove the comment line -> ['COMMIT','EVENT_PUBLISH'].",
    "pre_existing": true,
    "capability": "a routine keeps every attribute written before it, whatever trivia sits between them",
    "acceptance": "collect_routines keeps pending attributes across comment/pragma trivia (and across an #if wrapping only attributes, per arm); lowerer tests for an intervening line comment, block comment, pragma and multi-line attribute; the subscriber binds in the reproducer."
  },
  {
    "subsystem": "l5 / digest (changed-root resolution) + program model",
    "locator": "src/program/model/workspace.rs:881-904 (Interface/ControlAddIn skip); src/engine/l5/digest_cli.rs resolve_changed_roots",
    "symptom": "A change to an Interface or ControlAddIn method declaration yields no root for the routines that implement it, although every implementer's contract changed. Before #71 not even the method was a root (file-unmatched / diff-file-unmatched); after #71 the method itself is a declaration-only root with no effects.",
    "kind": "enhancement",
    "origin_issue": 71,
    "reproducer": "A workspace with 'interface IFoo { procedure Bar(); }' in src/IFoo.al and a codeunit implementing IFoo: alsem digest <ws> --file src/IFoo.al -> exit 2, file-unmatched.",
    "pre_existing": true,
    "capability": "digest maps a changed interface method to the routines that implement it",
    "acceptance": "--file / --diff on an interface method resolves to every workspace implementation of that method (codeunits declaring 'implements IFoo'); a test with two implementers."
  },
  {
    "subsystem": "program model (routine population)",
    "locator": "src/program/model/workspace.rs:880-904 (Interface/ControlAddIn skip and its comment)",
    "symptom": "The model excludes Interface/ControlAddIn procedure declarations from ModelRoutine, and the comment justifies it by byte-for-byte parity with the al-sem TS reference and 'tests/r3a1_vectors.rs' oracles. That reference is retired (CLAUDE.md: Rust-owned baselines), so the stated reason is gone; the skip now only hides declarations from every consumer (selectors, query, graph). Removing it is not free: detectors such as unused-parameter would fire on signature-only methods, so each consumer needs a body/declaration-only gate first.",
    "kind": "enhancement",
    "origin_issue": 71,
    "reproducer": "A workspace with 'interface IFoo { procedure Bar(); }': alsem digest <ws> --routine Bar -> selector-unmatched (the selector index has no row for Bar); read workspace.rs:880-904 for the stale justification.",
    "pre_existing": true,
    "capability": "the model holds every declared routine, marking declaration-only ones, with consumers gating on body availability",
    "acceptance": "Interface/ControlAddIn procedures are ModelRoutines flagged declaration-only; detectors that need a body skip them (a test per affected detector, e.g. unused-parameter does not fire on an interface method); the stale comment is rewritten; digest's ModelObject.declaration_only_routines carrier from #71 is folded into the routine population."
  },
  {
    "subsystem": "engine ids (object identity)",
    "locator": "src/engine/ids.rs:130-132 encode_object_id; src/program/model/workspace.rs:718 (o.id.unwrap_or(0))",
    "symptom": "Objects without a number (every Interface and ControlAddIn) all get the object id '<app>/<Kind>/0', so two interfaces collide on one ModelObject id and on anything keyed by it (object rows, object signature fingerprints, implements joins by id).",
    "kind": "bug",
    "origin_issue": 71,
    "reproducer": "A workspace with 'interface IFoo { procedure Bar(); }' and 'interface IQux { procedure Bar(); }': both ModelObject rows have id '<app>/Interface/0' (workspace.rs:718 + ids.rs:131). (#71's declaration-only digest roots avoid it by folding the object name into their id; every other consumer of the object id still sees the collision.)",
    "pre_existing": true,
    "capability": "every object, numbered or not, has a unique stable id",
    "acceptance": "Unnumbered objects get a unique object id (e.g. keyed by name); two interfaces get distinct ModelObject ids; #71's name-folded declaration-only id is re-derived from it; a test pins both object ids."
  },
  {
    "subsystem": "l5 / digest (changed-root resolution) + al-syntax lowerer",
    "locator": "crates/al-syntax/src/lower/mod.rs controladdin_events_are_never_lowered_as_routines (events are not RoutineDecls); src/program/model/workspace.rs declaration_only_routines (built from o.routines)",
    "symptom": "A change to a ControlAddIn `event` declaration (e.g. `event OnSaveHTML(htmlString: Text);`) yields no changed root: the lowerer never emits a RoutineDecl for a controladdin event, so it has no span. A page usercontrol's trigger subscribing to that event is affected by the signature change but is not reported.",
    "kind": "enhancement",
    "origin_issue": 71,
    "reproducer": "A workspace with 'controladdin MyAddIn { event OnReady(); procedure Init(); }': a --diff hunk on the 'event OnReady();' line -> hunk-outside-routines, exit 2; the same on 'procedure Init();' -> a declaration-only root.",
    "pre_existing": true,
    "capability": "digest maps a changed ControlAddIn event declaration to a root (the event itself, or the usercontrol triggers that handle it)",
    "acceptance": "A changed controladdin event line gives a declaration-only root (or the handling usercontrol triggers); a test through the alsem binary; the lowerer test controladdin_events_are_never_lowered_as_routines is revisited deliberately if events become routine-like declarations."
  }
]
```

## Not delivered

The acceptance is delivered in full (every routine with a span, Interface/ControlAddIn
procedures included). Outside #71, each recorded as a discovery, a stated limit or a
bc-dev-mcp follow-up:
- Mapping an Interface/ControlAddIn method change to its implementing routines (the
  Interface/ControlAddIn discovery); `--routine` selectors for declaration-only
  procedures.
- An attribute separated from its routine by trivia is outside the span (P14, the
  routine-attributes discovery).
- Object-level change mapping, incl. `#if` directive lines (the object-level discovery,
  A1i(b)).
- ControlAddIn `event` declarations: not lowered as routines, so not roots (the
  "ControlAddIn events" discovery). The acceptance covers procedures only.
- `#if`-split header arms with different signatures both become roots for a change in
  either header line (safe over-approximation, Known limits).
- Declaration-only roots are special-cased in digest rather than represented in the model
  (root cause: issue #82, the stale Interface/ControlAddIn skip).
- Concatenated `diff -u` without `diff --git` after a malformed short hunk: counts stay
  authoritative (as in git and GNU patch), so the next file's header pair can be read as
  body; an exact fill reports no error (Known limits, A1g(c2-exact)).
- bc-dev-mcp (separate repo, not changed or gated by #71): its reader ignores
  `routine.anchors` and calls `--file`, so for a multi-declaration id (two `#if` arms with
  one signature) it joins only the last declaration when subjects carry `endLine`
  (Entry anchor, A4(c4)). Expanding `parseAlSemDigest` over `routine.anchors`, or switching
  the provider to `--diff`, is a bc-dev-mcp change.
