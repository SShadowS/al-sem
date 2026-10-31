//! Compare `|`-joined sort keys without building them.
//!
//! Several sorts in the engine order rows by a key made of fields joined with
//! `|` (the snapshot's `capability_fact_sort_key`, the cone's
//! `inherited_output_sort_key`, policy's `fact_sort_key`). That order is a
//! byte order over the JOINED string, which is NOT the order of the fields
//! compared one by one: resource ids `a` and `ab` join to `a|…` and `ab|…`,
//! and `'|'` (0x7C) sorts after `'b'`, so `ab` comes first, while a field-wise
//! (tuple) comparison puts `a` first. A field may also contain `|` itself.
//!
//! [`cmp_joined`] gives exactly `a.join("|").cmp(&b.join("|"))` for every
//! input, without allocating. Building one key `String` per row cost one heap
//! allocation per row (17 M rows on the cross-app CDO snapshot).

use std::cmp::Ordering;

/// `a.join("|").cmp(&b.join("|"))`, without building either string.
///
/// The common case stays field-wise: equal fields are skipped, and at the first
/// unequal pair the answer is their first differing byte when both fields have
/// a byte there. Only when one field is a proper prefix of the other does the
/// virtual `|` after the shorter one take part; then the rest of both keys is
/// compared byte by byte, delimiters included.
pub(crate) fn cmp_joined(a: &[&str], b: &[&str]) -> Ordering {
    let n = a.len().min(b.len());
    for i in 0..n {
        let (x, y) = (a[i].as_bytes(), b[i].as_bytes());
        if x == y {
            continue;
        }
        let common = x.iter().zip(y).take_while(|(p, q)| p == q).count();
        if common < x.len() && common < y.len() {
            return x[common].cmp(&y[common]);
        }
        // One field is a proper prefix of the other: the next byte of the
        // shorter key is a virtual `|` (or its end), so the field boundaries
        // no longer line up. Everything before field `i` was equal, `|`
        // included, so the rest of the two keys decides.
        return joined_bytes(&a[i..]).cmp(joined_bytes(&b[i..]));
    }
    // The first `n` fields are equal: the key with more fields continues with
    // `|` where the other ends.
    a.len().cmp(&b.len())
}

/// The bytes of `fields.join("|")`, lazily.
fn joined_bytes<'a>(fields: &'a [&'a str]) -> impl Iterator<Item = u8> + 'a {
    fields
        .iter()
        .enumerate()
        .flat_map(|(k, f)| (k > 0).then_some(b'|').into_iter().chain(f.bytes()))
}

#[cfg(test)]
mod tests {
    use super::cmp_joined;
    use std::cmp::Ordering;

    fn joined(a: &[&str], b: &[&str]) -> Ordering {
        a.join("|").cmp(&b.join("|"))
    }

    /// T2 (cone-memory design §10): the cases that tell a joined order from a
    /// tuple order, one by one, each against the real `join("|")`.
    /// DISCRIMINATION: a tuple compare (`a.cmp(b)` on the slices) fails the
    /// `a`/`ab` case; dropping the virtual `|` (comparing the concatenation)
    /// fails the `"a|"` against `("a", "")` case.
    #[test]
    fn t2_named_cases_match_join() {
        let cases: &[(&[&str], &[&str])] = &[
            // the prefix trap: tuple order says a < ab, joined says ab < a
            (&["a", "z"], &["ab", "z"]),
            (&["s", "a", "x"], &["s", "ab", "x"]),
            // a field containing the delimiter
            (&["a|", "b"], &["a", "|b"]),
            (&["a|"], &["a", ""]),
            (&["a", "|"], &["a|", ""]),
            // empty fields
            (&["", ""], &[""]),
            (&["", "a"], &["", ""]),
            (&[""], &["a"]),
            // different field counts
            (&["a"], &["a", "b"]),
            (&["a", "b"], &["a"]),
            (&["ab"], &["a", "b"]),
            // equal
            (&["x", "y"], &["x", "y"]),
            // non-ASCII bytes above '|'
            (&["a", "é"], &["a", "~"]),
            (&["aé"], &["a", "é"]),
        ];
        for (a, b) in cases {
            assert_eq!(cmp_joined(a, b), joined(a, b), "{a:?} vs {b:?}");
            assert_eq!(cmp_joined(b, a), joined(b, a), "{b:?} vs {a:?}");
        }
        // The trap is real: the tuple order disagrees on the first case.
        assert_eq!(
            [&"a", &"z"].cmp(&[&"ab", &"z"]),
            Ordering::Less,
            "precondition: the tuple order puts a first"
        );
        assert_eq!(
            cmp_joined(&["a", "z"], &["ab", "z"]),
            Ordering::Greater,
            "the joined order puts ab first"
        );
    }

    /// T2, exhaustive: every pair of three-field keys over
    /// `{"", "a", "ab", "|", "a|", "b"}` (216 keys, 46,656 pairs), plus every
    /// pair of keys of one to three fields over that alphabet, agrees with
    /// `join("|")`.
    #[test]
    fn t2_exhaustive_small_alphabet_matches_join() {
        const ALPHABET: [&str; 6] = ["", "a", "ab", "|", "a|", "b"];
        let mut keys: Vec<Vec<&str>> = Vec::new();
        for len in 1..=3usize {
            let mut ix = vec![0usize; len];
            loop {
                keys.push(ix.iter().map(|&i| ALPHABET[i]).collect());
                let mut k = 0;
                while k < len {
                    ix[k] += 1;
                    if ix[k] < ALPHABET.len() {
                        break;
                    }
                    ix[k] = 0;
                    k += 1;
                }
                if k == len {
                    break;
                }
            }
        }
        assert_eq!(keys.len(), 6 + 36 + 216, "precondition: every key built");
        let mut disagree_with_tuple = 0usize;
        for a in &keys {
            for b in &keys {
                assert_eq!(cmp_joined(a, b), joined(a, b), "{a:?} vs {b:?}");
                if a.cmp(b) != joined(a, b) {
                    disagree_with_tuple += 1;
                }
            }
        }
        assert!(
            disagree_with_tuple > 0,
            "precondition: the alphabet has pairs where tuple and joined orders differ"
        );
    }
}
