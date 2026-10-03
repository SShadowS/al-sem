//! Source-identity verification: source is only "sound" if it provably
//! matches the artifact under analysis; mismatch fails closed.

use crate::snapshot::identity::AppId;
use crate::snapshot::provider::SourceRoot;

/// Outcome of checking that a source root matches the app it claims to be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdentityCheck {
    /// Identity corroborated (e.g. git commit / source hash recorded).
    Verified,
    /// Id/version match but no strong corroboration — usable, never "sound".
    Approximate(String),
    /// Wrong app/version — must fall back to symbol-only.
    Mismatch(String),
}

/// Verify a LOCAL source root against the expected app identity (from app.json
/// or the matched `.app`). Embedded source is implicitly bound to its `.app`
/// and does not pass through here.
pub fn verify_local_source(
    app: &AppId,
    _root: &SourceRoot,
    expected: Option<&AppId>,
) -> IdentityCheck {
    let Some(exp) = expected else {
        return IdentityCheck::Approximate("no expected app.json identity to compare".into());
    };
    if exp.guid != app.guid && !exp.guid.is_empty() && !app.guid.is_empty() {
        return IdentityCheck::Mismatch(format!("guid {} != {}", app.guid, exp.guid));
    }
    if exp.version != app.version {
        return IdentityCheck::Mismatch(format!(
            "version {} != expected {}",
            app.version, exp.version
        ));
    }
    // Id+version match; no commit/source-hash corroboration yet -> approximate.
    IdentityCheck::Approximate("id+version match; no build corroboration".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::identity::{AppId, TrustTier};
    use crate::snapshot::provider::SourceRoot;

    fn app(v: &str) -> AppId {
        AppId {
            guid: "g".into(),
            name: "Core".into(),
            publisher: "Continia".into(),
            version: v.into(),
        }
    }
    fn root() -> SourceRoot {
        SourceRoot {
            files: Default::default(),
            tier: TrustTier::LocalSourceApproximate,
            content_hash: "h".into(),
        }
    }

    #[test]
    fn matching_version_verifies() {
        let r = verify_local_source(&app("29.0.0.0"), &root(), Some(&app("29.0.0.0")));
        assert!(matches!(
            r,
            IdentityCheck::Verified | IdentityCheck::Approximate(_)
        ));
    }

    #[test]
    fn version_mismatch_fails_closed() {
        let r = verify_local_source(&app("29.0.0.0"), &root(), Some(&app("28.0.0.0")));
        assert!(matches!(r, IdentityCheck::Mismatch(_)));
    }

    #[test]
    fn no_expected_identity_is_approximate() {
        let r = verify_local_source(&app("29.0.0.0"), &root(), None);
        assert!(matches!(r, IdentityCheck::Approximate(_)));
    }

    #[test]
    fn guid_mismatch_fails_closed() {
        let mut other = app("29.0.0.0");
        other.guid = "different-guid".into();
        let r = verify_local_source(&app("29.0.0.0"), &root(), Some(&other));
        assert!(matches!(r, IdentityCheck::Mismatch(_)));
    }

    #[test]
    fn empty_guid_skips_guid_check() {
        let mut no_guid_app = app("29.0.0.0");
        no_guid_app.guid = "".into();
        let mut no_guid_exp = app("29.0.0.0");
        no_guid_exp.guid = "".into();
        let r = verify_local_source(&no_guid_app, &root(), Some(&no_guid_exp));
        assert!(matches!(r, IdentityCheck::Approximate(_)));
    }
}
