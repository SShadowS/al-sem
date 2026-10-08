# S10.1 — The LSP keeps a line index of dependency files, not their text

Spec: `docs/superpowers/specs/2026-10-04-compact-graph-core-design.md` §8 item 5 (re-priced
2026-10-08: first). Baseline: `tools/census-probe/runs-s10-base/`.

## What the bytes and the code say

- Dependency source text is **108.3 MiB of CG root 1's 251.2 MiB** (shared tier, counted
  once); CDO embedded likewise dominates its 381 MiB.
- The LSP reads it in one place: `LspSnapshot::decl_and_line_table`'s dependency branch builds
  `LineTable::new(text)` so `handlers` can turn a dependency routine's `origin` /
  `name_origin` into an editor `Range`. Only `LineTable::col_out` is called on it. `col_in`
  is never called on a dependency file, and no request converts an arbitrary byte offset
  inside a dependency file (non-EventFlow incoming edges come only from `edges_by_file`,
  which holds workspace callers; EventFlow uses the publisher's `name_origin`).
- Nothing displays dependency text: `al-dep-source:///` URIs have no content handler,
  `dependencyDocumentSymbol` and `al-preview://` read the ABI.
- Two strong holders keep the text alive: `DepLspTier.dep_texts` and the retained
  `LspSnapshot.snap` (`apps[dep].source.files`, the same `Arc<str>` allocations). Both must
  go, or nothing is freed.

## Design

1. **`LineIndex`** (`src/lsp/encoding.rs`): a text-free twin of `LineTable` that answers
   `col_out` exactly as `LineTable::col_out` does, for both encodings, including the clamps
   (out-of-range line → empty line; `byte_col` past the end → line end; a `byte_col` inside a
   multi-byte character counts that character whole). It stores each line's byte length
   (`\r` stripped) and, only for characters outside ASCII, their position in the line, UTF-8
   length and UTF-16 length. AL source is nearly all ASCII, so this is mostly 4 bytes per
   line (measured afterwards: 11.69 MiB for CG's 9,919 dependency files, against 109.45 MiB
   of source roots).
   UTF-16 column = `b - Σ min(len8, b - start) + Σ len16` over the line's non-ASCII
   characters starting before `b = min(byte_col, line_len)`.
2. **Tier**: `DepLspTier.dep_texts: Arc<DepTexts>` becomes `dep_lines: Arc<DepLines>`
   (`HashMap<(AppRef, String), LineIndex>`), built once per shared tier from the fresh
   snapshot's texts, which are then dropped.
3. **Reader**: `decl_and_line_table` returns a `Cols` view (a small enum over `&LineTable`
   for workspace files and `&LineIndex` for dependency files) with one method, `col_out`.
   `origin_to_range`, `canonical_span_to_range` and `build_item` take `Cols`. (As built: a
   `ColOut` trait instead of an enum, so the helpers take `&dyn ColOut` and the workspace call
   sites are unchanged. No guard on dependency call-site spans is needed: a `LineIndex` covers
   every line of its file, not just the declaration points.)
   `decl_and_text`'s dependency branch (one test caller) is deleted; the test checks
   `decl_and_line_table` instead.
4. **Retained snapshot**: `LspSnapshot::from_context` builds the tier's line index (if the tier
   has none yet), then replaces every non-primary `SourceRoot.files` with an empty list before
   `Arc::new(snap)`. `source` stays `Some` with its tier and content hash, so "this app has
   source" keeps its meaning; the doc on `LspSnapshot::snap` says the LSP keeps no dependency
   text. Nothing in the LSP reads those files after the build (survey above).
5. **Known cost, measured, not hidden**: `DepCache::source` shares extracted text between
   roots only while some root holds it. After this change no root does, so each new root
   build and each rung-3 rebuild re-extracts the dependency text from the `.app` / JSON cache
   (a streamed blake3 of the `.app`, then a read), transiently. The `DepNodes` hit still skips
   all parsing. Task 5 measures root-2..7 build time and peak before and after; if the cost
   is material it becomes a follow-up (skip source extraction on a tier hit), not a reason to
   keep 108 MiB.

`FULL` is unaffected: it never builds an `LspSnapshot`. No program-graph fact is dropped —
the text is still on disk and in `DepCache`'s source path for any build that needs it.

## Tasks (one commit each; `scripts/ci-steps task` green; CDO count unchanged)

- **T1 `LineIndex`.** Type + `col_out`. Test: for every byte column `0..=len+2` of every line
  (`0..=lines+1`) of a text with CRLF, a trailing line without `\n`, an empty line, 2-, 3-
  and 4-byte characters (incl. a surrogate pair) and pure ASCII, `LineIndex::col_out ==
  LineTable::col_out` in both encodings. Discrimination: drop the mid-character rule
  (`min(len8, b - start)` → `len8`) and the test fails.
- **T2 Tier and reader.** `dep_lines` replaces `dep_texts`; `Cols`; handlers on `Cols`;
  `debug_assert!` in `incoming`; tests that pinned `Arc` identity of `dep_texts` with the
  snapshot text (dep_cache.rs 577-821, lsp_incremental_parity.rs 423-431, 1390-1531,
  1592-1717, 1810-1840) re-pointed to `dep_lines` (sharing across roots and rungs by
  `Arc::ptr_eq`, equality with a cache-less build). New test: for every dependency decl of an
  embedded-source fixture with non-ASCII names, the four `origin` / `name_origin` columns from
  `decl_and_line_table` equal `LineTable::new(text).col_out` on the original text, both
  encodings. Discrimination: build the index without the non-ASCII list and it fails.
- **T3 Strip the retained snapshot.** `from_context` empties dependency `files`. Test, stated
  directly: after building a root on an embedded fixture, a `Weak<str>` taken from a
  dependency `SourceFile.text` before the build cannot be upgraded once the build returns and
  the caller's snapshot copy is dropped (no strong holder left). Discrimination: skip the
  stripping and it upgrades. Re-point `updater.rs` `rung3_rebuild_stays_in_symbols_mode`
  to the source tier if it relies on `files`.
- **T4 Probe.** Update `tools/census-probe` for `dep_lines` (cargo check first).
- **T5 Measure and document.** Probe runs (`runs-s10-1/`, same matrix as the baseline); per-root
  build time and peak for roots 2-7 and a rung-3 rebuild, before and after; measurement-
  auditor over the numbers; CHANGELOG; spec §8 status line.

## Result (T5, 2026-10-08; `tools/census-probe/runs-s10-1/` vs `runs-s10-base/`)

Counted heap, MiB. Each mode ran twice per side (the `-base` and `-updaters` runs build the
same roots); counted figures repeat to ±0.1 MiB between them. Audited by the
measurement-auditor agent (2026-10-08); its corrections are folded in.

| Cell (file) | before | after | change |
|---|---:|---:|---:|
| CG `embedded`, root 1 retained (`cg-embedded-base`) | 251.2 | 152.2 | −99.0 |
| CG `embedded`, 7 roots live (`cg-embedded-updaters`) | 292.9 | 193.8 | −99.1 |
| CG `embedded`, 7 roots idle with updaters (`cg-embedded-updaters`) | 327.5 | 228.4 | −99.1 |
| CDO `embedded`, retained (`cdo-embedded-base`) | 381.4 | 275.7 | −105.7 |
| CDO `embedded`, idle with updater (`cdo-embedded-updaters`) | 393.7 | 287.9 | −105.8 |
| CG and CDO `symbols` (no dependency text: the control) | — | — | ±0.1 |
| CG root 1 build peak (`cg-embedded-base`) | 297.2 | 297.2 | 0 |
| CDO build peak (`cdo-embedded-base`) | 408.1 | 410.5 | +2.4 |
| CG roots 2-7 build peak, relative to the live heap before each (`cg-embedded-base`) | 24.8-25.2 | 134.2-134.6 | +109.4 |
| CG build time, roots 1-7 alike (0.1 s resolution) | 0.9-1.0 s (root 1: 2.8-3.0 s) | 1.2-1.3 s (root 1: 3.1-3.3 s) | +0.3 s |

Where the −99.0 comes from (drop deltas, measured, `cg-embedded-base`): the dependency source
roots freed 109.45 MiB (108.33 MiB of it text bytes), the old `dep_texts` map 1.29 MiB, and the
new `dep_lines` costs **11.69 MiB** (CDO 12.44 MiB): −109.45 − 1.29 + 11.69 = −99.05.

**The cost:** with no root holding the extracted text, `DepCache::source` (which keeps it only
weakly) cannot share it, so each later root extracts it again while it builds and drops it at
publish. The phase trace shows it: root 2's `1.snapshot` phase ends 0.0 MiB above its start
before and 109.5 MiB after, and stays there until `9.publish_snapshot` (`cg-embedded-base`
lines 21-30). A rung-3 rebuild does the same by the code; no run measures it. The +0.3 s is
indicative only: root 1, which re-extracts nothing, slowed by 0.3 s too, and identical CDO
builds swung by 0.9 s between runs.

**Process heap peak** (by arithmetic, never measured: the probe reports relative build peaks
and settled live heaps): live heap before root 7 plus its build peak, 285.6 + 25.2 ≈ **311 MiB
before** and 186.6 + 134.6 ≈ **321 MiB after** (+10 MiB, root 7 both times). In the server's
shape (updaters running) the before run already held 327.5 MiB live while idle, so the process
peak did not rise there; it fell by at least 6 MiB, assuming updater start-up adds less than
about 93 MiB of transient heap (not measured). RSS context (one run, not a result): peak
working set 435.4 → 508.4 MiB in the `cg-embedded-updaters` runs.

Removing the re-extraction needs the build to skip source extraction when the dependency tier
is already live with its line index (S10.1b, owner's call).

## Held throughout

- Every LSP position is byte-identical: the handler tests, `lsp_incremental_parity`, and T2's
  four-points test.
- CDO workspace 0 unknown / 23 ambiguous, dependency bodies 0 / 459.
- No golden moves (the LSP surface owns none).
