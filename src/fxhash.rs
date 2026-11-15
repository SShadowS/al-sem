//! ⟨TLS-66⟩ A fast, unkeyed hasher for the engine's hot maps.
//!
//! The std `HashMap` hashes with SipHash-1-3 under a random key: built to resist
//! hash flooding by an attacker choosing the keys. The engine's keys come from the
//! workspace and its packages, and the CDO model build spent 12.6% of its CPU
//! samples hashing (TLS-66's profile: `String`, `reads::Query`, `ObligationId` and
//! `RoutineNodeId` keys; 43% of it rehashing as maps grew). This is the hasher
//! rustc uses for the same reason: the multiply-rotate "FxHash" of rustc-hash 1.x
//! (`rustc_hash::FxHasher`, after Firefox's), written out here rather than added
//! as a dependency (the owner's mantra 14).
//!
//! It has no random key, so iteration order is the same in every process. No
//! byte-compared output may depend on a hash map's order anyway (std's is random
//! per process); this hasher does not make such an order meaningful.
//!
//! The trade (review r1, Fable): with no key, colliding names can be built offline,
//! so a workspace or package crafted with very many of them could make a build
//! slow (CPU only). That is accepted for maps keyed by workspace and package
//! content: it needs a workspace the user chose to open, and a very large file
//! costs as much. Keep std's `RandomState` for keys from anywhere else (a network
//! peer). Use it through the aliases [`FxHashMap`] and [`FxHashSet`].

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

/// rustc-hash 1.x's 64-bit multiplier.
const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

/// The FxHash state: one word, folded with each input word by rotate, xor and
/// multiply.
#[derive(Default, Clone, Copy)]
pub struct FxHasher {
    hash: u64,
}

impl FxHasher {
    #[inline]
    fn add_to_hash(&mut self, word: u64) {
        self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(SEED);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        // Eight bytes at a time, then four, two and one, always little-endian:
        // rustc-hash 1.x reads native-endian words, so this is its 64-bit
        // little-endian variant (equal on x86-64 and aarch64; review r1, sol).
        let mut chunks = bytes.chunks_exact(8);
        for c in &mut chunks {
            self.add_to_hash(u64::from_le_bytes(c.try_into().expect("8 bytes")));
        }
        let mut rest = chunks.remainder();
        if rest.len() >= 4 {
            self.add_to_hash(u64::from(u32::from_le_bytes(
                rest[..4].try_into().expect("4 bytes"),
            )));
            rest = &rest[4..];
        }
        if rest.len() >= 2 {
            self.add_to_hash(u64::from(u16::from_le_bytes(
                rest[..2].try_into().expect("2 bytes"),
            )));
            rest = &rest[2..];
        }
        if let Some(&b) = rest.first() {
            self.add_to_hash(u64::from(b));
        }
    }
    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add_to_hash(u64::from(i));
    }
    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.add_to_hash(u64::from(i));
    }
    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add_to_hash(u64::from(i));
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add_to_hash(i);
    }
    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add_to_hash(i as u64);
    }
    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }
}

/// The `BuildHasher` of [`FxHasher`] (no state, so `Default` builds it).
pub type FxBuildHasher = BuildHasherDefault<FxHasher>;
/// A `HashMap` hashed with [`FxHasher`]: `FxHashMap::default()`, or
/// `FxHashMap::with_capacity_and_hasher(n, Default::default())`.
pub type FxHashMap<K, V> = HashMap<K, V, FxBuildHasher>;
/// A `HashSet` hashed with [`FxHasher`].
pub type FxHashSet<T> = HashSet<T, FxBuildHasher>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::BuildHasher;

    fn fx(f: impl FnOnce(&mut FxHasher)) -> u64 {
        let mut h = FxHasher::default();
        f(&mut h);
        h.finish()
    }

    /// The algorithm pinned on values derived by hand from rustc-hash 1.x's
    /// definition (`hash = (hash.rotl(5) ^ word) * SEED`, starting at 0), so a
    /// change to the rotate, the multiplier or the byte order fails here.
    /// The exact value guards no output (iteration order never reaches one); it
    /// keeps the hasher the one this doc names.
    ///
    /// Mutations (2026-11-15), each red here: the rotate 5 -> 6 (the exact
    /// `[1, 2, 3]` value fails); the 8-byte word read big-endian (the 15-byte
    /// case fails).
    #[test]
    fn fxhash_matches_its_definition() {
        // One word from 0: (0 ^ 1) * SEED = SEED.
        assert_eq!(fx(|h| h.write_u64(1)), SEED);
        // Two words: (SEED.rotl(5) ^ 2) * SEED.
        assert_eq!(
            fx(|h| {
                h.write_u64(1);
                h.write_u64(2);
            }),
            (SEED.rotate_left(5) ^ 2).wrapping_mul(SEED)
        );
        // Bytes: one little-endian word of 8, then 4, 2 and 1 (15 bytes).
        let bytes: Vec<u8> = (1..=15).collect();
        let mut want = FxHasher::default();
        want.add_to_hash(u64::from_le_bytes([1, 2, 3, 4, 5, 6, 7, 8]));
        want.add_to_hash(u64::from(u32::from_le_bytes([9, 10, 11, 12])));
        want.add_to_hash(u64::from(u16::from_le_bytes([13, 14])));
        want.add_to_hash(15);
        assert_eq!(fx(|h| h.write(&bytes)), want.finish());
        // Exact values, computed outside Rust from the definition (Python, 64-bit
        // wrapping), so `add_to_hash` itself is pinned too: [1, 2, 3] is the u16
        // 0x0201 then the byte 3.
        assert_eq!(fx(|h| h.write(&[1, 2, 3])), 0xd8f6_f76f_4682_00f2);
        assert_eq!(fx(|h| h.write_u8(0xff)), 0x2b44_f56f_fae8_8a6b);
    }

    /// Unkeyed: the same input hashes alike from any two builders (std's
    /// `RandomState` does not), and a map behaves as a map.
    #[test]
    fn fxhash_is_unkeyed_and_maps_work() {
        let (a, b) = (FxBuildHasher::default(), FxBuildHasher::default());
        assert_eq!(a.hash_one("CDO E-Mail"), b.hash_one("CDO E-Mail"));
        assert_ne!(a.hash_one("CDO E-Mail"), a.hash_one("CDO E-Mail "));
        let mut m: FxHashMap<String, u32> = FxHashMap::default();
        for (i, k) in ["a", "b", "c", "a"].iter().enumerate() {
            m.insert((*k).to_string(), i as u32);
        }
        assert_eq!((m.len(), m["a"], m["b"]), (3, 3, 1));
        let mut s: FxHashSet<u64> = FxHashSet::with_capacity_and_hasher(4, Default::default());
        assert!(s.insert(7) && !s.insert(7));
    }
}
