//! Bounded-read helper for every decompression path (Task T2.2): a hostile
//! `.app`'s zip entries and gzip'd snapshot streams must not be read via an
//! unbounded `read_to_end` — a few KB of DEFLATE can expand to an
//! attacker-chosen number of gigabytes, and release builds are
//! `panic = "abort"`, so even the resulting allocation failure aborts the
//! whole LSP/CLI process, not just the one request. Every site that
//! decompresses untrusted bytes reads through [`read_capped`], which turns
//! "the stream produced more than `cap` bytes" into a named, catchable error
//! instead of an unbounded `Vec` grow.
//!
//! This is the lowest common module reachable by every consumer: the zip
//! sites live in both the crate-root (`app_package`, `snapshot::embedded`)
//! and `engine::deps::*` module trees, and the gzip site lives in
//! `engine::gate::*` — the crate root is their only shared ancestor.

use std::fmt;
use std::io::Read;

/// A capped read failed: either the stream exceeded its byte cap, or the
/// underlying reader/decompressor itself errored (corrupt archive, truncated
/// deflate stream, etc — NOT a cap violation). Both variants implement
/// `std::error::Error`, so `?` composes into `anyhow::Result` unchanged at
/// call sites that already return one.
#[derive(Debug)]
pub enum CapReadError {
    /// The stream produced more than `cap` bytes before EOF.
    CapExceeded { cap: u64 },
    /// The underlying reader failed for a reason other than the cap.
    Io(std::io::Error),
    /// Our own temp storage failed (a full disk, an unwritable temp dir): the
    /// input was fine. Never a reason to treat the input as unreadable; see
    /// [`is_temp_storage_failure`].
    TempStorage(std::io::Error),
}

/// Whether `e` (anywhere in its chain) is a [`CapReadError::TempStorage`].
///
/// Callers that copy an entry to a temp file only to save memory use this to
/// fall back to reading it into memory: the result must not depend on the
/// state of the temp disk.
pub fn is_temp_storage_failure(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        matches!(
            c.downcast_ref::<CapReadError>(),
            Some(CapReadError::TempStorage(_))
        )
    })
}

impl fmt::Display for CapReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CapReadError::CapExceeded { cap } => {
                write!(f, "decompressed size exceeds the {cap}-byte cap")
            }
            CapReadError::Io(e) => write!(f, "read failed: {e}"),
            CapReadError::TempStorage(e) => write!(f, "temp storage failed: {e}"),
        }
    }
}

impl std::error::Error for CapReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CapReadError::CapExceeded { .. } => None,
            CapReadError::Io(e) | CapReadError::TempStorage(e) => Some(e),
        }
    }
}

/// Read all of `reader` into a `Vec<u8>`, capped at `cap` bytes.
///
/// Reads at most `cap + 1` bytes via [`Read::take`] — a hostile stream forces
/// at most a `cap + 1`-byte allocation before the cap trips, never its full
/// attacker-controlled expanded size. Reading back exactly `cap + 1` bytes
/// means the stream had strictly more than `cap` bytes available; we never
/// read further to find out how much more (the exact overage is not useful
/// and reading it would defeat the point of the cap).
pub fn read_capped<R: Read>(reader: R, cap: u64) -> Result<Vec<u8>, CapReadError> {
    let mut out = Vec::new();
    reader
        .take(cap.saturating_add(1))
        .read_to_end(&mut out)
        .map_err(CapReadError::Io)?;
    if out.len() as u64 > cap {
        return Err(CapReadError::CapExceeded { cap });
    }
    Ok(out)
}

/// Copy all of `reader` into an anonymous temp file, capped at `cap` bytes,
/// and return the file rewound to its start.
///
/// The disk-backed twin of [`read_capped`], for entries big enough (tens of
/// MB) that one contiguous heap buffer is a risk under a tight memory limit.
/// Same cap rule: at most `cap + 1` bytes are ever copied, and more than `cap`
/// is [`CapReadError::CapExceeded`]. The file has no name and is deleted when
/// the returned handle is dropped.
///
/// A failure of the temp file itself is [`CapReadError::TempStorage`], kept
/// apart from a failure of `reader` ([`CapReadError::Io`]).
pub fn copy_capped_to_tempfile<R: Read>(
    reader: R,
    cap: u64,
) -> Result<std::fs::File, CapReadError> {
    use std::io::{Seek, SeekFrom};
    let mut file = tempfile::tempfile().map_err(CapReadError::TempStorage)?;
    copy_capped(
        reader,
        std::io::BufWriter::with_capacity(1 << 20, &mut file),
        cap,
    )?;
    file.seek(SeekFrom::Start(0))
        .map_err(CapReadError::TempStorage)?;
    Ok(file)
}

/// The copy half of [`copy_capped_to_tempfile`]: `reader` failing is
/// [`CapReadError::Io`], `writer` failing (a full disk) is
/// [`CapReadError::TempStorage`].
fn copy_capped<R: Read, W: std::io::Write>(
    reader: R,
    mut writer: W,
    cap: u64,
) -> Result<(), CapReadError> {
    let mut reader = reader.take(cap.saturating_add(1));
    let mut buf = vec![0u8; 64 * 1024];
    let mut copied = 0u64;
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(CapReadError::Io(e)),
        };
        writer
            .write_all(&buf[..n])
            .map_err(CapReadError::TempStorage)?;
        copied += n as u64;
    }
    writer.flush().map_err(CapReadError::TempStorage)?;
    if copied > cap {
        return Err(CapReadError::CapExceeded { cap });
    }
    Ok(())
}

/// Map `file` read-only, or `None` when it is empty (mapping an empty file is
/// an error on some platforms). A read-only file-backed mapping is paged in
/// from the file and does not count against Windows commit, unlike a heap
/// buffer of the same size.
///
/// # Safety
///
/// Nothing may change or truncate the file while the mapping is alive.
pub unsafe fn map_read_only(file: &std::fs::File) -> std::io::Result<Option<memmap2::Mmap>> {
    if file.metadata()?.len() == 0 {
        return Ok(None);
    }
    // SAFETY: passed on to the caller (see the function's contract).
    unsafe { memmap2::Mmap::map(file) }.map(Some)
}

/// Check a zip entry's central-directory-declared uncompressed size against
/// `cap` BEFORE reading it — belt and suspenders alongside `read_capped`'s
/// read-time cap (the brief's "reject before reading when declared size
/// already exceeds the cap"). A hostile archive's central-directory metadata
/// is attacker-controlled same as the entry bytes, so this is a fast-reject
/// optimization that skips decompressing an entry that already announces
/// itself as too big — it is never a substitute for the read-time cap, which
/// is what actually bounds the allocation when the declared size lies (too
/// small, or a Zip64 field disagreement).
pub fn check_declared_size(declared: u64, cap: u64) -> Result<(), CapReadError> {
    if declared > cap {
        return Err(CapReadError::CapExceeded { cap });
    }
    Ok(())
}

// ===========================================================================
// Per-surface caps (Task T2.2). Each ceiling is grounded in a real size
// measured against the CDO_WS reference workspace
// (`U:/Git/DO.Support-SlowDOSetup/DocumentOutput/Cloud`, 10 real BC apps
// including Microsoft BaseApp/System Application) — see the task report —
// with headroom chosen to comfortably admit legitimate outliers while still
// giving a hostile archive a fixed, known ceiling instead of none.
// ===========================================================================

/// Cap for a `SymbolReference.json` zip entry. BaseApp — by far the largest
/// real app in the CDO workspace — measures ~58.3 MB uncompressed (28.0
/// release, 2026-04). 512 MB gives ~8.8x headroom over that for larger W1/
/// localized BaseApp variants while still bounding a hostile entry to a fixed
/// allocation.
pub const SYMBOL_REFERENCE_JSON_CAP: u64 = 512 * 1024 * 1024;

/// Cap for a `NavxManifest.xml` zip entry. A small, fixed-shape metadata
/// document (app identity + `<Dependency>`/`<Module>` lists); BaseApp
/// measures ~6.3 KB in the CDO workspace. 4 MB gives generous (~650x)
/// headroom for an app with an unusually large dependency/InternalsVisibleTo
/// list.
pub const NAVX_MANIFEST_XML_CAP: u64 = 4 * 1024 * 1024;

/// Cap for a single embedded `.al` source-file zip entry. The largest real
/// file in the CDO workspace is BaseApp's `SalesPost.Codeunit.al` at ~789 KB
/// uncompressed. 16 MB gives >20x headroom for an unusually large real AL
/// object while still bounding a hostile entry.
pub const EMBEDDED_AL_SOURCE_CAP: u64 = 16 * 1024 * 1024;

/// Cap for the nested `.app` entry inside a Ready-to-Run package (see
/// `engine::deps::app_package_zip::read_ready_to_run_app`). It is read into
/// memory whole from bytes, or into a temp file when opened from a path
/// (`open_app_file`). BaseApp's nested app measures ~44 MB (BC 28.0-28.4, 2026-10),
/// System Application ~15 MB. 512 MB gives >11x headroom over BaseApp.
pub const READY_TO_RUN_APP_CAP: u64 = 512 * 1024 * 1024;

/// Cap for a `.cbor.gz` capability-snapshot artifact (the cli-b diff engine's
/// serialized whole-workspace analysis — `deserialize_snapshot`'s `gunzip`).
/// This is a REASONED bound, not a directly measured one: generating a real
/// CDO snapshot via `alsem fingerprint --format cbor.gz` did not complete in
/// the debug build within this task's time budget (see the task report), so
/// there is no live artifact to size. The bound is grounded instead in the
/// format's own shape: a snapshot encodes compact per-routine capability
/// FACTS (call edges, cited evidence, order-index entries) for the analyzed
/// workspace, not raw source text — structurally smaller than the source it
/// summarizes. BaseApp's SymbolReference.json (this module's own largest
/// measured real-world decompression payload, at ~58.3 MB) stands in as an
/// upper reference point for "the biggest single real BC app's-worth of
/// data" this format would ever need to describe. 1 GB gives >17x headroom
/// over that reference point — enough for a snapshot spanning many apps at
/// once — while still giving a hostile snapshot file a fixed, known ceiling
/// instead of none.
pub const SNAPSHOT_GZ_CAP: u64 = 1024 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn under_cap_reads_fully() {
        let data = vec![7u8; 100];
        let out = read_capped(Cursor::new(data.clone()), 200).expect("under cap");
        assert_eq!(out, data);
    }

    #[test]
    fn exactly_at_cap_reads_fully() {
        let data = vec![7u8; 100];
        let out = read_capped(Cursor::new(data.clone()), 100).expect("exactly at cap");
        assert_eq!(out, data);
    }

    #[test]
    fn over_cap_by_one_byte_is_rejected() {
        let data = vec![7u8; 101];
        let err = read_capped(Cursor::new(data), 100).unwrap_err();
        assert!(matches!(err, CapReadError::CapExceeded { cap: 100 }));
    }

    /// A well-behaved `Read` impl that never reports EOF — proves
    /// `read_capped` stops after `cap + 1` bytes rather than draining the
    /// reader to exhaustion (which is exactly the OOM this task closes).
    struct Unbounded;
    impl Read for Unbounded {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            for b in buf.iter_mut() {
                *b = 0;
            }
            Ok(buf.len().max(1))
        }
    }

    #[test]
    fn unbounded_stream_is_capped_not_drained() {
        let err = read_capped(Unbounded, 4096).unwrap_err();
        assert!(matches!(err, CapReadError::CapExceeded { cap: 4096 }));
    }

    #[test]
    fn tempfile_copy_round_trips_at_cap_and_rejects_over_cap() {
        let data = vec![7u8; 100];
        let mut file = copy_capped_to_tempfile(Cursor::new(data.clone()), 100).expect("at cap");
        let mut back = Vec::new();
        file.read_to_end(&mut back).unwrap();
        assert_eq!(back, data);

        let err = copy_capped_to_tempfile(Cursor::new(vec![7u8; 101]), 100).unwrap_err();
        assert!(matches!(err, CapReadError::CapExceeded { cap: 100 }));
        let err = copy_capped_to_tempfile(Unbounded, 4096).unwrap_err();
        assert!(matches!(err, CapReadError::CapExceeded { cap: 4096 }));
    }

    /// A writer that accepts `room` bytes and then fails like a full disk.
    struct DiskFull {
        room: usize,
    }
    impl std::io::Write for DiskFull {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self.room == 0 {
                return Err(std::io::Error::other(
                    "There is not enough space on the disk",
                ));
            }
            let n = buf.len().min(self.room);
            self.room -= n;
            Ok(n)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A full disk part way through the copy is temp storage failing, not the
    /// input: callers fall back to memory on `TempStorage` (FW2) and must not
    /// on `Io`, so the two must never be confused.
    #[test]
    fn copy_tells_a_full_disk_from_a_bad_input() {
        let err = copy_capped(
            Cursor::new(vec![7u8; 200_000]),
            DiskFull { room: 100_000 },
            1 << 20,
        )
        .unwrap_err();
        assert!(matches!(err, CapReadError::TempStorage(_)), "{err}");
        let err = anyhow::Error::from(err).context("reading nested app x.app");
        assert!(is_temp_storage_failure(&err), "{err:#}");

        struct BadInput;
        impl Read for BadInput {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("corrupt deflate stream"))
            }
        }
        let err = copy_capped(BadInput, Vec::new(), 1 << 20).unwrap_err();
        assert!(matches!(err, CapReadError::Io(_)), "{err}");
        assert!(!is_temp_storage_failure(&anyhow::Error::from(err)));
    }

    #[test]
    fn declared_size_over_cap_rejected_before_read() {
        assert!(check_declared_size(1_000_000, 1_000).is_err());
        assert!(check_declared_size(1_000, 1_000).is_ok());
        assert!(check_declared_size(999, 1_000).is_ok());
    }

    /// A crafted in-memory zip whose entry declares (central-directory
    /// `uncompressed_size`) far more than a small cap, built from genuinely
    /// compressible content (a run of zero bytes) so the compressed payload
    /// stays tiny while the DECLARED size is large — the "small compressed /
    /// huge declared-size entry" TDD fixture from the task brief. Proves the
    /// declared-size pre-check rejects it WITHOUT decompressing.
    #[test]
    fn crafted_zip_huge_declared_size_rejected_before_read() {
        use zip::write::SimpleFileOptions;

        let mut buf = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buf);
            let opts =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            writer.start_file("bomb.txt", opts).expect("start_file");
            // 50 MB of zeros compresses to a few KB, but the entry's
            // central-directory `uncompressed_size` field records the real
            // 50 MB — that's the number `check_declared_size` inspects.
            let payload = vec![0u8; 50 * 1024 * 1024];
            std::io::Write::write_all(&mut writer, &payload).expect("write bomb payload");
            writer.finish().expect("finish zip");
        }
        let bytes = buf.into_inner();

        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).expect("open crafted zip");
        let entry = archive.by_name("bomb.txt").expect("bomb.txt entry");
        let cap = 1024 * 1024u64; // 1 MB — far under the declared 50 MB.
        let declared = entry.size();
        assert!(declared > cap, "fixture must declare more than the cap");
        assert!(check_declared_size(declared, cap).is_err());
    }

    /// The same crafted archive, but proving the READ-TIME cap independently:
    /// even without consulting `size()` first, draining the entry through
    /// `read_capped` stops at `cap + 1` bytes and reports the named error —
    /// the true backstop for a central directory that lies about the
    /// declared size. The brief's "genuinely-over-cap deflate stream"
    /// fixture.
    #[test]
    fn genuinely_over_cap_deflate_stream_is_capped_mid_read() {
        use zip::write::SimpleFileOptions;

        let mut buf = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buf);
            let opts =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            writer.start_file("bomb.txt", opts).expect("start_file");
            let payload = vec![0u8; 10 * 1024 * 1024]; // 10 MB of zeros.
            std::io::Write::write_all(&mut writer, &payload).expect("write bomb payload");
            writer.finish().expect("finish zip");
        }
        let bytes = buf.into_inner();

        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).expect("open crafted zip");
        let entry = archive.by_name("bomb.txt").expect("bomb.txt entry");
        let cap = 1024 * 1024u64; // 1 MB cap, 10 MB actual inflate.
        let err = read_capped(entry, cap).unwrap_err();
        assert!(matches!(err, CapReadError::CapExceeded { cap: c } if c == cap));
    }

    /// The gunzip-path equivalent: a genuinely-over-cap gzip stream (real
    /// DEFLATE, not a crafted header) must be capped mid-read with the named
    /// error, mirroring the zip fixture above.
    #[test]
    fn genuinely_over_cap_gzip_stream_is_capped_mid_read() {
        use flate2::Compression;
        use flate2::write::GzEncoder;

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        let payload = vec![0u8; 10 * 1024 * 1024]; // 10 MB of zeros.
        std::io::Write::write_all(&mut encoder, &payload).expect("write gz payload");
        let gz_bytes = encoder.finish().expect("finish gzip");

        let decoder = flate2::read::GzDecoder::new(Cursor::new(gz_bytes));
        let cap = 1024 * 1024u64; // 1 MB cap, 10 MB actual inflate.
        let err = read_capped(decoder, cap).unwrap_err();
        assert!(matches!(err, CapReadError::CapExceeded { cap: c } if c == cap));
    }

    /// A normal, well-under-cap gzip stream still round-trips byte-identical
    /// through `read_capped` — the cap must never perturb legitimate output.
    #[test]
    fn normal_gzip_stream_round_trips_under_cap() {
        use flate2::Compression;
        use flate2::write::GzEncoder;

        let original = b"the quick brown fox jumps over the lazy dog".repeat(100);
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        std::io::Write::write_all(&mut encoder, &original).expect("write payload");
        let gz_bytes = encoder.finish().expect("finish gzip");

        let decoder = flate2::read::GzDecoder::new(Cursor::new(gz_bytes));
        let out = read_capped(decoder, 10 * 1024 * 1024).expect("well under cap");
        assert_eq!(out, original);
    }
}
