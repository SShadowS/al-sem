//! AL identifier case-folding — the single choke point every consumer (LSP surface,
//! program engine, legacy L3) funnels through when it needs to compare or store an AL
//! identifier case-insensitively.
//!
//! AL identifiers are case-insensitive, and the AL compiler compares them like .NET
//! `StringComparison.OrdinalIgnoreCase` (ENG-131, measured with alc 18.0.41 on Linux;
//! Windows confirmation pending): two chars are one when their simple UPPERCASE mappings
//! are equal, and no non-ASCII char maps onto ASCII. This fold gives each such class one
//! char ([`fold_char`]), so binding, graph node ids and stable routine ids (which fold
//! with it too since ENG-131) agree with the compiler and with each other. The measured
//! pairs and the probe apps: `docs/superpowers/notes/2026-11-18-eng131-compiler-identifier-fold.md`.
//!
//! It is deliberately NOT `str::to_lowercase()`, which applies Unicode's FULL, context-
//! sensitive lowercase mapping: `İ` becomes two chars (`i` + U+0307), a word-final `Σ`
//! becomes `ς` but a medial one `σ`, and it merges pairs the compiler keeps apart (`ẞ`
//! with `ß`, the Kelvin sign with `k`) while keeping apart pairs it merges (`ς` with `σ`
//! in mid-word, micro `µ` with Greek `μ`).
//!
//! Behavior on the characters that matter:
//! - Latin letters in real BC identifiers (`Æ/æ Ø/ø Å/å Ä/ä Ö/ö Ü/ü Ñ/ñ Ç/ç È/è É/é`) fold to
//!   their lowercase, as always.
//! - `ß` stays `ß`, and the capital `ẞ` stays `ẞ`: two identifiers, as alc says. `ß` never
//!   folds to `ss`.
//! - `Σ`, `σ` and `ς` are one identifier char, `σ`; micro `µ` and Greek `μ` are one, `μ`.
//! - `İ` (U+0130) stays `İ`, apart from `i`; dotless `ı` (U+0131) stays `ı`; the Kelvin
//!   sign stays apart from `k`.
//!
//! ASCII fast path: for an all-ASCII input, `fold_identifier` is byte-identical to
//! `str::to_ascii_lowercase()` — today's behavior, unchanged for the overwhelming
//! majority of real AL source (measured: DO's entire primary source tree is 100% ASCII;
//! see the investigation doc). The `is_ascii()` guard is a single bulk byte scan, so the
//! hot resolver-join paths keep their current cost (~1.03–1.07x, measured, within noise).

/// Case-fold an AL identifier for case-insensitive comparison/storage.
///
/// ASCII fast path: identical to `s.to_ascii_lowercase()`. Non-ASCII path: a per-char
/// simple 1:1 Unicode case fold (see module docs for exact semantics, including the `ß`
/// and `İ`/`ı` behavior). Returns an owned `String` — every real call site swapped onto
/// this helper immediately stores or compares the result as an owned string (a
/// `name_lc` field, an index key, …), matching `to_ascii_lowercase`'s existing contract
/// 1:1 and avoiding `Cow`-unwrap boilerplate at the ~150+ call sites this replaces.
#[inline]
pub fn fold_identifier(s: &str) -> String {
    if s.is_ascii() {
        s.to_ascii_lowercase()
    } else {
        s.chars().map(fold_char).collect()
    }
}

/// Case-insensitive AL identifier equality — the allocation-free comparison path
/// mirroring `str::eq_ignore_ascii_case`'s role. ASCII fast path (both inputs ASCII):
/// identical to `a.eq_ignore_ascii_case(b)`. Mixed/non-ASCII path: compares the two folds
/// char by char, which equals `fold_identifier(a) == fold_identifier(b)` because the fold
/// maps every char to exactly one char. Never allocates (ENG-84: folding both sides into
/// `String`s here was ~1.2% of the CDO model build's CPU, Danish names in scope lookups).
#[inline]
pub fn eq_fold_identifier(a: &str, b: &str) -> bool {
    if a.is_ascii() && b.is_ascii() {
        a.eq_ignore_ascii_case(b)
    } else {
        a.chars().map(fold_char).eq(b.chars().map(fold_char))
    }
}

/// The compiler's fold for one char (ENG-131, measured with alc 18.0.41): two chars are
/// one identifier char when their simple UPPERCASE mappings are equal, as .NET
/// `StringComparison.OrdinalIgnoreCase` compares, except that a non-ASCII char never maps
/// onto ASCII (`ı` U+0131 and `ſ` U+017F stay apart from `i` and `s`). Each class is
/// written as its single lowercase char when one maps back to the class (`Σ`, `σ`, `ς` →
/// `σ`; micro `µ` and Greek `μ` → `μ`; `Ø` → `ø`), else as the uppercase char itself
/// (`İ`, `ẞ`, the Kelvin sign `K`), so every ASCII and nearly every other char keeps the
/// lowercase form it always had. The table of measured pairs is in
/// `docs/superpowers/notes/2026-11-18-eng131-compiler-identifier-fold.md`.
#[inline]
fn fold_char(c: char) -> char {
    if c.is_ascii() {
        return c.to_ascii_lowercase();
    }
    let upper = simple_upper(c);
    if upper.is_ascii() {
        return c; // `ı`, `ſ`: the compiler keeps them apart from ASCII
    }
    let lower = simple_lower(upper);
    if simple_upper(lower) == upper && !lower.is_ascii() {
        lower
    } else {
        upper
    }
}

/// Unicode's simple (1:1) uppercase mapping. `char::to_uppercase` gives the FULL mapping,
/// which differs from the simple one only where it expands to several chars: those keep
/// the char itself, except the Greek letters with a subscript iota, whose simple uppercase
/// is their titlecase form (`ᾳ` → `ᾼ`, `UnicodeData.txt`).
fn simple_upper(c: char) -> char {
    let mut up = c.to_uppercase();
    match (up.next(), up.next()) {
        (Some(u), None) => u,
        _ => match c as u32 {
            0x1F80..=0x1F87 | 0x1F90..=0x1F97 | 0x1FA0..=0x1FA7 => {
                char::from_u32(c as u32 + 8).unwrap_or(c)
            }
            0x1FB3 | 0x1FC3 | 0x1FF3 => char::from_u32(c as u32 + 9).unwrap_or(c),
            _ => c,
        },
    }
}

/// Unicode's simple (1:1) lowercase mapping: the full mapping when it is one char (every
/// char but `İ`, whose full lowercase is `i` + U+0307), else the char itself.
fn simple_lower(c: char) -> char {
    let mut low = c.to_lowercase();
    match (low.next(), low.next()) {
        (Some(l), None) => l,
        _ => c,
    }
}

/// Ergonomic method-call sugar for [`fold_identifier`]/[`eq_fold_identifier`], so the
/// large mechanical swap of existing `.to_ascii_lowercase()` / `.eq_ignore_ascii_case(…)`
/// call sites across the engine can rename in place (`.fold_identifier()` /
/// `.eq_fold_identifier(…)`) instead of restructuring every call into prefix
/// free-function form. Both methods are one-line delegations — the free functions above
/// remain the single choke point; this trait adds no behavior, only call-site syntax.
pub trait IdentifierFoldExt {
    /// See [`fold_identifier`].
    fn fold_identifier(&self) -> String;
    /// See [`eq_fold_identifier`].
    fn eq_fold_identifier(&self, other: &str) -> bool;
}

impl IdentifierFoldExt for str {
    #[inline]
    fn fold_identifier(&self) -> String {
        fold_identifier(self)
    }

    #[inline]
    fn eq_fold_identifier(&self, other: &str) -> bool {
        eq_fold_identifier(self, other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- ASCII identity: fold_identifier must match to_ascii_lowercase exactly ---

    #[test]
    fn ascii_matches_to_ascii_lowercase_property_style() {
        // Property-style sweep over every printable ASCII byte plus a handful of
        // realistic AL identifier shapes — the ASCII fast path must be byte-identical
        // to today's `to_ascii_lowercase()` behavior for every one of them.
        for b in 0u8..=127 {
            let s = (b as char).to_string();
            assert_eq!(fold_identifier(&s), s.to_ascii_lowercase(), "byte {b}");
        }
        let idents = [
            "Codeunit50000",
            "MyProcedure",
            "OnAfterGetRecord",
            "Rec.FIELDNAME",
            "SetRange",
            "",
            "_underscore_Name123",
            "ALL_CAPS_IDENT",
        ];
        for s in idents {
            assert_eq!(fold_identifier(s), s.to_ascii_lowercase());
        }
    }

    #[test]
    fn ascii_fast_path_taken_for_ascii_input() {
        // Not observable from the outside (no Cow to inspect), but pin the contract:
        // an ASCII input never touches the non-ASCII per-char branch by asserting
        // idempotence + exact ascii-lowercase equality together (covered above); this
        // test documents the intent for future readers.
        assert_eq!(fold_identifier("ABCxyz123"), "abcxyz123");
    }

    // --- Danish Ø/Æ/Å fold pairs (the arc's headline case) ---

    #[test]
    fn danish_oe_ae_aa_fold_pairs() {
        assert_eq!(fold_identifier("Løbenr"), fold_identifier("LØBENR"));
        assert_eq!(fold_identifier("Løbenr"), fold_identifier("løbenr"));
        assert_eq!(fold_identifier("Æble"), fold_identifier("ÆBLE"));
        assert_eq!(fold_identifier("Æble"), fold_identifier("æble"));
        assert_eq!(fold_identifier("Åben"), fold_identifier("ÅBEN"));
        assert_eq!(fold_identifier("Åben"), fold_identifier("åben"));
        // Concretely: Ø folds to ø.
        assert_eq!(fold_identifier("Ø"), "ø");
        assert_eq!(fold_identifier("Æ"), "æ");
        assert_eq!(fold_identifier("Å"), "å");
    }

    // --- German ß behavior, pinned ---

    #[test]
    fn german_sharp_s_stays_sharp_s() {
        // ß has no uppercase 1:1 partner under a simple fold; it is fixed.
        assert_eq!(fold_identifier("ß"), "ß");
        assert_eq!(fold_identifier("Straße"), "straße");
        assert_eq!(fold_identifier("STRASSE"), "strasse");
        // ß-spelled and SS-spelled variants are DIFFERENT identifiers under a simple
        // fold (the compiler does not perform the ß->SS linguistic expansion) —
        // documenting the boundary, not a bug.
        assert_ne!(fold_identifier("Straße"), fold_identifier("STRASSE"));
        // Ö/Ü fold normally alongside ß in the same identifier.
        assert_eq!(fold_identifier("GRÖSSE"), "grösse");
        assert_eq!(fold_identifier("Größe"), "größe");
    }

    // --- Turkish İ/ı, pinned ---

    #[test]
    fn turkish_capital_i_with_dot_stays_itself_one_char() {
        // İ (U+0130) is its own identifier char to alc ("İx" and "ix" compile side by
        // side; `"ix"()` does not bind to "İx"), and one char, never `to_lowercase()`'s
        // two (`i` + U+0307).
        let folded = fold_identifier("İ");
        assert_eq!(folded, "İ");
        assert_ne!(fold_identifier("İx"), fold_identifier("ix"));
        assert_eq!("İ".to_lowercase().chars().count(), 2);
    }

    /// ENG-131: every pair alc 18.0.41 was asked about (two procedures of the pair in one
    /// codeunit: AL0440 = one identifier; a call by the other spelling: AL0118 = two), and
    /// the fold agrees with each. Linux alc; Windows confirmation pending.
    #[test]
    fn the_fold_agrees_with_the_compiler_on_every_measured_pair() {
        let one = [
            ("Ix", "ix"),
            ("ΒΣ", "Βς"),
            ("Σx", "ςx"),
            ("µx", "μx"), // U+00B5 micro, U+03BC Greek
            // Greek subscript iota: the simple uppercase is the titlecase letter (Fable,
            // measured with alc: AL0440 for both pairs).
            ("ᾳx", "ᾼx"),
            ("ᾀx", "ᾈx"),
        ];
        let two = [
            ("İx", "ix"),
            ("aß", "aẞ"),
            ("\u{212A}x", "kx"), // Kelvin sign
            ("ıx", "ix"),
        ];
        for (a, b) in one {
            assert_eq!(fold_identifier(a), fold_identifier(b), "{a:?} = {b:?}");
            assert!(eq_fold_identifier(a, b), "{a:?} = {b:?}");
        }
        for (a, b) in two {
            assert_ne!(fold_identifier(a), fold_identifier(b), "{a:?} ≠ {b:?}");
            assert!(!eq_fold_identifier(a, b), "{a:?} ≠ {b:?}");
        }
        // The class representatives: lowercase where one maps back, ASCII unchanged.
        assert_eq!(fold_identifier("ΒΣ"), "βσ");
        assert_eq!(fold_identifier("µx"), "μx");
        assert_eq!(fold_identifier("ẞ"), "ẞ");
        assert_eq!(fold_identifier("\u{212A}"), "\u{212A}");
    }

    /// The number of scalars whose fold is not themselves. The classes follow the
    /// toolchain's Unicode tables (`char::to_uppercase`), so a Rust upgrade that adds case
    /// pairs moves stable ids of names using them: this count makes that a visible change
    /// (Fable: Unicode 17's Beria Erfe pairs, which .NET 10 and alc keep apart).
    #[test]
    fn the_number_of_chars_the_fold_changes_is_pinned() {
        let n = (0..=0x10FFFFu32)
            .filter_map(char::from_u32)
            .filter(|&c| fold_char(c) != c)
            .count();
        // Measured 2026-11-19, Rust's Unicode 17 tables (26 of them ASCII `A`-`Z`).
        assert_eq!(n, 1503, "chars whose fold is not themselves");
    }

    /// The fold is idempotent and one char per char on every scalar value, and never maps a
    /// non-ASCII char onto ASCII.
    #[test]
    fn the_fold_is_one_char_idempotent_and_keeps_non_ascii_off_ascii() {
        for c in (0..=0x10FFFFu32).filter_map(char::from_u32) {
            let f = fold_char(c);
            assert_eq!(fold_char(f), f, "{c:?} → {f:?}");
            assert_eq!(c.is_ascii(), f.is_ascii(), "{c:?} → {f:?}");
        }
    }

    #[test]
    fn turkish_dotless_i_folds_to_itself() {
        assert_eq!(fold_identifier("ı"), "ı");
    }

    // --- Mixed-script ---

    #[test]
    fn mixed_script_identifier() {
        assert_eq!(fold_identifier("KæreFrLbl"), "kærefrlbl");
        assert_eq!(fold_identifier("EstimadaSeñoraLbl"), "estimadaseñoralbl");
        assert_eq!(fold_identifier("TürkiyeLbl"), "türkiyelbl");
        assert_eq!(
            fold_identifier("PrüfungPRÜFUNG"),
            fold_identifier("prüfungprüfung")
        );
    }

    // --- Idempotence ---

    #[test]
    fn idempotent_ascii_and_non_ascii() {
        for s in ["MyProcedure", "Løbenr", "Größe", "İstanbul", "KæreFrLbl"] {
            let once = fold_identifier(s);
            let twice = fold_identifier(&once);
            assert_eq!(once, twice, "fold_identifier must be idempotent for {s:?}");
        }
    }

    // --- eq_fold_identifier consistency with fold_identifier ---

    #[test]
    fn eq_fold_consistent_with_fold_ascii() {
        assert!(eq_fold_identifier("Løbenr", "LØBENR"));
        assert!(eq_fold_identifier("MyProc", "MYPROC"));
        assert!(!eq_fold_identifier("MyProc", "OtherProc"));
    }

    #[test]
    fn eq_fold_consistent_with_fold_non_ascii() {
        let pairs: &[(&str, &str)] = &[
            ("Løbenr", "LØBENR"),
            ("Løbenr", "løbenr"),
            ("Größe", "GRÖße"),
            ("ß", "ß"),
            ("İ", "İ"),
            ("Σ", "ς"),
        ];
        for (a, b) in pairs {
            assert_eq!(
                eq_fold_identifier(a, b),
                fold_identifier(a) == fold_identifier(b),
                "eq_fold_identifier({a:?}, {b:?}) must agree with fold_identifier equality"
            );
            assert!(eq_fold_identifier(a, b), "{a:?} vs {b:?} should fold-equal");
        }
        assert!(!eq_fold_identifier("Løbenr", "Æble"));
    }

    /// ENG-84: the allocation-free char-by-char path answers exactly what comparing the two
    /// folded strings answers, for every pair: prefixes, mixed ASCII/non-ASCII, a char whose
    /// fold changes its byte length (`µ` 2 bytes → `μ` 2, `ς` → `σ`), and `İ` (`to_lowercase`
    /// is 2 chars).
    #[test]
    fn eq_fold_matches_folded_string_equality_on_every_pair() {
        let samples = [
            "", "a", "A", "Lobenr", "Løbenr", "LØBENR", "Løb", "Løbenrx", "Æble", "æble", "ẞ", "ß",
            "SS", "İ", "i", "I", "ı", "Größe", "GRÖSSE", "Σ", "σ", "ς", "µ", "μ", "\u{212A}", "k",
        ];
        for a in samples {
            for b in samples {
                assert_eq!(
                    eq_fold_identifier(a, b),
                    fold_identifier(a) == fold_identifier(b),
                    "{a:?} vs {b:?}"
                );
            }
        }
    }

    #[test]
    fn extension_trait_delegates_exactly() {
        for s in ["MyProc", "Løbenr", "Größe", "İ"] {
            assert_eq!(s.fold_identifier(), fold_identifier(s));
        }
        assert!("Løbenr".eq_fold_identifier("LØBENR"));
        assert_eq!(
            "Løbenr".eq_fold_identifier("Æble"),
            eq_fold_identifier("Løbenr", "Æble")
        );
    }
}
