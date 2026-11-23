//! TLS-117 design B, part A and B (design note section 3): the canonical hash of a PE32+ image's
//! read-only content, the same whether it is read from the loaded module in memory or from a
//! file on disk.
//!
//! - Every section without `IMAGE_SCN_MEM_WRITE` is hashed over its virtual size (file bytes past
//!   `SizeOfRawData` count as zero, as the loader maps them).
//! - Each `IMAGE_REL_BASED_DIR64` base-relocation site in a hashed section is normalised to
//!   `value - base`, where `base` is the address the image is relative to: the module's load
//!   address in memory, `OptionalHeader.ImageBase` for a file. Any other relocation type except
//!   the `IMAGE_REL_BASED_ABSOLUTE` padding refuses (no key), never a guess.
//! - The ranges the loader writes are zeroed: the IAT (data directory 12) and every import
//!   descriptor's `FirstThunk` array, the delay-load module handle and IAT, the TLS index, and the
//!   load-config pointers the loader fills (security cookie, CFG/XFG/RF/memcpy guard pointers).
//!   An image with dynamic value relocations or CHPE metadata (loader patching this code does not
//!   model) refuses.
//! - The headers are hashed whole, with `OptionalHeader.ImageBase` zeroed (the loader may rewrite
//!   it with the load address).
//!
//! Writable sections (`.data`, `.tls`, ...) are not hashed: by the time the engine looks, the C
//! runtime, Rust's runtime and the program itself have written them.

/// A source of image bytes by RVA, laid out as the loader maps them.
pub trait View {
    /// `len` bytes at `rva`, or `None` when the range is outside the image.
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
            .find(|s| rva >= s.va && end <= s.va as usize + s.mapped_len() as usize)?;
        let mut out = vec![0u8; len];
        let off = (rva - s.va) as usize;
        let raw = s.raw_size as usize;
        if off < raw {
            let n = (raw - off).min(len);
            let src = (s.raw_ptr as usize).checked_add(off)?;
            out[..n].copy_from_slice(self.file.get(src..src + n)?);
        }
        Some(out)
    }
}

const IMAGE_SCN_MEM_WRITE: u32 = 0x8000_0000;
const IMAGE_REL_BASED_ABSOLUTE: u16 = 0;
const IMAGE_REL_BASED_DIR64: u16 = 10;

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
    fn mapped_len(&self) -> u32 {
        if self.vsize != 0 {
            self.vsize
        } else {
            self.raw_size
        }
    }
}

#[derive(Debug)]
pub struct Headers {
    /// Offset of `OptionalHeader.ImageBase` (zeroed before hashing).
    image_base_at: usize,
    pub image_base: u64,
    pub size_of_image: u32,
    pub size_of_headers: u32,
    pub dirs: Vec<(u32, u32)>,
    pub sections: Vec<Section>,
}

fn u16_at(b: &[u8], o: usize) -> Result<u16, String> {
    b.get(o..o + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or_else(|| format!("truncated at {o:#x}"))
}
fn u32_at(b: &[u8], o: usize) -> Result<u32, String> {
    b.get(o..o + 4)
        .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
        .ok_or_else(|| format!("truncated at {o:#x}"))
}
fn u64_at(b: &[u8], o: usize) -> Result<u64, String> {
    b.get(o..o + 8)
        .map(|s| u64::from_le_bytes(s.try_into().unwrap()))
        .ok_or_else(|| format!("truncated at {o:#x}"))
}

impl Headers {
    /// Parse the headers at the start of `b` (a file, or a loaded image's first page(s)).
    pub fn parse(b: &[u8]) -> Result<Headers, String> {
        if b.get(..2) != Some(b"MZ") {
            return Err("no MZ header".into());
        }
        let pe = u32_at(b, 0x3c)? as usize;
        if b.get(pe..pe + 4) != Some(b"PE\0\0") {
            return Err("no PE signature".into());
        }
        let coff = pe + 4;
        if u16_at(b, coff)? != 0x8664 {
            return Err("not an x64 image".into());
        }
        let nsec = u16_at(b, coff + 2)? as usize;
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
        let st = opt + opt_size;
        let sections = (0..nsec)
            .map(|i| {
                let s = st + 40 * i;
                Ok(Section {
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
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let size_of_headers = u32_at(b, opt + 60)?;
        if (size_of_headers as usize) < st + 40 * nsec {
            return Err("SizeOfHeaders does not cover the section table".into());
        }
        Ok(Headers {
            image_base_at: opt + 24,
            image_base: u64_at(b, opt + 24)?,
            size_of_image: u32_at(b, opt + 56)?,
            size_of_headers,
            dirs,
            sections,
        })
    }

    fn dir(&self, i: usize) -> (u32, u32) {
        self.dirs.get(i).copied().unwrap_or((0, 0))
    }
}

/// The result: the hash, and what it covered (for the probe's report and the tests).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Canon {
    pub hash: [u8; 32],
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
                IMAGE_REL_BASED_DIR64 => sites.push(page + u32::from(ent & 0xfff)),
                other => return Err(format!("base relocation type {other} is not handled")),
            }
        }
        o += bsize;
    }
    Ok(sites)
}

/// The ranges (RVA, length) the loader writes, as far as this code models them.
pub fn loader_written(v: &dyn View, h: &Headers, base: u64) -> Result<Vec<(u32, u32)>, String> {
    let mut z = Vec::new();
    let va_to_rva = |va: u64| -> Result<u32, String> {
        u32::try_from(va.wrapping_sub(base))
            .map_err(|_| format!("address {va:#x} outside the image"))
    };
    // A null-terminated array of u64 at `rva`: its length in entries, terminator included.
    let count = |rva: u32| -> Result<u32, String> {
        let mut n = 0u32;
        loop {
            let e = v.read(rva + 8 * n, 8).ok_or("thunk array out of range")?;
            n += 1;
            if u64::from_le_bytes(e.try_into().unwrap()) == 0 {
                return Ok(n);
            }
            if n > 1 << 20 {
                return Err("thunk array without an end".into());
            }
        }
    };
    let (iat, iat_size) = h.dir(12);
    if iat_size != 0 {
        z.push((iat, iat_size));
    }
    let (imp, imp_size) = h.dir(1);
    if imp_size != 0 {
        for i in 0.. {
            let d = v
                .read(imp + 20 * i, 20)
                .ok_or("import descriptor out of range")?;
            if d.iter().all(|&b| b == 0) {
                break;
            }
            let (int, first) = (u32_at(&d, 0)?, u32_at(&d, 16)?);
            let n = count(if int != 0 { int } else { first })?;
            z.push((first, 8 * n));
        }
    }
    let (dl, dl_size) = h.dir(13);
    if dl_size != 0 {
        for i in 0.. {
            let d = v
                .read(dl + 32 * i, 32)
                .ok_or("delay descriptor out of range")?;
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
            if handle != 0 {
                z.push((handle, 8));
            }
            if iat != 0 {
                z.push((iat, 8 * count(if int != 0 { int } else { iat })?));
            }
        }
    }
    let (tls, tls_size) = h.dir(9);
    if tls_size != 0 {
        let t = v.read(tls, 40).ok_or("TLS directory out of range")?;
        let index = u64_at(&t, 16)?;
        if index != 0 {
            z.push((va_to_rva(index)?, 4));
        }
    }
    let (lc, lc_size) = h.dir(10);
    if lc_size != 0 {
        let head = v.read(lc, 4).ok_or("load config out of range")?;
        let size = (u32_at(&head, 0)? as usize).min(0x140);
        let c = v.read(lc, size).ok_or("load config out of range")?;
        let field = |o: usize| -> Option<u64> { (o + 8 <= size).then(|| u64_at(&c, o).unwrap()) };
        for (o, what) in [
            (0xC0, "DynamicValueRelocTable"),
            (0xC8, "CHPEMetadataPointer"),
        ] {
            if field(o).unwrap_or(0) != 0 {
                return Err(format!(
                    "load config has {what}: loader patching not modelled"
                ));
            }
        }
        if 0xE4 <= size && u32_at(&c, 0xE0)? != 0 {
            return Err("load config has DynamicValueRelocTableOffset: not modelled".into());
        }
        // SecurityCookie, GuardCF check/dispatch, GuardRF failure/verify-SP, GuardXFG check/
        // dispatch/table-dispatch, GuardMemcpy: pointers whose targets the loader may fill.
        for o in [0x58, 0x70, 0x78, 0xD8, 0xE8, 0x118, 0x120, 0x128, 0x138] {
            if let Some(p) = field(o).filter(|&p| p != 0) {
                z.push((va_to_rva(p)?, 8));
            }
        }
    }
    Ok(z)
}

/// The canonical hash of the image `v`, relative to `base`.
pub fn canonical(v: &dyn View, base: u64) -> Result<Canon, String> {
    let first = v.read(0, 0x400).ok_or("no headers")?;
    let h0 = Headers::parse(&first)?;
    let hdr = v
        .read(0, h0.size_of_headers as usize)
        .ok_or("headers out of range")?;
    let h = Headers::parse(&hdr)?;
    let sites = reloc_sites(v, &h)?;
    let zeroed = loader_written(v, &h, base)?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"tls117-pe-canonical-1");
    let mut hb = hdr.clone();
    hb[h.image_base_at..h.image_base_at + 8].fill(0);
    hasher.update(&(hb.len() as u64).to_le_bytes());
    hasher.update(&hb);
    let (mut nsec, mut nsites) = (0, 0);
    for (i, s) in h.sections.iter().enumerate() {
        let len = s.mapped_len();
        if s.characteristics & IMAGE_SCN_MEM_WRITE != 0 || len == 0 {
            continue;
        }
        let mut buf = v
            .read(s.va, len as usize)
            .ok_or_else(|| format!("section {i} out of range"))?;
        let end = s.va as u64 + len as u64;
        for &site in &sites {
            if site >= s.va && site as u64 + 8 <= end {
                let o = (site - s.va) as usize;
                let x = u64::from_le_bytes(buf[o..o + 8].try_into().unwrap());
                buf[o..o + 8].copy_from_slice(&x.wrapping_sub(base).to_le_bytes());
                nsites += 1;
            }
        }
        for &(zr, zl) in &zeroed {
            let (a, b) = (zr.max(s.va) as u64, (zr as u64 + zl as u64).min(end));
            if a < b {
                buf[(a - s.va as u64) as usize..(b - s.va as u64) as usize].fill(0);
            }
        }
        hasher.update(&(i as u64).to_le_bytes());
        hasher.update(&(buf.len() as u64).to_le_bytes());
        hasher.update(&buf);
        nsec += 1;
    }
    Ok(Canon {
        hash: *hasher.finalize().as_bytes(),
        sections: nsec,
        reloc_sites: nsites,
        zeroed,
    })
}

/// The canonical hash of a PE file's bytes.
pub fn canonical_file(file: &[u8]) -> Result<Canon, String> {
    let h = Headers::parse(file)?;
    canonical(&FileView::new(file)?, h.image_base)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &[u8] = include_bytes!("fixture.exe");

    /// What the loader does, for a test: map the file at `base`, apply every DIR64 relocation
    /// (in every section, as the loader does), rewrite `ImageBase`, and fill the loader-written
    /// ranges with addresses that change per load.
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

    fn section_named<'a>(h: &'a Headers, name: &str) -> &'a Section {
        h.sections
            .iter()
            .find(|s| s.name.starts_with(name.as_bytes()))
            .unwrap()
    }

    #[test]
    fn fixture_has_what_the_tests_need() {
        let h = Headers::parse(FIXTURE).unwrap();
        let c = canonical_file(FIXTURE).unwrap();
        assert!(
            c.reloc_sites >= 5,
            "DIR64 sites in read-only sections: {c:?}"
        );
        assert!(
            c.zeroed.iter().any(|&(r, _)| r == h.dir(12).0),
            "the IAT is zeroed: {c:?}"
        );
        let idata = section_named(&h, ".idata");
        assert_eq!(
            idata.characteristics & IMAGE_SCN_MEM_WRITE,
            0,
            "IAT in a read-only section"
        );
    }

    /// Two loads at different bases (relocated, ImageBase rewritten, IAT filled) and the file
    /// itself canonicalise to one hash.
    #[test]
    fn relocated_loads_canonicalise_like_the_file() {
        let file = canonical_file(FIXTURE).unwrap();
        for base in [0x7ff6_1234_0000u64, 0x1_8000_0000, 0x140000000] {
            let img = load(FIXTURE, base);
            let mem = canonical(&SliceView(&img), base).unwrap();
            assert_eq!(mem.hash, file.hash, "load at {base:#x}");
            assert_eq!(mem.reloc_sites, file.reloc_sites);
        }
        // Precondition: the two loads really differ in raw bytes.
        assert_ne!(
            load(FIXTURE, 0x7ff6_1234_0000),
            load(FIXTURE, 0x1_8000_0000)
        );
    }

    /// Without normalisation the relocated bytes would differ: hashing a load against the wrong
    /// base gives another hash.
    #[test]
    fn the_wrong_base_changes_the_hash() {
        let img = load(FIXTURE, 0x7ff6_1234_0000);
        let right = canonical(&SliceView(&img), 0x7ff6_1234_0000).unwrap();
        let wrong = canonical(&SliceView(&img), 0x7ff6_1234_1000).unwrap();
        assert_ne!(right.hash, wrong.hash);
    }

    #[test]
    fn a_code_or_constant_change_is_caught() {
        let h = Headers::parse(FIXTURE).unwrap();
        let base = canonical_file(FIXTURE).unwrap().hash;
        for name in [".text", ".rdata"] {
            let s = section_named(&h, name);
            let mut f = FIXTURE.to_vec();
            f[s.raw_ptr as usize] ^= 1;
            assert_ne!(canonical_file(&f).unwrap().hash, base, "{name}");
        }
    }

    /// The stated scope: initialised mutable data (.data) is not covered.
    #[test]
    fn a_data_only_change_is_not_covered() {
        let h = Headers::parse(FIXTURE).unwrap();
        let s = section_named(&h, ".data");
        assert_ne!(s.characteristics & IMAGE_SCN_MEM_WRITE, 0);
        let mut f = FIXTURE.to_vec();
        f[s.raw_ptr as usize] ^= 1;
        assert_eq!(
            canonical_file(&f).unwrap().hash,
            canonical_file(FIXTURE).unwrap().hash
        );
    }

    #[test]
    fn an_unhandled_relocation_type_refuses() {
        let h = Headers::parse(FIXTURE).unwrap();
        let (rva, _) = h.dir(5);
        let s = h
            .sections
            .iter()
            .find(|s| rva >= s.va && rva < s.va + s.mapped_len())
            .unwrap();
        let mut f = FIXTURE.to_vec();
        let first_entry = (s.raw_ptr + (rva - s.va) + 8) as usize;
        let e = u16::from_le_bytes([f[first_entry], f[first_entry + 1]]);
        assert_eq!(e >> 12, IMAGE_REL_BASED_DIR64, "precondition");
        let highlow = (3u16 << 12) | (e & 0xfff);
        f[first_entry..first_entry + 2].copy_from_slice(&highlow.to_le_bytes());
        let err = canonical_file(&f).unwrap_err();
        assert!(err.contains("type 3"), "{err}");
    }

    #[test]
    fn not_a_pe_refuses() {
        assert!(canonical_file(b"MZ but nothing else").is_err());
        assert!(canonical_file(&FIXTURE[..0x80]).is_err());
    }
}
