# TLS-86: CPU profiles of `impact --diff`, `prove`, `explain`, and the ENG-98 / ENG-100 profiles

Recorded on server1 (22 cores) with samply 0.13.1 (`scripts/profile`, the `profiling` build:
release-fast's code with symbols and line tables). The workspace is the pinned CDO baseline
(`/work/src/cdo-baseline/Cloud`, workspace git `bc3ccb18`). "CPU" is every thread's CPU, summed
from samply's per-sample deltas.

The raw profiles are not in git; they go stale with every engine change. They are on the
persistent volume:

- `/work/.al-sem/profiles/2026-11-16-tls86/`: the three new profiles (`tls86-{impact,prove,explain}.json.gz`
  with `.syms.json`), the diff (`tls86-range.diff`) and the run script (`tls86-profile.sh`).
- `/work/.al-sem/profiles/2026-11-16-eng98/` and `.../2026-11-16-eng100/`: the older profiles.
- `/work/.al-sem/profiles/`: copies of the two readers; the repo's are the source.

To regenerate a profile, build at the named commit and run the command through `scripts/profile -o <out>
alsem ...`. Two readers in `tools/census-probe/runs-eng84/` read it:
- `samply-shares.py <out> <out>.syms.json [needle ...]`: self and inclusive shares.
- `samply-chains.py <out> <out>.syms.json <needle> <depth>`: the frames above a leaf. Each chain
  list below names the needle and depth it came from.

**Reading the shares.**
- **Self shares are reliable.**
- **Inclusive shares stop at rayon task boundaries:** a stage that runs on the pool reads low by
  its name. For example, `resolve_full_program` shows 0.4 % in prove.
- **Inline labels on a non-leaf frame can belong to the code next to the call.** samply names
  each frame's inlined functions from the return address, and that address can sit in the inlined
  code of a neighbouring call. In the impact profile, `digest_one_root`'s own call-site frame
  carries the labels of a `HashMap<String, String>::get` and `eq<str, str>`. It sits above 26.5 %
  of all CPU, which makes a needle like `equal_same_length` read 27 %. The real callees below that
  frame are `reconstruct_witness_paths` (15.3 %) and `compute_ordering` (9.2 %). A memcmp leaf
  directly below an `equal_same_length` frame is only 0.11 %. Check a chain that is unlikely.
- **Two libc functions have no symbols.** Identified by their code (`objdump` of the container's
  libc):
  - `fun_1972e0` is the AVX2 `memcmp`/`bcmp`: two pointers and a length, `vpcmpeqb` on both.
  - `fun_197a40` is the AVX2 `memcpy`/`memmove`: `vmovdqu` load and store pairs. Its callers
    agree: `RawVec` growth through `realloc`, `write_str`, `copy_nonoverlapping`.
- **Chain depth:** `samply-chains.py` adds both a short and a qualified name for many frames, so
  depth 4 is about two real frames.

## 1. `alsem impact --diff` (TLS-86)

- **Binary:** master `9c1c099b`.
- **Command:**
  ```
  git -C /work/src/cdo-baseline diff --no-color --no-ext-diff --diff-algorithm=histogram \
    --src-prefix=a/ --dst-prefix=b/ bc3ccb18~5 bc3ccb18 -- Cloud > range.diff
  alsem impact /work/src/cdo-baseline/Cloud --diff - --diff-root /work/src/cdo-baseline \
    --format json < range.diff
  ```
- **Result:** 699 roots, the gate's diff impact. 536.1 s CPU at the one-shot's default 8
  workers. Wall time 337 s, which includes the profiling build.

| self | function |
|---:|---|
| 11.26 % | `fun_1972e0` (libc memcmp) |
| 10.26 % | SipHash (`write<Sip13Rounds>`) |
| 3.98 % | `impact_graph::reverse::walk_multi` |
| 2.94 % | `fold<Vec<QueryWitnessHop>>` (in `ordering_inter::reconstruct_call_chains`) |
| 2.83 % | `reach` |
| 2.12 % | `fun_197a40` (libc memcpy) |
| 2.10 % | `mi_page_malloc_zero` |
| 1.86 % | `is_background_target` |
| 1.83 % | `mi_page_free_list_extend` |
| 1.73 % | `step` |

| inclusive | stage |
|---:|---|
| 26.59 % | `digest::digest_one_root` |
| 23.70 % | `digest_cli::literal_reach_entries` |
| 21.53 % | `walk_multi`. In shares of all CPU: 15.72 % under `impact::up::up`'s closure #3, 5.77 % under closure #9 |
| 15.25 % | `digest::reconstruct_witness_paths` |
| 9.17 % | `ordering_engine::compute_ordering` |
| 5.86 % | `ordering_inter::reconstruct_call_chains` |
| 4.87 % | `al_syntax::lower::lower_file` |

**memcmp's callers** (`samply-chains.py ... fun_1972e0 5`), in shares of all CPU:
- `ordering_inter::reconstruct_call_chains` 2.73 %;
- `ordering_engine::compute_ordering` 1.34 %;
- `LiteralReach::first_live_owner` 0.89 %;
- `fact_equivalent`'s `rep_row` 0.39 %;
- `seed_reach` 0.30 %.

**SipHash's callers** (`samply-chains.py ... Sip13Rounds 4`, the frame above `hash_one`):
- `TrieWalk::descend` 1.22 %, `insert` 1.14 %, `prepare_resize` 0.74 %;
- `LiteralReach::bind_m` 0.71 %, `WitDag::marking` 0.53 %, `dead_at_m` 0.50 %;
- `lflow::Exec::exec` 0.45 %, `reserve_rehash` 0.41 %, `seed_reach` 0.41 %;
- `LiveTree::states_of` 0.24 %, `first_live_owner` 0.21 %.

## 2. `alsem prove` and `alsem explain` (TLS-86)

- **Binary:** master `9c1c099b`.
- **Commands:**
  - `alsem prove /work/src/cdo-baseline/Cloud "CDO E-Mail Handler::RunReportID" may-commit --format json`
  - `alsem explain /work/src/cdo-baseline/Cloud --routine "CDO E-Mail Handler::RunReportID" --format json`
- **Result:** prove took 81.4 s CPU (36 s wall), explain 76.3 s CPU (34 s wall). Both are
  one-shot runs, so most of their time is the model build.

| self | prove | explain |
|---|---:|---:|
| `fun_1972e0` (libc memcmp) | 7.77 % | 7.42 % |
| SipHash | 4.96 % | 4.59 % |
| `ts_tree_cursor_goto_sibling_internal` | 3.72 % | 3.70 % |
| `fun_197a40` (libc memcpy) | 3.34 % | 3.16 % |
| mimalloc free-list set-up (`mi_page_free_list_extend`; explain's inline label `mi_block_set_nextx`) | 3.14 % | 3.18 % |
| `ts_tree_cursor_goto_first_child_internal` | 2.77 % | 2.99 % |
| `mi_page_malloc_zero` | 2.09 % | 2.15 % |
| `_madvise` | 1.58 % | 1.38 % |

| inclusive | prove | explain |
|---|---:|---:|
| `ts_parser_parse` | 18.24 % | 19.19 % |
| `lower_file` | 15.49 % | 16.21 % |
| `pd_solve` | 8.03 % | 8.17 % |
| `recref_flow` | 7.48 % | 8.19 % |
| `capability_cone` | 6.88 % | 5.58 % |
| `digest_one_root` | 4.59 % | 4.85 % |

**prove's chains**, in shares of all CPU:
- **memcmp** (`fun_1972e0 4`): `fact_equivalent`'s `rep_row` 1.13 %, `pd_solve::carry_reqs`
  0.76 %, `capability_cone::merge_cone` 0.72 %, `ResInterner::intern`'s map 0.47 %.
- **memcpy** (`fun_197a40 6`): `RawVec` growth through `mi_theap_realloc_zero` 0.42 + 0.23 +
  0.08 %, `format`'s `write_str` 0.22 %, `fold_identifier` 0.17 %, `copy_from_slice` 0.08 %.
- **SipHash** (`Sip13Rounds 4`): `make_hash` 1.03 %, `prepare_resize` 0.43 %,
  `RecRefFlow::eval_routine` 0.39 %, `pd_solve`'s closure #9 0.33 %, `reserve_rehash` 0.32 %,
  `RecRefFlow::val_of_text` 0.25 %, `insert` 0.24 %.
- **Tree cursor** (`ts_tree_cursor_goto_sibling_internal 4`, 3.72 % in all), the frame above
  `ts_tree_cursor_goto_next_sibling`:
  - `RawNode::descendants_of_kinds` 0.92 %. Its one caller is `lower::collect_comments`, a
    whole-tree scan for comments.
  - `named_ranges_where` 0.51 %.
  - two `from_iter` 0.39 % and 0.24 % (child lists of the lowering walk).
  - `lower::collect_run_objects` 0.27 %.
  - `lower::lower_routine` 0.27 %.

## 3. The ENG-98 and ENG-100 profiles

These are kept so the next change has the "before" (ENG-98 and ENG-100 have landed; the
numbers below are historic). Workspace `bc3ccb18`.

- **ENG-98 `alsem analyze --format json /work/src/cdo-baseline/Cloud`**
  - Binary: master `52d349ab`, before ENG-98. Raw file: `eng98-analyze.json.gz`. 72.5 s CPU.
  - Self: memcmp 6.33 %, `ts_tree_cursor_goto_sibling_internal` 4.22 %, SipHash 3.78 %,
    `ts_tree_cursor_goto_first_child_internal` 3.52 %, `mi_page_free_list_extend` 3.42 %,
    memcpy 3.10 %, `ts_lex` 2.54 %, `ts_lexer__advance` 2.24 %, `mi_page_malloc_zero` 2.19 %,
    `ts_parser_parse` 2.04 %, `fact_pd_index` 2.01 %.
  - ENG-98 moved its hottest SipHash maps (`pd_solve`'s, `ResInterner::by_str`) to FxHash.
- **ENG-98 `alsem fingerprint /work/src/cdo-baseline/Cloud`**
  - Before: binary `52d349ab`, raw file `eng98-fp.json.gz`.
  - After: binary `6d12dc5e`, the ENG-98 change, raw file `eng98-fp2.json.gz`.
  - Their numbers: SipHash 20.7 % -> 2.82 % (the CHANGELOG's ENG-98 entry says 2.84 %), CPU
    256.5 -> 241.6 s.
- **ENG-100 `alsem fingerprint /work/src/cdo-baseline/Cloud`**
  - Raw file: `e100-fp.json.gz`. 99.3 s CPU.
  - Binary: the ENG-100 change before its rebase onto ENG-99. The profile started at 09:45 UTC.
    `47cf6aed`, the commit as it is now, is the rebased one (committed at 11:40, with ENG-99
    under it), so it is not this binary. The base is unknown, so this profile cannot be rebuilt
    by SHA.
  - Self: memcpy 8.07 %, `mi_page_free_list_extend` 7.87 %, SipHash 5.84 %,
    `reconstruct_witness_paths` 5.19 %, `get` 4.54 %, memcmp 4.16 %, `quicksort<Candidate>` 3.40 %,
    `ts_tree_cursor_goto_sibling_internal` 3.01 %, `_madvise` 2.89 %, `mi_page_malloc_zero` 2.58 %,
    `hash_one<&*const SnapshotGraphEdge>` 2.43 %.
  - ENG-105 then took the edge-pointer hashing, and ENG-106 the per-root slot-table churn.

## Hot spots of 3 % or more, filed

The new profiles give these (the ENG-98/100 ones are already handled by ENG-98, ENG-100,
ENG-105 and ENG-106):

| task | hot spot | where |
|---|---|---|
| ENG-116 | memcmp: 11.3 % self in impact, 7.8 % in prove, 7.4 % in explain, 6.3 % in analyze | impact: `reconstruct_call_chains` (5.9 % inclusive; with its `QueryWitnessHop` fold, 2.9 % self) and `compute_ordering`; prove: `rep_row`, `carry_reqs`, `merge_cone`, `ResInterner::intern` |
| ENG-117 | SipHash: 10.3 % in impact, 5.0 % in prove | `literal_reach` and `digest`'s walk maps, `recref_flow`; Fx only where no order reaches output |
| ENG-118 | `walk_multi`: 4.0 % self, 21.5 % inclusive (impact) | `impact::up::up`'s closures #3 and #9 |
| ENG-119 | tree-cursor stepping: 6.5 % self (prove), 6.7 % (explain), 7.7 % (analyze) | the scans beside the lowering walk (`collect_comments`, `named_ranges_where`, `collect_run_objects`) are about 1.7 of prove's 3.7 % sibling share; the rest is the lowering walk itself |
| ENG-120 | allocator page set-up and zeroing: 6.8 % (prove) | `mi_page_free_list_extend`, `mi_page_malloc_zero`, `madvise`; `purge_delay` is 0 on purpose (peak RSS), so measure, do not assume |
| ENG-121 | memcpy: 3.3 % (prove), 3.2 % (explain), 3.1 % (analyze) | `RawVec` growth through `realloc` (copies on grow), `format`'s `write_str`, `fold_identifier` |

The large inclusive stages in impact (`reconstruct_witness_paths` 15.3 %, `compute_ordering`
9.2 %) are not filed on their own: their self costs are the hot spots above.
