//! Extract embedded ShowMyCode `.al` source from a `.app` package.

use crate::engine::deps::app_package_zip::open_app_file;
use anyhow::{Context, Result};
use std::path::Path;

/// One embedded source file recovered from a `.app`.
///
/// `text` is `Arc<str>` (perf safe-wins Task 1): the SAME allocation is
/// shared by `ParsedFile.text` and `LspSnapshot::dep_texts` — embedded
/// dependency source (~114 MB on a real BC workspace) must exist in memory
/// exactly once. Serde's `rc` feature serializes it as a plain string, so
/// the content-addressed source cache format (`snapshot::cache`) is
/// unchanged.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SourceFile {
    pub virtual_path: String,
    pub text: std::sync::Arc<str>,
}

/// blake3 hex of the whole `.app` file (artifact identity).
///
/// Streamed through the hasher, never read whole: BaseApp's `.app` is ~105 MB.
pub fn app_content_hash(app_path: &Path) -> Result<String> {
    let file = std::fs::File::open(app_path)
        .with_context(|| format!("read .app: {}", app_path.display()))?;
    let mut hasher = blake3::Hasher::new();
    hasher
        .update_reader(file)
        .with_context(|| format!("read .app: {}", app_path.display()))?;
    Ok(hasher.finalize().to_hex().to_string())
}

/// Extract every `*.al` entry from the `.app`'s embedded zip. Returns an empty
/// `Vec` if the app ships no source (symbol-only / runtime app). For a
/// Ready-to-Run package, the source comes from its nested app.
///
/// Entry names are percent-decoded (the AL compiler URL-encodes them) and BOM
/// is stripped before the text is decoded as UTF-8 (lossy).
pub fn extract_embedded_source(app_path: &Path) -> Result<Vec<SourceFile>> {
    let Some(mut archive) = open_app_file(app_path)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for i in 0..archive.len() {
        let entry = archive.by_index(i)?;
        let name = entry.name().to_string();
        if !name.to_ascii_lowercase().ends_with(".al") {
            continue;
        }
        // T2.2: belt-and-suspenders cap — reject a hostile declared size
        // before decompressing, then bound the read itself (a lying central
        // directory). Cap-exceeded joins the SAME error path as any other
        // per-entry read failure on this surface (`?` propagates, matching
        // the module's engine-never-throws-silently posture).
        crate::capped_io::check_declared_size(
            entry.size(),
            crate::capped_io::EMBEDDED_AL_SOURCE_CAP,
        )
        .with_context(|| format!("embedded .al entry too large: {name}"))?;
        let raw = crate::capped_io::read_capped(entry, crate::capped_io::EMBEDDED_AL_SOURCE_CAP)
            .with_context(|| format!("failed to read embedded .al entry: {name}"))?;
        let text = crate::source_text::decode_al_source(&raw);
        let virtual_path = percent_encoding::percent_decode_str(&name)
            .decode_utf8_lossy()
            .into_owned();
        out.push(SourceFile {
            virtual_path,
            text: text.into(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    // Set CDO_APP to a real ShowMyCode .app to exercise extraction.
    fn cdo_app() -> Option<std::path::PathBuf> {
        std::env::var_os("CDO_APP")
            .map(std::path::PathBuf::from)
            .filter(|p| p.exists())
    }

    #[test]
    fn extracts_al_source_from_showmycode_app() {
        let Some(app) = cdo_app() else {
            return;
        };
        let files = extract_embedded_source(&app).expect("extract");
        assert!(
            files.len() > 100,
            "ShowMyCode app should yield many .al files, got {}",
            files.len()
        );
        assert!(
            files
                .iter()
                .all(|f| f.virtual_path.to_ascii_lowercase().ends_with(".al"))
        );
        assert!(
            files
                .iter()
                .any(|f| f.text.contains("codeunit") || f.text.contains("table"))
        );
    }

    #[test]
    fn content_hash_is_stable() {
        let Some(app) = cdo_app() else {
            return;
        };
        assert_eq!(
            app_content_hash(&app).unwrap(),
            app_content_hash(&app).unwrap()
        );
        assert_eq!(app_content_hash(Path::new(&app)).unwrap().len(), 64);
    }

    /// The streamed hash equals the old whole-file `blake3::hash` — it keys
    /// the on-disk source cache, so it must not change.
    #[test]
    fn streamed_content_hash_equals_whole_file_hash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.app");
        // Over blake3's 1 KiB chunk and 64 KiB reader buffer sizes.
        let bytes: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(
            app_content_hash(&path).unwrap(),
            blake3::hash(&bytes).to_hex().to_string()
        );
    }

    /// Task T2.2: an embedded `.al` entry declaring more than
    /// [`crate::capped_io::EMBEDDED_AL_SOURCE_CAP`] must be rejected via a
    /// named error, never a panic and never an unbounded allocation.
    #[test]
    fn oversized_al_entry_is_rejected_not_panicking() {
        use std::io::Write as _;

        let mut zip_buf = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut zip_buf);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            writer.start_file("Codeunit1.al", opts).unwrap();
            const CHUNK: usize = 1024 * 1024;
            let chunk = vec![0u8; CHUNK];
            let mut remaining = crate::capped_io::EMBEDDED_AL_SOURCE_CAP as usize + 1024;
            while remaining > 0 {
                let n = remaining.min(CHUNK);
                writer.write_all(&chunk[..n]).unwrap();
                remaining -= n;
            }
            writer.finish().unwrap();
        }
        let mut bytes = vec![0u8; crate::engine::deps::app_package_zip::NAVX_HEADER_SIZE as usize];
        bytes.extend_from_slice(&zip_buf.into_inner());

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("bomb.app");
        std::fs::write(&path, &bytes).expect("write crafted .app");

        let result = extract_embedded_source(&path);
        assert!(
            result.is_err(),
            "an oversized embedded .al entry must be rejected, not silently truncated"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("Codeunit1.al"),
            "error should name the offending entry: {msg}"
        );
    }

    /// A Ready-to-Run package's source lives in its nested app.
    #[test]
    fn extracts_al_source_from_the_nested_app_of_a_ready_to_run_package() {
        use crate::engine::deps::app_package_zip::test_apps;
        let app = test_apps::build_app(&[("src/Sales%20Post.Codeunit.al", b"codeunit 80 X { }")]);
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir
            .path()
            .join("Microsoft_Base Application_28.4.53241.53758.app");
        std::fs::write(&path, test_apps::wrap_ready_to_run(&app)).unwrap();

        let files = extract_embedded_source(&path).expect("extract");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].virtual_path, "src/Sales Post.Codeunit.al");
        assert_eq!(&*files[0].text, "codeunit 80 X { }");
    }
}
