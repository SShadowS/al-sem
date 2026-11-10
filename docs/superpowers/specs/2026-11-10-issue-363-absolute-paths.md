# #363: an absolute path to a workspace file names that file

Status: APPROVED (2026-11-10) by both panel reviewers, sol `gpt-6.1-sol` and a local Opus
reviewer, in round 3, after DO NOT APPROVE in rounds 1 and 2; every finding and where it went is
in §11. Issue #363,
split from #362 (TLS-03). Earlier work: the #362 attempt-1 ledger
(`/work/alsem/.agent/issue-362/ledger.md`: probes R5, R6, I1–I7, C4, C5 and a prototype) and sol's
review of it (`/work/alsem/.agent/runs/20261026-025200-dce629/spec-r1-sol.md`, S1–S14).

## 1. The bug

`alsem digest <ws> --file <ws-abs>/src/posting.al` is `file-unmatched` (exit 2), in every root
mode. The same holds for `--changed <abs>.al` (a file input since #362), for a diff whose
`---`/`+++` headers are absolute (`diff -u /abs/a /abs/b` writes them so), and for an absolute
`Only in` line. Cause: a changed path is read only relative to the diff root (`path_readings`,
`src/engine/l5/digest_cli.rs:414`). An absolute path keeps its root (`clean_segments`), so neither
the as-written reading nor `strip_diff_prefix` (`:374`) ever matches a workspace key, and nothing
knows where the workspace is on disk: `DiffRoot` (`:334`) holds only its path below the root.

## 2. Anchors

An **anchor** is the workspace's absolute location, held as a root and its folders, case kept:
- the root is `/` (Unix), `//` (a `//server/share` path) or a disk `X:/` (Windows: one ASCII
  letter, a colon, then `/` or `\`); the folders are the remaining non-empty segments, `\` read
  as `/` first, `.` and empty segments dropped (as `clean_segments` does). A drive-relative
  `C:x.al` or a bare `C:` has NO root: it is a relative path and never matches an anchor.
- Spelled (wire, messages) as the root then the folders `/`-joined: `/home/x/ws`, `//srv/s/ws`,
  `C:/Git/ws`; a root-only anchor as `/`, `//` or `C:/`.
- A workspace at a root itself (`/`, `C:\`) is an anchor with no folders: it is a prefix of every
  absolute path with that root.
- An anchor with a `..` folder is dropped (it would never match: `..` is never cancelled, #76).

There are at most two, in this order:
1. **lexical**: `absolute(workspace)` through the plan's file-system view (`cx.absolute`);
2. **canonical**: `canonicalize(workspace)` (`cx.canonicalize`), with the Windows `\\?\` disk
   prefix removed (`\\?\U:\Git\x` → root `U:`, folders `Git/x`); a `\\?\UNC\…` form gives no
   canonical anchor. Dropped when it equals the lexical one.

A failure of either is no anchor, never an error: planning errors stay exactly as today.

## 3. Reading rule

`strip_anchor(anchor, path)`: `path` is `\`→`/`-converted and split like an anchor. It matches when
the roots are equal and the anchor's folders are the path's first folders, compared with
`same_folder` (exact, case included, #77), with ONE exception: a disk root compares ASCII
case-insensitively (`c:` = `C:`). The result is the rest, lowercased (the key form), or no match
when the rest is empty (the path IS the workspace or above it).

`path_readings(root, path)`: if an anchor matches, the path has exactly ONE reading, that rest;
the first matching anchor in §2's order wins. Otherwise (no anchor matches, including an empty
rest) the readings are today's, unchanged. So an absolute path names one place; it can never be
`*-ambiguous` through the root's two readings, and two anchors cannot give two readings.

**S4.** `strip_diff_prefix` and its relative GitTop/Explicit prefixes are untouched: only
`strip_anchor` folds a disk letter, and only a disk root is a disk. A relative path never matches
an anchor (it has no root). A Unix folder literally named `C:` is a folder, compared exactly.

## 4. Where the anchors are decided (Q2)

**In the diff-root decision** (`decide_diff_root`, `src/bin/alsem.rs`), not in the job (the earlier
ledger's choice). The decision is what every path reader already receives, so the request's
file-system questions stay answered once, by the caller ("planning never asks the file system").
Today the decision calls `cx.canonicalize(workspace)` in every branch that reads paths, but
`cx.absolute(workspace)` only in the `--diff-root` branch; the lexical anchor is a new
`cx.absolute` call in every branch where `reads_paths` is true, made before any early return
(`Workspace` when canonicalize fails, or no `.git` found). The engine side's `ShippedFs` already
answers `absolute`. When `reads_paths` is false there are no anchors (no path is read).

**Shape inside the binary.** `pub struct PathRoot { pub root: DiffRoot, pub anchors: Vec<Anchor> }`
replaces `DiffRoot` wherever the decision travels: `PlanCx.diff_root_decided`, `Shipped`,
`decided_root`, `DigestRequest.diff_root`, `ChangedInput.diff_root`, `ExplainMode::Diff{root}`,
`RootsAsk::Diff{root}` (tests), `MainAsk::Diff{root}` (impact), `path_keys`, `account`.
`PathRoot::default()` (workspace, no anchors) is what impact's and tests' `files` mode, explain's
routine mode and prove use today (`DiffRoot::default()`), and what the in-process
`run_digest_pipeline*` APIs (tests only) keep: stated limits, §9.

**On the wire** (`WorkerJob.diff_root`, `Decisions.diff_root`, `host/mod.rs:270`): the existing
`diffRoot` value keeps its exact shape, and a sibling `pathAnchors` (a list of strings, §2's
spelling, `#[serde(default, skip_serializing_if = "Vec::is_empty")]`, named `pathAnchors` on both:
`WorkerJob` has no camelCase container attribute, so its field gets an explicit `rename`) carries
the anchors. `PROTOCOL_VERSION` stays 1: a front end and child of different builds still talk,
and the older side simply reads no anchors (absolute paths unmatched, today's behaviour). Tests
pin a round trip with and without the field, and the decoding of a legacy-shaped message (no
field) into no anchors. Both front ends fill it: `CliDecide` (MCP) and the LSP's `HelperDecide`
(through `engine-ask --params`) take it from the same `decide` result as `diffRoot`.

## 5. Which paths it reaches (Q1)

Every changed path, because they share `path_readings`: file inputs (`--file`, `--changed *.al`),
diff headers, digest's malformed-record veto (`poisoned`, `digest_cli.rs:963`, sol r1 S11), and
`Only in` paths, in digest, `explain --diff`, `tests --diff` and `impact --diff`. `ws:` ids are
unchanged (`file_hits` checks them first). Rationale: #339's rule that file inputs read like diff
paths.

**`Only in` at, below or above the workspace (S3).** `holds_workspace_files` (`:444`) gains the
anchor form of `at_or_above_prefix`: when the workspace has a file and the path's root and
folders are a prefix of an anchor's (the path is the workspace itself or a folder above it), it
holds workspace files. A folder below the workspace reads through the anchor to a folder key and
is found in the folder set as today. So `Only in <ws-abs>: src`, `Only in <t>/repo: Cloud` and
`Only in <t>: repo` keep #353's folder rule (`diff-file-unmatched`, roots lost, the existing
limit) instead of `diff-file-not-al`. An absolute folder outside every anchor stays
`diff-file-not-al`.

**impact's base side (round-1 I1).** `impact --base` reverses the diff and runs `impact --part
main --diff=-` on a copy of the workspace in a scratch folder (`src/engine/impact/before.rs:263`;
the copy keeps the workspace's path below the git top, `base.rs:92`, so relative paths read the
same there). The copy's own anchors are the scratch location, so a header naming the live
workspace would match on the new side and silently not on the base side. Fix: `base_side` is
handed the current side's decided `PathRoot` (the same ordered anchors, built by the one anchor
builder `decide_diff_root` uses, never recomputed). Where it comes from (Opus r3 I-B), per route:
today no route keeps it (the CLI's main part decides inside `run_part_with_stdin` and drops the
result, `src/bin/alsem.rs:~2652`; `base_side` holds only `req.diff_root: Option<String>`,
`before.rs:191`). So the impact orchestrator decides the impact command's root once — CLI through
`diff_root_of` on the impact command (`run_impact_cmd`), MCP through the `Decide` trait on the
impact argv (`shared.decide.decide(argv, diff_text)`, whose `Decisions` carry `pathAnchors`) — and
puts its anchors in `BaseCx.anchors`. As built (implementation note): the main part still makes
its own decision as today (CLI: `run_part_with_stdin`; MCP: the part's own `Decide` call); it is
the same builder on the same command in the same process, so its anchors are the same — only the
base run, which runs on the copy, needs them handed over. `base_side` passes them to the base run
through a hidden, repeatable `--path-anchor <spelling>` argument. Scope of that argument: it is
accepted only with `impact --part main --diff`, refused (usage error) anywhere else; its values
go through the same anchor parser (root-only forms, `..` dropped); when present it REPLACES the
lexical and canonical anchors entirely — "an override was given" is kept apart from "it parsed
to any anchor", so values that all drop give NO anchors, never the scratch copy's (sol r3) — with no file-system probe for them (the root itself is
decided as today). This holds on the CLI and the MCP routes alike, since both build the base argv
in `base_side`; on the MCP route the base part runs as a one-shot child whose decision
(`diff_root_of`'s impact arm → `decide_diff_root`) reads the argv anchors.

**Limit (Opus r2 N1).** The base run reads a modified record by the reversed diff's new path,
which is the ORIGINAL diff's OLD path (`reverse_diff`, `base.rs:244–257`). When that old path
has no base reading — it lies outside every live anchor and reads to no workspace file
relatively (`diff -u /abs/old/x.al /abs/ws/x.al`, or a two-tree relative diff `old/x.al
ws/x.al`) — the base side cannot map the record, today and after this change, while the new side
can. An ordinary rename with both paths below the live workspace maps on both sides (sol r3). Reporting the base side's
failed mapping as unmapped, instead of dropping it, is a follow-up issue (file it), not #363.

**The base cache key (sol r2).** The key for a diff's cached base result (`rdh`, `before.rs:219`)
is today the hash of the reversed diff alone, yet the same text maps differently under another
`rel` (the workspace's path below the root the user chose, `rel_below_root`, `before.rs:58–83,202`):
the base run always gets no `--diff-root` and the copy always has a `.git` at its top
(`base.rs:153`), so its root is the copy's git top with `rel` as prefix, and `rel` is what changes
the mapping (Opus r3 I-A). A pre-existing weakness. The key becomes the hash of an unambiguously
encoded, versioned tuple: a tag `rdh-v2`, the reversed diff, `rel` (with the root kind, harmless),
and — only when the reversed diff holds a rooted path — the ordered anchors (each
length-prefixed). "Rooted" uses the anchor parser's root grammar (`/`, `//`, `X:/`, `\` read as
`/`) over every path the parser returns: each record's path and old path, the paths of records
with a parse error (they drive the veto) and the `Only in` paths. A relative-only diff's key does not depend on where the checkout lives, so
worktrees keep sharing its entry (Opus r2 m1). An older entry misses once.

**impact's own veto (sol r1 minor).** impact's accounting vetoes malformed records by their raw
spelling (`accounting.rs:428,473`), digest by every key the path names. With absolute paths the
two can disagree (an absolute malformed record, a relative healthy one). impact's veto reads keys
through `path_keys` as digest's does: a malformed record's path is turned into every key it may
name, and a record is vetoed when its path's key is among them. impact keeps its additional
`old_path` veto (a rename whose old side was malformed, `accounting.rs:473–475`), now also by key;
digest's veto stays on the record's resolved key, as today. That difference is pre-existing and
not widened here.

**This also changes impact for relative spellings (Opus r2 N3).** Today a malformed `./src/x.al`
record does not veto a healthy `src/x.al` record in impact (raw spelling); after this change it
does, as digest has done since #76 T4. Under the git top a malformed record whose path reads two
ways vetoes both files, as in digest. Declared in §6 and the CHANGELOG; impact goldens may move,
and any move is triaged before regen.

## 6. Output

No new diagnostic kind, field or exit-code meaning; no schema version changes. A diagnostic's
`file` keeps the user's spelling.

| input | before | after |
|-------|--------|-------|
| `--file`/`--changed` `<ws-abs>/src/posting.al`, any root mode | `file-unmatched`, exit 2 | roots, exit 0 |
| the same through the workspace's real path (symlinked workspace) | `file-unmatched` | roots |
| diff header `<ws-abs>/src/posting.al` (digest, explain, tests, impact) | unmatched | resolves |
| the same in `impact --base`, both headers naming that one path | base side unmatched | resolves on both sides |
| `impact --base` whose original old path has no base reading (outside the workspace, e.g. `diff -u /abs/old /abs/ws`) | base side unmatched | unchanged (limit, §5) |
| impact: a malformed record and a healthy one naming the same file in two spellings (`./src/x.al`, `src/x.al`, or absolute) | the healthy one is accounted | both vetoed, as in digest |
| `Only in <ws-abs>/src: new.al` | `diff-file-unmatched` | whole-file root |
| `Only in <ws-abs>: src`, `Only in <parent>: Cloud` | `diff-file-not-al` | `diff-file-unmatched` (#353) |
| absolute path outside the workspace, or with `..` | unmatched | unchanged |

## 7. Tests (CLI in `tests/cli/cli_digest_changed_spans.rs`, a `// #363` section)

Each lists its break; every break is made with Edit, and its red and green runs recorded.

| # | test | expectation | break → result |
|---|------|-------------|----------------|
| A1 | absolute file, three root modes | `--file <ws-abs>/src/posting.al` → `assert_posting_file`, entries equal the mode's baseline: GitTop (`repo_ws`) and Workspace (no `.git`) against `--file src/posting.al`; Explicit (`--diff-root <t>/repo`) against `--file Cloud/src/posting.al` (S1). Also `--changed <ws-abs>/src/posting.al`, and the workspace passed relative (cwd `<t>`, `repo/Cloud`). | drop the anchor reading → `file_unmatched(<abs>)` |
| A2 | outside stays unmatched | a real copy `<t>/other/Cloud/src/posting.al` (the workspace's depth), `<ws-abs>/src/../src/posting.al`, and `<ws-abs>` itself → each `file_unmatched`, no roots, exit 2 | match on the path's tail, or on the anchor's depth without its folder names → the copy resolves |
| A3 | symlinked workspace (`#[cfg(unix)]`) | workspace `<t>/link/Cloud` (`link` → `repo`): the link spelling and the real spelling each resolve | drop the canonical anchor → the real spelling is unmatched |
| A4 | absolute diff headers | headers `<ws-abs>/src/posting.al` + `D8_POSTING_HUNK` → roots exactly `PostSalesDoc` | anchors applied to file inputs only → `diff-file-unmatched` |
| A5 | absolute `Only in` (S3) | `Only in <ws-abs>/src: <new>.al` (a workspace file) → its routines; `Only in <ws-abs>: src`, `Only in <t>/repo: Cloud`, `Only in <t>: repo` → `diff-file-unmatched`; `Only in <t>/elsewhere: x` → `diff-file-not-al` | drop the anchor branch in `holds_workspace_files` → `diff-file-not-al` for the workspace and its parent |
| A6 | digest veto with an absolute record (I3) | a malformed record on `<ws-abs>/src/posting.al` and a healthy one on `src/posting.al`, in both orders: no roots, the parse error. Precondition: the healthy one alone roots `PostSalesDoc`. | anchors not applied in `poisoned` → the healthy record's roots appear |
| A7 | batch, parent and worker | a ONE-batch manifest (runs in the parent) and a TWO-batch manifest (every batch in a worker) with `--file <ws-abs>/src/posting.al`, each equal to standalone, exit 0 (`batches_as_standalone`) | the worker drops the shipped anchors → the worker job is `file-unmatched` while standalone resolves |
| A8 | impact diff mode | `impact <ws> --diff <absolute headers>` accounts the hunk to `PostSalesDoc` | `account` ignores the anchors → the file is unmapped |
| A8b | impact `--base` (I1, sol r2, Opus r2 N2) | cache off; both headers name the same path. (i) A diff with absolute headers that DELETES a routine (base-only, so `--root-id` cannot supply it): the whole `delta` and the base side's roots equal the relative-header run's, and the deleted routine is reported. (ii) A diff with absolute headers whose old lines do not match the base: the delta is refused with the same preimage mismatch as the relative-header run. | the base run gets no `--path-anchor` → (i) the deleted routine and its delta are missing, (ii) no mismatch is reported |
| A8d | base cache key (sol r3, Opus r3 I-A) | warm cache (isolated folder), one diff text run twice, first with `--diff-root <ws>` (`rel` empty), then with the git-top default (`rel` = `Cloud`). As built: the diff changes a file both layouts map the same (so both warm runs have the same working-tree roots and the cache CAN answer the second) and deletes `Cloud/src/B.al` (a base-only routine, mapped only when `rel` = `Cloud`). Each run equals its own cold run: the second reports the deleted routine. (A fixture whose second run has a new root never reuses the entry — the cache answers only when it holds every root — so it could not fail under the break.) Anchor order cannot be varied through the CLI (always lexical, then canonical), so it is a unit test: the key differs when two anchors swap, when `rel` differs, and is equal for a relative-only diff from two checkout folders. | the key drops `rel` → the second warm run reuses the first's empty base mapping |
| A8c | impact veto (N3, sol r3) | (a) shared-key cases, the same as digest: A6's two records, and a malformed `./src/posting.al` plus a healthy `src/posting.al` → the healthy record is vetoed. (b) impact only: a rename record whose `old_path` names a malformed record's file in another spelling → vetoed in the accounting (digest has no `old_path` veto, so no parity is claimed; impact's roots come from digest's resolver, which keeps the rename's new file, as on master with a raw-spelled old path, so (b) asserts no root count). | raw-spelling veto → the healthy record is accounted in (a) and (b) |
| A9 | explain and tests diff modes (I2) | `explain --diff` and `tests --diff` with absolute headers name `PostSalesDoc` as their relative-header run does | the explain / tests call site builds `PathRoot` with no anchors → unmatched |
| A10 | engine route (I2, sol r2) | with `--deterministic`: the CLI run exits 0 with `PostSalesDoc` rooted (asserted on its own); `engine-ask -- digest <ws> --file <ws-abs>/src/posting.al` (decisions through `Decisions`) has the same exit and stdout; and the MCP `alsem_digest` with `files: [<ws-abs>/src/posting.al]` (`CliDecide`) answers the same document | each front end sends no `pathAnchors` → `file-unmatched` |
| A11 | #339's `file_input_boundaries_under_git_top` | drop the absolute spelling from its loop; keep `cloud/src/posting.al` (still `file-unmatched`) | (the old assert is the bug) |

Unit tests (`digest_cli.rs` and the wire module):
- `strip_anchor`: rooted match; a relative path never matches; empty rest is no match; `c:/ws/x.al`
  against `C:/ws` matches, `C:/WS/x.al` against `C:/ws` does not (only the disk folds); a root-only
  anchor `/` matches `/src/x.al` with rest `src/x.al`; `//server/share` root; `\` paths;
  `C:x.al` and `C:` never match (no root); root-only `/`, `C:/` and `//server/share` anchors
  round-trip their spelling through parse and print.
- `--path-anchor`: refused outside `impact --part main --diff`; when given, the decision's anchors
  are exactly its values, in order — asserted through `diff_root_of` (the decision the one-shot
  child makes), not only through the parser; all-dropped values give no anchors.
- `strip_diff_prefix("C:", "c:/x.al")` stays `None` (the relative compare is untouched, S4).
- First anchor wins (sol r1 #5): two anchors where both match with different rests → exactly one
  `path_keys` key, the first's.
- Anchor building: the `\\?\` strip, `\\?\UNC\` gives none, a `..` anchor is dropped, equal
  anchors give one.
- Wire: `pathAnchors` present and absent round-trip in `WorkerJob` and `Decisions`, and a
  legacy-shaped message (no field) decodes to no anchors.

## 8. Docs

CHANGELOG `### Fixed (#363 …)`, including impact's veto change for relative spellings (§5);
the test helper `batches_as_standalone`'s doc is corrected (with two or more batches every batch
runs in a worker; one batch runs in the parent); the #339 entry's limit sentence loses "absolute paths stay
`file-unmatched` (#363)". Help: digest's `--file`/`--changed`, and `--diff` / `--diff-root` of
digest, explain, tests and impact, gain "an absolute path below the workspace names that file";
impact's and tests' `--file` do not (they stay workspace-relative, §9). The MCP `alsem_digest`
`files` description says the same.

## 9. Limits (stated in CHANGELOG)

- An absolute path through a symlink that is neither the workspace's spelling nor its real path.
  A symlinked FILE inside the workspace resolves when its workspace-relative spelling is a
  discovered unit (sol r1 S5).
- `..` in an absolute path; a folder above the workspace in another case (apart from the disk
  letter); `\\?\UNC\` and `\\?\`-spelled inputs.
- impact's and tests' `--file` mode (and explain's routine mode, prove) read file inputs
  workspace-relative only, as today: they use the default root, so neither git-top-relative nor
  absolute file inputs resolve there (round-1 M6). Changing that is its own issue (file it).
- The in-process `run_digest_pipeline*` APIs (tests only) keep no anchors.

## 10. Questions for the owner

None. Q1 (every changed path), Q2 (the decision, §4) and Q3 (disk letter only, §3) are
engineering choices inside #363's scope.

## 11. Round-1 findings and where they went

sol (`scratchpad/pi-sol-tls06-r1.md`): 1 wire compatibility → §4 wire; 2 impact base → §5, A8b;
3 workspace `/` and `//server` → §2, unit tests; 4 parent vs worker → A7; 5 first-anchor guard →
unit test; minors: lexical anchor in every branch → §4, schema claim removed → §6, impact veto →
§5, A8c, in-process API → §9.
sol round 2 (`scratchpad/pi-sol-tls06-r2.md`): A8b's break → A8b (i)/(ii), cache off; base cache
key → §5 key tuple, A8d; forward the decided anchors → §5; disk grammar → §2, unit tests; worker
field rename and legacy decoding → §4; `old_path` veto → §5; A10 exits and MCP route → A10; help
vs limits → §8.
Round 3: sol APPROVE (`scratchpad/pi-sol-tls06-r3.md`; minors → §5 limit wording, A8c split,
A8d fixtures, override-vs-empty, rooted detection over error paths). Opus APPROVE with I-A → A8d,
§5 key reason; I-B → §5 per-route source; minors → A8c, §5 rooted grammar, `diff_root_of` unit
test.
Opus round 2: N1 → §5 limit, §6 rows (follow-up issue); N2 → A8b; N3 → §5, §6, A8c, CHANGELOG;
m1 → §5 key; m2 → §5 one builder; m3 → §5 scope, unit test; m4 → §4; m5 → unit tests; M8 doc → §8.
Opus: I1 → §5, A8b; I2 → §4 carrier list, A9, A10; I3 → A6; I4 → §4; M1 → §2; M2 → §2; M3 → §2,
§3; M4 → §3; M5 → A5; M6 → §9 (follow-up issue); M7 → §9; M8 → A7.
