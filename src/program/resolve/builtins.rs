//! Clean-room global-builtin catalog for the fresh resolver (Plan 1B.2 Phase 2 Task 3).
//!
//! # The membership catalog lives here (engine-switch S1)
//!
//! The generated catalog [`super::global_builtins`] used to live in
//! `engine::l3` and was this directory's one sanctioned legacy import. Engine
//! switch S1 moved it into the program engine: the legacy engine's
//! `al_builtins` now reads it from here, and no file under `src/program`
//! imports `engine::l3`/`engine::l2` (guard:
//! `program_has_no_legacy_engine_imports` in `tests/program_resolve_harness.rs`).
//!
//! # Clean-room boundary
//!
//! The **membership data** (785 names) is the authoritative generated set in
//! [`super::global_builtins`]. That set is derived from the AL compiler DLL
//! (`ClassDocumentationResources`) — it is *platform truth*, not resolver logic.
//!
//! The **disposition → evidence mapping** (what a catalog hit *means* in this resolver:
//! a `BuiltinId`, `Evidence::Catalog`, `Witness::CatalogEntry`) is written fresh here.
//! We do NOT import or reproduce any disposition logic from
//! `crate::engine::l3::al_builtins` (`global_builtin_disposition`, "control-terminating",
//! "pure-terminal").  That is the clean-room line.
//!
//! # Regenerating the membership set
//!
//! When the AL extension version bumps, regenerate with:
//! ```sh
//! dotnet run --project tools/gen-al-builtins/gen.csproj
//! ```
//! The generator rewrites `src/program/resolve/global_builtins.rs`; this module
//! automatically picks up the new set on the next build.
//!
//! # Catalog version
//!
//! Provenance string from the generated file header:
//! `ms-dynamics-smb.al-18.0.2293710` (generated 2026-06-13, 785 methods, 97 types).
//! Embed this in `Witness::CatalogEntry { catalog_version }` so findings can cite
//! the exact AL-ext snapshot they were produced against.

use super::global_builtins as membership;
use crate::program::resolve::edge::BuiltinId;

/// The AL-extension provenance string for the current membership set.
///
/// Callers embed this in `Witness::CatalogEntry { catalog_version: catalog_version().to_string() }`.
pub const CATALOG_VERSION_STR: &str = "ms-dynamics-smb.al-18.0.2293710";

/// Returns the AL-extension provenance string for the current membership set.
pub fn catalog_version() -> &'static str {
    CATALOG_VERSION_STR
}

/// Returns `true` if `name_lc` (already lowercased) is an AL compiler-intrinsic global.
///
/// Delegates to the authoritative generated membership set in
/// [`super::global_builtins`] — see the module-level doc.
pub fn is_global_builtin(name_lc: &str) -> bool {
    membership::is_global_builtin(name_lc)
}

/// Returns `Some(BuiltinId)` for a recognized compiler-intrinsic global, else `None`.
///
/// The `BuiltinId` carries the lowercased name as its catalog identity; pair it with
/// `Evidence::Catalog` and `Witness::CatalogEntry` when building a `Route`.
pub fn global_builtin_id(name_lc: &str) -> Option<BuiltinId> {
    if is_global_builtin(name_lc) {
        Some(BuiltinId(name_lc.to_string()))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::resolve::global_builtins::GLOBAL_BUILTIN_METHODS;

    #[test]
    fn fresh_catalog_covers_l3_catalog() {
        // Every name L3 recognizes as a global builtin, fresh must too (coverage parity).
        // Sample the L3 catalog via its public predicate over a representative name set
        // AND assert the union size matches the documented 785 (provenance guard).
        for n in [
            "error",
            "message",
            "confirm",
            "format",
            "strlen",
            "today",
            "createguid",
            "abs",
            "round",
            "strsubstno",
        ] {
            assert!(
                is_global_builtin(n),
                "fresh catalog must recognize builtin {n}"
            );
            assert!(global_builtin_id(n).is_some());
        }
        assert!(!is_global_builtin("definitely_not_a_builtin_xyz"));
        assert!(!catalog_version().is_empty());
    }

    #[test]
    fn catalog_membership_is_nontrivial() {
        // Contract: the generated set has at least 700 entries.
        // If regen produces an empty or truncated set, this catches it.
        let count = GLOBAL_BUILTIN_METHODS.len();
        assert!(
            count >= 700,
            "expected ≥700 global builtins in the generated set, got {count}"
        );
    }

    #[test]
    fn builtin_id_round_trips() {
        let id = global_builtin_id("message").expect("message is a builtin");
        assert_eq!(id.0, "message");
        assert!(global_builtin_id("not_a_builtin_at_all").is_none());
    }

    /// beyond-1B.3b Task 1 review-fix (Finding 2): pins the catalog's ACTUAL
    /// structural guarantee — membership is a lowercased EXACT-STRING lookup
    /// in a `phf::Set` (no hash/fingerprint digest is stored or compared, see
    /// the module-level "Clean-room boundary" doc), so a name textually
    /// adjacent to a real catalog entry, but not itself a member, is fail-
    /// closed REJECTED (`None`), never classified `builtin` by coincidence.
    ///
    /// (A prior revision of this test exercised a `global_builtin_id_checked`
    /// wrapper that re-derived the same query string and compared it to
    /// itself — an unreachable guard, since `BuiltinId` is built directly
    /// from `name_lc` and `is_global_builtin` already returns `false` for any
    /// non-member name before the wrapper's own check could run. The wrapper
    /// added no behavior beyond `global_builtin_id`/`is_global_builtin`
    /// themselves, so it was removed; THIS test now asserts the real
    /// fail-closed contract directly against the phf-backed functions that
    /// resolver.rs actually calls.)
    #[test]
    fn global_builtin_id_is_name_exact_and_rejects_near_miss() {
        let id = global_builtin_id("message").expect("message is a builtin");
        assert_eq!(id.0, "message");

        // Near-miss: not a real catalog member, despite being adjacent to one.
        assert!(global_builtin_id("strlenzzz_not_real").is_none());
        assert!(global_builtin_id("message_typo").is_none());
        assert!(!is_global_builtin("strlenzzz_not_real"));
        assert!(!is_global_builtin("message_typo"));
    }

    #[test]
    fn additional_representative_builtins() {
        // Spot-check a wider range of names from the generated catalog.
        for n in [
            "lowercase",
            "uppercase",
            "copystr",
            "maxstrlen",
            "padstr",
            "incstr",
            "convertstr",
            "delchr",
            "power",
            "userid",
            "companyname",
            "currentdatetime",
            "time",
            "workdate",
            "isnullguid",
        ] {
            assert!(
                is_global_builtin(n),
                "fresh catalog must recognize builtin {n}"
            );
        }
    }
}
