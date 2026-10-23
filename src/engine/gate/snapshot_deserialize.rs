//! cli-b/b4 — snapshot DESERIALIZER. The inverse of B0's serializers
//! (`snapshot_full` raw-JSON / envelope / CBOR / cbor.gz). Byte-parity port of
//! al-sem `src/snapshot/deserialize.ts`.
//!
//! Auto-detects the on-disk format from the leading bytes (unless a `formatHint`
//! is supplied) and produces an insertion-ordered [`CborValue`] tree — the SAME
//! shape the B0 serializers consume, so the diff engine can read snapshot facts
//! straight off it.
//!
//!   - first byte `0x7b` (`{`)        → JSON  (raw OR enveloped `capability-snapshot`)
//!   - first two bytes `1f 8b`        → gzip → inflate → CBOR
//!   - otherwise (e.g. `0xb9`)        → CBOR
//!
//! JSON: accepts BOTH the raw `CapabilitySnapshot` and the enveloped
//! `capability-snapshot` document (kind === "capability-snapshot"); the latter is
//! un-wrapped (payload lifted, `snapshotSchemaVersion → schemaVersion`,
//! `alsemVersion`/`generatedAt` folded back from the envelope).
//!
//! The CBOR decoder is the EXACT inverse of `cbor::encode` (cbor-x 1.6.4 byte
//! rules: always-map-16 `0xb9`, smallest-int ladder, `0xfb` f64, text/array
//! headers, bool/null/undefined). It is FALLIBLE and NEVER panics — a truncated
//! or malformed stream yields `Err`, honoring the engine-never-throws contract.

use indexmap::IndexMap;

use crate::engine::gate::cbor::CborValue;
use crate::engine::l5::snapshot_full::SNAPSHOT_SCHEMA_VERSION;

/// On-disk snapshot format. Mirrors al-sem `SnapshotFormat` (the diff-relevant
/// subset; `json` covers raw + enveloped).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotFormat {
    Json,
    Cbor,
    CborGz,
}

/// Deserialize a snapshot from bytes into an insertion-ordered [`CborValue`] map.
///
/// Auto-detects the format unless `format_hint` is given. Asserts the snapshot
/// `schemaVersion == SNAPSHOT_SCHEMA_VERSION` (pre-release policy: old versions
/// are rejected with a clear error and the remedy, never a panic).
pub fn deserialize_snapshot(
    bytes: &[u8],
    format_hint: Option<SnapshotFormat>,
) -> Result<CborValue, String> {
    let fmt = format_hint.unwrap_or_else(|| detect_format(bytes));
    let parsed: CborValue = match fmt {
        SnapshotFormat::Json => {
            let text = std::str::from_utf8(bytes)
                .map_err(|e| format!("snapshot JSON is not valid UTF-8: {e}"))?;
            let v: serde_json::Value = serde_json::from_str(text)
                .map_err(|e| format!("snapshot JSON parse error: {e}"))?;
            let tree = json_to_cbor(&v);
            // Enveloped document → un-wrap to the raw snapshot.
            if let CborValue::Map(m) = &tree {
                if matches!(m.get("kind"), Some(CborValue::Text(k)) if k == "capability-snapshot") {
                    unwrap_snapshot_document(&tree)
                } else {
                    tree
                }
            } else {
                tree
            }
        }
        SnapshotFormat::CborGz => {
            let inflated = gunzip(bytes)?;
            cbor_decode(&inflated)?
        }
        SnapshotFormat::Cbor => cbor_decode(bytes)?,
    };

    // schemaVersion gate.
    let schema_version = match &parsed {
        CborValue::Map(m) => match m.get("schemaVersion") {
            Some(CborValue::Int(n)) => Some(*n),
            _ => None,
        },
        _ => None,
    };
    match schema_version {
        Some(SNAPSHOT_SCHEMA_VERSION) => {}
        Some(other) => {
            return Err(format!(
                "deserializeSnapshot: snapshot schemaVersion {other} is not supported by this build (v{SNAPSHOT_SCHEMA_VERSION}); re-create it with 'alsem fingerprint'"
            ));
        }
        None => {
            return Err("deserializeSnapshot: snapshot has no integer schemaVersion".to_string());
        }
    }

    // Map key insertion order is already preserved (serde_json `preserve_order` is
    // on; the CBOR decoder uses IndexMap) — nothing more to do.
    Ok(parsed)
}

/// Auto-detect the snapshot format from the leading bytes (al-sem `detectFormat`).
fn detect_format(bytes: &[u8]) -> SnapshotFormat {
    if bytes.len() >= 2 && bytes[0] == 0x1f && bytes[1] == 0x8b {
        return SnapshotFormat::CborGz;
    }
    if !bytes.is_empty() && bytes[0] == 0x7b {
        return SnapshotFormat::Json;
    }
    SnapshotFormat::Cbor
}

/// Un-wrap the enveloped `capability-snapshot` document: lift `payload`, RENAME
/// `snapshotSchemaVersion → schemaVersion`, and fold `alsemVersion` /
/// `generatedAt` back from the envelope (al-sem `unwrapSnapshotDocument`).
fn unwrap_snapshot_document(tree: &CborValue) -> CborValue {
    let CborValue::Map(env) = tree else {
        return tree.clone();
    };
    let payload = match env.get("payload") {
        Some(CborValue::Map(p)) => p,
        _ => return tree.clone(),
    };
    let mut snap: IndexMap<String, CborValue> = IndexMap::new();
    // schemaVersion (lifted from snapshotSchemaVersion) first.
    if let Some(v) = payload.get("snapshotSchemaVersion") {
        snap.insert("schemaVersion".into(), v.clone());
    }
    if let Some(v) = env.get("alsemVersion") {
        snap.insert("alsemVersion".into(), v.clone());
    }
    if let Some(v) = env.get("generatedAt") {
        snap.insert("generatedAt".into(), v.clone());
    }
    for (k, v) in payload {
        if k == "snapshotSchemaVersion" {
            continue;
        }
        snap.insert(k.clone(), v.clone());
    }
    CborValue::Map(snap)
}

// ===========================================================================
// JSON → CborValue. serde_json preserves number distinction; integral f64 → Int,
// otherwise Float. Object key order is preserved iff serde_json's `preserve_order`
// feature is on; the diff engine reads by key (order-independent), so a sorted
// IndexMap is acceptable for the JSON path.
// ===========================================================================

fn json_to_cbor(v: &serde_json::Value) -> CborValue {
    match v {
        serde_json::Value::Null => CborValue::Null,
        serde_json::Value::Bool(b) => CborValue::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                CborValue::Int(i)
            } else if let Some(u) = n.as_u64() {
                // > i64::MAX: fall back to f64 (corpus has none).
                CborValue::Float(u as f64)
            } else {
                CborValue::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => CborValue::Text(s.clone()),
        serde_json::Value::Array(items) => {
            CborValue::Array(items.iter().map(json_to_cbor).collect())
        }
        serde_json::Value::Object(map) => {
            let mut m: IndexMap<String, CborValue> = IndexMap::new();
            for (k, val) in map {
                m.insert(k.clone(), json_to_cbor(val));
            }
            CborValue::Map(m)
        }
    }
}

// ===========================================================================
// gunzip — inflate the gzip container. Decompression is unambiguous (any inflate
// reproduces the original CBOR bytes regardless of the encoder's level/OS byte).
// ===========================================================================

fn gunzip(bytes: &[u8]) -> Result<Vec<u8>, String> {
    use flate2::read::GzDecoder;
    // T2.2: this was previously an UNBOUNDED `read_to_end` — the classic
    // zip-bomb vector (a tiny `.cbor.gz` inflating to an attacker-chosen
    // number of gigabytes). Bound it via the shared cap helper; a
    // cap-exceeded stream fails through the SAME `Result<_, String>` a
    // corrupt/truncated gzip stream already used here.
    let decoder = GzDecoder::new(bytes);
    crate::capped_io::read_capped(decoder, crate::capped_io::SNAPSHOT_GZ_CAP)
        .map_err(|e| format!("snapshot gunzip failed: {e}"))
}

// ===========================================================================
// CBOR decoder — the exact inverse of `cbor::encode`. Hand-rolled, fallible,
// non-panicking. Supports exactly the byte forms B0 emits:
//   - major 0/1 ints: 0x00..0x17 inline, 0x18 (u8), 0x19 (u16), 0x1a (u32).
//   - 0xfb f64 (and any integral f64 is kept as Float — the diff engine never
//     reads a numeric snapshot field as an integer beyond schemaVersion, which is
//     a small Int).
//   - major 3 text, major 4 array (same length ladder).
//   - 0xb9 + u16 map (always-map-16). Also tolerates 0xa0..0xb7 / 0xb8 / 0xba
//     canonical map headers for robustness (a foreign-encoder snapshot).
//   - 0xf4 false, 0xf5 true, 0xf6 null, 0xf7 undefined.
//
// The byte rules MIRROR `engine::gate::cbor::encode` (the B0 encoder) — they MUST
// stay in lockstep. Rather than share a constants module across the frozen B0
// encoder and this decoder (a refactor that would touch byte-frozen B0 code), the
// agreement is GUARANTEED by the `cbor_and_gz_roundtrip_drives_identical_diff`
// differential (encode a real snapshot via B0, decode here, re-run the diff →
// byte-identical golden) plus the round-trip unit oracles below. Any drift between
// encoder and decoder fails those tests.
// ===========================================================================

/// Maximum CBOR array/map nesting depth. Real snapshots nest only a handful of
/// levels (e.g. `capabilityFacts[].extra.tempState`); 256 is far above that yet
/// well below any stack-overflow risk, so a crafted nesting bomb yields `Err`,
/// never a process abort.
const MAX_CBOR_DEPTH: usize = 256;

struct Decoder<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Decoder<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Decoder { buf, pos: 0 }
    }

    fn read_u8(&mut self) -> Result<u8, String> {
        let b = *self
            .buf
            .get(self.pos)
            .ok_or_else(|| "CBOR: unexpected end of input".to_string())?;
        self.pos += 1;
        Ok(b)
    }

    fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| "CBOR: length overflow".to_string())?;
        if end > self.buf.len() {
            return Err("CBOR: unexpected end of input (truncated)".to_string());
        }
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    /// Read the argument for a given additional-info value (low 5 bits of the
    /// initial byte). Only the forms B0 emits (0..23, 24/25/26) are supported.
    fn read_argument(&mut self, info: u8) -> Result<u64, String> {
        match info {
            0..=23 => Ok(info as u64),
            24 => Ok(self.read_u8()? as u64),
            25 => {
                let b = self.read_bytes(2)?;
                Ok(u16::from_be_bytes([b[0], b[1]]) as u64)
            }
            26 => {
                let b = self.read_bytes(4)?;
                Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as u64)
            }
            27 => {
                let b = self.read_bytes(8)?;
                Ok(u64::from_be_bytes([
                    b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
                ]))
            }
            _ => Err(format!("CBOR: unsupported additional-info {info}")),
        }
    }

    fn decode_value(&mut self, depth: usize) -> Result<CborValue, String> {
        // Depth guard: array/map nesting recurses, and the release profile builds with
        // `panic = "abort"` — an unbounded recursion on a crafted (trivially small on
        // disk) deeply-nested header stream would overflow the stack and ABORT the
        // process, breaching "engine never panics on untrusted input". Real snapshots
        // nest only a handful of levels; cap well above that.
        if depth > MAX_CBOR_DEPTH {
            return Err("CBOR: nesting too deep".to_string());
        }
        let initial = self.read_u8()?;
        let major = initial >> 5;
        let info = initial & 0x1f;
        match major {
            0 => {
                // unsigned int.
                let n = self.read_argument(info)?;
                Ok(int_to_cbor(n, false))
            }
            1 => {
                // negative int: value = -1 - n.
                let n = self.read_argument(info)?;
                Ok(int_to_cbor(n, true))
            }
            3 => {
                // text string.
                let len = self.read_argument(info)? as usize;
                let bytes = self.read_bytes(len)?;
                let s = std::str::from_utf8(bytes)
                    .map_err(|e| format!("CBOR: invalid UTF-8 text: {e}"))?;
                Ok(CborValue::Text(s.to_string()))
            }
            4 => {
                // array.
                let len = self.read_argument(info)? as usize;
                let mut items = Vec::with_capacity(len.min(1024));
                for _ in 0..len {
                    items.push(self.decode_value(depth + 1)?);
                }
                Ok(CborValue::Array(items))
            }
            5 => {
                // map. info 25 (0xb9) is B0's always-map-16; others tolerated.
                let len = self.read_argument(info)? as usize;
                let mut m: IndexMap<String, CborValue> = IndexMap::new();
                for _ in 0..len {
                    let key = self.decode_value(depth + 1)?;
                    let key_str = match key {
                        CborValue::Text(s) => s,
                        other => {
                            return Err(format!("CBOR: non-text map key {other:?}"));
                        }
                    };
                    let val = self.decode_value(depth + 1)?;
                    m.insert(key_str, val);
                }
                Ok(CborValue::Map(m))
            }
            7 => {
                // simple values + floats.
                match info {
                    20 => Ok(CborValue::Bool(false)),
                    21 => Ok(CborValue::Bool(true)),
                    22 => Ok(CborValue::Null),
                    23 => Ok(CborValue::Undefined),
                    25 => {
                        // half-float (cbor-x never emits this for snapshots, but decode).
                        let b = self.read_bytes(2)?;
                        let h = u16::from_be_bytes([b[0], b[1]]);
                        Ok(CborValue::Float(half_to_f64(h)))
                    }
                    26 => {
                        let b = self.read_bytes(4)?;
                        let f = f32::from_be_bytes([b[0], b[1], b[2], b[3]]);
                        Ok(CborValue::Float(f as f64))
                    }
                    27 => {
                        let b = self.read_bytes(8)?;
                        let f =
                            f64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
                        Ok(float_to_cbor(f))
                    }
                    _ => Err(format!("CBOR: unsupported simple value {info}")),
                }
            }
            _ => Err(format!("CBOR: unsupported major type {major}")),
        }
    }
}

/// An unsigned argument → an `Int` (or `Float` when it overflows i64; the corpus
/// never hits this).
fn int_to_cbor(n: u64, negative: bool) -> CborValue {
    if negative {
        // value = -1 - n.
        if n <= i64::MAX as u64 {
            CborValue::Int(-1 - n as i64)
        } else {
            CborValue::Float(-1.0 - n as f64)
        }
    } else if n <= i64::MAX as u64 {
        CborValue::Int(n as i64)
    } else {
        CborValue::Float(n as f64)
    }
}

/// A decoded f64 → `Int` when it is integral and fits i64 (cbor-x encodes large
/// integers as `0xfb` f64; the snapshot has no genuine non-integer numbers, so an
/// integral f64 round-trips back to `Int`), else `Float`.
fn float_to_cbor(f: f64) -> CborValue {
    if f.is_finite() && f == f.trunc() && f.abs() < 9.007_199_254_740_992e15 {
        CborValue::Int(f as i64)
    } else {
        CborValue::Float(f)
    }
}

/// IEEE-754 half-precision → f64 (only used for the tolerated 0xf9 form).
fn half_to_f64(h: u16) -> f64 {
    let sign = (h >> 15) & 1;
    let exp = (h >> 10) & 0x1f;
    let frac = h & 0x3ff;
    let val = if exp == 0 {
        (frac as f64) * 2f64.powi(-24)
    } else if exp == 0x1f {
        if frac == 0 { f64::INFINITY } else { f64::NAN }
    } else {
        (1.0 + (frac as f64) / 1024.0) * 2f64.powi(exp as i32 - 15)
    };
    if sign == 1 { -val } else { val }
}

/// Decode a CBOR byte stream into a [`CborValue`]. Trailing bytes after the
/// top-level value are ignored (the snapshot is a single top-level object).
pub fn cbor_decode(bytes: &[u8]) -> Result<CborValue, String> {
    let mut d = Decoder::new(bytes);
    d.decode_value(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::gate::cbor::encode;

    fn m(pairs: &[(&str, CborValue)]) -> CborValue {
        let mut map = IndexMap::new();
        for (k, v) in pairs {
            map.insert((*k).to_string(), v.clone());
        }
        CborValue::Map(map)
    }

    fn structural_eq(a: &CborValue, b: &CborValue) -> bool {
        match (a, b) {
            (CborValue::Null, CborValue::Null) => true,
            (CborValue::Undefined, CborValue::Undefined) => true,
            (CborValue::Bool(x), CborValue::Bool(y)) => x == y,
            (CborValue::Int(x), CborValue::Int(y)) => x == y,
            (CborValue::Float(x), CborValue::Float(y)) => x == y,
            (CborValue::Text(x), CborValue::Text(y)) => x == y,
            (CborValue::Array(x), CborValue::Array(y)) => {
                x.len() == y.len() && x.iter().zip(y).all(|(p, q)| structural_eq(p, q))
            }
            (CborValue::Map(x), CborValue::Map(y)) => {
                x.len() == y.len()
                    && x.iter().all(|(k, v)| match y.get(k) {
                        Some(w) => structural_eq(v, w),
                        None => false,
                    })
            }
            _ => false,
        }
    }

    #[test]
    fn roundtrip_simple_object() {
        let v = m(&[
            ("schemaVersion", CborValue::Int(V)),
            ("name", CborValue::Text("hello".into())),
            ("flag", CborValue::Bool(true)),
            ("none", CborValue::Null),
            ("undef", CborValue::Undefined),
            (
                "arr",
                CborValue::Array(vec![
                    CborValue::Int(1),
                    CborValue::Int(300),
                    CborValue::Int(-100),
                ]),
            ),
        ]);
        let encoded = encode(&v);
        let decoded = cbor_decode(&encoded).expect("decode");
        assert!(structural_eq(&v, &decoded), "decoded={decoded:?}");
    }

    #[test]
    fn roundtrip_nested() {
        let inner = m(&[("a", CborValue::Int(65536)), ("b", CborValue::Int(255))]);
        let v = m(&[
            ("schemaVersion", CborValue::Int(V)),
            ("nested", inner),
            (
                "list",
                CborValue::Array(vec![m(&[("x", CborValue::Text("y".into()))])]),
            ),
        ]);
        let encoded = encode(&v);
        let decoded = cbor_decode(&encoded).expect("decode");
        assert!(structural_eq(&v, &decoded));
    }

    #[test]
    fn truncated_input_errors_not_panics() {
        let v = m(&[("k", CborValue::Text("hello".into()))]);
        let encoded = encode(&v);
        // Chop the stream at every prefix length — none may panic.
        for n in 0..encoded.len() {
            let _ = cbor_decode(&encoded[..n]);
        }
    }

    #[test]
    fn nesting_bomb_errors_not_aborts() {
        // A crafted deeply-nested array stream: N `0x81` (array, len 1) headers, each
        // wrapping the next, then an int leaf. Trivially small on disk but would
        // recurse N deep — past MAX_CBOR_DEPTH it must yield Err, NOT overflow the
        // stack (which, under release `panic="abort"`, would abort the process).
        let mut bomb = vec![0x81u8; 100_000];
        bomb.push(0x00); // leaf int 0
        let r = cbor_decode(&bomb);
        assert!(r.is_err(), "deep nesting must error, got {r:?}");
        assert_eq!(r.unwrap_err(), "CBOR: nesting too deep");

        // A map nesting bomb too: N `0xa1 61 6b` (map len 1, key "k") then a leaf.
        let mut map_bomb = Vec::new();
        for _ in 0..100_000 {
            map_bomb.extend_from_slice(&[0xa1, 0x61, 0x6b]); // {"k": <next>}
        }
        map_bomb.push(0x00);
        let r2 = cbor_decode(&map_bomb);
        assert!(r2.is_err(), "deep map nesting must error, got {r2:?}");
        assert_eq!(r2.unwrap_err(), "CBOR: nesting too deep");

        // A shallow nesting just under the cap still decodes (no false positive).
        let mut ok = vec![0x81u8; 200];
        ok.push(0x00);
        assert!(cbor_decode(&ok).is_ok(), "200-deep must still decode");
    }

    #[test]
    fn detect_format_works() {
        assert_eq!(detect_format(b"{\"a\":1}"), SnapshotFormat::Json);
        assert_eq!(detect_format(&[0x1f, 0x8b, 0x08]), SnapshotFormat::CborGz);
        assert_eq!(detect_format(&[0xb9, 0x00, 0x00]), SnapshotFormat::Cbor);
    }

    /// The build's snapshot schema version (#220: the one pub constant).
    const V: i64 = crate::engine::l5::snapshot_full::SNAPSHOT_SCHEMA_VERSION;

    #[test]
    fn deserialize_json_raw() {
        let json = format!(
            r#"{{"schemaVersion":{V},"alsemVersion":"cli-b-v1","analysisUniverse":"single-app","apps":[]}}"#
        );
        let tree = deserialize_snapshot(json.as_bytes(), None).expect("deserialize");
        let CborValue::Map(m) = tree else {
            panic!("not a map")
        };
        assert!(matches!(m.get("schemaVersion"), Some(CborValue::Int(v)) if *v == V));
    }

    #[test]
    fn deserialize_json_enveloped() {
        let json = format!(
            r#"{{"kind":"capability-snapshot","schemaVersion":"1.1.0","alsemVersion":"cli-b-v1","deterministic":true,"generatedAt":"1970-01-01T00:00:00Z","diagnostics":[],"payload":{{"snapshotSchemaVersion":{V},"analysisUniverse":"single-app","apps":[]}}}}"#
        );
        let tree = deserialize_snapshot(json.as_bytes(), None).expect("deserialize");
        let CborValue::Map(m) = tree else {
            panic!("not a map")
        };
        // schemaVersion lifted from snapshotSchemaVersion.
        assert!(matches!(m.get("schemaVersion"), Some(CborValue::Int(v)) if *v == V));
        assert!(matches!(m.get("alsemVersion"), Some(CborValue::Text(s)) if s == "cli-b-v1"));
    }

    /// #220 (D5.1): schema 3 is refused too, with the remedy.
    #[test]
    fn wrong_schema_version_errors() {
        for v in [2, 3] {
            let json = format!(r#"{{"schemaVersion":{v}}}"#);
            let err = deserialize_snapshot(json.as_bytes(), None)
                .expect_err(&format!("schemaVersion {v} is refused"));
            if v == 3 {
                assert!(
                    err.contains("re-create it with 'alsem fingerprint'"),
                    "{err}"
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // Task T2.2: the gunzip decompression cap — previously an UNBOUNDED
    // `read_to_end`, the classic zip-bomb vector for this surface.
    // -----------------------------------------------------------------------

    /// A genuinely-over-cap gzip stream (real DEFLATE, not a crafted header)
    /// must be capped mid-read and surfaced through `deserialize_snapshot`'s
    /// existing `Result<_, String>` — never a panic, never an unbounded
    /// allocation. Mirrors the brief's "gunzip path equivalent" TDD fixture.
    #[test]
    fn oversized_gz_snapshot_is_rejected_not_panicking() {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write as _;

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        // Compressible zeros well past the cap — a small compressed footprint
        // hiding a huge inflate, same shape as the zip fixtures elsewhere in
        // this task.
        const CHUNK: usize = 1024 * 1024;
        let chunk = vec![0u8; CHUNK];
        let mut remaining = crate::capped_io::SNAPSHOT_GZ_CAP as usize + 1024;
        while remaining > 0 {
            let n = remaining.min(CHUNK);
            encoder.write_all(&chunk[..n]).unwrap();
            remaining -= n;
        }
        let gz_bytes = encoder.finish().unwrap();

        let result = deserialize_snapshot(&gz_bytes, Some(SnapshotFormat::CborGz));
        assert!(
            result.is_err(),
            "an over-cap gz snapshot must be rejected, not silently truncated"
        );
        assert!(
            result.unwrap_err().contains("gunzip failed"),
            "error should name the surface"
        );
    }

    /// A normal, well-under-cap gz-encoded snapshot round-trips unaffected by
    /// the cap — the cap must never perturb legitimate output.
    #[test]
    fn normal_gz_snapshot_round_trips_under_cap() {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write as _;

        let v = m(&[
            ("schemaVersion", CborValue::Int(V)),
            ("analysisUniverse", CborValue::Text("single-app".into())),
            ("name", CborValue::Text("hello".into())),
        ]);
        let cbor_bytes = encode(&v);

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&cbor_bytes).unwrap();
        let gz_bytes = encoder.finish().unwrap();

        // Auto-detect must recognize the gzip magic bytes too.
        assert_eq!(detect_format(&gz_bytes), SnapshotFormat::CborGz);

        let tree = deserialize_snapshot(&gz_bytes, None).expect("well under cap");
        let CborValue::Map(map) = tree else {
            panic!("not a map")
        };
        assert!(matches!(map.get("schemaVersion"), Some(CborValue::Int(v)) if *v == V));
        assert!(matches!(map.get("name"), Some(CborValue::Text(s)) if s == "hello"));
    }
}
