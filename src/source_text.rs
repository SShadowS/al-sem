//! Workspace `.al` source: which files, and how their bytes become text.
//!
//! Every reader of AL source uses this: the snapshot provider (program engine
//! and LSP), embedded `.app` source, L2/L3, inline suppressions and the LSP's
//! own re-reads. The program engine and L3 must see the same files with the
//! same text, because the L3 adapter joins call sites across the two engines
//! on byte spans. A file only one engine sees is a silent hole: a call into it
//! from a shared file gets the program engine's answer, which has no target.
//!
//! The decode is lossy. A file saved as Windows-1252 (common in code that came
//! from NAV through txt2al) must still be analyzed, not fail the run. An
//! invalid byte becomes U+FFFD. A leading UTF-8 BOM is dropped, as editors do,
//! so line-0 columns match what the editor shows.
//!
//! The file walk ([`discover_al_files`]) follows the AL compiler, which takes
//! every `*.al` under the project folder through the operating system: the
//! extension is matched without case (Windows file names are case-blind),
//! symbolic links and junctions are followed, and the dependency/output
//! folders [`SKIP_DIRS`] are skipped at any case.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Folder names never walked for workspace source, at any depth below the
/// root, compared without ASCII case.
pub const SKIP_DIRS: [&str; 3] = [".alpackages", ".snapshots", "node_modules"];

/// True for a folder name in [`SKIP_DIRS`].
pub fn is_skipped_dir_name(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    SKIP_DIRS.iter().any(|d| name.eq_ignore_ascii_case(d))
}

/// True when `path` ends in `.al`, in any case.
pub fn has_al_extension(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("al"))
}

/// What the walk does at a folder (below the root) that holds its own
/// `app.json`: a separate AL project.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NestedApps {
    /// Walk into it (the program engine's snapshot and L2's projection).
    Walk,
    /// Stop there (L3's app-scoped discovery). This difference from the
    /// program engine is deliberate and pinned by
    /// `program_calls::tests::nested_app_json_folder_is_program_only`.
    Skip,
}

/// One workspace `.al` file.
#[derive(Clone, Debug)]
pub struct AlFile {
    /// Root-relative, `/`-separated, case kept (the `ws:` unit id suffix and
    /// the snapshot's `virtual_path`).
    pub rel_posix: String,
    pub abs_path: PathBuf,
}

/// A path as a lossless key (TLS-30: the resident manifest's keys). A path
/// whose text is UTF-8 is its text, so every such key is what it was. Any
/// other path is a NUL followed by the hex of its raw bytes (Unix bytes;
/// Windows UTF-16 units, little-endian). No file name holds a NUL, so the two
/// forms never meet, and two names that differ only in non-UTF-8 bytes
/// (`b"\xff.al"`, `b"\xfe.al"`, both `\u{fffd}.al` lossily) get two keys.
#[must_use]
pub fn path_key(p: &Path) -> String {
    p.to_str()
        .map_or_else(|| raw_key(&os_bytes(p.as_os_str())), str::to_string)
}

/// [`path_key`] of a walk's relative path: `rel_posix` (lossy,
/// `/`-separated) when the file's last components are UTF-8, else the raw
/// bytes of those components joined by `/`. A lossy spelling keeps the
/// component count (no lost byte becomes a `/`), so the last
/// `rel_posix.split('/').count()` components of `abs` are the relative path.
#[must_use]
pub fn rel_key(rel_posix: &str, abs: &Path) -> String {
    let parts: Vec<&OsStr> = abs.components().map(|c| c.as_os_str()).collect();
    let tail = &parts[parts.len().saturating_sub(rel_posix.split('/').count())..];
    if tail.iter().all(|c| c.to_str().is_some()) {
        return rel_posix.to_string();
    }
    // Joined as one OS string, so the separator is encoded like the names
    // (one byte on Unix, one UTF-16 unit on Windows: review r1, sol and Opus).
    let mut joined = std::ffi::OsString::new();
    for (i, c) in tail.iter().enumerate() {
        if i > 0 {
            joined.push("/");
        }
        joined.push(c);
    }
    raw_key(&os_bytes(&joined))
}

/// A [`path_key`] / [`rel_key`] back as a path, for messages and path tests:
/// the text as is, or the raw bytes again.
#[must_use]
pub fn key_path(key: &str) -> PathBuf {
    let Some(hex) = key.strip_prefix('\u{0}') else {
        return PathBuf::from(key);
    };
    let raw: Vec<u8> = (0..hex.len() / 2)
        .filter_map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok())
        .collect();
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        PathBuf::from(std::ffi::OsString::from_vec(raw))
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        let wide: Vec<u16> = raw
            .chunks(2)
            .map(|b| u16::from_le_bytes([b[0], *b.get(1).unwrap_or(&0)]))
            .collect();
        PathBuf::from(std::ffi::OsString::from_wide(&wide))
    }
}

fn raw_key(raw: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(1 + 2 * raw.len());
    s.push('\u{0}');
    for b in raw {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn os_bytes(s: &OsStr) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        s.as_bytes().to_vec()
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        s.encode_wide().flat_map(u16::to_le_bytes).collect()
    }
}

/// Every workspace `.al` file under `root`, sorted by `rel_posix`.
///
/// A walk error (an unreadable folder, a link loop) fails the walk: a
/// partial file list would be analyzed as if it were the whole app. A
/// dangling link is skipped: there is no content behind it to lose.
pub fn discover_al_files(root: &Path, nested: NestedApps) -> std::io::Result<Vec<AlFile>> {
    let walker = walkdir::WalkDir::new(root)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| {
            e.depth() == 0
                || !e.file_type().is_dir()
                || !(is_skipped_dir_name(e.file_name())
                    || (nested == NestedApps::Skip && dir_has_app_json(e.path())))
        });
    let mut files = Vec::new();
    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(e)
                if e.io_error().map(std::io::Error::kind) == Some(std::io::ErrorKind::NotFound) =>
            {
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        if !entry.file_type().is_file() || !has_al_extension(entry.path()) {
            continue;
        }
        let rel = entry.path().strip_prefix(root).unwrap_or(entry.path());
        let rel_posix = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        files.push(AlFile {
            rel_posix,
            abs_path: entry.into_path(),
        });
    }
    files.sort_by(|a, b| a.rel_posix.cmp(&b.rel_posix));
    Ok(files)
}

/// True when `dir` directly holds an `app.json` file (any case).
pub(crate) fn dir_has_app_json(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|entries| {
        entries.flatten().any(|e| {
            e.file_name().eq_ignore_ascii_case("app.json")
                && e.file_type().is_ok_and(|t| t.is_file())
        })
    })
}

/// Decode `.al` source bytes: drop a leading UTF-8 BOM, then UTF-8 (lossy).
pub fn decode_al_source(bytes: &[u8]) -> String {
    let body = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8_lossy(body).into_owned()
}

/// Read a `.al` file from disk through [`decode_al_source`].
pub fn read_al_source(path: &Path) -> std::io::Result<String> {
    Ok(decode_al_source(&std::fs::read(path)?))
}

/// Parse `app.json` text, ignoring a leading UTF-8 BOM. The AL compiler
/// accepts one and VS Code on Windows often writes one; `serde_json` rejects
/// it, which used to empty the whole snapshot without saying why.
pub fn parse_app_json(text: &str) -> serde_json::Result<serde_json::Value> {
    serde_json::from_str(text.strip_prefix('\u{feff}').unwrap_or(text))
}

/// Read the workspace ROOT's `app.json` `id` field VERBATIM when it is a
/// non-empty string. Mirrors `providers/workspace.ts` (GAP 2). (Moved here from
/// `program::body::l2_workspace` in engine-switch S1.)
pub fn read_root_app_guid(workspace: &Path) -> Option<String> {
    let text = std::fs::read_to_string(workspace.join("app.json")).ok()?;
    let value = parse_app_json(&text).ok()?;
    let id = value.get("id")?.as_str()?;
    if id.is_empty() {
        None
    } else {
        Some(id.to_string())
    }
}

/// Collect the absolute paths of every `app.json` anywhere under `workspace`,
/// excluding the shared skip folders ([`SKIP_DIRS`], any case). Used by the gate's
/// `workspace_diagnostics` to reproduce the provider's multi-app fail-closed
/// message (which sorts these paths). (Moved here from
/// `program::body::l2_workspace` in engine-switch S1.)
pub fn count_app_json_paths(workspace: &Path) -> Vec<std::path::PathBuf> {
    let mut paths: Vec<std::path::PathBuf> = Vec::new();
    let mut stack = vec![workspace.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(ftype) = entry.file_type() else {
                continue;
            };
            if ftype.is_dir() {
                if is_skipped_dir_name(&entry.file_name()) {
                    continue;
                }
                stack.push(entry.path());
            } else if ftype.is_file()
                && entry.file_name().to_string_lossy().to_lowercase() == "app.json"
            {
                paths.push(entry.path());
            }
        }
    }
    paths
}

/// Why [`SourceText::mapped`] refused a range.
#[derive(Debug)]
pub enum MappedTextError {
    /// The range ends past the mapping (or starts after its end).
    OutOfRange,
    /// The bytes are not UTF-8.
    NotUtf8(std::str::Utf8Error),
}

impl std::fmt::Display for MappedTextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MappedTextError::OutOfRange => f.write_str("text range outside the mapped file"),
            MappedTextError::NotUtf8(e) => write!(f, "mapped text is not UTF-8: {e}"),
        }
    }
}

impl std::error::Error for MappedTextError {}

/// ENG-113: an AL source text, read as a `str`: owned (workspace files, source-folder
/// dependencies, a package whose cache could not be written) or a range of a read-only
/// mapping of a package's cache file (`snapshot::cache`), which the page cache shares across
/// engine processes. Cloning is a handle copy either way.
#[derive(Clone)]
pub struct SourceText(Repr);

#[derive(Clone)]
enum Repr {
    Owned(std::sync::Arc<str>),
    /// `map[range]`, checked to be UTF-8 on a character boundary at both ends when made.
    Mapped {
        map: std::sync::Arc<memmap2::Mmap>,
        range: std::ops::Range<usize>,
    },
}

impl SourceText {
    /// `map[range]` as text.
    ///
    /// # Errors
    /// The range is outside the map, or its bytes are not UTF-8 (which also refuses a range
    /// that splits a character).
    pub fn mapped(
        map: &std::sync::Arc<memmap2::Mmap>,
        range: std::ops::Range<usize>,
    ) -> Result<SourceText, MappedTextError> {
        let bytes = map.get(range.clone()).ok_or(MappedTextError::OutOfRange)?;
        std::str::from_utf8(bytes).map_err(MappedTextError::NotUtf8)?;
        Ok(SourceText(Repr::Mapped {
            map: std::sync::Arc::clone(map),
            range,
        }))
    }

    /// The same text allocation (one `Arc<str>`, or one map and range): equal without reading
    /// the bytes. `false` says nothing (two copies of one text).
    #[must_use]
    pub fn ptr_eq(a: &SourceText, b: &SourceText) -> bool {
        match (&a.0, &b.0) {
            (Repr::Owned(x), Repr::Owned(y)) => std::sync::Arc::ptr_eq(x, y),
            (Repr::Mapped { map: m, range: r }, Repr::Mapped { map: n, range: s }) => {
                std::sync::Arc::ptr_eq(m, n) && r == s
            }
            _ => false,
        }
    }

    /// Whether the text is a range of a mapped file (for the census and tests).
    #[must_use]
    pub fn is_mapped(&self) -> bool {
        matches!(self.0, Repr::Mapped { .. })
    }

    /// The owned text's allocation, when owned (the census counts it as heap; a mapped text
    /// is no heap of this process).
    #[must_use]
    pub fn owned_arc(&self) -> Option<&std::sync::Arc<str>> {
        match &self.0 {
            Repr::Owned(a) => Some(a),
            Repr::Mapped { .. } => None,
        }
    }

    /// The mapping a mapped text reads (tests probe that it outlives nothing it should not).
    #[must_use]
    pub fn mapping(&self) -> Option<&std::sync::Arc<memmap2::Mmap>> {
        match &self.0 {
            Repr::Owned(_) => None,
            Repr::Mapped { map, .. } => Some(map),
        }
    }
}

impl std::ops::Deref for SourceText {
    type Target = str;
    fn deref(&self) -> &str {
        match &self.0 {
            Repr::Owned(a) => a,
            Repr::Mapped { map, range } => {
                // SAFETY: `mapped` checked these bytes are UTF-8 when the text was made, and
                // the mapping is read-only and never changes (snapshot::cache: entries are
                // replaced whole by a rename, never written in place).
                unsafe { std::str::from_utf8_unchecked(&map[range.clone()]) }
            }
        }
    }
}

impl AsRef<str> for SourceText {
    fn as_ref(&self) -> &str {
        self
    }
}

impl std::borrow::Borrow<str> for SourceText {
    fn borrow(&self) -> &str {
        self
    }
}

impl std::fmt::Debug for SourceText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&**self, f)
    }
}

impl std::fmt::Display for SourceText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&**self, f)
    }
}

impl PartialEq for SourceText {
    fn eq(&self, other: &SourceText) -> bool {
        SourceText::ptr_eq(self, other) || **self == **other
    }
}

impl Eq for SourceText {}

impl PartialEq<str> for SourceText {
    fn eq(&self, other: &str) -> bool {
        &**self == other
    }
}

impl From<std::sync::Arc<str>> for SourceText {
    fn from(a: std::sync::Arc<str>) -> SourceText {
        SourceText(Repr::Owned(a))
    }
}

impl From<String> for SourceText {
    fn from(s: String) -> SourceText {
        SourceText(Repr::Owned(s.into()))
    }
}

impl From<&str> for SourceText {
    fn from(s: &str) -> SourceText {
        SourceText(Repr::Owned(s.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::decode_al_source;

    #[test]
    fn strips_bom_and_replaces_invalid_bytes() {
        // BOM + Windows-1252 "Kære" (0xE6 is not valid UTF-8 on its own).
        assert_eq!(decode_al_source(b"\xEF\xBB\xBFK\xE6re"), "K\u{FFFD}re");
        // Only a LEADING BOM is dropped; plain UTF-8 passes through unchanged.
        assert_eq!(decode_al_source("a\u{FEFF}æ".as_bytes()), "a\u{FEFF}æ");
        assert_eq!(decode_al_source(b""), "");
    }

    /// TLS-30: a UTF-8 path's key is its text (keys did not move); a
    /// non-UTF-8 one is lossless and comes back as the same path; two names
    /// that read alike lossily get two keys.
    #[cfg(unix)]
    #[test]
    fn path_keys_are_lossless() {
        use super::{key_path, path_key, rel_key};
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        use std::path::Path;
        let utf8 = Path::new("/w/ws/src/A \u{fffd}.al");
        assert_eq!(path_key(utf8), utf8.to_str().unwrap());
        assert_eq!(rel_key("src/A \u{fffd}.al", utf8), "src/A \u{fffd}.al");
        let ff = Path::new(OsStr::from_bytes(b"/w/ws/src/\xff.al"));
        let fe = Path::new(OsStr::from_bytes(b"/w/ws/src/\xfe.al"));
        assert_ne!(path_key(ff), path_key(fe));
        assert_eq!(key_path(&path_key(ff)), ff);
        let (rf, re) = (
            rel_key("src/\u{fffd}.al", ff),
            rel_key("src/\u{fffd}.al", fe),
        );
        assert_ne!(rf, re);
        assert_eq!(key_path(&rf), Path::new(OsStr::from_bytes(b"src/\xff.al")));
    }

    fn map_of(bytes: &[u8]) -> std::sync::Arc<memmap2::Mmap> {
        let mut f = tempfile::tempfile().unwrap();
        std::io::Write::write_all(&mut f, bytes).unwrap();
        // SAFETY: a private anonymous temp file nothing else writes.
        std::sync::Arc::new(unsafe { memmap2::Mmap::map(&f) }.unwrap())
    }

    /// `mapped` refuses a range past the map and one that splits a character (its `Deref`
    /// reads the bytes unchecked, so every text it makes must be whole UTF-8).
    #[test]
    fn mapped_text_is_whole_utf8_inside_the_map() {
        use super::{MappedTextError, SourceText};
        let map = map_of("aÅb".as_bytes()); // `Å` is bytes 1..3
        assert_eq!(&*SourceText::mapped(&map, 0..4).unwrap(), "aÅb");
        assert_eq!(&*SourceText::mapped(&map, 1..3).unwrap(), "Å");
        assert!(matches!(
            SourceText::mapped(&map, 0..2),
            Err(MappedTextError::NotUtf8(_))
        ));
        assert!(matches!(
            SourceText::mapped(&map, 2..4),
            Err(MappedTextError::NotUtf8(_))
        ));
        assert!(matches!(
            SourceText::mapped(&map, 3..5),
            Err(MappedTextError::OutOfRange)
        ));
    }

    /// `ptr_eq` is the same map and range, or the same `Arc<str>`; `==` is the text, across
    /// forms.
    #[test]
    fn source_text_identity_and_equality() {
        use super::SourceText;
        let map = map_of(b"abab");
        let first = SourceText::mapped(&map, 0..2).unwrap();
        let second = SourceText::mapped(&map, 2..4).unwrap();
        assert!(SourceText::ptr_eq(&first, &first.clone()));
        assert!(
            !SourceText::ptr_eq(&first, &second),
            "same map, other range"
        );
        assert_eq!(first, second, "equal text");
        let other_map = SourceText::mapped(&map_of(b"abab"), 0..2).unwrap();
        assert!(
            !SourceText::ptr_eq(&first, &other_map),
            "same range, another map"
        );
        assert_eq!(first, other_map, "equal text on another map");
        let owned = SourceText::from("ab");
        assert!(SourceText::ptr_eq(&owned, &owned.clone()));
        assert!(!SourceText::ptr_eq(&owned, &SourceText::from("ab")));
        assert_eq!(owned, first, "equal text across forms");
        assert_ne!(owned, SourceText::from("ba"));
    }
}
