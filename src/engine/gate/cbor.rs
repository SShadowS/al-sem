//! Hand-rolled CBOR encoder — byte-compatible with `cbor-x` 1.6.4 encoding
//! `Encoder({ useRecords:false, mapsAsObjects:true, pack:false }).encode(obj)`.
//!
//! al-sem serializes the `CapabilitySnapshot` to CBOR via cbor-x for the
//! `.cbor` / `.cbor.gz` snapshot goldens (Stage cli-b). No Rust CBOR crate
//! reproduces cbor-x's NON-CANONICAL choices (always-map-16 header, insertion
//! key order, undefined slots kept), so this module implements the exact byte
//! rules, proven against cbor-x by the `cbor_oracles` tests below.
//!
//! ## Exact cbor-x byte rules (proven against cbor-x 1.6.4)
//!
//! - object → `0xb9` + `(count as u16).to_be_bytes()` ALWAYS (the non-canonical
//!   always-map-16 header, even for 0/1 entries), then each entry: key as a
//!   text-string, value. Keys in INSERTION order; keys are text strings.
//! - integer → smallest CBOR int (major 0 for >=0, major 1 for <0 encoding -1-n).
//! - non-integer number → ALWAYS `0xfb` + `f64::to_be_bytes()` (no float32/16).
//! - string → major 3 (`0x60|len` / `0x78` / `0x79` / `0x7a`) + UTF-8 bytes.
//! - array → major 4 (`0x80|len` / `0x98` / `0x99` / `0x9a`) + elements.
//! - bool → `0xf5`/`0xf4`, null → `0xf6`, undefined → `0xf7` (KEY KEPT — emit the
//!   `f7` slot, the same as cbor-x with `mapsAsObjects:true`).
//! - no tags.

use indexmap::IndexMap;

/// An insertion-ordered CBOR value model. Objects MUST preserve insertion order
/// (never a sorted/Hash map) — CBOR byte parity depends on the key order, which
/// mirrors al-sem's `composeSnapshot` literal key order.
#[derive(Debug, Clone)]
pub enum CborValue {
    /// A JSON/JS `null` → `0xf6`.
    Null,
    /// A JS `undefined` → `0xf7`. The KEY is still emitted (cbor-x keeps the slot
    /// under `mapsAsObjects:true`).
    Undefined,
    Bool(bool),
    /// An integer (encoded as the smallest CBOR int).
    Int(i64),
    /// A non-integer number — ALWAYS encoded as `0xfb` + f64 BE bytes.
    Float(f64),
    Text(String),
    Array(Vec<CborValue>),
    /// An object — insertion-ordered key/value pairs. Keys are text strings.
    Map(IndexMap<String, CborValue>),
}

impl CborValue {
    /// Convenience: an empty insertion-ordered map.
    pub fn map() -> CborValue {
        CborValue::Map(IndexMap::new())
    }

    /// The heap bytes the tree owns, for the `batch.held` census (#277
    /// D6.3): text capacities and each array's and map's slots. A map's
    /// hash-index bytes are not counted (shallow there).
    pub fn heap_bytes(&self) -> u64 {
        let slot = std::mem::size_of::<CborValue>() as u64;
        match self {
            CborValue::Text(s) => s.capacity() as u64,
            CborValue::Array(v) => {
                v.capacity() as u64 * slot + v.iter().map(CborValue::heap_bytes).sum::<u64>()
            }
            CborValue::Map(m) => m
                .iter()
                .map(|(k, v)| {
                    let key = std::mem::size_of::<String>() as u64 + k.capacity() as u64;
                    key + slot + v.heap_bytes()
                })
                .sum(),
            _ => 0,
        }
    }
}

/// Encode a `CborValue` to its cbor-x-compatible byte form.
pub fn encode(value: &CborValue) -> Vec<u8> {
    let mut out = Vec::new();
    encode_into(value, &mut out);
    out
}

fn encode_into(value: &CborValue, out: &mut Vec<u8>) {
    match value {
        CborValue::Null => out.push(0xf6),
        CborValue::Undefined => out.push(0xf7),
        CborValue::Bool(true) => out.push(0xf5),
        CborValue::Bool(false) => out.push(0xf4),
        CborValue::Int(n) => encode_int(*n, out),
        CborValue::Float(f) => encode_f64(*f, out),
        CborValue::Text(s) => encode_text(s, out),
        CborValue::Array(items) => {
            encode_array_header(items.len(), out);
            for item in items {
                encode_into(item, out);
            }
        }
        CborValue::Map(entries) => {
            // ALWAYS the non-canonical map-16 header: 0xb9 + (count as u16) BE,
            // even for 0/1 entries (proven against cbor-x).
            //
            // MUST-FIX 3 — guard the u16 truncation: an object with >65535 keys
            // would silently wrap the count and corrupt the stream. Snapshot maps
            // are bounded (a fixed field set + per-routine frame tables), so this is
            // an invariant we ENFORCE, not hope for — `encode`/`encode_into` are
            // infallible by signature and called from dozens of snapshot-building
            // sites, so threading a `Result` through isn't the shallow fix; a plain
            // `assert!` (release-alive, unlike `debug_assert!` which compiles out
            // under `--release` and lets the wrap through) is the honest minimal fix.
            assert!(
                entries.len() <= u16::MAX as usize,
                "cbor-x map-16 header: >65535 keys unsupported (got {})",
                entries.len()
            );
            out.push(0xb9);
            let count = entries.len() as u16;
            out.extend_from_slice(&count.to_be_bytes());
            for (k, v) in entries {
                // Keys are ALWAYS text strings (smallest-length text header).
                //
                // SHOULD-FIX 4 — V8/cbor-x hoists integer-like STRING keys (e.g.
                // "0", "42") to the FRONT in ascending numeric order before the
                // insertion-order string keys. VERIFIED MOOT for the snapshot: every
                // object key is a fixed field name or a guid-prefixed StableRoutineId
                // (routineOrderFrames), none integer-like — so V8 iteration order ==
                // insertion order here, and we emit insertion order directly. (A map
                // with an integer-like key would need the hoisting; none exists.)
                encode_text(k, out);
                encode_into(v, out);
            }
        }
    }
}

/// Encode a numeric value cbor-x emits as a double — ALWAYS `0xfb` + f64 BE bytes
/// (no float32/16). Used for genuine non-integer numbers AND for integers whose
/// magnitude exceeds 2^32 (see [`encode_int`]). `-0.0` / NaN / ±Infinity would also
/// land here as their f64 form, but are unreachable-by-construction: the snapshot
/// has no non-integer numeric fields, and integer-valued `CborValue::Int` ≤ 2^32 in
/// the corpus never produces them.
fn encode_f64(f: f64, out: &mut Vec<u8>) {
    out.push(0xfb);
    out.extend_from_slice(&f.to_be_bytes());
}

/// Encode an integer the way cbor-x does. cbor-x NEVER emits the 8-byte int header
/// (`0x1b` / `0x3b`): at `|n| > 2^32` it switches to a `0xfb` f64 (proven against
/// cbor-x 1.6.4 — `2^32 → fb 41 f0 …`, `-2^32-1 → fb c1 f0 …`). So:
///   - `0 <= n <= 2^32-1`            → smallest unsigned int ladder (`00`..`1a`).
///   - `-(2^32) <= n < 0`            → smallest negative int ladder (`20`..`3a`).
///   - `|n|` beyond those bounds     → `0xfb` + `(n as f64)` BE.
fn encode_int(n: i64, out: &mut Vec<u8>) {
    if n >= 0 {
        if (n as u64) <= 0xffff_ffff {
            encode_uint_header(0x00, n as u64, out);
        } else {
            // > 2^32-1: cbor-x emits a double, not an 8-byte int.
            encode_f64(n as f64, out);
        }
    } else {
        // major 1 encodes -1-n (the magnitude minus one). The 4-byte negative
        // header covers down to -(2^32) (mag 2^32-1 → `3a ff ff ff ff`); anything
        // more negative (mag >= 2^32) becomes a double.
        let mag = (-(n + 1)) as u64;
        if mag <= 0xffff_ffff {
            encode_uint_header(0x20, mag, out);
        } else {
            encode_f64(n as f64, out);
        }
    }
}

/// Emit `major | argument` using the smallest CBOR additional-info encoding, for
/// arguments that FIT in 32 bits. `major` is the major-type bits already in the
/// high 3 bits (`0x00` unsigned, `0x20` negative, `0x60` text, `0x80` array). The
/// caller guarantees `n <= 0xffff_ffff` — cbor-x never uses the 8-byte (`0x1b`)
/// form (integers beyond 32 bits go through [`encode_f64`]), so there is no
/// `0x1b` branch here. `encode_text`/`encode_array_header` pass a real
/// string/array length through, so a >4GB string or array would violate this —
/// a plain `assert!` (release-alive; a `debug_assert!` here would compile out
/// under `--release` and let `(n as u32).to_be_bytes()` below silently
/// truncate, corrupting the header/data-length pairing) is the honest guard.
fn encode_uint_header(major: u8, n: u64, out: &mut Vec<u8>) {
    assert!(
        n <= 0xffff_ffff,
        "encode_uint_header: argument {n} exceeds 32 bits — must route through encode_f64"
    );
    if n <= 23 {
        out.push(major | (n as u8));
    } else if n <= 0xff {
        out.push(major | 0x18);
        out.push(n as u8);
    } else if n <= 0xffff {
        out.push(major | 0x19);
        out.extend_from_slice(&(n as u16).to_be_bytes());
    } else {
        out.push(major | 0x1a);
        out.extend_from_slice(&(n as u32).to_be_bytes());
    }
}

fn encode_text(s: &str, out: &mut Vec<u8>) {
    let bytes = s.as_bytes();
    encode_uint_header(0x60, bytes.len() as u64, out);
    out.extend_from_slice(bytes);
}

fn encode_array_header(len: usize, out: &mut Vec<u8>) {
    encode_uint_header(0x80, len as u64, out);
}

#[cfg(test)]
mod cbor_oracles {
    //! Hex-diff oracles for each encoding rule. The expected bytes are the
    //! VERBATIM output of cbor-x 1.6.4
    //! `Encoder({useRecords:false,mapsAsObjects:true,pack:false}).encode(x)`,
    //! captured by `bun -e 'import{Encoder}from"cbor-x";...'` and pinned here.
    use super::*;

    fn m(pairs: &[(&str, CborValue)]) -> CborValue {
        let mut map = IndexMap::new();
        for (k, v) in pairs {
            map.insert((*k).to_string(), v.clone());
        }
        CborValue::Map(map)
    }

    fn hex(bytes: &[u8]) -> String {
        bytes
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn empty_object_is_always_map16() {
        // cbor-x: {} → b9 00 00
        assert_eq!(hex(&encode(&CborValue::map())), "b9 00 00");
    }

    #[test]
    fn single_key_int() {
        // cbor-x: {a:1} → b9 00 01 61 61 01
        let v = m(&[("a", CborValue::Int(1))]);
        assert_eq!(hex(&encode(&v)), "b9 00 01 61 61 01");
    }

    #[test]
    fn bools_and_null() {
        // cbor-x: {a:true,b:false,c:null} → b9 00 03 61 61 f5 61 62 f4 61 63 f6
        let v = m(&[
            ("a", CborValue::Bool(true)),
            ("b", CborValue::Bool(false)),
            ("c", CborValue::Null),
        ]);
        assert_eq!(hex(&encode(&v)), "b9 00 03 61 61 f5 61 62 f4 61 63 f6");
    }

    #[test]
    fn undefined_value_keeps_key() {
        // cbor-x: {a:undefined,b:1} → b9 00 02 61 61 f7 61 62 01
        let v = m(&[("a", CborValue::Undefined), ("b", CborValue::Int(1))]);
        assert_eq!(hex(&encode(&v)), "b9 00 02 61 61 f7 61 62 01");
    }

    #[test]
    fn nested_array_with_big_int() {
        // cbor-x: {x:[1,2,300]} → b9 00 01 61 78 83 01 02 19 01 2c
        let v = m(&[(
            "x",
            CborValue::Array(vec![
                CborValue::Int(1),
                CborValue::Int(2),
                CborValue::Int(300),
            ]),
        )]);
        assert_eq!(hex(&encode(&v)), "b9 00 01 61 78 83 01 02 19 01 2c");
    }

    #[test]
    fn integer_boundaries() {
        // cbor-x: {a:23,b:24,c:255,d:256,e:65535,f:65536,g:-1,h:-100,i:-300} →
        // b9 00 09 61 61 17 61 62 18 18 61 63 18 ff 61 64 19 01 00 61 65 19 ff ff
        // 61 66 1a 00 01 00 00 61 67 20 61 68 38 63 61 69 39 01 2b
        let v = m(&[
            ("a", CborValue::Int(23)),
            ("b", CborValue::Int(24)),
            ("c", CborValue::Int(255)),
            ("d", CborValue::Int(256)),
            ("e", CborValue::Int(65535)),
            ("f", CborValue::Int(65536)),
            ("g", CborValue::Int(-1)),
            ("h", CborValue::Int(-100)),
            ("i", CborValue::Int(-300)),
        ]);
        assert_eq!(
            hex(&encode(&v)),
            "b9 00 09 61 61 17 61 62 18 18 61 63 18 ff 61 64 19 01 00 \
             61 65 19 ff ff 61 66 1a 00 01 00 00 61 67 20 61 68 38 63 61 69 39 01 2b"
                .replace("             ", "")
        );
    }

    #[test]
    fn float_is_always_f64() {
        // cbor-x: {a:1.5} → b9 00 01 61 61 fb 3f f8 00 00 00 00 00 00
        let v = m(&[("a", CborValue::Float(1.5))]);
        assert_eq!(
            hex(&encode(&v)),
            "b9 00 01 61 61 fb 3f f8 00 00 00 00 00 00"
        );
    }

    #[test]
    fn string_value() {
        // cbor-x: {k:"hello"} → b9 00 01 61 6b 65 68 65 6c 6c 6f
        let v = m(&[("k", CborValue::Text("hello".to_string()))]);
        assert_eq!(hex(&encode(&v)), "b9 00 01 61 6b 65 68 65 6c 6c 6f");
    }

    #[test]
    fn map_header_for_24_keys() {
        // cbor-x: 24-key object header → b9 00 18 …
        let mut map = IndexMap::new();
        for i in 0..24 {
            map.insert(format!("k{i}"), CborValue::Int(i));
        }
        let bytes = encode(&CborValue::Map(map));
        assert_eq!(hex(&bytes[..3]), "b9 00 18");
    }

    // -- MUST-FIX 1: integer >2^32 → f64 (cbor-x never emits the 8-byte int form) --

    #[test]
    fn integer_boundary_2pow32_switches_to_f64() {
        // cbor-x 1.6.4:
        //   2^32-1 (4294967295)  → 1a ff ff ff ff          (4-byte unsigned, kept)
        //   2^32   (4294967296)  → fb 41 f0 00 00 00 00 00 00   (f64)
        //   -2^32  (-4294967296) → 3a ff ff ff ff          (4-byte negative, kept)
        //   -2^32-1(-4294967297) → fb c1 f0 00 00 00 10 00 00   (f64)
        let v = m(&[
            ("a", CborValue::Int(4_294_967_295)),
            ("b", CborValue::Int(4_294_967_296)),
            ("c", CborValue::Int(-4_294_967_296)),
            ("d", CborValue::Int(-4_294_967_297)),
        ]);
        assert_eq!(
            hex(&encode(&v)),
            "b9 00 04 61 61 1a ff ff ff ff 61 62 fb 41 f0 00 00 00 00 00 00 \
             61 63 3a ff ff ff ff 61 64 fb c1 f0 00 00 00 10 00 00"
                .replace("             ", "")
        );
    }

    // -- SHOULD-FIX 5: unexercised header branches (array >23, string >255, mid ints) --

    #[test]
    fn array_header_above_23_uses_98() {
        // cbor-x: [0..30) → 98 1e + elements (the 1-byte-length array header).
        let v = m(&[("a", CborValue::Array((0..30).map(CborValue::Int).collect()))]);
        assert_eq!(
            hex(&encode(&v)),
            "b9 00 01 61 61 98 1e 00 01 02 03 04 05 06 07 08 09 0a 0b 0c 0d 0e 0f \
             10 11 12 13 14 15 16 17 18 18 18 19 18 1a 18 1b 18 1c 18 1d"
                .replace("             ", "")
        );
    }

    #[test]
    fn string_header_above_255_uses_79() {
        // cbor-x: a 300-byte string → 79 01 2c + 300 'x' bytes (2-byte-length text).
        let s = "x".repeat(300);
        let v = m(&[("s", CborValue::Text(s))]);
        let bytes = encode(&v);
        // header: b9 00 01  61 73 (key "s")  79 01 2c (text, len 300)
        assert_eq!(hex(&bytes[..8]), "b9 00 01 61 73 79 01 2c");
        // tail is 300 'x' (0x78) bytes.
        assert_eq!(bytes.len(), 8 + 300);
        assert!(bytes[8..].iter().all(|&b| b == 0x78));
    }

    #[test]
    fn mid_range_int_headers() {
        // cbor-x: {a:256,b:65536} → 19 01 00 (2-byte) / 1a 00 01 00 00 (4-byte).
        let v = m(&[("a", CborValue::Int(256)), ("b", CborValue::Int(65536))]);
        assert_eq!(
            hex(&encode(&v)),
            "b9 00 02 61 61 19 01 00 61 62 1a 00 01 00 00"
        );
    }

    // -- T2.4 (2): MUST-FIX 3's guard must be release-alive, not `debug_assert!` --

    #[test]
    #[should_panic(expected = "cbor-x map-16 header")]
    fn map_over_65535_keys_panics_instead_of_wrapping() {
        // A `debug_assert!` here compiles out under `--release`, letting
        // `entries.len() as u16` silently wrap to 0 and corrupt the stream. The
        // guard must be a plain `assert!` — release-alive in EVERY profile, so
        // this test's pass/fail is identical under `cargo test` and
        // `cargo test --release` (verified separately; not `cfg`-gated here).
        let mut map = IndexMap::new();
        for i in 0..(u16::MAX as usize + 1) {
            map.insert(i.to_string(), CborValue::Int(0));
        }
        let _ = encode(&CborValue::Map(map));
    }

    // -- T2.4 (4) sweep sibling: `encode_uint_header`'s ">32 bits" guard --

    #[test]
    #[should_panic(expected = "exceeds 32 bits")]
    fn uint_header_over_32_bits_panics_instead_of_truncating() {
        // Same shape as MUST-FIX 3 above: the OLD `debug_assert!` compiled out
        // under `--release`, letting `(n as u32).to_be_bytes()` silently
        // truncate a >32-bit length/magnitude to its low 32 bits — a header
        // that no longer matches the data that follows it (corrupted stream).
        // Reachable via `encode_text`/`encode_array_header` for a string or
        // array whose length exceeds `u32::MAX`. Call the boundary check
        // directly (no need to allocate a 4GB buffer to prove it).
        let mut out = Vec::new();
        encode_uint_header(0x60, u32::MAX as u64 + 1, &mut out);
    }
}
