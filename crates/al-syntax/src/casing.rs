//! AL identifier case-folding — the single choke point every consumer (LSP surface,
//! program engine, legacy L3) funnels through when it needs to compare or store an AL
//! identifier case-insensitively.
//!
//! AL identifiers are case-insensitive. The real AL compiler's fold is best-evidence a
//! **simple, culture-invariant, 1:1 Unicode case fold** — `StringComparison
//! .OrdinalIgnoreCase`-class or Roslyn `CaseInsensitiveComparison`-class; the two agree
//! on every letter that occurs in real BC identifiers (the Latin-1 Supplement and
//! Latin-Extended ranges: `Æ/æ Ø/ø Å/å Ä/ä Ö/ö Ü/ü Ñ/ñ Ç/ç È/è É/é`, plus `ß`). See
//! `.superpowers/sdd/unicode-fold-investigation.md` for the full evidence trail.
//!
//! This is deliberately **NOT** `str::to_lowercase()`. `to_lowercase()` performs
//! Unicode's *full* case mapping (`SpecialCasing.txt`), which for exactly one
//! codepoint — `İ` (U+0130, LATIN CAPITAL LETTER I WITH DOT ABOVE) — is a **1:n**
//! mapping (`İ` → `i` + U+0307 COMBINING DOT ABOVE, two chars; verified empirically by
//! scanning every Unicode scalar value for a multi-char `to_lowercase()` result — U+0130
//! is the *only* one). A 1:n fold breaks the "identifier folds to one canonical string"
//! invariant every fold call site in this codebase relies on, and is a *linguistic*
//! casing operation the compiler does not perform on symbol names (matching .NET
//! `OrdinalIgnoreCase`, which never applies full/special casing). `str::to_lowercase()`
//! also differs on one context-sensitive case: a word-final capital sigma `Σ` (U+03A3)
//! becomes `ς`, where this fold gives `σ` (TLS-42 measured both, over every scalar).
//!
//! Fold behavior on the characters that matter:
//! - **`ß` (German sharp S) stays `ß`.** Its only case transform is an *uppercase*
//!   expansion (`ß` → `SS`), never touched by a lowercase fold; `ß` has no distinct
//!   uppercase letter to fold *from*, so it is fixed under this fold (`ß` is
//!   fold-equivalent only to `ß`, never to `ss`).
//! - **`İ` (Turkish dotted capital I) folds to plain `i` (U+0069), not `i̇`
//!   (U+0069 U+0307).** We take `char::to_lowercase(c).next()` — for every codepoint
//!   except `İ` this is exactly the (single-char) full mapping; for `İ` specifically it
//!   yields just `i`, discarding the combining-dot tail. That single-char result also
//!   happens to equal Unicode's *simple* (non-special) lowercase mapping for `İ`
//!   (`UnicodeData.txt`'s `Simple_Lowercase_Mapping` field), so `.next()` is a simple
//!   fold everywhere, not an ad-hoc truncation. Practical note: `İ` merging with plain
//!   `i`/`I` is harmless for AL — Turkish `İ` does not occur in any real BC/CDO/DO
//!   identifier population sampled for this change.
//! - **`ı` (Turkish dotless lowercase i, U+0131) folds to itself** — it is already
//!   lowercase and has no uppercase 1:1 partner under a simple fold.
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
/// identical to `a.eq_ignore_ascii_case(b)`, no allocation. Mixed/non-ASCII path: folds
/// both sides via [`fold_identifier`] and compares (the only case where this allocates —
/// real AL source is overwhelmingly ASCII, so this path is rarely taken).
#[inline]
pub fn eq_fold_identifier(a: &str, b: &str) -> bool {
    if a.is_ascii() && b.is_ascii() {
        a.eq_ignore_ascii_case(b)
    } else {
        fold_identifier(a) == fold_identifier(b)
    }
}

/// Simple 1:1 lowercase fold for one char. See the module doc for the `İ` rationale —
/// this is the *only* codepoint (of all of Unicode, empirically verified) where
/// `char::to_lowercase()` yields more than one char; `.next()` degrades it to a single
/// char everywhere, and that single char is a simple (non-linguistic) fold.
#[inline]
fn fold_char(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
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
    fn turkish_capital_i_with_dot_folds_to_plain_i_not_two_chars() {
        // İ (U+0130) must NOT become "i̇" (U+0069 U+0307, two chars) — that is
        // `str::to_lowercase()`'s 1:n special-casing behavior, explicitly rejected.
        let folded = fold_identifier("İ");
        assert_eq!(folded.chars().count(), 1, "İ must fold to exactly one char");
        assert_eq!(folded, "i");
        // Sanity: str::to_lowercase() DOES take the 1:n path (pins the contrast).
        assert_eq!("İ".to_lowercase().chars().count(), 2);
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
            ("İ", "i"),
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
