//! Content-addressed source cache for embedded `.app` extraction.
//!
//! The cache key is the blake3 hex of the whole `.app` image, so a changed
//! app is always a different key — stale reads are structurally impossible.
//! The entries are the one mapping of package-derived bytes allowed: sealed
//! private files, written once and renamed into place, never modified.
//!
//! ENG-113 (design note `docs/superpowers/notes/2026-11-18-eng113-mapped-dependency-text.md`):
//! one file per package holds a header, an index and every file's decoded text, and a hit
//! hands out [`SourceText`]s that are ranges of a read-only mapping of that file, so the page
//! cache holds a package's text once for every engine process. A miss writes the file and then
//! reads it back the same way, so package text has one form when the cache is writable.

use crate::app_image::AppImage;
use crate::snapshot::embedded::{SourceFile, extract_embedded_source};
use crate::source_text::SourceText;
use anyhow::{Context, Result, bail};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Return the stable cache directory, creating it if necessary.
///
/// Uses the OS cache directory (`dirs::cache_dir`) when available, with
/// `al-ch-snapshot-cache` appended.  Falls back to
/// `<temp_dir>/al-ch-snapshot-cache` when the OS cache dir is unavailable.
pub fn cache_dir() -> PathBuf {
    let base = dirs::cache_dir().unwrap_or_else(std::env::temp_dir);
    let dir = base.join("al-ch-snapshot-cache");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!("Failed to create snapshot cache dir {}: {e}", dir.display());
    }
    dir
}

/// The version of what extraction produces for a package, in the cache file's
/// name. Bump it when [`extract_embedded_source`]'s output for an unchanged
/// `.app` changes (2: AL synthesized from transpiled C#; its own changes too;
/// 3: synthesized routines carry their `[MethodId(n)]`; 4: the id is quoted,
/// since a negative one made the file `Recovered`; 5: generic types split no
/// local, and a call keeps its receiver's member path; 6: a property-backed
/// global `δx` is `x`; 7: an `if` on never-written Boolean parameters and
/// locals is written out, G51).
pub const EXTRACTION_SCHEME: u32 = 7;

/// The cache file's layout, in its name and header. 1 (ENG-113): header, JSON index, text.
/// Before it, a package's cache was one JSON document (`<hash>.s<scheme>.json`), which the new
/// name never matches, so an old cache misses cleanly.
pub const CACHE_FORMAT: u32 = 1;

const MAGIC: &[u8; 8] = b"ALSEMSRC";
/// Magic, format, scheme, index length, text length, the text's blake3.
const HEADER_LEN: usize = 8 + 4 + 4 + 8 + 8 + 32;

/// One file's place in the text, and its path.
#[derive(serde::Serialize, serde::Deserialize)]
struct IndexEntry {
    path: String,
    offset: u64,
    len: u64,
}

/// Return embedded AL source for the package `image`, using the
/// content-addressed on-disk cache when available.
///
/// On a miss the source is extracted from the image, written to
/// `<cache_dir>/<blake3-hex>.s<EXTRACTION_SCHEME>.f<CACHE_FORMAT>.bin` and read back from it;
/// on a hit it is read from that file. Either way the texts are ranges of a mapping of the
/// file.
///
/// Returns `(files, content_hash)`.  `files` is empty for symbol-only apps
/// (same semantics as [`extract_embedded_source`]).  The hash is the image's
/// blake3 hex ([`AppImage::content_hash`]): the key and the extraction come
/// from the SAME bytes, so a package rewritten after its hash was taken can
/// never be extracted under the old key (context-server design r5 §5.2).
///
/// Cache I/O failures (unwritable dir, locked file, corrupt entry) are
/// **non-fatal**: a warn is logged and the extraction is used as owned text.
pub fn cached_source(image: &Arc<AppImage>) -> Result<(Vec<SourceFile>, String)> {
    cached_source_in(&cache_dir(), image)
}

/// [`cached_source`] with the cache directory given (tests write a temporary one).
pub fn cached_source_in(dir: &Path, image: &Arc<AppImage>) -> Result<(Vec<SourceFile>, String)> {
    let hash = image.content_hash().to_string();
    // The extraction scheme and the cache format are part of the name, not the hash (the hash
    // stays the app's identity): a package with only transpiled C# extracts to synthesized AL
    // since scheme 2, so an older entry must not be read for it.
    let cache_file = dir.join(format!("{hash}.s{EXTRACTION_SCHEME}.f{CACHE_FORMAT}.bin"));

    // Cache hit; fall through to re-extract on a refused entry.
    if cache_file.exists() {
        match read_cache(&cache_file) {
            Ok(files) => return Ok((files, hash)),
            Err(e) => log::warn!(
                "snapshot cache entry {} refused ({e:#}); re-extracting",
                cache_file.display()
            ),
        }
    }

    // Cache miss (or a refused entry): extract, write, then read the written file, so the
    // text is the mapped form.
    let files = extract_embedded_source(image)?;
    let written = write_cache(dir, &cache_file, &hash, &files);
    if let Err(e) = &written {
        log::warn!(
            "snapshot source cache write failed for {} ({e:#})",
            image.path().display()
        );
    }
    // Read back even when this write failed: another process may have just written the same
    // entry (on Windows a rename over a mapped file fails).
    if cache_file.exists() {
        match read_cache(&cache_file) {
            Ok(mapped) => return Ok((mapped, hash)),
            Err(e) => log::warn!(
                "snapshot cache entry {} refused after writing ({e:#})",
                cache_file.display()
            ),
        }
    }
    log::warn!(
        "snapshot source for {} kept as owned text (the cache could not be used)",
        image.path().display()
    );
    Ok((files, hash))
}

/// Read a cache entry: map it, check the header, then the text's length and blake3, then each
/// index entry's range and UTF-8 (`SourceText::mapped`). Any failure refuses the entry.
///
/// The blake3 check is NOT optional: nothing fsyncs, so after a crash a renamed file can have
/// the right length and zero content, which the length and UTF-8 checks both accept.
fn read_cache(cache_file: &Path) -> Result<Vec<SourceFile>> {
    let file = std::fs::File::open(cache_file)
        .with_context(|| format!("open cache {}", cache_file.display()))?;
    // SAFETY: entries are only ever replaced whole by a rename of a unique temp file
    // (`write_cache`), never written or truncated in place, so the mapped bytes never change.
    // An outside in-place copy over the cache (cp, rsync --inplace, a restore) breaks this;
    // the design note states the risk and its blast radius (the engine process).
    let map = unsafe { crate::capped_io::map_read_only(&file) }
        .with_context(|| format!("map cache {}", cache_file.display()))?
        .with_context(|| format!("cache {} is empty", cache_file.display()))?;
    let map = Arc::new(map);
    if map.len() < HEADER_LEN || &map[..8] != MAGIC {
        bail!("not a cache file (header)");
    }
    let u32_at = |at: usize| u32::from_le_bytes(map[at..at + 4].try_into().expect("4 bytes"));
    let u64_at = |at: usize| u64::from_le_bytes(map[at..at + 8].try_into().expect("8 bytes"));
    if u32_at(8) != CACHE_FORMAT || u32_at(12) != EXTRACTION_SCHEME {
        bail!("cache format or extraction scheme differs");
    }
    let index_len = usize::try_from(u64_at(16)).context("index length")?;
    let text_len = usize::try_from(u64_at(24)).context("text length")?;
    let hash: [u8; 32] = map[32..64].try_into().expect("32 bytes");
    let text_start = HEADER_LEN
        .checked_add(index_len)
        .context("index length overflows")?;
    if text_start.checked_add(text_len) != Some(map.len()) {
        bail!("cache length is not header + index + text");
    }
    if blake3::hash(&map[text_start..]).as_bytes() != &hash {
        bail!("cache text hash differs");
    }
    let index: Vec<IndexEntry> =
        serde_json::from_slice(&map[HEADER_LEN..text_start]).context("cache index")?;
    index
        .into_iter()
        .map(|e| {
            let start = text_start
                .checked_add(usize::try_from(e.offset).context("offset")?)
                .context("offset overflows")?;
            let end = start
                .checked_add(usize::try_from(e.len).context("length")?)
                .context("length overflows")?;
            let text = SourceText::mapped(&map, start..end)
                .with_context(|| format!("cache text of {}", e.path))?;
            Ok(SourceFile {
                virtual_path: e.path,
                text,
            })
        })
        .collect()
}

/// Write `files` as one cache entry: a unique temp file in `dir`, renamed into place. Two
/// writers never share a temp file, so a rename only ever installs a complete file.
fn write_cache(dir: &Path, cache_file: &Path, hash: &str, files: &[SourceFile]) -> Result<()> {
    let mut index = Vec::with_capacity(files.len());
    let mut text_len = 0u64;
    let mut hasher = blake3::Hasher::new();
    for f in files {
        let len = f.text.len() as u64;
        index.push(IndexEntry {
            path: f.virtual_path.clone(),
            offset: text_len,
            len,
        });
        hasher.update(f.text.as_bytes());
        text_len += len;
    }
    let index = serde_json::to_vec(&index).context("cache index")?;
    let mut tmp = tempfile::Builder::new()
        .prefix(&format!("{hash}-"))
        .suffix(".tmp")
        .tempfile_in(dir)
        .with_context(|| format!("create cache temp in {}", dir.display()))?;
    {
        let mut out = std::io::BufWriter::new(tmp.as_file_mut());
        out.write_all(MAGIC)?;
        out.write_all(&CACHE_FORMAT.to_le_bytes())?;
        out.write_all(&EXTRACTION_SCHEME.to_le_bytes())?;
        out.write_all(&(index.len() as u64).to_le_bytes())?;
        out.write_all(&text_len.to_le_bytes())?;
        out.write_all(hasher.finalize().as_bytes())?;
        out.write_all(&index)?;
        for f in files {
            out.write_all(f.text.as_bytes())?;
        }
        out.flush()?;
    }
    tmp.persist(cache_file)
        .map_err(|e| e.error)
        .with_context(|| format!("rename cache temp → {}", cache_file.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::deps::app_package_zip::test_apps::build_app;

    const A: &str = "codeunit 50100 \"Ålborg\" { }\n";
    const B: &str = "table 50101 Bee { }\n";

    fn image(entries: &[(&str, &[u8])]) -> Arc<AppImage> {
        AppImage::from_bytes("pkg.app", build_app(entries))
    }

    fn two_files() -> Arc<AppImage> {
        image(&[("src/A.al", A.as_bytes()), ("src/B.al", B.as_bytes())])
    }

    fn entry(dir: &Path, img: &AppImage) -> PathBuf {
        dir.join(format!(
            "{}.s{EXTRACTION_SCHEME}.f{CACHE_FORMAT}.bin",
            img.content_hash()
        ))
    }

    fn texts(files: &[SourceFile]) -> Vec<(&str, &str)> {
        let mut v: Vec<_> = files
            .iter()
            .map(|f| (f.virtual_path.as_str(), &*f.text))
            .collect();
        v.sort_unstable();
        v
    }

    /// A miss extracts, writes one entry and hands out mapped texts read from it; a second
    /// call reads the entry (one map shared by every file), with the same texts.
    #[test]
    fn a_miss_writes_and_a_hit_maps() {
        let dir = tempfile::tempdir().unwrap();
        let img = two_files();
        let (cold, hash) = cached_source_in(dir.path(), &img).unwrap();
        assert_eq!(hash, img.content_hash());
        assert_eq!(texts(&cold), [("src/A.al", A), ("src/B.al", B)]);
        assert!(cold.iter().all(|f| f.text.is_mapped()), "a miss maps too");
        assert!(entry(dir.path(), &img).is_file());

        let (warm, _) = cached_source_in(dir.path(), &img).unwrap();
        assert_eq!(texts(&warm), texts(&cold));
        let map = warm[0].text.mapping().expect("mapped");
        assert!(
            warm.iter()
                .all(|f| f.text.mapping().is_some_and(|m| Arc::ptr_eq(m, map))),
            "one map per package"
        );
    }

    /// An entry whose text changed in place with its length and UTF-8 kept (one letter
    /// flipped) is refused by the hash alone, re-extracted and rewritten.
    #[test]
    fn a_broken_entry_is_refused_and_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let img = two_files();
        cached_source_in(dir.path(), &img).unwrap();
        let path = entry(dir.path(), &img);
        let mut bytes = std::fs::read(&path).unwrap();
        let at = bytes.len() - B.len() + "table 50101 ".len();
        assert_eq!(
            bytes[at], b'B',
            "precondition: the flipped byte is B's name"
        );
        bytes[at] = b'X';
        std::fs::write(&path, &bytes).unwrap();

        let (files, _) = cached_source_in(dir.path(), &img).unwrap();
        assert_eq!(texts(&files), [("src/A.al", A), ("src/B.al", B)]);
        assert!(files.iter().all(|f| f.text.is_mapped()));
        assert_ne!(
            std::fs::read(&path).unwrap(),
            bytes,
            "the entry was rewritten"
        );
    }

    /// The pre-ENG-113 JSON entry of the same package is never read (its name differs).
    #[test]
    fn an_old_json_entry_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let img = two_files();
        let old = dir
            .path()
            .join(format!("{}.s{EXTRACTION_SCHEME}.json", img.content_hash()));
        std::fs::write(&old, r#"[{"virtual_path":"src/Old.al","text":"old"}]"#).unwrap();
        let (files, _) = cached_source_in(dir.path(), &img).unwrap();
        assert_eq!(texts(&files), [("src/A.al", A), ("src/B.al", B)]);
    }

    /// A package with no source is a valid entry: written once, then a hit with no files.
    #[test]
    fn a_package_with_no_source_is_a_hit() {
        let dir = tempfile::tempdir().unwrap();
        let img = image(&[("SymbolReference.json", b"{}")]);
        let (files, _) = cached_source_in(dir.path(), &img).unwrap();
        assert!(files.is_empty());
        let path = entry(dir.path(), &img);
        assert!(
            read_cache(&path).is_ok_and(|f| f.is_empty()),
            "a valid empty entry"
        );
        let (files, _) = cached_source_in(dir.path(), &img).unwrap();
        assert!(files.is_empty());
    }

    /// A cache that cannot be written (the directory is a file) keeps the extraction as
    /// owned text.
    #[test]
    fn an_unwritable_cache_keeps_owned_text() {
        let dir = tempfile::tempdir().unwrap();
        let not_a_dir = dir.path().join("file");
        std::fs::write(&not_a_dir, "").unwrap();
        let (files, _) = cached_source_in(&not_a_dir, &two_files()).unwrap();
        assert_eq!(texts(&files), [("src/A.al", A), ("src/B.al", B)]);
        assert!(files.iter().all(|f| !f.text.is_mapped()));
    }
}
