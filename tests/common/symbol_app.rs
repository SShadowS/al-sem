//! Write a symbol-only dependency `.app` for a test workspace: a 40-byte NAVX
//! header, then a zip of `NavxManifest.xml` and `SymbolReference.json`.

use std::io::Write;
use std::path::Path;

pub fn write_symbol_app(path: &Path, guid: &str, name: &str, version: &str, symbols: &str) {
    let manifest = format!(
        r#"<?xml version="1.0" encoding="utf-8"?><Package xmlns="http://schemas.microsoft.com/navx/2015/manifest"><App Id="{guid}" Name="{name}" Publisher="probe" Version="{version}" Runtime="13.0" /></Package>"#
    );
    let mut bytes = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut bytes);
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("NavxManifest.xml", opts).unwrap();
        zip.write_all(manifest.as_bytes()).unwrap();
        zip.start_file("SymbolReference.json", opts).unwrap();
        zip.write_all(symbols.as_bytes()).unwrap();
        zip.finish().unwrap();
    }
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut out = std::fs::File::create(path).unwrap();
    out.write_all(&[0u8; 40]).unwrap();
    out.write_all(bytes.get_ref()).unwrap();
}
