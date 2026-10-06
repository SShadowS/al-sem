//! Compute the al-sem GATE `modelInstanceId` — the content-derived id
//! `discoverSources` (al-sem `src/providers/discover.ts`) stamps onto every internal
//! RoutineId when `analyzeWorkspace` is called WITHOUT a `modelInstanceIdOverride`.
//!
//! WHY THE GATE NEEDS THIS (and the R4 dump does not): the al-sem R4 finding dump pins
//! `modelInstanceId="r0"`, and the Rust engine's default assembly also uses `"r0"`. But
//! the al-sem `analyze` CLI (which produced the gate-SARIF goldens via
//! `dump-gate-sarif.ts`) uses the UNPINNED, content-derived id. A finding's
//! `rootCauseKey` embeds `${modelInstanceId}/${hash}` internal RoutineIds, and the
//! SARIF fingerprint is `sha256(... | rootCauseKey)`. So to byte-match the GATE
//! fingerprints the Rust engine must assemble with the SAME content-derived id.
//!
//! Derivation (`discover.ts` + `providers/workspace.ts` + `hash.ts`):
//!   appVersion        = app.json `version` ?? "0.0.0.0"
//!   appGuid           = app.json `id` (non-empty string; fail-closed otherwise)
//!   dependencyGraphHash = sha256OfStrings( [ "${appGuid}@${appVersion}" ].sorted() )
//!                        (source-only: exactly one app)
//!   unitIds           = [ "ws:${relPosix}" for every discovered .al file ]
//!   modelInstanceId   = sha256OfStrings( [ dependencyGraphHash, ...unitIds.sorted() ] )
//!                        .first16HexChars
//!
//! `sha256OfStrings` (hash.ts) is a length-prefixed digest: for each part, update with
//! the DECIMAL string of the part's length, then ":", then the part's UTF-8 bytes.

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::source_text::{NestedApps, discover_al_files, read_root_app_guid};

/// `sha256OfStrings(parts)` — length-prefixed sha256 hex (al-sem `hash.ts`). NOTE:
/// al-sem prefixes with `String(part.length)` — the JS string LENGTH, i.e. the UTF-16
/// code-unit count. For ASCII inputs (guids, versions, `ws:` paths) this equals the
/// byte length; the corpus is ASCII-only, so `chars().count()` matches. We use the
/// UTF-16 unit count to be faithful to the JS semantics.
fn sha256_of_strings(parts: &[String]) -> String {
    let mut h = Sha256::new();
    for part in parts {
        let len_utf16: usize = part.chars().map(|c| c.len_utf16()).sum();
        h.update(len_utf16.to_string().as_bytes());
        h.update(b":");
        h.update(part.as_bytes());
    }
    hex_lower(&h.finalize())
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Read the root `app.json` `version` field (string), defaulting to "0.0.0.0" when
/// absent or unreadable (mirrors `providers/workspace.ts`).
fn read_root_app_version(workspace: &Path) -> String {
    const DEFAULT: &str = "0.0.0.0";
    let Ok(text) = std::fs::read_to_string(workspace.join("app.json")) else {
        return DEFAULT.to_string();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return DEFAULT.to_string();
    };
    value
        .get("version")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| DEFAULT.to_string())
}

/// Compute the al-sem gate `modelInstanceId` for a source-only workspace. Returns
/// `None` for a fail-closed layout (no readable root `app.json` with a string `id`)
/// — the caller then falls back to the engine default / empty model.
pub fn compute_gate_model_instance_id(workspace: &Path) -> Option<String> {
    let app_guid = read_root_app_guid(workspace)?;
    let app_version = read_root_app_version(workspace);

    // dependencyGraphHash — source-only: a single `appGuid@version` entry.
    let dep_graph_hash = sha256_of_strings(&[format!("{app_guid}@{app_version}")]);

    // unitIds — `ws:<relPosix>` for every discovered .al file, sorted.
    let discovered = discover_al_files(workspace, NestedApps::Walk).ok()?;
    let mut unit_ids: Vec<String> = discovered
        .iter()
        .map(|f| format!("ws:{}", f.rel_posix))
        .collect();
    unit_ids.sort();

    let mut parts: Vec<String> = Vec::with_capacity(1 + unit_ids.len());
    parts.push(dep_graph_hash);
    parts.extend(unit_ids);

    let full = sha256_of_strings(&parts);
    Some(full.chars().take(16).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_of_strings_length_prefixed() {
        // Distinguishes ["ab","c"] from ["a","bc"] (the length prefix is the point).
        let a = sha256_of_strings(&["ab".to_string(), "c".to_string()]);
        let b = sha256_of_strings(&["a".to_string(), "bc".to_string()]);
        assert_ne!(a, b);
    }

    #[test]
    fn sha256_of_strings_matches_known_vector() {
        // sha256OfStrings(["x"]) = sha256("1:x"); precomputed reference.
        let got = sha256_of_strings(&["x".to_string()]);
        // sha256 of the bytes "1:x":
        let mut h = Sha256::new();
        h.update(b"1:x");
        assert_eq!(got, hex_lower(&h.finalize()));
    }
}
