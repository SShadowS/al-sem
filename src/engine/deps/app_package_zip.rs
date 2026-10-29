//! Rust port of al-sem's `src/symbols/app-package-zip.ts` +
//! `src/symbols/symbol-reference-reader.ts` (entry pick + BOM strip).
//!
//! A BC `.app` file is a ZIP archive that may carry a binary header before the
//! ZIP local-file signature `PK\x03\x04`. We strip that header (scanning at most
//! the first 4096 bytes — the TS `Math.min(len-4, 4096)` bound), then select the
//! `SymbolReference.json` / `NavxManifest.xml` entry by normalizing entry names
//! (`\` → `/`), lowercasing, and matching `ends_with(...)`. The FIRST matching
//! entry in archive iteration order wins — mirroring TS `Object.keys(entries)[0]`
//! over `fflate`'s entry map.
//!
//! Never panics: a malformed archive / missing entry yields `None`, matching the
//! TS "never throws" posture.
//!
//! This is also the ONE place that knows how to open a `.app`, including a
//! Ready-to-Run package (see [`read_ready_to_run_app`]). Every reader routes
//! through [`app_zip_bytes`] (bytes in hand) or [`open_app_file`] (a path).

use anyhow::Context as _;
use std::borrow::Cow;
use std::io::{BufReader, Cursor, Read, Seek, SeekFrom};
use std::path::Path;

/// `.app` files start with a 40-byte NAVX header, then a standard zip.
pub const NAVX_HEADER_SIZE: u64 = 40;

/// The bytes at offset 36 of a runtime `.app`: the header's closing `NAVX`
/// followed by `.NEA` where an ordinary package's zip (`PK\x03\x04`) starts.
/// The mark's trailing `00 00 00 01` matches every community-documented sample
/// (hougaard.com, D365BCAppHelper).
const RUNTIME_MARK: &[u8; 8] = b"NAVX.NEA";

/// The fixed RC4 key a runtime package's payload is scrambled with — public and
/// community-documented for years (hougaard.com's runtime-app writeups,
/// `SimonOfHH/D365BCAppHelper`, `businessdev.ALbuild`'s `Expand-BcAppFile`).
/// Obfuscation, not secrecy: the same key on every package, the same key at
/// every BC version measured.
const RUNTIME_RC4_KEY: [u8; 6] = [15, 11, 81, 137, 184, 120];

/// A runtime (`.NEA`) `.app` package whose payload could not be descrambled
/// (truncated file, stream past the cap, not a zip): it holds an app, but
/// nothing of it can be read, so its id, name and version are unknown.
///
/// A well-formed runtime package — one the key opens — never produces this
/// error: its manifest, symbols and source read through the same public
/// readers as a plain package (see [`open_app_file`]). The error exists so a
/// future format or key change from the publisher surfaces by name, not as
/// the app silently vanishing.
#[derive(Debug)]
pub struct EncryptedApp {
    pub path: std::path::PathBuf,
}

impl std::fmt::Display for EncryptedApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "encrypted .app package, its manifest, symbols and source cannot be read: {}",
            self.path.display()
        )
    }
}

impl std::error::Error for EncryptedApp {}

/// `head` (a `.app`'s first bytes) is a runtime package's header: a normal
/// NAVX header whose payload is an RC4-scrambled zip instead of the zip
/// itself. `false` for a plain package and for anything unreadable.
pub fn is_encrypted_app_header(head: &[u8]) -> bool {
    head.starts_with(b"NAVX") && head.get(36..44) == Some(&RUNTIME_MARK[..])
}

/// The `.app` at `path` is an encrypted package (`false` when it cannot be read).
fn is_encrypted_app_file(path: &Path) -> bool {
    let mut head = [0u8; 44];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok_and(|()| is_encrypted_app_header(&head))
}

/// Descramble a runtime package's whole RC4 stream (see [`RUNTIME_RC4_KEY`])
/// into a `Vec`. RC4 is symmetric, so this is also the scramble direction.
fn rc4_descramble_stream(stream: &[u8]) -> Vec<u8> {
    let mut s: [u8; 256] = core::array::from_fn(|i| i as u8);
    let mut j = 0u8;
    for i in 0..256 {
        j = j
            .wrapping_add(s[i])
            .wrapping_add(RUNTIME_RC4_KEY[i % RUNTIME_RC4_KEY.len()]);
        s.swap(i, j as usize);
    }
    let mut out = Vec::with_capacity(stream.len());
    let (mut i, mut j) = (0u8, 0u8);
    for &b in stream {
        i = i.wrapping_add(1);
        j = j.wrapping_add(s[i as usize]);
        s.swap(i as usize, j as usize);
        let k = usize::from(s[i as usize]) + usize::from(s[j as usize]);
        out.push(b ^ s[k & 0xff]);
    }
    out
}

/// The RC4 stream length declared by the runtime header in `head`
/// (`None` when `head` is shorter than a full 40-byte NAVX header).
fn runtime_stream_len(head: &[u8]) -> Option<u64> {
    Some(u64::from_le_bytes(head.get(28..36)?.try_into().ok()?))
}

/// Open a runtime (`.NEA`) `.app` package: read its whole RC4 stream
/// (length from the header, capped), descramble it, and open the plain zip
/// it wraps. Errors when the stream cannot be read or the descrambled bytes
/// are not a zip — the loud failure channel for a format or key change from
/// the publisher.
fn open_runtime_app_file(path: &Path) -> anyhow::Result<zip::ZipArchive<AppReader>> {
    let mut file =
        std::fs::File::open(path).with_context(|| format!("open .app: {}", path.display()))?;
    let mut head = [0u8; 48];
    file.read_exact(&mut head)
        .with_context(|| format!("read .app header: {}", path.display()))?;
    let stream_len =
        runtime_stream_len(&head).context("runtime package header declares no stream length")?;
    crate::capped_io::check_declared_size(stream_len, crate::capped_io::RUNTIME_APP_STREAM_CAP)
        .with_context(|| format!("runtime package stream length: {}", path.display()))?;
    let mut stream = vec![0u8; stream_len as usize];
    file.read_exact(&mut stream)
        .with_context(|| format!("read .app stream: {}", path.display()))?;
    let clear = rc4_descramble_stream(&stream);
    drop(stream);
    let reader = Box::new(Cursor::new(clear)) as AppReader;
    zip::ZipArchive::new(reader).with_context(|| format!("reading zip in .app: {}", path.display()))
}

/// The root entry that marks a Ready-to-Run package.
const READY_TO_RUN_MANIFEST: &str = "readytorunappmanifest.json";

/// `Read + Seek` as one trait, so an app's zip can come from a file or from memory.
pub trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

/// The reader behind an opened `.app` zip: the file itself, or the temp file
/// (or, when temp storage fails, the buffer) holding the nested app of a
/// Ready-to-Run package.
pub type AppReader = Box<dyn ReadSeek + Send>;

/// If `archive` is a Ready-to-Run package, return the bytes of the app it nests.
///
/// A Ready-to-Run package is Microsoft's precompiled form of an app. Microsoft
/// ships Base Application, System Application, Business Foundation and others
/// this way (seen on BC 28.0 to 28.4). The package is itself a `.app`, but its
/// zip holds only `readytorunappmanifest.json`, precompiled `.dll` files and the
/// real `.app` as one entry, named by the manifest's `EmbeddedAppFileName`.
/// `NavxManifest.xml`, `SymbolReference.json` and the source live only in that
/// nested app — read the wrapper directly and the whole app is silently missed.
///
/// `Ok(None)` for an ordinary `.app`.
pub fn read_ready_to_run_app<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
) -> anyhow::Result<Option<Vec<u8>>> {
    with_ready_to_run_app(archive, |app| {
        Ok(crate::capped_io::read_capped(
            app,
            crate::capped_io::READY_TO_RUN_APP_CAP,
        )?)
    })
}

/// The shared half of [`read_ready_to_run_app`]: find the nested app entry,
/// check its declared size, and hand the entry to `read`, which must enforce
/// [`crate::capped_io::READY_TO_RUN_APP_CAP`] on the bytes it reads.
fn with_ready_to_run_app<R: Read + Seek, T>(
    archive: &mut zip::ZipArchive<R>,
    read: impl FnOnce(zip::read::ZipFile<'_, R>) -> anyhow::Result<T>,
) -> anyhow::Result<Option<T>> {
    let manifest = match archive.by_name(READY_TO_RUN_MANIFEST) {
        Ok(f) => f,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(e).context("reading readytorunappmanifest.json"),
    };
    crate::capped_io::check_declared_size(manifest.size(), crate::capped_io::NAVX_MANIFEST_XML_CAP)
        .context("readytorunappmanifest.json declared size exceeds cap")?;
    let bytes = crate::capped_io::read_capped(manifest, crate::capped_io::NAVX_MANIFEST_XML_CAP)
        .context("reading readytorunappmanifest.json")?;

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct ReadyToRunManifest {
        embedded_app_file_name: String,
    }
    let manifest: ReadyToRunManifest =
        serde_json::from_str(&decode_text(&bytes)).context("parsing readytorunappmanifest.json")?;

    let name = manifest.embedded_app_file_name;
    let app = archive
        .by_name(&name)
        .with_context(|| format!("Ready-to-Run package has no entry {name}"))?;
    crate::capped_io::check_declared_size(app.size(), crate::capped_io::READY_TO_RUN_APP_CAP)
        .with_context(|| format!("nested app {name} declared size exceeds cap"))?;
    let out = read(app).with_context(|| format!("reading nested app {name}"))?;
    Ok(Some(out))
}

/// Copy a nested app entry into an anonymous temp file holding exactly
/// `strip_app_header(<entry bytes>)`, rewound, with the same
/// [`crate::capped_io::READY_TO_RUN_APP_CAP`] on the whole entry.
///
/// The disk-backed form of `strip_app_header(&read_ready_to_run_app(..))`:
/// BaseApp's nested app is ~44 MB, too big to hold as one heap buffer under a
/// tight container memory limit.
fn copy_nested_app_to_tempfile<R: Read>(mut app: R) -> anyhow::Result<std::fs::File> {
    use crate::capped_io::{CapReadError, copy_capped_to_tempfile};
    let cap = crate::capped_io::READY_TO_RUN_APP_CAP;
    // `strip_app_header` looks at no more than the first 4096 + 4 bytes, so
    // this head is enough to find the same offset it would on the whole entry.
    let mut head = Vec::new();
    (&mut app)
        .take(4096 + 4)
        .read_to_end(&mut head)
        .map_err(CapReadError::Io)?;
    let offset = (head.len() - strip_app_header(&head).len()) as u64;
    let rest = Cursor::new(&head[offset as usize..]).chain(app);
    // Entry bytes = offset + copied, so `copied <= cap - offset` is the same
    // test as `entry <= cap`. The reported cap stays the entry cap.
    match copy_capped_to_tempfile(rest, cap - offset) {
        Err(CapReadError::CapExceeded { .. }) => Err(CapReadError::CapExceeded { cap }.into()),
        r => Ok(r?),
    }
}

/// The zip bytes of a `.app`: its header stripped and, for a Ready-to-Run
/// package, the nested app's zip in place of the wrapper's.
///
/// A package whose nested app cannot be read is logged and yields the wrapper
/// unchanged, so every caller fails closed exactly as it does for any other
/// unreadable app (no manifest, no symbols, no source).
///
/// ponytail: a caller that reads several entries calls this once per entry,
/// so a Ready-to-Run app is unpacked once per read; pass the result down if
/// that cost ever shows up.
pub fn app_zip_bytes(app_bytes: &[u8]) -> Cow<'_, [u8]> {
    let zip = strip_app_header(app_bytes);
    let Ok(mut archive) = zip::ZipArchive::new(Cursor::new(zip)) else {
        return Cow::Borrowed(zip);
    };
    match read_ready_to_run_app(&mut archive) {
        Ok(Some(app)) => Cow::Owned(strip_app_header(&app).to_vec()),
        Ok(None) => Cow::Borrowed(zip),
        Err(e) => {
            log::warn!("unreadable Ready-to-Run package: {e:#}");
            Cow::Borrowed(zip)
        }
    }
}

/// Open a `.app` file's zip: skip the NAVX header and, for a Ready-to-Run
/// package, open the nested app in the wrapper's place. An ordinary `.app` is
/// still read straight from the file. A nested app is decompressed into an
/// anonymous temp file (deleted when the archive is dropped), not into memory.
///
/// A runtime (`.NEA`) package — one whose head is
/// [`is_encrypted_app_header`]-shaped — is descrambled with the public RC4
/// key and its plain zip opened in the file's place, so its manifest,
/// symbols and source read like any other package's. `Err(EncryptedApp)` only
/// when the descramble fails (truncated stream, cap exceeded, not a zip): a
/// future format or key change surfaces by name, not as silent absence.
///
/// `Ok(None)` when the file holds no zip at all (a symbol-only runtime app).
pub fn open_app_file(path: &Path) -> anyhow::Result<Option<zip::ZipArchive<AppReader>>> {
    if is_encrypted_app_file(path) {
        // Descramble the runtime payload into memory and open its plain zip.
        // A nested Ready-to-Run app inside a runtime package has never been
        // seen; if one appears, the R2R branch below unwraps it the usual way.
        let mut archive = open_runtime_app_file(path).map_err(|_| {
            anyhow::Error::new(EncryptedApp {
                path: path.to_path_buf(),
            })
        })?;
        let app: AppReader = match with_ready_to_run_app(&mut archive, |app| {
            copy_nested_app_to_tempfile(app)
        }) {
            Ok(None) => return Ok(Some(archive)),
            Ok(Some(file)) => Box::new(BufReader::new(file)),
            Err(e) if crate::capped_io::is_temp_storage_failure(&e) => {
                log::warn!(
                    "{}: temp storage failed ({e:#}); reading the nested app into memory",
                    path.display()
                );
                let mut app = read_ready_to_run_app(&mut archive)
                    .with_context(|| format!("Ready-to-Run package: {}", path.display()))?
                    .with_context(|| format!("Ready-to-Run package: {}", path.display()))?;
                let header = app.len() - strip_app_header(&app).len();
                app.drain(..header);
                Box::new(Cursor::new(app))
            }
            Err(e) => {
                return Err(e).with_context(|| format!("Ready-to-Run package: {}", path.display()));
            }
        };
        let nested = zip::ZipArchive::new(app)
            .with_context(|| format!("reading nested app in {}", path.display()))?;
        return Ok(Some(nested));
    }
    let file =
        std::fs::File::open(path).with_context(|| format!("open .app: {}", path.display()))?;
    let mut reader = BufReader::new(file);
    reader
        .seek(SeekFrom::Start(NAVX_HEADER_SIZE))
        .with_context(|| format!("skip NAVX header: {}", path.display()))?;
    let mut archive = match zip::ZipArchive::new(Box::new(reader) as AppReader) {
        Ok(a) => a,
        Err(zip::result::ZipError::InvalidArchive(_)) => return Ok(None),
        Err(e) => {
            return Err(e).with_context(|| format!("reading zip in .app: {}", path.display()));
        }
    };
    let app: AppReader =
        match with_ready_to_run_app(&mut archive, |app| copy_nested_app_to_tempfile(app)) {
            Ok(None) => return Ok(Some(archive)),
            Ok(Some(file)) => Box::new(BufReader::new(file)),
            // The temp file only saves memory: never let a full temp disk
            // turn into a missing app. Read the nested app into memory.
            Err(e) if crate::capped_io::is_temp_storage_failure(&e) => {
                log::warn!(
                    "{}: temp storage failed ({e:#}); reading the nested app into memory",
                    path.display()
                );
                let mut app = read_ready_to_run_app(&mut archive)
                    .with_context(|| format!("Ready-to-Run package: {}", path.display()))?
                    .with_context(|| format!("Ready-to-Run package: {}", path.display()))?;
                let header = app.len() - strip_app_header(&app).len();
                app.drain(..header);
                Box::new(Cursor::new(app))
            }
            Err(e) => {
                return Err(e).with_context(|| format!("Ready-to-Run package: {}", path.display()));
            }
        };
    let nested = zip::ZipArchive::new(app)
        .with_context(|| format!("reading nested app in {}", path.display()))?;
    Ok(Some(nested))
}

/// One big zip entry's bytes: a read-only map of an anonymous temp file, or
/// a heap buffer when temp storage failed. See [`read_large_entry`].
pub enum LargeEntry {
    // `map` is declared first so it is dropped before its file.
    Mapped {
        map: Option<memmap2::Mmap>,
        _file: std::fs::File,
    },
    Memory(Vec<u8>),
}

impl std::ops::Deref for LargeEntry {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            LargeEntry::Mapped { map, .. } => map.as_deref().unwrap_or(&[]),
            LargeEntry::Memory(v) => v,
        }
    }
}

/// Read the zip entry `name` (declared size and bytes capped at `cap`) for a
/// caller that needs it whole, e.g. BaseApp's ~58 MB `SymbolReference.json`.
///
/// The entry goes into an anonymous temp file, mapped read-only: a file-backed
/// mapping does not count against Windows commit, unlike one big heap buffer.
/// When the temp file cannot be written (a full disk), the entry is read into
/// memory instead, so the result never depends on the temp disk.
pub fn read_large_entry<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
    cap: u64,
) -> anyhow::Result<LargeEntry> {
    use crate::capped_io::{CapReadError, check_declared_size, copy_capped_to_tempfile};
    fn open<'a, R: Read + Seek>(
        archive: &'a mut zip::ZipArchive<R>,
        name: &str,
        cap: u64,
    ) -> anyhow::Result<zip::read::ZipFile<'a, R>> {
        let entry = archive
            .by_name(name)
            .with_context(|| format!("{name} not found in app package"))?;
        check_declared_size(entry.size(), cap)
            .with_context(|| format!("{name} declared size exceeds cap"))?;
        Ok(entry)
    }
    let to_file = copy_capped_to_tempfile(open(archive, name, cap)?, cap)
        .map_err(anyhow::Error::from)
        .and_then(|file| {
            // SAFETY: the file is an anonymous temp file only this function
            // can reach (no name, never shared), so nothing changes it while
            // mapped; the mapping is dropped before the file.
            let map = unsafe { crate::capped_io::map_read_only(&file) }
                .map_err(CapReadError::TempStorage)?;
            Ok(LargeEntry::Mapped { map, _file: file })
        });
    match to_file {
        Err(e) if crate::capped_io::is_temp_storage_failure(&e) => {
            log::warn!("{name}: temp storage failed ({e:#}); reading it into memory");
            let bytes = crate::capped_io::read_capped(open(archive, name, cap)?, cap)
                .with_context(|| format!("Failed to read {name}"))?;
            Ok(LargeEntry::Memory(bytes))
        }
        r => r.with_context(|| format!("Failed to read {name}")),
    }
}

/// Scan the first `≤4096` bytes of `bytes` for the ZIP local-file signature
/// `PK\x03\x04` and return the slice starting there. If no signature is found in
/// the bound, assume it is already a plain ZIP and return the input unchanged.
///
/// Mirrors al-sem `stripAppHeader`: `limit = min(len - 4, 4096)`; on a match at
/// `i == 0` the input is returned verbatim, otherwise the tail from `i`.
pub fn strip_app_header(bytes: &[u8]) -> &[u8] {
    if bytes.len() < 4 {
        return bytes;
    }
    // limit = min(len - 4, 4096); loop is `for i in 0..limit` (exclusive),
    // and we index i..i+4 so the last inspected window is [limit-1 .. limit+2].
    let limit = std::cmp::min(bytes.len() - 4, 4096);
    for i in 0..limit {
        if bytes[i] == 0x50 && bytes[i + 1] == 0x4b && bytes[i + 2] == 0x03 && bytes[i + 3] == 0x04
        {
            return &bytes[i..];
        }
    }
    bytes
}

/// Normalize a ZIP entry key: backslashes → forward slashes. Mirrors al-sem
/// `normalizeZipEntryName`.
pub fn normalize_zip_entry_name(key: &str) -> String {
    key.replace('\\', "/")
}

/// Extract the bytes of the FIRST entry whose normalized, lowercased name ends
/// with `suffix_lower`. Iterates entries in archive order (the `zip` crate's
/// `by_index`, which preserves the central-directory order — the analogue of
/// `fflate`'s insertion-ordered entry map). Returns `None` when the archive is
/// unreadable, no entry matches, OR the matched entry exceeds `cap` bytes
/// (Task T2.2 — a hostile declared/actual size joins the SAME fail-closed
/// `None` path this function already used for every other failure mode, so
/// callers need no new wiring). Never panics.
fn extract_entry_bytes(app_bytes: &[u8], suffix_lower: &str, cap: u64) -> Option<Vec<u8>> {
    let cursor = Cursor::new(app_zip_bytes(app_bytes).into_owned());
    let mut archive = match zip::ZipArchive::new(cursor) {
        Ok(a) => a,
        Err(_) => return None,
    };
    let len = archive.len();
    for i in 0..len {
        // Read the name first (immutable view), then re-borrow to read bytes.
        // zip 9: `name()` fails only on a name that is neither UTF-8 nor CP437;
        // skip that entry as an unreadable one is skipped.
        let name = match archive
            .by_index(i)
            .map(|f| f.name().map(|n| n.into_owned()))
        {
            Ok(Ok(n)) => n,
            _ => continue,
        };
        let normalized = normalize_zip_entry_name(&name).to_lowercase();
        if normalized.ends_with(suffix_lower) {
            let file = match archive.by_index(i) {
                Ok(f) => f,
                Err(_) => return None,
            };
            // Belt-and-suspenders: reject a hostile declared size before
            // decompressing, then bound the read itself (a lying central
            // directory) — both fold into the same `None`.
            if crate::capped_io::check_declared_size(file.size(), cap).is_err() {
                return None;
            }
            return crate::capped_io::read_capped(file, cap).ok();
        }
    }
    None
}

/// Decode bytes as UTF-8 (lossy — engine never panics on bad input) and strip a
/// leading UTF-8 BOM if present. Mirrors al-sem `decodeText`.
fn decode_text(bytes: &[u8]) -> String {
    // Strip a UTF-8 BOM (EF BB BF) at the byte level first, matching the TS
    // behaviour of dropping the U+FEFF code unit after decode.
    let body = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        &bytes[3..]
    } else {
        bytes
    };
    String::from_utf8_lossy(body).into_owned()
}

/// Extract the `SymbolReference.json` text from raw `.app` bytes. Returns `None`
/// if absent. Never panics. Mirrors al-sem `extractSymbolReferenceJson`.
pub fn extract_symbol_reference_json(app_bytes: &[u8]) -> Option<String> {
    extract_entry_bytes(
        app_bytes,
        "symbolreference.json",
        crate::capped_io::SYMBOL_REFERENCE_JSON_CAP,
    )
    .map(|b| decode_text(&b))
}

/// Extract the `NavxManifest.xml` text from raw `.app` bytes. Returns `None` if
/// absent. Never panics. Mirrors the manifest entry pick in al-sem
/// `readAppManifest` (UTF-8 decode; no BOM strip in TS for the manifest, but a
/// leading BOM is harmless to the regex scan — we leave the XML bytes as-is via
/// lossy UTF-8).
pub fn extract_navx_manifest_xml(app_bytes: &[u8]) -> Option<String> {
    extract_entry_bytes(
        app_bytes,
        "navxmanifest.xml",
        crate::capped_io::NAVX_MANIFEST_XML_CAP,
    )
    .map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// Builders for real `.app` bytes in tests, shared with every module whose
/// reader must handle a Ready-to-Run package.
#[cfg(test)]
pub(crate) mod test_apps {
    use std::io::Write as _;

    /// A `.app`: a 40-byte NAVX header followed by a zip of `entries`.
    pub(crate) fn build_app(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip_buf = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut zip_buf);
            let opts = zip::write::SimpleFileOptions::default();
            for (name, content) in entries {
                writer.start_file(*name, opts).unwrap();
                writer.write_all(content).unwrap();
            }
            writer.finish().unwrap();
        }
        let mut out = b"NAVX".to_vec();
        out.resize(super::NAVX_HEADER_SIZE as usize, 0);
        out.extend_from_slice(zip_buf.get_ref());
        out
    }

    /// Wrap `app` the way Microsoft ships a Ready-to-Run package: a `.app`
    /// whose zip holds `readytorunappmanifest.json`, the real app as an entry
    /// named by `EmbeddedAppFileName`, and a precompiled `.dll` (layout copied
    /// from Microsoft's BC 28.4 Base Application package).
    pub(crate) fn wrap_ready_to_run(app: &[u8]) -> Vec<u8> {
        let inner = "437dbf0e84ff417a965ded2bb9650972_28.4.53241.53758_28_28014.app";
        let manifest = format!(
            r#"{{"EmbeddedAppId":"437dbf0e-84ff-417a-965d-ed2bb9650972","EmbeddedAppName":"Base Application","EmbeddedAppPublisher":"Microsoft","EmbeddedAppVersion":"28.4.53241.53758","EmbeddedAppFileName":"{inner}"}}"#
        );
        build_app(&[
            ("readytorunappmanifest.json", manifest.as_bytes()),
            (inner, app),
            (
                "publishedartifacts/file:///S:/s/Ready2RunApps/F9DA3E40.dll",
                b"MZ not a real dll",
            ),
        ])
    }

    /// A `NavxManifest.xml` for app `guid`.
    pub(crate) fn manifest_xml(guid: &str, name: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?><Package xmlns="http://schemas.microsoft.com/navx/2015/manifest"><App Id="{guid}" Name="{name}" Publisher="Microsoft" Version="28.4.53241.53758" Runtime="16.0" /></Package>"#
        )
    }

    /// A runtime (`.NEA`) `.app` around `inner_zip`: a 40-byte NAVX header,
    /// the `.NEA 00 00 00 01` mark, `inner_zip` scrambled with the public RC4
    /// key, then the `NXSB`-magic signature trailer the real packages carry
    /// (its bytes are opaque here; nothing reads it).
    ///
    /// The stream length at offset 28 counts from the mark's END (offset 48),
    /// not from the header's end — the community-documented layout, verified
    /// against all 36 Continia 29.0 sample files.
    pub(crate) fn wrap_runtime(inner_zip: &[u8]) -> Vec<u8> {
        let scrambled = super::rc4_descramble_stream(inner_zip);
        let mut out = Vec::with_capacity(48 + scrambled.len() + 10);
        out.extend_from_slice(b"NAVX"); // 0..4   magic
        out.extend_from_slice(&40u32.to_le_bytes()); // 4..8   header size
        out.extend_from_slice(&2u32.to_le_bytes()); // 8..12  header version
        out.extend_from_slice(&[0x22u8; 16]); // 12..28 any GUID; not the app id
        out.extend_from_slice(&(scrambled.len() as u64).to_le_bytes()); // 28..36 stream length
        out.extend_from_slice(b"NAVX"); // 36..40 closing magic
        out.extend_from_slice(b".NEA\x00\x00\x00\x01"); // 40..48 the runtime mark
        out.extend_from_slice(&scrambled); // 48..   the RC4 stream
        // A plausible trailer (NXSB magic + DER bytes); nothing reads it.
        out.extend_from_slice(b"NXSB");
        out.extend_from_slice(&[0x30, 0x82, 0x36, 0xb5, 0x06, 0x09]);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Ready-to-Run package must yield its NESTED app's manifest and symbols
    /// through the same public readers every engine caller uses — before the
    /// fix both returned `None`, because the wrapper's zip holds neither entry.
    #[test]
    fn ready_to_run_package_yields_the_nested_apps_manifest_and_symbols() {
        let manifest =
            test_apps::manifest_xml("437dbf0e-84ff-417a-965d-ed2bb9650972", "Base Application");
        let symbols = br#"{"Codeunits":[{"Id":80,"Name":"Sales-Post"}]}"#;
        let app = test_apps::build_app(&[
            ("NavxManifest.xml", manifest.as_bytes()),
            ("SymbolReference.json", symbols),
        ]);
        let package = test_apps::wrap_ready_to_run(&app);

        assert_eq!(
            extract_navx_manifest_xml(&package).as_deref(),
            Some(manifest.as_str())
        );
        assert_eq!(
            extract_symbol_reference_json(&package).as_deref(),
            Some(std::str::from_utf8(symbols).unwrap())
        );
    }

    /// An ordinary `.app` is unaffected: same bytes back, nothing unwrapped.
    #[test]
    fn ordinary_app_is_not_unwrapped() {
        let app = test_apps::build_app(&[("NavxManifest.xml", b"<Package/>")]);
        assert!(matches!(app_zip_bytes(&app), Cow::Borrowed(_)));
        assert_eq!(
            extract_navx_manifest_xml(&app).as_deref(),
            Some("<Package/>")
        );
    }

    /// A Ready-to-Run package naming a nested app it does not contain fails
    /// closed: no manifest, never a panic.
    #[test]
    fn ready_to_run_package_missing_its_nested_app_fails_closed() {
        let package = test_apps::build_app(&[(
            "readytorunappmanifest.json",
            br#"{"EmbeddedAppFileName":"missing.app"}"#,
        )]);
        assert!(extract_navx_manifest_xml(&package).is_none());
        let mut archive = zip::ZipArchive::new(Cursor::new(strip_app_header(&package))).unwrap();
        let err = read_ready_to_run_app(&mut archive).unwrap_err();
        assert!(format!("{err:#}").contains("missing.app"), "{err:#}");
    }

    /// The path-based opener (used by the dependency loader, ABI ingest and
    /// embedded-source extraction) opens the nested app too.
    #[test]
    fn open_app_file_opens_the_nested_app_of_a_ready_to_run_package() {
        let app = test_apps::build_app(&[("NavxManifest.xml", b"<Package/>")]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("Microsoft_Base Application_28.4.53241.53758.app");
        std::fs::write(&path, test_apps::wrap_ready_to_run(&app)).unwrap();

        let mut archive = open_app_file(&path).unwrap().expect("a zip");
        assert!(archive.by_name("NavxManifest.xml").is_ok());
        assert!(archive.by_name(READY_TO_RUN_MANIFEST).is_err());
    }

    /// RC4 test vector: the public runtime key applied to `b"hello world"`.
    /// A wrong key or KSA order produces different bytes, so this pins both.
    #[test]
    fn rc4_public_key_test_vector() {
        assert_eq!(
            rc4_descramble_stream(b"hello world"),
            vec![
                0x95, 0xed, 0xfc, 0x70, 0x27, 0xad, 0x0d, 0x87, 0x4e, 0xb1, 0x08
            ]
        );
        // Symmetry: scrambling back yields the input.
        assert_eq!(
            rc4_descramble_stream(&rc4_descramble_stream(b"hello world")),
            b"hello world".to_vec()
        );
    }

    /// A runtime package must open through the same public path-based opener
    /// the dependency loader, ABI ingest and embedded-source extraction use:
    /// its descrambled manifest and symbols read like a plain package's.
    /// Before this, a `.NEA` package was `EncryptedApp` — unreadable by name.
    #[test]
    fn runtime_package_reads_through_the_public_opener() {
        let manifest = test_apps::manifest_xml("437dbf0e-84ff-417a-965d-ed2bb9650972", "Base App");
        let symbols = br#"{"Codeunits":[{"Id":80,"Name":"Sales-Post"}]}"#;
        let plain = test_apps::build_app(&[
            ("NavxManifest.xml", manifest.as_bytes()),
            ("SymbolReference.json", symbols),
        ]);
        // strip the 40-byte NAVX header `build_app` adds: the runtime wrapper
        // must wrap the bare zip, and `wrap_runtime` re-adds its own header.
        let inner_zip = strip_app_header(&plain);
        let runtime = test_apps::wrap_runtime(inner_zip);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Continia_Runtime_29.0.213759.app");
        std::fs::write(&path, &runtime).unwrap();

        assert!(is_encrypted_app_file(&path));
        let mut archive = open_app_file(&path).unwrap().expect("a zip");
        let mut m = String::new();
        archive
            .by_name("NavxManifest.xml")
            .unwrap()
            .read_to_string(&mut m)
            .unwrap();
        assert_eq!(m, manifest);
        let mut s = String::new();
        archive
            .by_name("SymbolReference.json")
            .unwrap()
            .read_to_string(&mut s)
            .unwrap();
        assert_eq!(s.as_bytes(), symbols.as_slice());
    }

    /// A `.NEA`-marked package whose stream cannot be descrambled into a zip
    /// fails as `EncryptedApp` — the loud channel for a format or key change.
    #[test]
    fn runtime_package_that_does_not_descramble_fails_as_encrypted_app() {
        // Mark + header right, stream garbage: whatever RC4 makes of it is not
        // a zip, so the archive open fails and maps to `EncryptedApp`.
        let mut out = Vec::new();
        out.extend_from_slice(b"NAVX");
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&[0x22u8; 16]);
        out.extend_from_slice(&0x10u64.to_le_bytes());
        out.extend_from_slice(b"NAVX");
        out.extend_from_slice(b".NEA\x00\x00\x00\x01");
        out.extend_from_slice(&[0xa5u8; 16]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Broken_Runtime.app");
        std::fs::write(&path, &out).unwrap();

        let err = match open_app_file(&path) {
            Err(e) => e,
            // `ZipArchive<AppReader>` is not `Debug`; report its presence.
            Ok(_) => panic!("a broken runtime package must not open"),
        };
        let e = format!("{err:#}");
        assert!(
            e.contains("encrypted .app package"),
            "expected EncryptedApp, got: {e}"
        );
    }

    /// `is_encrypted_app_header` accepts only the runtime shape: a plain
    /// package's head (zip at offset 40) and a short head are not runtime.
    #[test]
    fn is_encrypted_app_header_discriminates() {
        let runtime = test_apps::wrap_runtime(b"PK\x03\x04 whatever");
        assert!(is_encrypted_app_header(&runtime[..44]));
        let plain = test_apps::build_app(&[("NavxManifest.xml", b"<Package/>")]);
        assert!(!is_encrypted_app_header(&plain[..44]));
        assert!(!is_encrypted_app_header(b"NAVX short"));
    }

    #[test]
    fn strip_header_no_header_returns_input() {
        let plain = b"PK\x03\x04rest-of-zip";
        assert_eq!(strip_app_header(plain), plain);
    }

    #[test]
    fn strip_header_skips_binary_prefix() {
        let mut bytes = vec![0x00, 0xAA, 0xBB];
        bytes.extend_from_slice(b"PK\x03\x04tail");
        assert_eq!(strip_app_header(&bytes), b"PK\x03\x04tail");
    }

    #[test]
    fn strip_header_short_input_is_safe() {
        assert_eq!(strip_app_header(b"PK"), b"PK");
        assert_eq!(strip_app_header(b""), b"");
    }

    #[test]
    fn strip_header_signature_beyond_4096_not_found() {
        // Header longer than the 4096 scan bound → signature not found → input
        // returned unchanged (TS behaviour: assume plain ZIP).
        let mut bytes = vec![0u8; 5000];
        bytes.extend_from_slice(b"PK\x03\x04tail");
        assert_eq!(strip_app_header(&bytes), &bytes[..]);
    }

    #[test]
    fn normalize_backslashes() {
        assert_eq!(normalize_zip_entry_name("a\\b\\c"), "a/b/c");
        assert_eq!(normalize_zip_entry_name("a/b"), "a/b");
    }

    #[test]
    fn decode_text_strips_bom() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(b"{}");
        assert_eq!(decode_text(&bytes), "{}");
        assert_eq!(decode_text(b"{}"), "{}");
    }

    /// Build a plain (no NAVX header) in-memory zip with one entry holding
    /// `size` bytes of compressible zero-padding.
    fn build_zip_with_entry(entry_name: &str, size: usize) -> Vec<u8> {
        use std::io::Write as _;

        let mut buf = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            writer.start_file(entry_name, opts).unwrap();
            const CHUNK: usize = 1024 * 1024;
            let chunk = vec![0u8; CHUNK];
            let mut remaining = size;
            while remaining > 0 {
                let n = remaining.min(CHUNK);
                writer.write_all(&chunk[..n]).unwrap();
                remaining -= n;
            }
            writer.finish().unwrap();
        }
        buf.into_inner()
    }

    /// Task T2.2: an oversized `SymbolReference.json` entry must fail closed
    /// to `None` — the SAME path this fail-closed function already uses for
    /// a missing entry or an unreadable archive — never a panic.
    #[test]
    fn oversized_symbol_reference_entry_fails_closed_to_none() {
        let bytes = build_zip_with_entry(
            "SymbolReference.json",
            crate::capped_io::SYMBOL_REFERENCE_JSON_CAP as usize + 1024,
        );
        assert!(extract_symbol_reference_json(&bytes).is_none());
    }

    /// Same fail-closed contract for the (much smaller) manifest cap.
    #[test]
    fn oversized_navx_manifest_entry_fails_closed_to_none() {
        let bytes = build_zip_with_entry(
            "NavxManifest.xml",
            crate::capped_io::NAVX_MANIFEST_XML_CAP as usize + 1024,
        );
        assert!(extract_navx_manifest_xml(&bytes).is_none());
    }

    /// A well-under-cap entry still round-trips unaffected by the cap.
    #[test]
    fn normal_sized_entry_still_extracts() {
        let bytes = build_zip_with_entry_with_content("SymbolReference.json", b"{\"ok\":true}");
        let text = extract_symbol_reference_json(&bytes).expect("under cap");
        assert_eq!(text, "{\"ok\":true}");
    }

    fn build_zip_with_entry_with_content(entry_name: &str, content: &[u8]) -> Vec<u8> {
        use std::io::Write as _;
        let mut buf = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            writer.start_file(entry_name, opts).unwrap();
            writer.write_all(content).unwrap();
            writer.finish().unwrap();
        }
        buf.into_inner()
    }
}
