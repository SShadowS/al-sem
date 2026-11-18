# ENG-113: memory-mapped dependency source text (design note, 2026-11-18, rev. 2)

Rev. 2 folds Fable's design review (11 findings; the temp-file collision, the mandatory hash, the
pointer fast paths and the empty case were the ones that changed the design).

## Goal and bar

Every engine process holds each package's AL source text on its own heap: the census bucket
`bodies: source text + paths + provenance` is 120.37 MiB per process on CDO (ENG-107). The gate runs
several engine processes at once. Mapping the text from a read-only file lets the page cache hold
it once for all of them.

**Bar** (orchestrator, 2026-11-18): (a) the census bucket falls by at least 100 MiB, with the mapped
bytes shown on a separate census line and any `Owned` fallback still counted as heap (so a silent
fallback fails the bar instead of passing it by definition); (b) on a WARM cache, two concurrent
engine processes on CDO show the blob as shared file-backed pages: its own `/proc/<pid>/smaps` entry
with Shared_Clean about the blob's size, Pss about half of it, Private_Clean about 0. RSS still counts
mapped file pages, so cdo-speed's peak-RSS ratio may not move, and one engine alone saves nothing in
RSS; the CHANGELOG says so.

## Today

- Package text is decompressed from each `.app`'s zip (`snapshot::embedded::extract_embedded_source`)
  and decoded once (`source_text::decode_al_source`: BOM stripped, UTF-8 lossy).
- `snapshot::cache::cached_source` keys a package by its image's blake3 and caches the extraction as
  ONE JSON document per package (`<cache_dir>/<blake3>.s7.json`). A hit maps the file only while it
  parses the JSON, copying every text into a fresh heap `Arc<str>`.
- Entries are written to `<hash>-<pid>.json.tmp` and renamed into place. That temp name is NOT
  unique per writer (two threads of one process, or two containers with separate pid numbering
  sharing the directory, open the same file with a truncating create); today a torn write only
  fails the JSON parse.
- The text is `Arc<str>` in `SourceFile` and `ParsedFile`, cloned (one handle each) into the session,
  the row cache and the resident guard cache. Workspace files and source-folder dependencies use the
  same `SourceFile`, read from disk.

## Design

1. **One cache file per package** (`<blake3>.s<SCHEME>.f<FORMAT>.bin`, the extraction scheme and the
   cache layout both in the name), written once and renamed into place: a small header (magic, format,
   scheme, index length, text length, blake3 of the text), the index `[(path, offset, len)]` as JSON,
   then every file's decoded UTF-8 text concatenated. One file and one rename: a reader never pairs an
   index with another write's text.
2. **Unique temp files**: `tempfile::NamedTempFile::new_in(cache_dir)` + `persist` (the `tempfile`
   crate is already a dependency). The writer never maps its own temp file; it maps the renamed file.
3. **`SourceText`** (new, `src/source_text.rs`): `Clone`, `Deref<Target = str>`, `Debug`/`Display` as
   the text, `From<&str>`, `From<String>`, `From<Arc<str>>`; two forms, `Owned(Arc<str>)` and
   `Mapped { map: Arc<Mmap>, range }`, and a `ptr_eq` that is true for the same `Arc<str>` or the same
   map and range (the row cache's `Arc::ptr_eq || ==` fast path, `program/model/row_cache.rs`, keeps
   its O(1) answer). It replaces `Arc<str>` as the text of `SourceFile` and `ParsedFile` and in the
   holders that clone it. The identity probes that read the text's `Arc` (the census, `dep_cache`'s
   `same_allocations` and its S10.1 guard `the_lsp_keeps_no_dependency_text`) move to the new type;
   the S10.1 guard gets a probe for the mapped form and a discrimination proof.
4. **One path for package text when the cache is writable.** On a hit: map the file, check the header,
   then in this order the text's length, its blake3 (MANDATORY: nothing fsyncs, so after a crash a
   renamed file can have the right length and zero content; length and UTF-8 checks both pass on
   that), then UTF-8, then each index entry's range and character boundaries; only then hand out
   `Mapped` slices. On a miss: extract, write the file, then take the hit path. If the write fails,
   try the hit path once more (another process may have just written it; on Windows a rename over a
   mapped file fails) before falling back to `Owned`, logged at warn and counted as heap.
5. **No source** (a symbol-only package): a valid entry with zero files and empty text, a hit as
   today (no re-extraction, no re-synthesis). `map_read_only` answers `None` for a zero-length file,
   so the header is never empty.
6. **Serde**: `SourceFile` leaves the JSON cache; it is serialized nowhere else.
7. **Test seam**: `cached_source_in(dir, image)`, so the failure tests write a temporary directory, not
   the user's cache.

## Failure cases

- **A cache file truncated or rewritten in place under a live map** raises SIGBUS (or changes bytes
  under a validated `str`). Our code never does it: entries are only replaced whole by a rename
  (restated in a SAFETY comment where the map is made), and the temp files are unique (design 2).
  The map now lives as long as a resident engine, so an OUTSIDE in-place copy of the cache directory
  (`cp` over it, `rsync --inplace`, a backup restore, seeding a container's cache) or a cache on a
  network file system can still do it. That cannot be prevented cheaply; the blast radius is the
  engine child, which the front end restarts. Unlinking (a cleaner, a temp-dir cleaner when the cache
  falls back to the temp dir) is safe on Linux.
- **A hash, length, UTF-8 or index mismatch**: refuse the entry, re-extract, rewrite. Tested with a
  hand-written broken file in a temporary cache directory.
- **An old cache** (`.s7.json`): the new name does not match, so it misses cleanly. Tested. The old
  files stay on disk (no pruner exists): the cache roughly doubles per package once; the CHANGELOG
  says so.
- **"Package text is Mapped" holds only when the cache is writable**; every consumer accepts both
  forms (source-folder dependencies and workspace files are `Owned`).

## Measures

Identity: goldens, `cdo-before-after`, refresh verify in both modes, `cdo-gate --only
harness,l4,output`. Memory: the census pair (the bucket, the new mapped line); the blob's own
`/proc/<pid>/smaps` entry and `smaps_rollup` Pss of two concurrent `alsem engine-host` processes on a
warm cache, before and after; cold-miss peak RSS (owned and mapped text coexist for a moment, about
+114 MiB). Speed: cdo-speed twice (the hash and UTF-8 passes add one read of ~114 MB per load; a
callers-context answer, which re-read every package's JSON before, should get faster), and a refresh
timing (the row cache's fast path). Major faults under memory pressure (mapped pages can be evicted
and read back).
