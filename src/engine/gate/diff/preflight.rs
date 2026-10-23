//! Preflight checks: schema-version / analysis-universe / alsem-version /
//! app-identity. Port of al-sem `src/diff/diff-preflight.ts` (the universe
//! check is #220's).

use crate::engine::gate::cbor::CborValue;
use crate::program::model::role::Role;

use super::{CoveragePolicy, DiffDiagnostic, get_array, get_str};

pub struct PreflightResult {
    pub diagnostics: Vec<DiffDiagnostic>,
    pub fatal: bool,
}

/// The snapshot's `schemaVersion` as an integer (the deserialized tree carries it
/// as `Int`).
fn schema_version(snap: &CborValue) -> Option<i64> {
    match snap {
        CborValue::Map(m) => match m.get("schemaVersion") {
            Some(CborValue::Int(n)) => Some(*n),
            _ => None,
        },
        _ => None,
    }
}

/// The primary app guid: the `apps[]` row whose `role` is `primary`, never
/// `apps[0]` (#220 D5.4), if any.
fn primary_app_id(snap: &CborValue) -> Option<String> {
    get_array(snap, "apps")?
        .iter()
        .find(|a| get_str(a, "role") == Some(Role::Primary.as_str()))
        .and_then(|a| get_str(a, "appGuid"))
        .map(str::to_string)
}

pub fn run_preflight(
    old_snap: &CborValue,
    new_snap: &CborValue,
    coverage_policy: CoveragePolicy,
) -> PreflightResult {
    let mut diagnostics: Vec<DiffDiagnostic> = Vec::new();
    let mut fatal = false;

    // Schema version mismatch — always fatal.
    let old_sv = schema_version(old_snap);
    let new_sv = schema_version(new_snap);
    if old_sv != new_sv {
        diagnostics.push(DiffDiagnostic {
            kind: "schema-version-mismatch".into(),
            fields: vec![
                (
                    "kind".into(),
                    CborValue::Text("schema-version-mismatch".into()),
                ),
                ("old".into(), CborValue::Int(old_sv.unwrap_or(0))),
                ("new".into(), CborValue::Int(new_sv.unwrap_or(0))),
            ],
        });
        fatal = true;
    }

    // #220 D5.3: analysis universe mismatch — always fatal.
    let old_u = get_str(old_snap, "analysisUniverse").unwrap_or("");
    let new_u = get_str(new_snap, "analysisUniverse").unwrap_or("");
    if old_u != new_u {
        diagnostics.push(DiffDiagnostic {
            kind: "analysis-universe-mismatch".into(),
            fields: vec![
                (
                    "kind".into(),
                    CborValue::Text("analysis-universe-mismatch".into()),
                ),
                ("old".into(), CborValue::Text(old_u.to_string())),
                ("new".into(), CborValue::Text(new_u.to_string())),
            ],
        });
        fatal = true;
    }

    // alsemVersion difference — warning loose, error strict.
    let old_av = get_str(old_snap, "alsemVersion").unwrap_or("");
    let new_av = get_str(new_snap, "alsemVersion").unwrap_or("");
    if old_av != new_av {
        let severity = if coverage_policy == CoveragePolicy::Strict {
            "error"
        } else {
            "warning"
        };
        diagnostics.push(DiffDiagnostic {
            kind: "alsem-version-mismatch".into(),
            fields: vec![
                (
                    "kind".into(),
                    CborValue::Text("alsem-version-mismatch".into()),
                ),
                ("old".into(), CborValue::Text(old_av.to_string())),
                ("new".into(), CborValue::Text(new_av.to_string())),
                ("severity".into(), CborValue::Text(severity.into())),
            ],
        });
        if severity == "error" {
            fatal = true;
        }
    }

    // App identity mismatch — always fatal.
    let old_app = primary_app_id(old_snap);
    let new_app = primary_app_id(new_snap);
    if let (Some(o), Some(n)) = (&old_app, &new_app)
        && o != n
    {
        diagnostics.push(DiffDiagnostic {
            kind: "app-identity-mismatch".into(),
            fields: vec![
                (
                    "kind".into(),
                    CborValue::Text("app-identity-mismatch".into()),
                ),
                ("oldAppId".into(), CborValue::Text(o.clone())),
                ("newAppId".into(), CborValue::Text(n.clone())),
            ],
        });
        fatal = true;
    }

    PreflightResult { diagnostics, fatal }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn txt(s: &str) -> CborValue {
        CborValue::Text(s.to_string())
    }

    fn map(pairs: Vec<(&str, CborValue)>) -> CborValue {
        CborValue::Map(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    /// A v4 tree whose `apps[]` lists the dependency `0000…d0` FIRST and the
    /// primary second (T8's shape).
    fn side(primary: &str) -> CborValue {
        let app = |guid: &str, role: &str| map(vec![("appGuid", txt(guid)), ("role", txt(role))]);
        map(vec![
            ("schemaVersion", CborValue::Int(4)),
            ("alsemVersion", txt("v")),
            ("analysisUniverse", txt("cross-app")),
            (
                "apps",
                CborValue::Array(vec![
                    app("00000000-0000-0000-0000-0000000000d0", "dependency"),
                    app(primary, "primary"),
                ]),
            ),
        ])
    }

    /// T8b (#220 spec §5; O15): app identity reads the `apps[]` row whose
    /// `role` is `primary`, never `apps[0]`. Two sides with different
    /// primaries and the SAME `apps[0]` raise `app-identity-mismatch`.
    ///
    /// Break: read `apps[0]` -> no diagnostic.
    #[test]
    fn preflight_identity_reads_role() {
        let old = side("aaaa0220-0000-0000-0000-0000000000aa");
        let new = side("cccc0220-0000-0000-0000-0000000000cc");
        assert_eq!(
            format!("{:?}", get_array(&old, "apps").and_then(|a| a.first())),
            format!("{:?}", get_array(&new, "apps").and_then(|a| a.first())),
            "precondition: the same apps[0]"
        );
        let r = run_preflight(&old, &new, CoveragePolicy::Loose);
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.kind == "app-identity-mismatch"),
            "{:?}",
            r.diagnostics.iter().map(|d| &d.kind).collect::<Vec<_>>()
        );
        assert!(r.fatal);
    }
}
