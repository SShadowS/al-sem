//! Platform-native routines whose behaviour is known: a curated catalog.
//!
//! Microsoft's `System` symbol package (`8874ed3a-0643-4247-9ced-7a7002f7135d`)
//! declares codeunits whose methods are `[Native]`: the platform implements
//! them, so there is no AL body to analyse, and a call into one is an
//! `opaque-dependency` that blocks a `prove … no`. Most are harmless, but not
//! all (`Agent Utilities.DeleteCustomAgent`, `Index Management.DisableKey`,
//! `Feedback.RequestFeedback`), so `[Native]` alone is not a licence. This
//! catalog names the ones whose behaviour is known, and only those are
//! modelled: as a routine with a body and no database, UI, commit or
//! transaction effect, plus an implicit error where the method raises one on
//! bad input.
//!
//! Continia DO reaches `Base64Convert` through System Application's
//! `Base64 Convert Impl.`: 35 call sites in its prove claims' cones.

use super::workspace::ModelRoutine;

/// The `System` symbol package's app id.
pub const SYSTEM_APP: &str = "8874ed3a-0643-4247-9ced-7a7002f7135d";

/// What a cataloged native does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeBehaviour {
    /// The method raises an error on bad input (a malformed Base64 string).
    pub may_error: bool,
}

/// (object type, object id, method (lowercase), behaviour).
const CATALOG: &[(&str, i64, &str, NativeBehaviour)] = &[
    // Base64Convert: encoding and decoding text or streams.
    (
        "codeunit",
        2000000024,
        "tobase64",
        NativeBehaviour { may_error: false },
    ),
    (
        "codeunit",
        2000000024,
        "frombase64",
        NativeBehaviour { may_error: true },
    ),
];

/// The catalog entry for `r`, when it is a body-less routine of the `System`
/// package the catalog names.
pub fn pure_native(r: &ModelRoutine) -> Option<NativeBehaviour> {
    if r.body_available || !r.app_guid.eq_ignore_ascii_case(SYSTEM_APP) {
        return None;
    }
    let ty = r.object_type.to_ascii_lowercase();
    let name = r.name.to_ascii_lowercase();
    CATALOG
        .iter()
        .find(|(t, id, m, _)| *t == ty && *id == r.object_number && *m == name)
        .map(|(_, _, _, b)| *b)
}
