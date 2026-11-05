//! The impact-base cache (agent-tasks design §10.7): integrity, atomic
//! publication, the bounds (B-6b) and the key's parts (B-6).

use std::time::{Duration, SystemTime};

use serde_json::json;

use super::*;

fn cache(limits: Limits) -> (tempfile::TempDir, Cache) {
    let t = tempfile::tempdir().unwrap();
    let c = Cache::new(t.path().join("impact-base"), limits);
    (t, c)
}

/// A written entry reads back whole; an unknown key misses.
#[test]
fn an_entry_round_trips() {
    let (_t, c) = cache(Limits::default());
    let v = json!({"summaries": {"a": 1}, "byDiff": {}});
    c.write("k1", &v).unwrap();
    assert_eq!(c.read("k1"), Lookup::Hit(v));
    assert_eq!(c.read("k2"), Lookup::Miss);
}

/// A corrupted entry (one payload byte flipped) is refused, deleted, and
/// reported so the caller rebuilds it.
///
/// Break: skip the payload hash check → `Hit` of the corrupted payload or a
/// decode error unreported.
#[test]
fn a_corrupted_entry_is_rebuilt() {
    let (_t, c) = cache(Limits::default());
    c.write("k1", &json!({"x": "y".repeat(100)})).unwrap();
    let p = entry_path(&c.dir, "k1");
    let mut bytes = std::fs::read(&p).unwrap();
    let nl = bytes.iter().position(|b| *b == b'\n').unwrap();
    // Rewrite the payload as a different (valid) gzip: the hash no longer holds.
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(6));
    std::io::Write::write_all(&mut gz, br#"{"x":"z"}"#).unwrap();
    let other = gz.finish().unwrap();
    bytes.truncate(nl + 1);
    bytes.extend(other);
    std::fs::write(&p, &bytes).unwrap();
    match c.read("k1") {
        Lookup::Rebuilt(why) => assert!(why.contains("size") || why.contains("hash"), "{why}"),
        other => panic!("expected Rebuilt, got {other:?}"),
    }
    assert!(!p.exists(), "the bad entry is deleted");
}

/// An entry whose header names another key is not read as this key's.
#[test]
fn an_entry_under_another_name_is_refused() {
    let (_t, c) = cache(Limits::default());
    c.write("k1", &json!({"a": 1})).unwrap();
    std::fs::copy(entry_path(&c.dir, "k1"), entry_path(&c.dir, "k2")).unwrap();
    assert!(matches!(c.read("k2"), Lookup::Rebuilt(_)));
}

/// B-6b: at most `max_entries` entries; the least recently READ goes first,
/// and the entry just written stays.
///
/// Break: no pruning → 4 entries remain.
#[test]
fn the_count_bound_drops_the_least_recently_read() {
    let (_t, c) = cache(Limits {
        max_entries: 3,
        ..Limits::default()
    });
    for k in ["a", "b", "c"] {
        c.write(k, &json!({"k": k})).unwrap();
    }
    // Age them, then read `a`: `b` is now the least recently read.
    let old = SystemTime::now() - Duration::from_secs(600);
    for (i, k) in ["a", "b", "c"].iter().enumerate() {
        let f = std::fs::File::options()
            .write(true)
            .open(entry_path(&c.dir, k))
            .unwrap();
        f.set_modified(old + Duration::from_secs(i as u64)).unwrap();
    }
    assert!(matches!(c.read("a"), Lookup::Hit(_)));
    c.write("d", &json!({"k": "d"})).unwrap();
    assert_eq!(c.usage().0, 3);
    assert!(
        matches!(c.read("b"), Lookup::Miss),
        "b was the least recently read"
    );
    for k in ["a", "c", "d"] {
        assert!(matches!(c.read(k), Lookup::Hit(_)), "{k} kept");
    }
}

/// B-6b: the folder's bytes stay within `max_total`.
#[test]
fn the_byte_bound_holds() {
    let (_t, c) = cache(Limits {
        max_total: 3000,
        ..Limits::default()
    });
    // Incompressible-ish payloads of ~1 KB each.
    for i in 0..8 {
        let noise: String = (0..1200)
            .map(|j| char::from(b'a' + ((i * 7919 + j * 104_729) % 26) as u8))
            .collect();
        c.write(&format!("k{i}"), &json!({"n": noise})).unwrap();
        assert!(c.usage().1 <= 3000, "after {i}: {:?}", c.usage());
    }
    assert!(
        matches!(c.read("k7"), Lookup::Hit(_)),
        "the newest entry stays"
    );
}

/// An entry over the age bound is pruned.
#[test]
fn the_age_bound_holds() {
    let (_t, c) = cache(Limits {
        max_age: Duration::from_secs(60),
        ..Limits::default()
    });
    c.write("old", &json!({})).unwrap();
    let f = std::fs::File::options()
        .write(true)
        .open(entry_path(&c.dir, "old"))
        .unwrap();
    f.set_modified(SystemTime::now() - Duration::from_secs(120))
        .unwrap();
    c.write("new", &json!({})).unwrap();
    assert!(matches!(c.read("old"), Lookup::Miss));
}

/// The size caps refuse a write and a read over them.
#[test]
fn the_size_caps_refuse() {
    let (_t, c) = cache(Limits {
        max_decoded: 50,
        ..Limits::default()
    });
    let e = c.write("big", &json!({"x": "y".repeat(100)})).unwrap_err();
    assert!(e.contains("over the 50 byte cap"), "{e}");
    assert_eq!(c.usage().0, 0);
}

/// B-6: the key changes with each part (a package hash, an option, the
/// tree), never with nothing.
#[test]
fn the_key_holds_every_part() {
    let base = KeyParts {
        tree: "t1".into(),
        packages_digest: "packages:aa".into(),
        source_deps: vec![],
        options: "cross-app".into(),
    };
    let k = base.key();
    assert_eq!(k, base.clone().key());
    for changed in [
        KeyParts {
            tree: "t2".into(),
            ..base.clone()
        },
        KeyParts {
            packages_digest: "packages:ab".into(),
            ..base.clone()
        },
        KeyParts {
            options: "single-app".into(),
            ..base.clone()
        },
        KeyParts {
            source_deps: vec![("/d".into(), "blake3:x".into())],
            ..base.clone()
        },
    ] {
        assert_ne!(changed.key(), k, "{changed:?}");
    }
}
