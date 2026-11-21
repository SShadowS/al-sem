//! One package's IR cache entry (spec Design 3): a header, an index of `(offset, len, stored)`
//! per file in `SourceRoot.files` order, the payload (each stored file's encoding), and a
//! trailing blake3 over every byte before it. Little-endian throughout.
//!
//! Header: magic `ALSEMIR\0` (8), format `u32`, key (32), text digest (32), the grammar commit
//! and the binary's version (each a `u16` length and UTF-8; diagnosis only, never compared),
//! file count `u64`, payload length `u64`.

use super::key::IrKey;
use crate::snapshot::cache::TextDigest;
use anyhow::{Context, Result, bail, ensure};
use std::io::Write;

/// The largest entry written or read (spec Design 8).
pub(crate) const MAX_ENTRY: u64 = 1 << 30;
/// The largest one file's encoding stored or read (spec "Kept: integrity checks").
pub(crate) const MAX_FILE: u64 = 64 << 20;

const MAGIC: &[u8; 8] = b"ALSEMIR\0";
/// One index record: offset, length, stored flag.
const RECORD_LEN: usize = 8 + 8 + 1;

/// One file's place in an entry: its encoding, or not stored (its round trip failed or it
/// passed [`MAX_FILE`]; a hit parses it).
pub(crate) enum Slot {
    Stored(Vec<u8>),
    NotStored,
}

/// The header's diagnosis fields, never compared.
pub(crate) struct Diag<'a> {
    pub grammar_commit: &'a str,
    pub version: &'a str,
}

/// A writer that hashes what it passes on.
struct Hashing<'w> {
    out: &'w mut dyn Write,
    hash: blake3::Hasher,
}

impl Write for Hashing<'_> {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        let n = self.out.write(b)?;
        self.hash.update(&b[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }
}

/// Write one whole entry for `files` (in `SourceRoot.files` order) to `out`. Refuses an entry
/// over [`MAX_ENTRY`] or a stored file over [`MAX_FILE`]: never publish what every read
/// refuses.
pub(crate) fn write_envelope(
    out: &mut dyn Write,
    key: &IrKey,
    digest: &TextDigest,
    diag: &Diag<'_>,
    files: &[Slot],
) -> Result<()> {
    let short = |s: &str| u16::try_from(s.len()).context("diagnosis field over 64 KiB");
    let (g, v) = (short(diag.grammar_commit)?, short(diag.version)?);
    let mut payload_len = 0u64;
    for f in files {
        if let Slot::Stored(b) = f {
            ensure!(
                b.len() as u64 <= MAX_FILE,
                "a file's IR over {MAX_FILE} bytes"
            );
            payload_len += b.len() as u64;
        }
    }
    let total = header_len(g as usize, v as usize) as u64
        + (files.len() * RECORD_LEN) as u64
        + payload_len
        + 32;
    ensure!(total <= MAX_ENTRY, "entry over {MAX_ENTRY} bytes");
    let mut w = Hashing {
        out,
        hash: blake3::Hasher::new(),
    };
    w.write_all(MAGIC)?;
    w.write_all(&super::IR_FORMAT.to_le_bytes())?;
    w.write_all(&key.0)?;
    w.write_all(digest)?;
    for (len, s) in [(g, diag.grammar_commit), (v, diag.version)] {
        w.write_all(&len.to_le_bytes())?;
        w.write_all(s.as_bytes())?;
    }
    w.write_all(&(files.len() as u64).to_le_bytes())?;
    w.write_all(&payload_len.to_le_bytes())?;
    let mut offset = 0u64;
    for f in files {
        let (len, stored) = match f {
            Slot::Stored(b) => (b.len() as u64, 1u8),
            Slot::NotStored => (0, 0),
        };
        w.write_all(&offset.to_le_bytes())?;
        w.write_all(&len.to_le_bytes())?;
        w.write_all(&[stored])?;
        offset += len;
    }
    for f in files {
        if let Slot::Stored(b) = f {
            w.write_all(b)?;
        }
    }
    let hash = w.hash.finalize();
    w.out.write_all(hash.as_bytes())?;
    w.out.flush()?;
    Ok(())
}

/// The fixed header's length with diagnosis fields of `g` and `v` bytes.
fn header_len(g: usize, v: usize) -> usize {
    8 + 4 + 32 + 32 + 2 + g + 2 + v + 8 + 8
}

/// Check entry `bytes` for `key`, `digest` and a package of `file_count` files, in the spec's
/// order (Design 3): size, magic, format, key, digest, count, exact total length, the trailing
/// blake3, then index ranges that tile the payload exactly (a not-stored record has length 0;
/// every sum checked), each stored range within [`MAX_FILE`]. Returns each file's exact
/// encoding (`None`: not stored).
pub(crate) fn open_envelope<'b>(
    bytes: &'b [u8],
    key: &IrKey,
    digest: &TextDigest,
    file_count: usize,
) -> Result<Vec<Option<&'b [u8]>>> {
    ensure!(
        bytes.len() as u64 <= MAX_ENTRY,
        "entry over {MAX_ENTRY} bytes"
    );
    let mut at = 0usize;
    let mut take = |n: usize| -> Result<&'b [u8]> {
        let s = bytes
            .get(at..at.checked_add(n).context("length overflows")?)
            .context("entry shorter than its header")?;
        at += n;
        Ok(s)
    };
    let u16_of = |s: &[u8]| u16::from_le_bytes(s.try_into().expect("2 bytes"));
    let u64_of = |s: &[u8]| u64::from_le_bytes(s.try_into().expect("8 bytes"));
    ensure!(take(8)? == MAGIC, "not an IR cache entry (magic)");
    ensure!(
        take(4)? == super::IR_FORMAT.to_le_bytes(),
        "IR cache format differs"
    );
    ensure!(take(32)? == key.0, "IR cache key differs");
    ensure!(take(32)? == digest, "text digest differs");
    for _ in 0..2 {
        let n = u16_of(take(2)?);
        take(usize::from(n))?;
    }
    let count = u64_of(take(8)?);
    ensure!(
        count == file_count as u64,
        "entry holds {count} files, the package {file_count}"
    );
    let payload_len = u64_of(take(8)?);
    let index_start = at;
    let index_len = file_count
        .checked_mul(RECORD_LEN)
        .context("index length overflows")?;
    let payload_start = index_start
        .checked_add(index_len)
        .context("index length overflows")?;
    let want = usize::try_from(payload_len)
        .ok()
        .and_then(|p| payload_start.checked_add(p))
        .and_then(|e| e.checked_add(32));
    ensure!(
        want == Some(bytes.len()),
        "entry length is not header + index + payload + hash"
    );
    let body_end = bytes.len() - 32;
    ensure!(
        blake3::hash(&bytes[..body_end]).as_bytes() == &bytes[body_end..],
        "IR cache entry hash differs"
    );
    let payload = &bytes[payload_start..body_end];
    let mut next = 0u64;
    let mut out = Vec::with_capacity(file_count);
    for r in bytes[index_start..payload_start].chunks_exact(RECORD_LEN) {
        let (offset, len, stored) = (u64_of(&r[..8]), u64_of(&r[8..16]), r[16]);
        if offset != next {
            bail!("index does not tile the payload");
        }
        match stored {
            0 if len == 0 => out.push(None),
            1 => {
                ensure!(len <= MAX_FILE, "a file's IR over {MAX_FILE} bytes");
                next = match next.checked_add(len) {
                    Some(n) if n <= payload_len => n,
                    _ => bail!("index range past the payload"),
                };
                out.push(Some(&payload[offset as usize..next as usize]));
            }
            _ => bail!("bad index record"),
        }
    }
    ensure!(next == payload_len, "index does not cover the payload");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: IrKey = IrKey([1; 32]);
    const DIGEST: TextDigest = [2; 32];
    const DIAG: Diag<'static> = Diag {
        grammar_commit: "88bb6871",
        version: "0.0.0-test",
    };

    fn files() -> Vec<Slot> {
        vec![
            Slot::Stored(b"first".to_vec()),
            Slot::NotStored,
            Slot::Stored(b"second file".to_vec()),
            Slot::Stored(b"3".to_vec()),
        ]
    }

    fn entry(files: &[Slot]) -> Vec<u8> {
        let mut out = Vec::new();
        write_envelope(&mut out, &KEY, &DIGEST, &DIAG, files).unwrap();
        out
    }

    fn open(bytes: &[u8]) -> Result<Vec<Option<&[u8]>>> {
        open_envelope(bytes, &KEY, &DIGEST, 4)
    }

    /// Where the index starts in an entry written with [`DIAG`].
    fn index_at() -> usize {
        header_len(DIAG.grammar_commit.len(), DIAG.version.len())
    }

    /// Recompute the trailing hash, so only the later checks see a change.
    fn rehash(b: &mut [u8]) {
        let end = b.len() - 32;
        let h = blake3::hash(&b[..end]);
        b[end..].copy_from_slice(h.as_bytes());
    }

    fn set_u64(b: &mut [u8], at: usize, v: u64) {
        b[at..at + 8].copy_from_slice(&v.to_le_bytes());
    }

    /// Record `i`'s offset field position.
    fn rec(i: usize) -> usize {
        index_at() + i * RECORD_LEN
    }

    fn refused(b: &[u8], why: &str) {
        let e = open(b).expect_err(why);
        eprintln!("{why}: {e:#}");
    }

    #[test]
    fn each_file_gets_exactly_its_range() {
        let e = entry(&files());
        let got = open(&e).unwrap();
        assert_eq!(
            got,
            [
                Some(&b"first"[..]),
                None,
                Some(&b"second file"[..]),
                Some(&b"3"[..])
            ]
        );
        let payload: Vec<u8> = got
            .iter()
            .flatten()
            .flat_map(|s| s.iter().copied())
            .collect();
        assert_eq!(payload, b"firstsecond file3");
    }

    #[test]
    fn header_refusals() {
        let good = entry(&files());
        let mut b = good.clone();
        b[0] = b'X';
        refused(&b, "magic");
        let mut b = good.clone();
        b[8] ^= 1;
        refused(&b, "format");
        assert!(
            open_envelope(&good, &IrKey([9; 32]), &DIGEST, 4).is_err(),
            "key"
        );
        assert!(open_envelope(&good, &KEY, &[9; 32], 4).is_err(), "digest");
        assert!(open_envelope(&good, &KEY, &DIGEST, 3).is_err(), "count");
        assert!(open_envelope(&good, &KEY, &DIGEST, 5).is_err(), "count");
        // Fable r1: the header's count wrong, everything else (index, lengths, hash) right
        // for the package's 4 files: only the count check sees it.
        let mut b = good.clone();
        set_u64(&mut b, index_at() - 16, 5);
        rehash(&mut b);
        refused(&b, "header count differs, all else consistent");
        assert!(open(&good).is_ok(), "precondition: the good entry opens");
    }

    #[test]
    fn length_and_hash_refusals() {
        let good = entry(&files());
        refused(&[good.clone(), vec![0]].concat(), "one byte past the end");
        refused(&good[..good.len() - 1], "one byte short");
        // The header's payload length one past the bytes, the hash recomputed: only the
        // exact-length check stands between it and a range past the payload.
        let mut b = good.clone();
        let pl = index_at() - 8;
        let n = u64::from_le_bytes(b[pl..pl + 8].try_into().unwrap());
        set_u64(&mut b, pl, n + 1);
        set_u64(&mut b, rec(3) + 8, 2);
        rehash(&mut b);
        refused(&b, "payload length past the bytes, hash kept");
        let mut b = good.clone();
        let at = b.len() - 32 - 3;
        b[at] ^= 1;
        refused(&b, "flipped payload byte");
        let mut b = good.clone();
        b[rec(0) + 8] ^= 1;
        refused(&b, "flipped index byte, hash kept");
    }

    /// Index damage with the hash recomputed: only the tiling check sees it.
    #[test]
    fn tiling_refusals() {
        let good = entry(&files());
        // Gap: record 2 starts one past where record 0 ended.
        let mut b = good.clone();
        set_u64(&mut b, rec(2), 6);
        rehash(&mut b);
        refused(&b, "gap");
        // Overlap: record 2 starts inside record 0.
        let mut b = good.clone();
        set_u64(&mut b, rec(2), 4);
        rehash(&mut b);
        refused(&b, "overlap");
        // The first range not at 0.
        let mut b = good.clone();
        set_u64(&mut b, rec(0), 1);
        rehash(&mut b);
        refused(&b, "first range not at 0");
        // The last range short of the end.
        let mut b = good.clone();
        set_u64(&mut b, rec(3) + 8, 0);
        rehash(&mut b);
        refused(&b, "last range short");
        // A not-stored record with a length.
        let mut b = good.clone();
        set_u64(&mut b, rec(1) + 8, 1);
        rehash(&mut b);
        refused(&b, "not stored with a length");
        // Records 0 and 2 swapped.
        let mut b = good.clone();
        let (r0, r2) = (b[rec(0)..rec(1)].to_vec(), b[rec(2)..rec(3)].to_vec());
        b[rec(0)..rec(1)].copy_from_slice(&r2);
        b[rec(2)..rec(3)].copy_from_slice(&r0);
        rehash(&mut b);
        refused(&b, "two records swapped");
    }

    /// A stored range over [`MAX_FILE`] is refused on read even when it tiles, and never
    /// written.
    #[test]
    fn a_file_over_the_cap_is_refused() {
        let big = vec![7u8; MAX_FILE as usize + 1];
        let mut out = Vec::new();
        assert!(
            write_envelope(&mut out, &KEY, &DIGEST, &DIAG, &[Slot::Stored(big.clone())]).is_err()
        );
        // Hand-built: one record of MAX_FILE + 1 bytes, tiling the payload exactly.
        let fits = vec![Slot::Stored(vec![7u8; MAX_FILE as usize])];
        let mut b = entry(&fits);
        b.insert(b.len() - 32, 7);
        set_u64(&mut b, rec(0) + 8, MAX_FILE + 1);
        let pl = index_at() - 8;
        set_u64(&mut b, pl, MAX_FILE + 1);
        rehash(&mut b);
        let e = open_envelope(&b, &KEY, &DIGEST, 1).expect_err("over the cap");
        assert!(e.to_string().contains("over"), "{e:#}");
    }
}
