//! TLS-117: stamping a built Windows binary's data-digest slot, and verifying a stamped one
//! (design note `docs/superpowers/notes/2026-11-21-tls117-windows-image-key.md`, section 3, "The
//! data-digest slot"). The `alsem-stamp` binary is a thin front end over these two functions.
//! The slot's magic comes from this binary's own slot ([`super::key::slot_magic`]), never a
//! second literal, so the stamped binaries carry it exactly once.

use std::path::Path;

/// Stamp `input` into `out` (they may be the same path): find the slot (exactly once, in a
/// read-only data section), write the digest of the writable sections and masked bytes into it,
/// recompute a non-zero PE checksum, and check that nothing else changed. Returns a one-line
/// summary. Stamping is idempotent.
pub fn stamp_file(input: &Path, out: &Path) -> Result<String, String> {
    let mut bytes = std::fs::read(input).map_err(|e| format!("{}: {e}", input.display()))?;
    let st = super::pe::stamp(&mut bytes, &super::key::slot_magic())?;
    std::fs::write(out, &bytes).map_err(|e| format!("{}: {e}", out.display()))?;
    let digest: String = st.digest.iter().map(|b| format!("{b:02x}")).collect();
    let checksum = match st.checksum {
        Some((old, new)) => format!("checksum {old:#010x} -> {new:#010x}"),
        None => "checksum 0 (not used; left 0)".to_string(),
    };
    Ok(format!(
        "stamped {}: slot at file offset {:#x} (RVA {:#x}), digest {digest}, {checksum}",
        out.display(),
        st.slot_offset,
        st.slot_rva
    ))
}

/// Check a stamped binary: the slot occurs once, holds a non-zero digest equal to the digest
/// recomputed from the file, and the file carries the IR marker. Returns a one-line summary.
pub fn verify_file(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let off = super::pe::find_unique(&bytes, &super::key::slot_magic())?;
    let slot = &bytes[off..off + super::pe::SLOT_LEN];
    if slot[16..].iter().all(|&b| b == 0) {
        return Err(format!(
            "{}: not stamped (the slot is empty)",
            path.display()
        ));
    }
    if super::pe::data_digest(&bytes)? != slot[16..] {
        return Err(format!(
            "{}: the slot does not match the file's writable sections and masked bytes",
            path.display()
        ));
    }
    if super::key::key_of(&mut &bytes[..]).is_none() {
        return Err(format!("{}: no IR marker", path.display()));
    }
    Ok(format!(
        "verified {}: slot matches, IR marker present",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fixture (`tests/fixtures/tls117`) with this binary's slot magic put in place of its
    /// test-only one at run time (the product magic is never a second literal here), and the IR
    /// marker appended past the sections (not mapped, so the PE is unchanged).
    fn product_fixture(dir: &Path) -> std::path::PathBuf {
        let mut f = include_bytes!("../../../tests/fixtures/tls117/fixture.exe").to_vec();
        assert_eq!(
            &f[0x800..0x810],
            b"TLS117-FIXT-SLOT",
            "precondition: the fixture's slot"
        );
        f[0x800..0x810].copy_from_slice(&super::super::key::slot_magic());
        f.extend_from_slice(&super::super::key::marker_bytes());
        let p = dir.join("fixture.exe");
        std::fs::write(&p, f).unwrap();
        p
    }

    /// Stamp, then verify; a changed `.data` byte after stamping and an unstamped file are
    /// refused. Discrimination (2026-11-23): `verify_file`'s digest comparison removed: the
    /// changed `.data` byte verifies and the test fails; restored, passes.
    #[test]
    fn stamp_then_verify() {
        let d = tempfile::tempdir().unwrap();
        let p = product_fixture(d.path());
        let unstamped = verify_file(&p).unwrap_err();
        assert!(unstamped.contains("not stamped"), "{unstamped}");
        let out = d.path().join("stamped.exe");
        stamp_file(&p, &out).unwrap();
        verify_file(&out).expect("a stamped file verifies");
        let mut f = std::fs::read(&out).unwrap();
        f[0x600] ^= 1; // `counter` in .data
        std::fs::write(&out, &f).unwrap();
        let changed = verify_file(&out).unwrap_err();
        assert!(changed.contains("does not match"), "{changed}");
    }
}
