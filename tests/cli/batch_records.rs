//! The `tests/gate-goldens/batch/` family (#277 golden plan). A directory that
//! holds a `RECORD` file holds base-binary records: bytes the `alsem` binary at
//! the branch base printed. They are read through [`read_record`] and never
//! rewritten: [`write_golden`], the family's only writer, refuses them, so
//! `REGEN_TEMP_GOLDENS=1` cannot bless a record away.

use std::path::{Path, PathBuf};

/// The family root.
pub(crate) fn batch_goldens() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gate-goldens/batch")
}

fn is_record(path: &Path) -> bool {
    path.parent().is_some_and(|d| d.join("RECORD").is_file())
}

/// A base-binary record's bytes; panics when `path` is not in a `RECORD` directory.
pub(crate) fn read_record(path: &Path) -> Vec<u8> {
    assert!(
        is_record(path),
        "not a base-binary record: {}",
        path.display()
    );
    std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Write a regenerable golden of the family. Panics on a record.
pub(crate) fn write_golden(path: &Path, bytes: &[u8]) {
    assert!(
        !is_record(path),
        "refusing to rewrite a base-binary record: {}",
        path.display()
    );
    std::fs::write(path, bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}

/// The guard itself: a write into a `RECORD` directory panics and leaves the
/// record's bytes as they were. It runs on a copy (a tempdir with a `RECORD`
/// marker), so a broken guard cannot damage a committed record.
#[test]
fn record_dirs_refuse_regen() {
    for dir in ["merged-streams", "update-baseline"] {
        assert!(
            batch_goldens().join(dir).join("RECORD").is_file(),
            "precondition: {dir} is a record directory"
        );
    }
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("RECORD"), "test record\n").unwrap();
    let path = d.path().join("base.stderr.golden");
    std::fs::write(&path, "recorded\n").unwrap();
    let before = read_record(&path);
    let r = std::panic::catch_unwind(|| write_golden(&path, b"rewritten\n"));
    let msg = r.expect_err("write_golden must refuse a record");
    let msg = msg.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(
        msg.starts_with("refusing to rewrite a base-binary record: "),
        "{msg}"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the record is unchanged"
    );
}
