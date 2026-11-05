//! Write a dependency `.app` for a test workspace: a 40-byte NAVX header, then a
//! zip of `NavxManifest.xml`, `SymbolReference.json` and any embedded `.al` source.

use std::io::Write;
use std::path::Path;

pub fn write_symbol_app(path: &Path, guid: &str, name: &str, version: &str, symbols: &str) {
    write_app(path, guid, name, version, symbols, &[], "");
}

/// A dependency `.app` that also embeds `.al` source: `(entry name, text)` pairs.
/// `manifest_extra` is raw XML placed after `<App>` (e.g. `<Dependencies>`).
#[allow(dead_code)]
pub fn write_source_app(
    path: &Path,
    guid: &str,
    name: &str,
    version: &str,
    symbols: &str,
    sources: &[(&str, &str)],
    manifest_extra: &str,
) {
    write_app(path, guid, name, version, symbols, sources, manifest_extra);
}

fn write_app(
    path: &Path,
    guid: &str,
    name: &str,
    version: &str,
    symbols: &str,
    sources: &[(&str, &str)],
    manifest_extra: &str,
) {
    let bytes = source_app_bytes(guid, name, version, symbols, sources, manifest_extra);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// The bytes [`write_source_app`] writes, for a caller that writes them itself. A
/// COMMITTED `.app` (a fixture other tests read while it is regenerated) must go
/// through `regen::write_golden`, never through [`write_source_app`], which writes
/// in place.
#[allow(dead_code)]
pub fn source_app_bytes(
    guid: &str,
    name: &str,
    version: &str,
    symbols: &str,
    sources: &[(&str, &str)],
    manifest_extra: &str,
) -> Vec<u8> {
    let manifest = format!(
        r#"<?xml version="1.0" encoding="utf-8"?><Package xmlns="http://schemas.microsoft.com/navx/2015/manifest"><App Id="{guid}" Name="{name}" Publisher="probe" Version="{version}" Runtime="13.0" />{manifest_extra}</Package>"#
    );
    let mut bytes = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut bytes);
        // A fixed timestamp keeps the `.app` bytes a pure function of the inputs; the
        // default stamps the current time, so two builds seconds apart differed (#70 2h).
        let opts =
            zip::write::SimpleFileOptions::default().last_modified_time(zip::DateTime::default());
        zip.start_file("NavxManifest.xml", opts).unwrap();
        zip.write_all(manifest.as_bytes()).unwrap();
        zip.start_file("SymbolReference.json", opts).unwrap();
        zip.write_all(symbols.as_bytes()).unwrap();
        for (entry, text) in sources {
            zip.start_file(*entry, opts).unwrap();
            zip.write_all(text.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }
    let mut out = vec![0u8; 40];
    out.extend_from_slice(bytes.get_ref());
    out
}
