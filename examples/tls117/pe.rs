//! TLS-117 design B (design note section 3): the canonical hash of a PE32+ image's read-only
//! content, the same whether it is read from the loaded module in memory or from a file on disk.
//!
//! - Headers: read by `e_lfanew` and `SizeOfHeaders`, hashed whole with
//!   `OptionalHeader.ImageBase` zeroed (the loader may rewrite it with the load address). A
//!   candidate file's headers are compared with the module's before any section work, so every
//!   size used afterwards is the running image's own ([`check_candidate`]).
//! - Every section without `IMAGE_SCN_MEM_WRITE` is hashed over its virtual size (file bytes past
//!   `SizeOfRawData` count as zero, as the loader maps them).
//! - Each `IMAGE_REL_BASED_DIR64` base-relocation site in a hashed section is normalised to
//!   `value - base` (base: the module's load address in memory, `ImageBase` for a file). Any other
//!   type except the `IMAGE_REL_BASED_ABSOLUTE` padding refuses.
//! - The ranges the loader writes are zeroed: the IAT and every import descriptor's `FirstThunk`
//!   array, the delay-load module handle and IAT, the TLS index, and the load-config pointer
//!   targets the loader fills. A mask never covers an executable section, and the metadata a mask
//!   comes from must itself lie in hashed bytes. Unmodelled loader patching refuses: an import
//!   without a separate lookup table, dynamic value relocations, CHPE (ARM64EC/ARM64X) metadata,
//!   hot-patch tables, non-zero load-config fields this code does not know.
//!
//! Writable sections (`.data`, `.tls`, ...) are not hashed: by the time the engine looks, the C
//! runtime, Rust's runtime and the program itself have written them. Every value read from an image
//! is bounded and every offset computed with checked arithmetic: a malformed image is a refusal
//! (no key), never a panic.

/// A source of image bytes by RVA, laid out as the loader maps them.
pub trait View {
    /// `len` bytes at `rva`, or `None` when the range cannot be read.
    fn read(&self, rva: u32, len: usize) -> Option<Vec<u8>>;
}

/// A loaded image as a byte slice (a test's simulated load).
#[cfg(test)]
pub struct SliceView<'a>(pub &'a [u8]);

#[cfg(test)]
impl View for SliceView<'_> {
    fn read(&self, rva: u32, len: usize) -> Option<Vec<u8>> {
        let start = rva as usize;
        self.0
            .get(start..start.checked_add(len)?)
            .map(<[u8]>::to_vec)
    }
}

/// The largest image, header block and load-config this code reads (anything bigger refuses).
pub const MAX_IMAGE: u32 = 1 << 30;
const MAX_HEADERS: u32 = 1 << 20;
const MAX_LOAD_CONFIG: usize = 0x1000;
/// The load-config fields this code knows end here; non-zero bytes past it refuse.
const KNOWN_LOAD_CONFIG: usize = 0x140;

const IMAGE_SCN_CNT_CODE: u32 = 0x0000_0020;
const IMAGE_SCN_MEM_EXECUTE: u32 = 0x2000_0000;
const IMAGE_SCN_MEM_WRITE: u32 = 0x8000_0000;
const IMAGE_REL_BASED_ABSOLUTE: u16 = 0;
const IMAGE_REL_BASED_DIR64: u16 = 10;
const IMAGE_FILE_MACHINE_AMD64: u16 = 0x8664;

/// A PE file on disk, read as the loader would map it: headers at RVA 0, each section at its
/// virtual address, zero past its raw data.
pub struct FileView<'a> {
    file: &'a [u8],
    headers_len: u32,
    sections: Vec<Section>,
}

impl<'a> FileView<'a> {
    pub fn new(file: &'a [u8]) -> Result<FileView<'a>, String> {
        let h = Headers::parse(file)?;
        Ok(FileView {
            file,
            headers_len: h.size_of_headers,
            sections: h.sections,
        })
    }
}

impl View for FileView<'_> {
    fn read(&self, rva: u32, len: usize) -> Option<Vec<u8>> {
        let end = (rva as usize).checked_add(len)?;
        if end <= self.headers_len as usize {
            return self.file.get(rva as usize..end).map(<[u8]>::to_vec);
        }
        let s = self
            .sections
            .iter()
            .find(|s| rva >= s.va && end as u64 <= s.end())?;
        let mut out = vec![0u8; len];
        let off = (rva - s.va) as usize;
        let raw = s.raw_size as usize;
        if off < raw {
            let n = (raw - off).min(len);
            let src = (s.raw_ptr as usize).checked_add(off)?;
            out[..n].copy_from_slice(self.file.get(src..src.checked_add(n)?)?);
        }
        Some(out)
    }
}

#[derive(Clone, Debug)]
pub struct Section {
    pub name: [u8; 8],
    pub vsize: u32,
    pub va: u32,
    pub raw_size: u32,
    pub raw_ptr: u32,
    pub characteristics: u32,
}

impl Section {
    /// The length the loader maps (the virtual size, or the raw size when it is 0).
    pub fn mapped_len(&self) -> u32 {
        if self.vsize != 0 {
            self.vsize
        } else {
            self.raw_size
        }
    }
    /// One past the last mapped RVA (checked against `SizeOfImage` in [`Headers::parse`]).
    fn end(&self) -> u64 {
        u64::from(self.va) + u64::from(self.mapped_len())
    }
    pub fn writable(&self) -> bool {
        self.characteristics & IMAGE_SCN_MEM_WRITE != 0
    }
    fn executable(&self) -> bool {
        self.characteristics & (IMAGE_SCN_MEM_EXECUTE | IMAGE_SCN_CNT_CODE) != 0
    }
    pub fn named(&self, n: &str) -> bool {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(8);
        &self.name[..end] == n.as_bytes()
    }
}

#[derive(Debug)]
pub struct Headers {
    /// Offset of `OptionalHeader.ImageBase` (zeroed before hashing and comparing).
    image_base_at: usize,
    /// Offset of `OptionalHeader.CheckSum`.
    checksum_at: usize,
    pub image_base: u64,
    pub time_date_stamp: u32,
    pub size_of_image: u32,
    pub size_of_headers: u32,
    pub dirs: Vec<(u32, u32)>,
    pub sections: Vec<Section>,
}

fn u16_at(b: &[u8], o: usize) -> Result<u16, String> {
    o.checked_add(2)
        .and_then(|e| b.get(o..e))
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or_else(|| format!("truncated at {o:#x}"))
}
fn u32_at(b: &[u8], o: usize) -> Result<u32, String> {
    o.checked_add(4)
        .and_then(|e| b.get(o..e))
        .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
        .ok_or_else(|| format!("truncated at {o:#x}"))
}
fn u64_at(b: &[u8], o: usize) -> Result<u64, String> {
    o.checked_add(8)
        .and_then(|e| b.get(o..e))
        .map(|s| u64::from_le_bytes(s.try_into().unwrap()))
        .ok_or_else(|| format!("truncated at {o:#x}"))
}

impl Headers {
    /// Parse the headers at the start of `b` (a file, or the header block of a loaded image).
    pub fn parse(b: &[u8]) -> Result<Headers, String> {
        if b.get(..2) != Some(b"MZ") {
            return Err("no MZ header".into());
        }
        let pe = u32_at(b, 0x3c)? as usize;
        if pe.checked_add(4).and_then(|e| b.get(pe..e)) != Some(b"PE\0\0") {
            return Err("no PE signature".into());
        }
        let coff = pe + 4;
        let machine = u16_at(b, coff)?;
        if machine != IMAGE_FILE_MACHINE_AMD64 {
            // ARM64 (0xAA64) and every other machine get no key.
            return Err(format!("machine {machine:#x} is not x64: no key"));
        }
        let nsec = u16_at(b, coff + 2)? as usize;
        let time_date_stamp = u32_at(b, coff + 4)?;
        let opt_size = u16_at(b, coff + 16)? as usize;
        let opt = coff + 20;
        if u16_at(b, opt)? != 0x20b {
            return Err("not PE32+".into());
        }
        let ndirs = (u32_at(b, opt + 108)? as usize).min(16);
        if 112 + ndirs * 8 > opt_size {
            return Err("optional header too small for its directories".into());
        }
        let dirs = (0..ndirs)
            .map(|i| Ok((u32_at(b, opt + 112 + 8 * i)?, u32_at(b, opt + 116 + 8 * i)?)))
            .collect::<Result<Vec<_>, String>>()?;
        let size_of_image = u32_at(b, opt + 56)?;
        let size_of_headers = u32_at(b, opt + 60)?;
        if size_of_image > MAX_IMAGE || size_of_headers > MAX_HEADERS {
            return Err("image or headers too large".into());
        }
        let st = opt + opt_size;
        if (size_of_headers as usize) < st + 40 * nsec || size_of_headers > size_of_image {
            return Err("SizeOfHeaders does not fit the section table and the image".into());
        }
        let sections = (0..nsec)
            .map(|i| {
                let s = st + 40 * i;
                let sec = Section {
                    name: b
                        .get(s..s + 8)
                        .ok_or("truncated section table")?
                        .try_into()
                        .unwrap(),
                    vsize: u32_at(b, s + 8)?,
                    va: u32_at(b, s + 12)?,
                    raw_size: u32_at(b, s + 16)?,
                    raw_ptr: u32_at(b, s + 20)?,
                    characteristics: u32_at(b, s + 36)?,
                };
                if sec.va < size_of_headers || sec.end() > u64::from(size_of_image) {
                    return Err(format!("section {i} outside the image"));
                }
                Ok(sec)
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Headers {
            image_base_at: opt + 24,
            checksum_at: opt + 64,
            image_base: u64_at(b, opt + 24)?,
            time_date_stamp,
            size_of_image,
            size_of_headers,
            dirs,
            sections,
        })
    }

    pub fn dir(&self, i: usize) -> (u32, u32) {
        self.dirs.get(i).copied().unwrap_or((0, 0))
    }

    /// Whether `[rva, rva + len)` lies in the headers or in one non-writable section (bytes the
    /// hash covers), so metadata read from there cannot differ unseen.
    fn in_hashed(&self, rva: u32, len: u64) -> bool {
        let end = u64::from(rva) + len;
        end <= u64::from(self.size_of_headers)
            || self
                .sections
                .iter()
                .any(|s| !s.writable() && rva >= s.va && end <= s.end())
    }
}

/// The header block of `v`, found by `e_lfanew` and `SizeOfHeaders`, not a fixed size.
pub fn read_headers(v: &dyn View) -> Result<Vec<u8>, String> {
    let dos = v.read(0, 0x40).ok_or("no DOS header")?;
    let pe = u32_at(&dos, 0x3c)?;
    let fixed = pe.checked_add(24).ok_or("bad e_lfanew")?;
    if fixed > MAX_HEADERS {
        return Err("e_lfanew too large".into());
    }
    let upto = v.read(0, fixed as usize).ok_or("no COFF header")?;
    let opt_size = u32::from(u16_at(&upto, pe as usize + 20)?);
    let opt_end = fixed + opt_size;
    let upto = v.read(0, opt_end as usize).ok_or("no optional header")?;
    let soh = u32_at(&upto, fixed as usize + 60)?;
    if soh > MAX_HEADERS || soh < opt_end {
        return Err("bad SizeOfHeaders".into());
    }
    v.read(0, soh as usize)
        .ok_or_else(|| "headers out of range".into())
}

/// The result: the hash, the headers it covered (ImageBase zeroed), and what it covered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Canon {
    pub hash: [u8; 32],
    pub headers: Vec<u8>,
    pub sections: usize,
    pub reloc_sites: usize,
    pub zeroed: Vec<(u32, u32)>,
}

/// The DIR64 base-relocation sites (RVAs). Any type but DIR64 and ABSOLUTE refuses.
pub fn reloc_sites(v: &dyn View, h: &Headers) -> Result<Vec<u32>, String> {
    let (rva, size) = h.dir(5);
    if size == 0 {
        return Ok(Vec::new());
    }
    if size > h.size_of_image {
        return Err("relocation table larger than the image".into());
    }
    let t = v
        .read(rva, size as usize)
        .ok_or("relocation table out of range")?;
    let mut sites = Vec::new();
    let mut o = 0usize;
    while o + 8 <= t.len() {
        let page = u32_at(&t, o)?;
        let bsize = u32_at(&t, o + 4)? as usize;
        if bsize < 8 || !bsize.is_multiple_of(2) || o + bsize > t.len() {
            return Err(format!("bad relocation block at {o:#x} (size {bsize})"));
        }
        for e in (o + 8..o + bsize).step_by(2) {
            let ent = u16_at(&t, e)?;
            match ent >> 12 {
                IMAGE_REL_BASED_ABSOLUTE => {}
                IMAGE_REL_BASED_DIR64 => sites.push(
                    page.checked_add(u32::from(ent & 0xfff))
                        .ok_or("relocation site overflows")?,
                ),
                other => return Err(format!("base relocation type {other} is not handled")),
            }
        }
        o += bsize;
    }
    Ok(sites)
}

/// The ranges (RVA, length) the loader writes, as far as this code models them; refuses what it
/// does not model.
pub fn loader_written(v: &dyn View, h: &Headers, base: u64) -> Result<Vec<(u32, u32)>, String> {
    let mut z: Vec<(u32, u32)> = Vec::new();
    let va_to_rva = |va: u64| -> Result<u32, String> {
        u32::try_from(va.wrapping_sub(base))
            .ok()
            .filter(|&r| r < h.size_of_image)
            .ok_or_else(|| format!("address {va:#x} outside the image"))
    };
    // Every metadata range a mask is derived from: it must be in hashed bytes, and (checked at
    // the end, once every mask is known) no mask may zero any of it.
    let meta: std::cell::RefCell<Vec<(u32, u64, String)>> = Default::default();
    let hashed = |rva: u32, len: u64, what: &str| -> Result<(), String> {
        meta.borrow_mut().push((rva, len, what.to_string()));
        if h.in_hashed(rva, len) {
            Ok(())
        } else {
            Err(format!("{what} at {rva:#x} is not in hashed bytes"))
        }
    };
    // A lookup table (in hashed bytes) of u64 entries ending in 0: its length in entries,
    // terminator included.
    let count = |rva: u32| -> Result<u32, String> {
        let mut n = 0u32;
        loop {
            let at = rva
                .checked_add(n.checked_mul(8).ok_or("lookup table too long")?)
                .ok_or("lookup table overflows")?;
            hashed(at, 8, "import lookup table")?;
            let e = v.read(at, 8).ok_or("lookup table out of range")?;
            n += 1;
            if u64::from_le_bytes(e.try_into().unwrap()) == 0 {
                return Ok(n);
            }
            if n > 1 << 20 {
                return Err("lookup table without an end".into());
            }
        }
    };
    let (iat, iat_size) = h.dir(12);
    if iat_size != 0 {
        z.push((iat, iat_size));
    }
    let (imp, imp_size) = h.dir(1);
    if imp_size != 0 {
        for i in 0u32.. {
            let at = imp
                .checked_add(i.checked_mul(20).ok_or("too many imports")?)
                .ok_or("import table overflows")?;
            hashed(at, 20, "import descriptor")?;
            let d = v.read(at, 20).ok_or("import descriptor out of range")?;
            if d.iter().all(|&b| b == 0) {
                break;
            }
            let (int, first) = (u32_at(&d, 0)?, u32_at(&d, 16)?);
            if int == 0 {
                return Err(
                    "an import without a separate lookup table (OriginalFirstThunk 0)".into(),
                );
            }
            let n = count(int)?;
            z.push((first, n.checked_mul(8).ok_or("import table too long")?));
        }
    }
    let (dl, dl_size) = h.dir(13);
    if dl_size != 0 {
        for i in 0u32.. {
            let at = dl
                .checked_add(i.checked_mul(32).ok_or("too many delay imports")?)
                .ok_or("delay table overflows")?;
            hashed(at, 32, "delay-load descriptor")?;
            let d = v.read(at, 32).ok_or("delay descriptor out of range")?;
            if d.iter().all(|&b| b == 0) {
                break;
            }
            let rva_based = u32_at(&d, 0)? & 1 != 0;
            let f = |o: usize| -> Result<u32, String> {
                let x = u32_at(&d, o)?;
                if rva_based || x == 0 {
                    Ok(x)
                } else {
                    va_to_rva(u64::from(x))
                }
            };
            let (handle, iat, int) = (f(8)?, f(12)?, f(16)?);
            if int == 0 {
                return Err("a delay import without a lookup table".into());
            }
            if handle != 0 {
                z.push((handle, 8));
            }
            if iat != 0 {
                z.push((iat, count(int)?.checked_mul(8).ok_or("delay IAT too long")?));
            }
        }
    }
    let (tls, tls_size) = h.dir(9);
    if tls_size != 0 {
        hashed(tls, 40, "TLS directory")?;
        let t = v.read(tls, 40).ok_or("TLS directory out of range")?;
        let index = u64_at(&t, 16)?;
        if index != 0 {
            z.push((va_to_rva(index)?, 4));
        }
    }
    let (lc, lc_size) = h.dir(10);
    if lc_size != 0 {
        hashed(lc, 4, "load config")?;
        let head = v.read(lc, 4).ok_or("load config out of range")?;
        let size = u32_at(&head, 0)? as usize;
        if size > MAX_LOAD_CONFIG {
            return Err("load config too large".into());
        }
        hashed(lc, size as u64, "load config")?;
        let c = v.read(lc, size).ok_or("load config out of range")?;
        let u64f = |o: usize| -> u64 {
            if o + 8 <= size {
                u64_at(&c, o).unwrap()
            } else {
                0
            }
        };
        let u32f = |o: usize| -> u32 {
            if o + 4 <= size {
                u32_at(&c, o).unwrap()
            } else {
                0
            }
        };
        let u16f = |o: usize| -> u16 {
            if o + 2 <= size {
                u16_at(&c, o).unwrap()
            } else {
                0
            }
        };
        // An ARM64EC/ARM64X image has an AMD64 header too; its CHPE metadata (0xC8) is what tells,
        // so a load config too short to hold that field refuses. (An image with no load config at
        // all cannot be ARM64EC, which requires one.)
        if size < 0xD0 {
            return Err("load config too short to show there is no CHPE metadata".into());
        }
        for (what, nonzero) in [
            ("DynamicValueRelocTable (0xC0)", u64f(0xC0) != 0),
            (
                "CHPEMetadataPointer (0xC8): an ARM64EC/ARM64X image",
                u64f(0xC8) != 0,
            ),
            ("DynamicValueRelocTableOffset (0xE0)", u32f(0xE0) != 0),
            ("DynamicValueRelocTableSection (0xE4)", u16f(0xE4) != 0),
            ("HotPatchTableOffset (0xF0)", u32f(0xF0) != 0),
        ] {
            if nonzero {
                return Err(format!(
                    "load config has {what}: loader patching not modelled"
                ));
            }
        }
        if c.get(KNOWN_LOAD_CONFIG.min(size)..)
            .is_some_and(|t| t.iter().any(|&b| b != 0))
        {
            return Err("load config has non-zero fields past 0x140: not modelled".into());
        }
        // SecurityCookie, GuardCF check/dispatch, GuardRF failure/verify-SP, GuardXFG check/
        // dispatch/table-dispatch, CastGuardOsDeterminedFailureMode, GuardMemcpy: pointers whose
        // targets the loader may fill.
        for o in [
            0x58, 0x70, 0x78, 0xD8, 0xE8, 0x118, 0x120, 0x128, 0x130, 0x138,
        ] {
            let p = u64f(o);
            if p != 0 {
                z.push((va_to_rva(p)?, 8));
            }
        }
    }
    for &(r, l) in &z {
        let end = u64::from(r) + u64::from(l);
        if end > u64::from(h.size_of_image) {
            return Err(format!(
                "loader-written range {r:#x}+{l:#x} outside the image"
            ));
        }
        if let Some(s) = h
            .sections
            .iter()
            .find(|s| s.executable() && u64::from(r) < s.end() && end > u64::from(s.va))
        {
            return Err(format!(
                "a loader-written range {r:#x}+{l:#x} falls in executable section {:?}",
                String::from_utf8_lossy(&s.name)
            ));
        }
    }
    // "In a hashed section" is not "hashed": metadata a mask zeroes would be unseen.
    for (rva, len, what) in meta.into_inner() {
        if let Some(&(r, l)) = masked_overlap(rva, len, &z) {
            return Err(format!(
                "a loader-written range {r:#x}+{l:#x} overlaps {what} at {rva:#x}"
            ));
        }
    }
    Ok(z)
}

/// The first mask in `z` that overlaps `[rva, rva + len)`.
pub fn masked_overlap(rva: u32, len: u64, z: &[(u32, u32)]) -> Option<&(u32, u32)> {
    let (a, b) = (u64::from(rva), u64::from(rva) + len);
    z.iter()
        .find(|&&(r, l)| a < u64::from(r) + u64::from(l) && u64::from(r) < b)
}

/// The canonical hash of the image `v` (whose header block is `hdr`), relative to `base`.
fn canonical_with(v: &dyn View, hdr: Vec<u8>, base: u64) -> Result<Canon, String> {
    let h = Headers::parse(&hdr)?;
    if let Some(s) = h.sections.iter().find(|s| s.writable() && s.executable()) {
        return Err(format!(
            "section {:?} is writable and executable",
            String::from_utf8_lossy(&s.name)
        ));
    }
    let sites = reloc_sites(v, &h)?;
    let zeroed = loader_written(v, &h, base)?;
    let (rr, rl) = h.dir(5);
    if rl != 0
        && (!h.in_hashed(rr, u64::from(rl)) || masked_overlap(rr, u64::from(rl), &zeroed).is_some())
    {
        return Err("the relocation table is not in hashed, unmasked bytes".into());
    }
    let mut hb = hdr;
    hb[h.image_base_at..h.image_base_at + 8].fill(0);
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"tls117-pe-canonical-2");
    hasher.update(&(hb.len() as u64).to_le_bytes());
    hasher.update(&hb);
    let (mut nsec, mut nsites) = (0, 0);
    for (i, s) in h.sections.iter().enumerate() {
        let len = s.mapped_len();
        if s.writable() || len == 0 {
            continue;
        }
        let mut buf = v
            .read(s.va, len as usize)
            .ok_or_else(|| format!("section {i} cannot be read"))?;
        let end = s.end();
        for &site in &sites {
            if site >= s.va && u64::from(site) + 8 <= end {
                let o = (site - s.va) as usize;
                let x = u64::from_le_bytes(buf[o..o + 8].try_into().unwrap());
                buf[o..o + 8].copy_from_slice(&x.wrapping_sub(base).to_le_bytes());
                nsites += 1;
            }
        }
        for &(zr, zl) in &zeroed {
            let (a, b) = (
                u64::from(zr.max(s.va)),
                (u64::from(zr) + u64::from(zl)).min(end),
            );
            if a < b {
                buf[(a - u64::from(s.va)) as usize..(b - u64::from(s.va)) as usize].fill(0);
            }
        }
        hasher.update(&(i as u64).to_le_bytes());
        hasher.update(&(buf.len() as u64).to_le_bytes());
        hasher.update(&buf);
        nsec += 1;
    }
    Ok(Canon {
        hash: *hasher.finalize().as_bytes(),
        headers: hb,
        sections: nsec,
        reloc_sites: nsites,
        zeroed,
    })
}

/// Part A (or a test's simulated load): the canonical hash of a loaded image relative to the
/// address it is loaded at.
pub fn canonical(v: &dyn View, base: u64) -> Result<Canon, String> {
    let hdr = read_headers(v)?;
    canonical_with(v, hdr, base)
}

/// The canonical hash of a PE file's bytes on its own (tests, W10).
pub fn canonical_file(file: &[u8]) -> Result<Canon, String> {
    let fv = FileView::new(file)?;
    let hdr = read_headers(&fv)?;
    let base = Headers::parse(&hdr)?.image_base;
    canonical_with(&fv, hdr, base)
}

/// Part B's verdict on a candidate file.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Accepted,
    HeadersDiffer,
    HashDiffers,
}

/// Part B: compare the candidate's headers (ImageBase zeroed) with the running module's first,
/// then its canonical hash with A's.
pub fn check_candidate(file: &[u8], a: &Canon) -> Result<Verdict, String> {
    let fv = FileView::new(file)?;
    let hdr = read_headers(&fv)?;
    let h = Headers::parse(&hdr)?;
    let mut hb = hdr.clone();
    hb[h.image_base_at..h.image_base_at + 8].fill(0);
    if hb != a.headers {
        return Ok(Verdict::HeadersDiffer);
    }
    let b = canonical_with(&fv, hdr, h.image_base)?;
    Ok(if b.hash == a.hash {
        Verdict::Accepted
    } else {
        Verdict::HashDiffers
    })
}

// ---------------------------------------------------------------------------------------------
// The data-digest slot (design note section 3, rev 6). The product and the probe share this code:
// `examples/tls117_stamp.rs` writes the slot, the probe stamps its copies with it, and the key
// checks it. The magic is never a literal here: the caller passes it (the probe reads it from its
// own slot), so stamped binaries carry it exactly once.
// ---------------------------------------------------------------------------------------------

/// The slot: a 16-byte magic, then the 32-byte data digest (zero until stamped).
pub const SLOT_LEN: usize = 48;

/// The data digest: blake3 over a domain tag and, for every writable section in table order, its
/// name, virtual address, virtual size, raw size and raw bytes (lengths as u32 LE); then, after a
/// separator, every range canonicalisation masks (the file's loader-written ranges, sorted by RVA
/// and length, duplicates dropped) as a record of the holding section's name (8 zero bytes for
/// the headers), the RVA, the length and the range's initial file bytes (zero past the raw data,
/// as the loader maps them). So the bytes the canonical hash cannot see, writable sections and
/// masked ranges alike (a delay-load IAT pointing at a linker thunk, say), are bound by the digest.
pub fn data_digest(file: &[u8]) -> Result<[u8; 32], String> {
    let h = Headers::parse(file)?;
    let mut d = blake3::Hasher::new();
    d.update(b"tls117-data-digest-2");
    let masked = {
        let fv = FileView::new(file)?;
        let mut z = loader_written(&fv, &h, h.image_base)?;
        z.sort_unstable();
        z.dedup();
        z.into_iter()
            .map(|(r, l)| {
                let name = h
                    .sections
                    .iter()
                    .find(|s| r >= s.va && u64::from(r) < s.end())
                    .map_or([0u8; 8], |s| s.name);
                let bytes = fv
                    .read(r, l as usize)
                    .ok_or_else(|| format!("masked range {r:#x}+{l:#x} cannot be read"))?;
                Ok((name, r, l, bytes))
            })
            .collect::<Result<Vec<_>, String>>()?
    };
    for s in h.sections.iter().filter(|s| s.writable()) {
        let start = s.raw_ptr as usize;
        let end = start
            .checked_add(s.raw_size as usize)
            .ok_or("section overflows")?;
        let raw = file
            .get(start..end)
            .ok_or("writable section past the file")?;
        d.update(&s.name);
        d.update(&s.va.to_le_bytes());
        d.update(&s.vsize.to_le_bytes());
        d.update(&s.raw_size.to_le_bytes());
        d.update(raw);
    }
    d.update(b"masked ranges");
    for (name, r, l, bytes) in masked {
        d.update(&name);
        d.update(&r.to_le_bytes());
        d.update(&l.to_le_bytes());
        d.update(&bytes);
    }
    Ok(*d.finalize().as_bytes())
}

/// The file offset of `magic`, which must occur exactly once.
pub fn find_unique(file: &[u8], magic: &[u8; 16]) -> Result<usize, String> {
    let mut hits = file
        .windows(16)
        .enumerate()
        .filter(|(_, w)| *w == magic)
        .map(|(i, _)| i);
    let first = hits.next().ok_or("no slot magic in the file")?;
    if hits.next().is_some() {
        return Err("the slot magic occurs more than once".into());
    }
    Ok(first)
}

/// The section and RVA holding file offset range `[off, off + len)`, which must lie in one
/// section's raw data.
fn locate(h: &Headers, off: usize, len: usize) -> Result<(&Section, u32), String> {
    let s = h
        .sections
        .iter()
        .find(|s| {
            off >= s.raw_ptr as usize
                && off + len <= s.raw_ptr as usize + s.raw_size.min(s.mapped_len()) as usize
        })
        .ok_or("the slot is not inside a section's mapped raw data")?;
    Ok((s, s.va + (off - s.raw_ptr as usize) as u32))
}

/// The PE checksum (the algorithm of `CheckSumMappedFile`): 16-bit one's-complement-style sum of
/// the file with the CheckSum field taken as zero, folded, plus the file length.
pub fn pe_checksum(file: &[u8]) -> Result<u32, String> {
    let at = Headers::parse(file)?.checksum_at;
    let mut sum: u64 = 0;
    for (i, c) in file.chunks(2).enumerate() {
        let o = 2 * i;
        if o == at || o == at + 2 {
            continue;
        }
        let w = u64::from(c[0]) | (u64::from(*c.get(1).unwrap_or(&0)) << 8);
        sum += w;
        sum = (sum & 0xffff) + (sum >> 16);
    }
    sum = (sum & 0xffff) + (sum >> 16);
    Ok((sum as u32).wrapping_add(file.len() as u32))
}

/// What [`stamp`] did.
#[derive(Debug)]
pub struct Stamped {
    pub slot_offset: usize,
    pub slot_rva: u32,
    pub digest: [u8; 32],
    /// `Some((old, new))` when the file carried a PE checksum (non-zero), recomputed; `None` when
    /// it was 0 (not used: Windows checks it only for drivers and boot images).
    pub checksum: Option<(u32, u32)>,
}

/// Stamp `file`: find the slot by `magic` (exactly once, in a non-writable, non-executable
/// section), write the data digest into it, fix a non-zero PE checksum, and check that nothing
/// but the 32 digest bytes and the checksum field changed.
pub fn stamp(file: &mut [u8], magic: &[u8; 16]) -> Result<Stamped, String> {
    let before = file.to_vec();
    let h = Headers::parse(file)?;
    let off = find_unique(file, magic)?;
    let (s, rva) = locate(&h, off, SLOT_LEN)?;
    if s.writable() || s.executable() {
        return Err("the slot is not in a read-only data section".into());
    }
    let z = loader_written(&FileView::new(file)?, &h, h.image_base)?;
    if masked_overlap(rva, SLOT_LEN as u64, &z).is_some() {
        return Err("the slot overlaps a loader-written range".into());
    }
    let digest = data_digest(file)?;
    file[off + 16..off + SLOT_LEN].copy_from_slice(&digest);
    let at = h.checksum_at;
    let old = u32_at(file, at)?;
    let checksum = if old != 0 {
        let new = pe_checksum(file)?;
        file[at..at + 4].copy_from_slice(&new.to_le_bytes());
        Some((old, new))
    } else {
        None
    };
    let changed = before
        .iter()
        .zip(file.iter())
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(i, _)| i)
        .find(|&i| !((off + 16..off + SLOT_LEN).contains(&i) || (at..at + 4).contains(&i)));
    if let Some(i) = changed {
        return Err(format!(
            "stamping changed byte {i:#x} outside the slot and checksum"
        ));
    }
    if data_digest(file)? != digest {
        return Err("stamping changed a writable section".into());
    }
    Ok(Stamped {
        slot_offset: off,
        slot_rva: rva,
        digest,
        checksum,
    })
}

/// The slot check at key time.
#[derive(Debug, PartialEq, Eq)]
pub enum SlotVerdict {
    Matches,
    /// The loaded slot's digest is all zero: an unstamped build.
    Unstamped,
    /// The candidate's writable sections do not hash to the loaded digest.
    DataDiffers,
    /// The candidate's own slot differs from the loaded one.
    SlotDiffers,
}

/// Check candidate `file` against the running module's slot (`loaded`, read from memory at
/// `slot_rva`, with the module's masks `zeroed`). The slot must lie in a non-writable section and
/// outside every mask, so part A's hash covers it.
pub fn check_slot(
    file: &[u8],
    slot_rva: u32,
    loaded: &[u8; SLOT_LEN],
    zeroed: &[(u32, u32)],
) -> Result<SlotVerdict, String> {
    let h = Headers::parse(file)?;
    let in_ro = h.sections.iter().any(|s| {
        !s.writable() && slot_rva >= s.va && u64::from(slot_rva) + SLOT_LEN as u64 <= s.end()
    });
    if !in_ro || masked_overlap(slot_rva, SLOT_LEN as u64, zeroed).is_some() {
        return Err("the slot is not in hashed, unmasked bytes".into());
    }
    if loaded[16..].iter().all(|&b| b == 0) {
        return Ok(SlotVerdict::Unstamped);
    }
    if data_digest(file)? != loaded[16..] {
        return Ok(SlotVerdict::DataDiffers);
    }
    let own = FileView::new(file)?
        .read(slot_rva, SLOT_LEN)
        .ok_or("the candidate's slot cannot be read")?;
    Ok(if own == loaded[..] {
        SlotVerdict::Matches
    } else {
        SlotVerdict::SlotDiffers
    })
}

/// The raw bytes of every writable section with raw data, in table order (W10's `.data`
/// comparison).
pub fn writable_bytes(file: &[u8]) -> Result<Vec<u8>, String> {
    let h = Headers::parse(file)?;
    let mut out = Vec::new();
    for s in h.sections.iter().filter(|s| s.writable()) {
        let start = s.raw_ptr as usize;
        let end = start
            .checked_add(s.raw_size.min(s.mapped_len()) as usize)
            .ok_or("section overflows")?;
        out.extend_from_slice(file.get(start..end).ok_or("section past the file")?);
    }
    Ok(out)
}

/// What W10 reports about a build: the COFF TimeDateStamp, whether a `/Brepro` (REPRO, type 16)
/// debug entry is present, and the CodeView (RSDS) PDB GUID and age.
#[derive(Debug)]
pub struct DebugInfo {
    pub time_date_stamp: u32,
    pub repro: bool,
    pub pdb_guid: Option<String>,
}

pub fn debug_info(file: &[u8]) -> Result<DebugInfo, String> {
    let h = Headers::parse(file)?;
    let fv = FileView::new(file)?;
    let (rva, size) = h.dir(6);
    let mut info = DebugInfo {
        time_date_stamp: h.time_date_stamp,
        repro: false,
        pdb_guid: None,
    };
    if size == 0 || size > MAX_HEADERS {
        return Ok(info);
    }
    let d = fv
        .read(rva, size as usize)
        .ok_or("debug directory out of range")?;
    for e in d.chunks_exact(28) {
        let (kind, len, ptr) = (
            u32_at(e, 12)?,
            u32_at(e, 16)? as usize,
            u32_at(e, 24)? as usize,
        );
        match kind {
            16 => info.repro = true,
            2 if len >= 24 => {
                let r = ptr
                    .checked_add(24)
                    .and_then(|end| file.get(ptr..end))
                    .ok_or("CodeView record past the file")?;
                if &r[..4] == b"RSDS" {
                    let g: String = r[4..20].iter().map(|b| format!("{b:02x}")).collect();
                    info.pdb_guid = Some(format!("{g} age {}", u32_at(r, 20)?));
                }
            }
            _ => {}
        }
    }
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &[u8] = include_bytes!("fixture.exe");

    /// What the loader does, for a test: map the file at `base`, apply every DIR64 relocation
    /// (in every section, as the loader does), rewrite `ImageBase`, and fill the loader-written
    /// ranges with bytes that change per load.
    fn load(file: &[u8], base: u64) -> Vec<u8> {
        let h = Headers::parse(file).unwrap();
        let fv = FileView::new(file).unwrap();
        let mut img = vec![0u8; h.size_of_image as usize];
        img[..h.size_of_headers as usize].copy_from_slice(&file[..h.size_of_headers as usize]);
        for s in &h.sections {
            let data = fv.read(s.va, s.mapped_len() as usize).unwrap();
            img[s.va as usize..s.va as usize + data.len()].copy_from_slice(&data);
        }
        let delta = base.wrapping_sub(h.image_base);
        for site in reloc_sites(&fv, &h).unwrap() {
            let o = site as usize;
            let x = u64::from_le_bytes(img[o..o + 8].try_into().unwrap());
            img[o..o + 8].copy_from_slice(&x.wrapping_add(delta).to_le_bytes());
        }
        img[h.image_base_at..h.image_base_at + 8].copy_from_slice(&base.to_le_bytes());
        for (r, l) in loader_written(&fv, &h, h.image_base).unwrap() {
            for (k, b) in img[r as usize..(r + l) as usize].iter_mut().enumerate() {
                *b = (base >> (8 * (k % 8))) as u8 ^ 0x5a;
            }
        }
        img
    }

    fn section<'a>(h: &'a Headers, name: &str) -> &'a Section {
        h.sections.iter().find(|s| s.named(name)).unwrap()
    }

    /// A file offset inside section `name` that no zeroed range covers.
    fn safe_offset(file: &[u8], name: &str) -> usize {
        let h = Headers::parse(file).unwrap();
        let z = canonical_file(file).unwrap().zeroed;
        let s = section(&h, name);
        (0..s.raw_size.min(s.mapped_len()))
            .rev()
            .find(|&o| {
                let rva = s.va + o;
                !z.iter().any(|&(r, l)| rva >= r && rva < r + l)
            })
            .map(|o| (s.raw_ptr + o) as usize)
            .unwrap()
    }

    #[test]
    fn fixture_has_what_the_tests_need() {
        let h = Headers::parse(FIXTURE).unwrap();
        let fv = FileView::new(FIXTURE).unwrap();
        // Read off `objdump -p` once (2026-11-23), not from the code under test.
        assert_eq!(
            reloc_sites(&fv, &h).unwrap(),
            vec![0x3040, 0x3048, 0x3050, 0x3058, 0x3060]
        );
        assert_eq!(h.dir(12), (0x6038, 0x10), "the IAT");
        let c = canonical_file(FIXTURE).unwrap();
        assert_eq!(c.reloc_sites, 5);
        assert!(c.zeroed.contains(&(0x6038, 0x10)), "{c:?}");
        assert!(
            !section(&h, ".idata").writable(),
            "IAT in a read-only section"
        );
        assert!(section(&h, ".data").writable());
    }

    /// Two loads at different bases (relocated, ImageBase rewritten, IAT filled) and the file
    /// itself canonicalise to one hash, and part B accepts the file against either load.
    #[test]
    fn relocated_loads_canonicalise_like_the_file() {
        let file = canonical_file(FIXTURE).unwrap();
        for base in [0x7ff6_1234_0000u64, 0x1_8000_0000, 0x140000000] {
            let img = load(FIXTURE, base);
            let mem = canonical(&SliceView(&img), base).unwrap();
            assert_eq!(mem.hash, file.hash, "load at {base:#x}");
            assert_eq!(check_candidate(FIXTURE, &mem), Ok(Verdict::Accepted));
        }
        assert_ne!(
            load(FIXTURE, 0x7ff6_1234_0000),
            load(FIXTURE, 0x1_8000_0000)
        );
    }

    #[test]
    fn the_wrong_base_changes_the_hash() {
        let img = load(FIXTURE, 0x7ff6_1234_0000);
        let right = canonical(&SliceView(&img), 0x7ff6_1234_0000).unwrap();
        let wrong = canonical(&SliceView(&img), 0x7ff6_1234_1000).unwrap();
        assert_ne!(right.hash, wrong.hash);
    }

    #[test]
    fn a_code_or_constant_change_is_caught() {
        let base = 0x7ff6_1234_0000;
        let a = canonical(&SliceView(&load(FIXTURE, base)), base).unwrap();
        for name in [".text", ".rdata", ".idata"] {
            let mut f = FIXTURE.to_vec();
            f[safe_offset(FIXTURE, name)] ^= 1;
            assert_eq!(check_candidate(&f, &a), Ok(Verdict::HashDiffers), "{name}");
        }
    }

    #[test]
    fn a_header_change_is_caught_before_the_sections() {
        let base = 0x1_8000_0000;
        let a = canonical(&SliceView(&load(FIXTURE, base)), base).unwrap();
        let h = Headers::parse(FIXTURE).unwrap();
        let mut f = FIXTURE.to_vec();
        let ts = u32_at(FIXTURE, 0x3c).unwrap() as usize + 8; // the COFF TimeDateStamp
        f[ts] ^= 1;
        assert_eq!(check_candidate(&f, &a), Ok(Verdict::HeadersDiffer));
        // ImageBase alone is not a header difference (the sections then decide).
        let mut g = FIXTURE.to_vec();
        g[h.image_base_at + 3] ^= 1;
        assert_ne!(check_candidate(&g, &a), Ok(Verdict::HeadersDiffer));
    }

    /// The stated scope: initialised mutable data (.data) is not covered.
    #[test]
    fn a_data_only_change_is_not_covered() {
        let mut f = FIXTURE.to_vec();
        f[section(&Headers::parse(FIXTURE).unwrap(), ".data").raw_ptr as usize] ^= 1;
        assert_eq!(
            canonical_file(&f).unwrap().hash,
            canonical_file(FIXTURE).unwrap().hash
        );
        assert_ne!(
            writable_bytes(&f).unwrap(),
            writable_bytes(FIXTURE).unwrap()
        );
    }

    #[test]
    fn an_unhandled_relocation_type_refuses() {
        let h = Headers::parse(FIXTURE).unwrap();
        let (rva, _) = h.dir(5);
        let s = h
            .sections
            .iter()
            .find(|s| rva >= s.va && u64::from(rva) < s.end())
            .unwrap();
        let mut f = FIXTURE.to_vec();
        let first = (s.raw_ptr + (rva - s.va) + 8) as usize;
        let e = u16::from_le_bytes([f[first], f[first + 1]]);
        assert_eq!(e >> 12, IMAGE_REL_BASED_DIR64, "precondition");
        f[first..first + 2].copy_from_slice(&((3u16 << 12) | (e & 0xfff)).to_le_bytes());
        let err = canonical_file(&f).unwrap_err();
        assert!(err.contains("type 3"), "{err}");
    }

    #[test]
    fn an_import_without_a_lookup_table_refuses() {
        let h = Headers::parse(FIXTURE).unwrap();
        let (imp, _) = h.dir(1);
        let s = h
            .sections
            .iter()
            .find(|s| imp >= s.va && u64::from(imp) < s.end())
            .unwrap();
        let mut f = FIXTURE.to_vec();
        let oft = (s.raw_ptr + (imp - s.va)) as usize;
        assert_ne!(u32_at(&f, oft).unwrap(), 0, "precondition: a lookup table");
        f[oft..oft + 4].fill(0);
        let err = canonical_file(&f).unwrap_err();
        assert!(err.contains("lookup table"), "{err}");
    }

    #[test]
    fn not_a_pe_or_truncated_refuses() {
        assert!(canonical_file(b"MZ but nothing else").is_err());
        assert!(canonical_file(&FIXTURE[..0x80]).is_err());
        // Cut inside .rdata's raw data (file offset 0x800).
        assert!(canonical_file(&FIXTURE[..0x810]).is_err());
    }

    const MAGIC: &[u8; 16] = b"TLS117-DATA-SLOT";
    const BASE: u64 = 0x7ff6_1234_0000;

    /// The slot as part A reads it from a simulated load of `file` at [`BASE`].
    fn loaded_slot(file: &[u8], slot_rva: u32) -> ([u8; SLOT_LEN], Canon) {
        let img = load(file, BASE);
        let a = canonical(&SliceView(&img), BASE).unwrap();
        let s = slot_rva as usize;
        (img[s..s + SLOT_LEN].try_into().unwrap(), a)
    }

    #[test]
    fn a_stamped_file_verifies() {
        let mut f = FIXTURE.to_vec();
        let st = stamp(&mut f, MAGIC).unwrap();
        // Read off the fixture once (2026-11-23): the slot is the start of .rdata.
        assert_eq!((st.slot_offset, st.slot_rva), (0x800, 0x3000));
        assert_ne!(st.digest, [0; 32]);
        let (slot, a) = loaded_slot(&f, st.slot_rva);
        assert_eq!(check_candidate(&f, &a), Ok(Verdict::Accepted));
        assert_eq!(
            check_slot(&f, st.slot_rva, &slot, &a.zeroed),
            Ok(SlotVerdict::Matches)
        );
        // The stamp changed .rdata and the checksum, so the unstamped file no longer matches
        // part A (here at the headers, through the checksum field).
        assert_eq!(check_candidate(FIXTURE, &a), Ok(Verdict::HeadersDiffer));
    }

    #[test]
    fn a_data_change_after_stamping_fails_the_slot() {
        let mut f = FIXTURE.to_vec();
        let st = stamp(&mut f, MAGIC).unwrap();
        let (slot, a) = loaded_slot(&f, st.slot_rva);
        let data = section(&Headers::parse(&f).unwrap(), ".data").raw_ptr as usize;
        f[data] ^= 1;
        // The canonical hash cannot see it; the slot does.
        assert_eq!(check_candidate(&f, &a), Ok(Verdict::Accepted));
        assert_eq!(
            check_slot(&f, st.slot_rva, &slot, &a.zeroed),
            Ok(SlotVerdict::DataDiffers)
        );
    }

    /// A masked range's initial file bytes are bound by the digest: an IAT entry changed after
    /// stamping passes the canonical comparison (the IAT is masked) but fails the slot.
    #[test]
    fn a_masked_iat_change_after_stamping_fails_the_slot() {
        let mut f = FIXTURE.to_vec();
        let st = stamp(&mut f, MAGIC).unwrap();
        let (slot, a) = loaded_slot(&f, st.slot_rva);
        let h = Headers::parse(&f).unwrap();
        let (iat, _) = h.dir(12);
        let s = section(&h, ".idata");
        assert_eq!(iat, 0x6038, "precondition: the IAT, read off objdump -p");
        let at = (s.raw_ptr + (iat - s.va)) as usize;
        f[at] ^= 1;
        assert_eq!(
            check_candidate(&f, &a),
            Ok(Verdict::Accepted),
            "masked from the hash"
        );
        assert_eq!(
            check_slot(&f, st.slot_rva, &slot, &a.zeroed),
            Ok(SlotVerdict::DataDiffers)
        );
    }

    #[test]
    fn an_unstamped_build_gives_no_key() {
        let (slot, a) = loaded_slot(FIXTURE, 0x3000);
        assert_eq!(&slot[..16], MAGIC, "precondition: the slot is at 0x3000");
        assert_eq!(
            check_slot(FIXTURE, 0x3000, &slot, &a.zeroed),
            Ok(SlotVerdict::Unstamped)
        );
    }

    #[test]
    fn stamping_refuses_a_missing_or_repeated_magic() {
        let mut f = FIXTURE.to_vec();
        assert!(stamp(&mut f, b"NOT-IN-THE-FILE!").is_err());
        let mut g = FIXTURE.to_vec();
        g[0x900..0x910].copy_from_slice(MAGIC); // a second copy, in .rdata padding
        assert!(stamp(&mut g, MAGIC).unwrap_err().contains("more than once"));
    }

    /// The independent check of the checksum code: on the unstamped fixture it reproduces the
    /// value the linker wrote (0xe3a3, read off `objdump -p`).
    #[test]
    fn pe_checksum_matches_the_linker() {
        assert_eq!(pe_checksum(FIXTURE).unwrap(), 0xe3a3);
        let mut f = FIXTURE.to_vec();
        let st = stamp(&mut f, MAGIC).unwrap();
        let (old, new) = st.checksum.unwrap();
        assert_eq!(old, 0xe3a3);
        assert_eq!(pe_checksum(&f).unwrap(), new);
    }

    /// A mask over mask-deriving metadata refuses: here the IAT directory is widened to cover
    /// the import descriptors.
    #[test]
    fn a_mask_over_import_metadata_refuses() {
        let h = Headers::parse(FIXTURE).unwrap();
        let (imp, _) = h.dir(1);
        let mut f = FIXTURE.to_vec();
        let iat_dir = h.image_base_at - 24 + 112 + 8 * 12;
        f[iat_dir..iat_dir + 4].copy_from_slice(&imp.to_le_bytes());
        f[iat_dir + 4..iat_dir + 8].copy_from_slice(&0x48u32.to_le_bytes());
        let err = canonical_file(&f).unwrap_err();
        assert!(err.contains("overlaps import descriptor"), "{err}");
    }

    #[test]
    fn debug_info_reads_a_build_without_a_debug_directory() {
        let d = debug_info(FIXTURE).unwrap();
        assert!(!d.repro && d.pdb_guid.is_none());
        assert_eq!(
            d.time_date_stamp,
            Headers::parse(FIXTURE).unwrap().time_date_stamp
        );
    }
}
