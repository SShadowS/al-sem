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
4. **Guard:** `parse_for_build` debug-asserts it never parses a dependency whose text was
   deferred.

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
