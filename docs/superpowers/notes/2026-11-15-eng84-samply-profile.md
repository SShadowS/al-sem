# ENG-84: where B1, B2 and B7 sit in a samply profile of the CDO model build

Recorded 2026-11-15 on server1 (master `9059eda0`), the pinned CDO baseline, under `scripts/cdo-slot`.
The first profile of this build that samples every thread: `samply record --save-only
--unstable-presymbolicate` around one resident model build (a scratch probe running
`WorkspaceInputs::read` → `WorkspaceSession` → `build_target`, the engine child's path; `release`
with line tables, mimalloc). Build wall 15.0 s, **62.6 s of thread CPU** (samples weighted by each
thread's CPU delta). ENG-82's pprof profile kept only a quarter of its samples, mostly the main
thread's; this one does not have that bias.

**Read the self shares; distrust the inclusive ones.** The binaries have no frame pointers, so
samply's stack walk is unreliable above the leaf: inclusive totals came out implausible
(`unit_id`/`format` 20.6%, `lower_object`/`child_by_field_name` 24.0%, identical pairs). Self
shares (the leaf frame) are reliable. Identical-code folding also gives merged functions one
arbitrary name (see the binary search below).

## The three ENG-84 parts

| Part | Self | Inclusive (shaky) | Verdict |
|---|---:|---:|---|
| B1 dependency closure (`DependencyGraph::closure`) | small | 0.9% | drop (as ENG-82 said) |
| B2 identifier folding (`fold_identifier` 1.53%, `eq_fold_identifier` 1.65%) | 3.2% | 5.0% / 2.3% | worth it: a fold that allocates only when it changes the text |
| B7 recorder (`reads::Recorder`) | small | 2.6% | small; not worth a change on its own |

## Other leaf hot spots (self)

- tree-sitter: cursor walks `ts_tree_cursor_goto_sibling_internal` 4.4%,
  `goto_first_child_internal` 3.9% (the lowerer's child iteration, ENG-83's ground), lexing
  `ts_lex` 3.0% + `ts_lexer__advance` 2.6%, `ts_parser_parse` 2.3%, `ts_subtree_release` 1.9%.
- **Routine lookups by id:** `binary_search_by<RoutineNode, …>` 1.75% + 1.47% + 0.82% ≈ **4%**.
  The names say `interface_holder::source_codeunits` and `partition_point`, but that function does
  one search per holder source: the linker folded every identical binary search over
  `graph.routines` into one symbol, so this is the cost of all lookups by `RoutineNodeId` (string
  comparisons) through the sorted list, from many callers.
- **SipHash still 2.3%** (`write<Sip13Rounds>`): some maps on this path still use the default
  hasher after TLS-66.
- mimalloc `mi_page_malloc_zero` 1.9%, `mi_free` 1.7%; `core::str::from_utf8` 1.3%.

## What this suggests for ENG-84

B2 (~3% self) plus two items the task did not name: the routine-by-id lookups (~4%) and the
remaining SipHash maps (~2%). Together roughly 6-9% of the build's CPU; B1 and B7 are dropped.
Measure the result with `cdo-speed` (CPU ratio), not this profile.

## Second profile, with frame pointers (the inclusive shares are real)

The same probe rebuilt with `RUSTFLAGS='-C force-frame-pointers=yes'` (own target dir), recorded
the same way: build wall 19.3 s, **67.3 s of thread CPU**. Inclusive shares from
`tools/census-probe/runs-eng84/samply-shares.py` (each needle: the largest inclusive share of any
name containing it):

| Needle | Inclusive |
|---|---:|
| `lower_file` | 29.60% |
| `ts_parser_parse` | 22.86% |
| `resolve_recorded` | 19.10% |
| `fold_identifier` (`eq_fold_identifier` alone: 1.74%) | 2.66% |
| `Recorder` (B7) | 2.55% |
| `DependencyGraph::closure` (B1) | 1.00% |

The bare needle `closure` matches rayon's worker closures (78.85%) and says nothing about B1; B1
was measured by its full name. Self shares on this profile: `fold_identifier` 1.58%,
`eq_fold_identifier` 1.72%, `write<Sip13Rounds>` 2.39%, the folded `binary_search_by<RoutineNode>`
1.74% + 0.73%.

**Decision (confirms the first profile):** B1 dropped (1.0%, a one-off per build). B7 dropped
(2.55% spread over every recorded read; its cost is cloning query keys, with no hot call site to
fix). B2 kept: fold without allocating when the text is already folded, in the hot callers. The
routine and object lookups by id now go through `NodeSet::find` (a hash index from id to
position, built on first use; equal ids and hash collisions fall back to the binary search, so
every answer is the binary search's).
