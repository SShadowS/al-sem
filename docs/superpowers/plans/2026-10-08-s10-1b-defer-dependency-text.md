# S10.1b — Do not extract dependency text a build will not read

Follows S10.1 (`2026-10-08-s10-1-dependency-line-index.md`), whose measured cost this removes:
once no root keeps dependency text, every later root (and every rung-3 rebuild) re-extracts it
while it builds — CG roots 2-7 peak 109 MiB higher — and then drops it unread.

## Why the text is extracted today, and why it need not be

1. `SnapshotBuilder::build_with_options` asks each `EmbeddedAppProvider` for the dependency's
   source; `DepCache::source` shares a live extraction or calls `cached_source` (a streamed
   blake3 of the `.app`, about 105 MB for BaseApp, plus the JSON cache read).
2. `build_context_from_snapshot_cached` computes `DepKey::of(&snap)` — it needs, per app, only
   whether the app ships source (`has_source`) — and asks the cache for the tier.
3. On a hit, `parse_for_build` skips every dependency; if the tier's LSP part (`lsp`, the line
   indexes) is already published, `from_context` reads no dependency text either.

So on a hit with a published LSP part, the text is extracted only for `DepKey` to learn
"this app ships source", which the cache already learned the first time.

## Design

1. **Descriptors outlive the text.** `DepCache::sources` already stores, per `(path, stamp)`,
   the tier and content hash next to a `Weak` to the file list. Keep that entry after the text
   dies (today a later miss purges dead entries); replace only an entry for the same path with
   an older stamp, so the map stays one entry per `.app` path. Also remember a `.app` that
   ships NO source (`load` returned `None`), which today is re-hashed by every build.
2. **A deferred source.** `DepCache::source` takes `defer_text`. With it, a known `.app` whose
   text is dead yields `SourceRoot { files: empty, tier, content_hash }`; a known source-less
   `.app` yields `None` without loading (that part regardless of `defer_text`). No real
   provider produces `Some` with no files, so an empty list on a dependency means "text not
   loaded".
3. **Two-phase LSP build** (`build_context_with`): build the snapshot with `defer_text`; if
   any dependency source is deferred, take the tier for its key and continue only if the tier
   is live AND its `lsp` part is published — holding that `Arc` for the rest of the build so it
   cannot die in between. Otherwise rebuild the snapshot with text and continue as today.
4. **Guard:** `parse_for_build` asserts (a plain `assert!`: a wrong tier would be shared by
   every root) it never parses a dependency whose text was deferred.

Only the LSP path defers (`build_context_with`, LIGHT); every other snapshot build is
unchanged.

## Tasks

- **T1 descriptors and no-source memory** in `DepCache::source`; `defer_text`;
  `SnapshotBuilder::build_with_options_deferring`; `EmbeddedAppProvider::defer_text`. Tests
  (stated directly, each with a discrimination proof): a source-less `.app` is hashed once
  across two builds; a deferred build of a known `.app` with dead text extracts nothing.
- **T2 two-phase build + guard.** Tests: (a) a second root on a live tier with its LSP part
  published extracts no dependency text and answers exactly like a cache-less build; (b) after
  the tier died (descriptor known, text dead), a root still answers exactly like a cache-less
  build (the fallback rebuild). Discrimination: skip the fallback and (b) fails; never defer
  and (a)'s extraction count rises.
- **T3 measure** (probe, same matrix; roots 2-7 build peak and time), audit, CHANGELOG.

T1 and T2 landed as one commit (`f0d3cadf`). Beyond the plan, `build_dep_lines` (the only other
reader of dependency text) asserts the same as `parse_for_build`. Discrimination (each break
made with Edit, run, reverted; observed in the session's test logs, not saved in the repo):
never deferring fails the live-tier test (2 extractions, not 1); not remembering source-less
apps fails its test (2, not 1); ignoring the stamp fails the stamp test (`"old"`, not `"new"`);
skipping the fallback rebuild fails the dead-tier test three ways independently (the parse
assertion; with it disabled, the extraction count 1 ≠ 2; with that relaxed too, the answers
differ from a cache-less build).

## Result (T3, 2026-10-08; `tools/census-probe/runs-s10-1b/` vs `runs-s10-1/`)

Counted heap, MiB; same binary build matrix as S10.1 (CG 7 roots, CDO; both modes; with and
without updaters).

| Cell (file) | S10.1 | S10.1b |
|---|---:|---:|
| CG roots 2-7 build peak, relative to the live heap before each (`cg-embedded-base`) | 134.2-134.6 | 24.8-25.1 |
| same, `cg-embedded-updaters` | 134.1-134.6 | 24.7-25.2 |
| CG roots 2-7 `1.snapshot` end-live (`cg-embedded-base`) | 109.5 | 0.0 |
| CG root 1 build peak (`cg-embedded-base`) | 297.2 | 297.2 |
| CG 7 roots live / idle with updaters (`cg-embedded-updaters`) | 193.8 / 228.4 | 193.9 / 228.4 |
| CDO retained / idle with updater (`cdo-embedded-*`) | 275.7 / 287.9 | 275.7 / 288.0 |
| `symbols` mode, every build-peak / retained / live / idle cell (the control: no text to defer) | — | ±0.1 |

(`symbols` phase-level figures swing more, e.g. root 1's `1.snapshot` in-phase peak 41.0 ↔ 43.7;
`runs-s10-base/` shows the same noise.) CDO retained / live / idle cells are within ±0.2
(`cdo-embedded-updaters` "ALL 1 ROOTS LIVE" 279.2 → 279.4).

The pre-S10.1 base (`runs-s10-base/`) had roots 2-7 at 24.8-25.2, so the re-extraction cost is
gone entirely: the 109.4 MiB drop equals the 109.5 MiB the snapshot phase used to hold until
publish. Process heap peak, by arithmetic as in S10.1, assuming roots build one after another
(as the probe does) and a live heap of about 0 before root 1: root 7 is now 186.6 + 25.1 ≈ 212,
so root 1's 297.2 is the peak (S10.1: about 321; pre-S10.1: about 311). RSS peak working set
(one run, context only): 508.4 → 430.5 MiB (`cg-embedded-updaters`). Build times are not a
result: `cg-embedded-base` roots 2-7 went 1.2-1.3 → 1.0-1.2 s, inside the run-to-run swing
(root 1, identical work, 3.3 vs 3.8 s). CDO is a single root, so it has nothing to defer; its
cells match. The descriptors the cache now keeps are one small entry per `.app` path; not
measured separately (CG all-roots-live moved at most 0.1 MiB).

Not measured: a rung-3 rebuild takes the same path by the code (`apply_rung3` →
`build_full_with_parsed_with_cache` → `build_context_with`), but no probe run or test counts
its extractions. And a root whose tier died now builds its snapshot twice (once without text,
once with it); the probe's roots stay live, so it never exercises that fallback.
