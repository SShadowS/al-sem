//! The IR cache's key (spec "The key"): the blake3 of the running executable's bytes and the
//! IR format, so whatever produces or reads the IR (grammar, lowerer, codec, toolchain,
//! features) is in the key by construction. Linux only: the image is read through
//! `/proc/self/exe`, the inode the process runs even when its path was rebuilt since.

use std::io::Read;

/// A cache key: names one key folder (`ir/<64 hex>/`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct IrKey(pub [u8; 32]);

impl IrKey {
    pub(crate) fn hex(&self) -> String {
        blake3::Hash::from_bytes(self.0).to_hex().to_string()
    }
}

/// Marks an image as an engine binary of this repository: under valgrind or a similar loader
/// `/proc/self/exe` is the tool, which lacks it, so no key is made. `#[used]` and read through
/// `black_box` (in [`image_key`]), so neither the compiler nor the linker drops it.
/// `scripts/check-ir-image` searches a built binary for these bytes.
#[used]
static IR_IMAGE_MARKER: [u8; 16] = *b"ALSEM-IR-IMAGE-1";

/// The key of the running image, or `None` (the cache is off for this process) when it
/// cannot be read or lacks the marker.
#[cfg(target_os = "linux")]
pub(crate) fn image_key() -> Option<IrKey> {
    let mut f = std::fs::File::open("/proc/self/exe").ok()?;
    key_of(&mut f)
}

/// No key off Linux (spec: the cache is off on Windows in v1).
#[cfg(not(target_os = "linux"))]
pub(crate) fn image_key() -> Option<IrKey> {
    None
}

/// The key of `image` read to its end: blake3 over a domain tag, the IR format and every byte;
/// `None` on a read error or when the bytes hold no [`IR_IMAGE_MARKER`].
#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // off Linux only its tests call it
pub(crate) fn key_of(image: &mut impl Read) -> Option<IrKey> {
    let marker: [u8; 16] = *std::hint::black_box(&IR_IMAGE_MARKER);
    let finder = memchr::memmem::Finder::new(&marker);
    let mut h = blake3::Hasher::new();
    h.update(b"alsem-ir-key");
    h.update(&super::IR_FORMAT.to_le_bytes());
    let mut buf = vec![0u8; 1 << 20];
    // The last 15 bytes of the previous chunk, so a marker across a chunk boundary is found.
    let mut tail: Vec<u8> = Vec::with_capacity(2 * marker.len());
    let mut found = false;
    loop {
        let n = match image.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        };
        let chunk = &buf[..n];
        h.update(chunk);
        if !found {
            let k = marker.len() - 1;
            tail.extend_from_slice(&chunk[..chunk.len().min(k)]);
            found = finder.find(&tail).is_some() || finder.find(chunk).is_some();
            if chunk.len() >= k {
                tail.clear();
                tail.extend_from_slice(&chunk[chunk.len() - k..]);
            } else {
                // A short read: `tail` holds the old tail and the whole chunk.
                let cut = tail.len().saturating_sub(k);
                tail.drain(..cut);
            }
        }
    }
    found.then(|| IrKey(*h.finalize().as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marked(extra: &[u8]) -> Vec<u8> {
        [b"prefix".as_slice(), &IR_IMAGE_MARKER, extra].concat()
    }

    #[test]
    fn bytes_without_marker_have_no_key() {
        assert!(key_of(&mut &b"no marker in here at all"[..]).is_none());
        let mut almost = IR_IMAGE_MARKER;
        almost[15] ^= 1;
        assert!(key_of(&mut &almost[..]).is_none());
        assert!(key_of(&mut &marked(b"")[..]).is_some());
    }

    #[test]
    fn different_bytes_different_keys() {
        let (a, b) = (marked(b"x"), marked(b"y"));
        let (ka, kb) = (key_of(&mut &a[..]), key_of(&mut &b[..]));
        assert!(ka.is_some() && kb.is_some());
        assert_ne!(ka, kb);
        assert_eq!(ka, key_of(&mut &a[..]), "deterministic");
    }

    /// A marker split across two reads is still found.
    #[test]
    fn marker_across_a_read_boundary_is_found() {
        struct Chunks<'a>(Vec<&'a [u8]>);
        impl Read for Chunks<'_> {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                if self.0.is_empty() {
                    return Ok(0);
                }
                let c = self.0.remove(0);
                out[..c.len()].copy_from_slice(c);
                Ok(c.len())
            }
        }
        let bytes = marked(b"tail");
        for split in 1..bytes.len() {
            let mut r = Chunks(vec![&bytes[..split], &bytes[split..]]);
            assert_eq!(key_of(&mut r), key_of(&mut &bytes[..]), "split at {split}");
        }
        let mut bytewise = Chunks(bytes.chunks(1).collect());
        assert_eq!(
            key_of(&mut bytewise),
            key_of(&mut &bytes[..]),
            "one byte a read"
        );
        assert!(key_of(&mut &bytes[..]).is_some());
    }

    /// The child half of [`key_is_the_running_image_not_the_path`]: waits for a line on
    /// stdin, then prints the running image's key.
    #[test]
    #[ignore = "run by key_is_the_running_image_not_the_path as a child"]
    fn key_child_mode() {
        if std::env::var_os("ALSEM_IR_KEY_CHILD").is_none() {
            return;
        }
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).unwrap();
        println!(
            "IRKEY={}",
            image_key().map_or_else(|| "none".to_string(), |k| k.hex())
        );
    }

    /// An engine whose executable path is replaced while it runs (a rebuild) still keys on
    /// the image it runs, not on the file now at its path.
    #[cfg(target_os = "linux")]
    #[test]
    fn key_is_the_running_image_not_the_path() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("engine");
        std::fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
        let original = key_of(&mut std::fs::File::open(&exe).unwrap()).expect("marked");
        let mut child = std::process::Command::new(&exe)
            .args([
                "--ignored",
                "--exact",
                "program::ir_cache::key::tests::key_child_mode",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("ALSEM_IR_KEY_CHILD", "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        // A rebuild: the path now names another file (marked, so it would have a key).
        std::fs::remove_file(&exe).unwrap();
        std::fs::write(&exe, marked(b"another build")).unwrap();
        writeln!(child.stdin.take().unwrap()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success());
        let key = String::from_utf8_lossy(&out.stdout)
            .lines()
            // libtest prints the test's name on the same line first.
            .find_map(|l| l.split("IRKEY=").nth(1).map(|k| k.trim().to_string()))
            .expect("the child printed its key");
        assert_eq!(key, original.hex());
    }
}
