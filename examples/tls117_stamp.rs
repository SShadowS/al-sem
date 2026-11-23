//! TLS-117: write the data digest into a Windows PE's slot (design note
//! `docs/superpowers/notes/2026-11-21-tls117-windows-image-key.md`, section 3, "The data-digest
//! slot"). Runs on any system, on an x64 PE built for Windows:
//!
//! ```text
//! cargo run --example tls117_stamp -- <file.exe> [<out.exe>]
//! ```
//!
//! It finds the slot by its magic (which must occur exactly once, in a read-only data section),
//! writes the blake3 of every writable section (name, virtual address, virtual size, raw size,
//! raw bytes) into it, recomputes a non-zero PE checksum, checks that nothing else changed, and
//! writes the result in place (or to `<out.exe>`). Stamp before Authenticode signing: signing
//! appends the certificate and rewrites the checksum but changes no section.
//!
//! This is the code the product's install script and CI use (the stamping lives in
//! `examples/tls117/pe.rs`, shared with the probe; the product phase moves it into the crate).

#[allow(dead_code)]
#[path = "tls117/pe.rs"]
mod pe;

/// The slot's magic. Only this tool spells it: it is never stamped itself.
const MAGIC: [u8; 16] = *b"TLS117-DATA-SLOT";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if !(2..=3).contains(&args.len()) {
        eprintln!("usage: tls117_stamp <file.exe> [<out.exe>]");
        std::process::exit(2);
    }
    let (input, out) = (&args[1], args.get(2).unwrap_or(&args[1]));
    let run = || -> Result<pe::Stamped, String> {
        let mut bytes = std::fs::read(input).map_err(|e| format!("{input}: {e}"))?;
        let st = pe::stamp(&mut bytes, &MAGIC)?;
        std::fs::write(out, &bytes).map_err(|e| format!("{out}: {e}"))?;
        Ok(st)
    };
    match run() {
        Ok(st) => {
            let digest: String = st.digest.iter().map(|b| format!("{b:02x}")).collect();
            let checksum = match st.checksum {
                Some((old, new)) => format!("checksum {old:#010x} -> {new:#010x}"),
                None => "checksum 0 (not used; left 0)".to_string(),
            };
            println!(
                "stamped {out}: slot at file offset {:#x} (RVA {:#x}), digest {digest}, {checksum}",
                st.slot_offset, st.slot_rva
            );
        }
        Err(e) => {
            eprintln!("tls117_stamp: {e}");
            std::process::exit(1);
        }
    }
}
