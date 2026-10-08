//! 1B.3b Task 1: domain-separated, versioned, stable anonymization for the
//! committed CDO-derived golden (`tests/goldens/semantic-edges/cdo-compiler-anon.json`
//! since engine-switch S9.0d; the L3-minted `cdo-anon.json` family it replaced
//! used the same scheme).
//!
//! # Why anonymize
//!
//! CDO is a real customer workspace. A committed per-site golden's site keys
//! (file path, routine name, object name/number) and target identities leak
//! proprietary names if stored as plaintext. [`anon`] replaces every
//! identifying string with a stable, deterministic, opaque [`AnonId`] so the
//! committed golden encodes GRAPH STRUCTURE (which site resolves to which
//! target, which dispatch kind, which classification) without the names. The
//! same function re-anonymizes the FRESH side at audit time with the SAME key,
//! so the diff aligns on opaque ids — a regression shows as a reviewable
//! anonymized ±edge diff.
//!
//! # Governance: a FIXED, COMMITTED salt (1B.3b Task 1 fix)
//!
//! **Decision: HMAC-SHA256 keyed by [`ANON_SALT`], a FIXED, COMMITTED constant.**
//! This module previously keyed the HMAC by [`ANON_KEY_ENV`], a secret meant to
//! live only in the gated/internal CDO runner's secret store. In practice that
//! secret was SESSION-LOCAL: it was never persisted anywhere, so the FIRST
//! committed `cdo-anon.json`/`cdo-trigger-anon.json`/`cdo-event-anon.json` were
//! minted with a key nobody could reproduce. Every subsequent mint or audit
//! either used a DIFFERENT key (every anonymized site lookup misses,
//! `checked_sites == 0`, and `ENFORCE_CDO_WS=1` PANICS) or fell back to
//! [`ANON_SALT`] (silently auditing nothing on the default dev path, since
//! `checked_sites == 0` was previously gated behind `ENFORCE_CDO_WS=1` — see
//! Fix 3 in `tests/program_resolve_harness.rs`'s `enforce_audit_ran`). The
//! committed goldens were UNAUDITABLE. A committed artifact that nobody can
//! reproduce is worse than a weaker one that everybody can: REPRODUCIBILITY is
//! the load-bearing property here, not secrecy.
//!
//! **Trade-off, made explicit:** a fixed, committed salt is deterministic and
//! reproducible — anyone with CDO source access can re-run the dev-mint tool
//! and byte-match the committed goldens, with no secret to lose, rotate, or
//! fail to persist. It is, however, weak against an adversary: AL
//! object/procedure names are drawn from a small, guessable vocabulary
//! (`OnInsert`, `PostInvoice`, `"Sales Header"`, …), so a fixed public salt
//! lets anyone dictionary-attack the committed golden back to plaintext. The
//! round-2 external reviewers (1B.3b plan) endorsed a fixed salt as
//! **"adequate for diffing"** — this golden's job is regression detection
//! (stable id ↔ stable id across runs), not secrecy. The dictionary-attack
//! weakness is ACCEPTABLE here because: (a) this is an INTERNAL regression
//! artifact, not a public-facing one; (b) the de-anonymization map
//! (`cdo-deanon-map.json`, `AnonId -> plaintext`) is GITIGNORED and never
//! committed, so even a successful dictionary attack only recovers what the
//! committed golden's STRUCTURE already implies (which sites resolve to which
//! targets), not fresh plaintext beyond that; (c) the committed golden has no
//! public exposure (this is a private repo's test fixture, not a published
//! artifact). If this trade-off is ever revisited (e.g. the repo goes public),
//! bump the domain version tags and re-mint — see "Domain separation" below.
//!
//! [`ANON_KEY_ENV`] still exists as an OPTIONAL override for a developer who
//! wants a non-reproducible, session-local anonymization (e.g. testing key
//! rotation, or genuinely wanting the stronger-secrecy scheme back for some
//! ad hoc local comparison) — but the DEFAULT, and the key every COMMITTED
//! golden must be minted with, is [`ANON_SALT`]. A golden minted with the
//! override set must NEVER be committed (the dev-mint tool warns when this
//! happens — see `src/bin/mint-goldens.rs`).
//!
//! # Domain separation
//!
//! [`anon`] takes a `domain` tag as well as the value to hash:
//! `anon(domain, s) = HMAC-SHA256(key, domain || 0x00 || s)`, truncated to 16
//! bytes (128 bits — collision-safe for the ~13k-site CDO golden, keeps the
//! minified JSON compact). The SAME plaintext string hashed under two
//! different domains yields two DIFFERENT, uncorrelatable ids. This matters
//! because several plaintext roles can share literal values — e.g. a call
//! site's caller object name and a resolved target's object name can be the
//! identical string ("Codeunit 50100 runs Codeunit 50100"); without domain
//! separation the same hash would appear in both the site-identity and the
//! target-identity positions, leaking a correlation the anonymization is
//! supposed to remove. Each domain is `:v1`-suffixed so a future scheme change
//! is reviewable as a version bump, not a silent reinterpretation of old ids.
//!
//! The one live domain is [`PAIR_DOMAIN_V1`] (the compiler-minted pair golden).
//! Within it, caller and target strings carry their own `caller`/`target`
//! prefix, so the same routine as caller and as target hashes to two
//! uncorrelated ids. The `site:v1`, `target:v1`, `trigger-op:v1` and
//! `event-pair:v1` domains of the retired L3-minted goldens are gone; never
//! reuse those tags.
//!
//! # The re-hash-don't-decrypt principle
//!
//! [`AnonId`] is one-way: there is no "de-anonymize this id" operation here.
//! Every consumer that needs to test "does this committed opaque id correspond
//! to plaintext value X?" must hold a CANDIDATE plaintext X (e.g. from a live,
//! local re-resolution against the real CDO source) and re-hash it with
//! [`anon`] under the same domain, then compare ids for equality. The compiler
//! audit (`compiler_golden.rs`) works this way: it anonymizes the program
//! resolver's pairs and compares ids, never inverting a committed one.

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// OPTIONAL override env var for the HMAC key used by [`anon`]. When unset
/// (the default — and the ONLY state every COMMITTED golden may be minted
/// under), [`anon_key`] uses [`ANON_SALT`]. Setting this to a private value
/// produces a DIFFERENT, non-reproducible anonymization — never commit a
/// golden minted with it set. See the module docs' "Governance" section.
pub const ANON_KEY_ENV: &str = "CDO_ANON_KEY";

/// Domain for the caller and target routine identities of the compiler-minted
/// pair golden (`cdo-compiler-anon.json`, engine-switch S9.0d).
pub const PAIR_DOMAIN_V1: &str = "pair:v1";

/// The FIXED, COMMITTED HMAC key (1B.3b Task 1 fix). This is the DEFAULT key
/// for every call to [`anon`] — used by the dev-mint tool when minting the
/// committed goldens, by every runtime audit re-anonymizing the fresh side,
/// and by `cargo test --workspace` (no CDO access, including public CI). A
/// fixed, public, committed salt is INTENTIONAL: see the module docs'
/// "Governance" section for the full reproducibility-over-secrecy rationale.
/// Bump the trailing version tag (and re-mint every committed golden) if this
/// value ever needs to change.
///
/// The leading `al-call-hierarchy` is a FROZEN BYTE STRING, not a reference to the crate
/// name — it deliberately survived the 2026-08-07 rename to `al-sem`. Every committed
/// anonymized golden was minted with these exact bytes, so "tidying" it to match the
/// crate name silently invalidates all of them.
const ANON_SALT: &[u8] = b"al-call-hierarchy/1B.3b/anon-fixed-salt/v1";

/// Number of HMAC-SHA256 output bytes kept per [`AnonId`] (truncated from the
/// full 32-byte digest). 128 bits is collision-safe for the ~13k-site CDO
/// golden and keeps the minified JSON compact.
const ID_BYTES: usize = 16;

/// An opaque, deterministic, domain-separated identifier produced by [`anon`].
///
/// Hex-encoded (lowercase), `serde(transparent)` so it round-trips as a bare
/// JSON string in the committed goldens (e.g. `"3f9a2b71c44e08d5..."`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AnonId(pub String);

impl std::fmt::Display for AnonId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Resolve the HMAC key: [`ANON_KEY_ENV`] when set and non-empty (an
/// intentional, non-reproducible OVERRIDE — see the module docs), else the
/// committed [`ANON_SALT`] (the default every committed golden uses).
fn anon_key() -> Vec<u8> {
    match std::env::var(ANON_KEY_ENV) {
        Ok(k) if !k.is_empty() => k.into_bytes(),
        _ => ANON_SALT.to_vec(),
    }
}

/// Domain-separated, versioned, stable (deterministic) anonymization.
///
/// `anon(domain, s) = HMAC-SHA256(key, domain || 0x00 || s)[..16]`, hex-encoded.
/// `key` is the committed [`ANON_SALT`] by default, or [`ANON_KEY_ENV`] when
/// that override is set (see the module docs). Deterministic: the same
/// `(domain, s)` pair under the same key always yields the same [`AnonId`] —
/// across processes, across runs, at both mint time and audit time. With the
/// default fixed salt, this means the committed goldens are REPRODUCIBLE:
/// re-minting from the same workspace state yields byte-identical output.
///
/// The `domain` value is expected to be one of the `*_DOMAIN_V1` constants in
/// this module (or a future versioned successor); it is not itself validated
/// here so callers remain free to add new domains without touching this
/// function.
#[must_use]
pub fn anon(domain: &str, s: &str) -> AnonId {
    let key = anon_key();
    // `Hmac::new_from_slice` only fails for algorithms with a fixed/maximum key
    // size; HMAC-SHA256 accepts any key length, so this cannot fail.
    let mut mac =
        <HmacSha256 as Mac>::new_from_slice(&key).expect("HMAC-SHA256 accepts any key length");
    mac.update(domain.as_bytes());
    mac.update(&[0u8]); // domain/input separator — `:v1` domain tags never contain NUL.
    mac.update(s.as_bytes());
    let digest = mac.finalize().into_bytes();
    let hex: String = digest[..ID_BYTES]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    AnonId(hex)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Same `(domain, input)` pair → same `AnonId`, repeatedly. Determinism is
    /// the entire point: the dev-mint tool and the runtime audit must agree.
    #[test]
    fn same_domain_and_input_yields_same_id() {
        let a = anon(PAIR_DOMAIN_V1, "Codeunit 50100 PostInvoice");
        let b = anon(PAIR_DOMAIN_V1, "Codeunit 50100 PostInvoice");
        assert_eq!(a, b);
        // And a third call, well after the first two, to rule out any
        // process-local mutable state leaking into the result.
        let c = anon(PAIR_DOMAIN_V1, "Codeunit 50100 PostInvoice");
        assert_eq!(a, c);
    }

    /// The SAME plaintext under DIFFERENT domains must yield DIFFERENT ids —
    /// the core domain-separation / no-cross-namespace-collision property.
    #[test]
    fn same_input_under_different_domains_yields_different_ids() {
        let s = "Codeunit 50100";
        let domains = [PAIR_DOMAIN_V1, "other:v1", "pair:v2"];
        let ids: Vec<AnonId> = domains.iter().map(|d| anon(d, s)).collect();
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                assert_ne!(
                    ids[i], ids[j],
                    "domains {:?} and {:?} collided on input {s:?}: {ids:?}",
                    domains[i], domains[j]
                );
            }
        }
    }

    /// Different inputs under the SAME domain must (overwhelmingly) yield
    /// different ids — sanity-checks that the hash actually depends on `s`,
    /// not just `domain`.
    #[test]
    fn different_inputs_under_same_domain_yield_different_ids() {
        let a = anon(PAIR_DOMAIN_V1, "ProcA");
        let b = anon(PAIR_DOMAIN_V1, "ProcB");
        assert_ne!(a, b);
    }

    /// Domain values are NOT just concatenated with `s` — a naive
    /// `domain + s` scheme would collide `("site:v", "1x")` with
    /// `("site:v1", "x")`. The NUL separator must prevent this class of
    /// boundary-shift collision.
    #[test]
    fn domain_value_boundary_is_not_naively_concatenable() {
        let a = anon("d", "ab");
        let b = anon("da", "b");
        assert_ne!(a, b, "domain/input boundary must be unambiguous");
    }

    /// `AnonId`s are lowercase hex of the expected truncated length (16 bytes
    /// → 32 hex chars), and visibly opaque (not a recognizable transform of
    /// the input — i.e. not a no-op/identity stand-in).
    #[test]
    fn id_shape_is_fixed_length_lowercase_hex() {
        let id = anon(PAIR_DOMAIN_V1, "anything");
        assert_eq!(id.0.len(), ID_BYTES * 2);
        assert!(
            id.0.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "AnonId must be lowercase hex: {:?}",
            id.0
        );
        assert_ne!(id.0, "anything");
    }

    /// Anonymizing a small synthetic "workspace" (a handful of site→target
    /// records) twice in a row and serializing both runs to minified JSON
    /// must produce BYTE-IDENTICAL output — the anon-determinism property the
    /// dev-mint tool's "verify frozen==live" step (1B.3b Task 1 Step 5)
    /// depends on. This does not touch CDO/L3 — it is a pure exercise of
    /// `anon` + serde, standing in for the dev tool's mint pipeline.
    #[test]
    fn two_mints_of_a_synthetic_workspace_are_byte_identical_minified_json() {
        #[derive(Serialize)]
        struct SyntheticEntry {
            site: AnonId,
            edge_kind: u8,
            targets: Vec<AnonId>,
        }

        fn mint() -> String {
            let sites = [
                (
                    "Codeunit 50100 PostInvoice#L42",
                    0u8,
                    vec!["Codeunit 50101 Helper#Run"],
                ),
                (
                    "Codeunit 50100 PostInvoice#L43",
                    1u8,
                    vec!["Codeunit 50102 Other#Post"],
                ),
                (
                    "Page 50200 Card#L10",
                    2u8,
                    vec![
                        "Table 50300 MyTable#OnInsert",
                        "TableExt 50301 MyExt#OnInsert",
                    ],
                ),
            ];
            let mut entries: Vec<SyntheticEntry> = sites
                .iter()
                .map(|(site, kind, targets)| SyntheticEntry {
                    site: anon(PAIR_DOMAIN_V1, site),
                    edge_kind: *kind,
                    targets: targets.iter().map(|t| anon(PAIR_DOMAIN_V1, t)).collect(),
                })
                .collect();
            entries.sort_by(|a, b| a.site.cmp(&b.site));
            serde_json::to_string(&entries).expect("minified serialize")
        }

        let first = mint();
        let second = mint();
        assert_eq!(
            first, second,
            "anonymizing the same synthetic workspace twice must be byte-identical"
        );
    }

    /// `AnonId` round-trips through serde as a bare JSON string (not an
    /// object wrapper) — required for the committed goldens to stay compact
    /// and for `cdo-deanon-map.json` (`AnonId → plaintext`) to use it as a
    /// natural JSON object key string.
    #[test]
    fn anon_id_serializes_as_bare_json_string() {
        let id = anon(PAIR_DOMAIN_V1, "x");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{}\"", id.0));
        let back: AnonId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }

    /// The committed fixed [`ANON_SALT`] path is exercised whenever
    /// [`ANON_KEY_ENV`] is unset — confirm `anon_key()` does not panic and
    /// produces the documented default when the env var is absent. Run
    /// serially with the env-var-set test would race; this test only reads
    /// state when the var happens to be unset, which is the default for
    /// `cargo test --workspace` (no CDO override configured).
    #[test]
    fn anon_key_falls_back_to_fixed_salt_when_env_var_unset() {
        // Best-effort: only assert the default path when the var is not
        // already set in this process's environment (avoids flaking under
        // `cargo test -- --test-threads=1` with an override exported).
        if std::env::var(ANON_KEY_ENV).is_err() {
            assert_eq!(anon_key(), ANON_SALT.to_vec());
        }
    }

    /// 1B.3b Task 1 fix (Fix 5): pins the FIXED SALT's effect on a known
    /// `(domain, input)` pair. This is the reproducibility contract the
    /// re-mint relies on: anyone with CDO source + this crate can re-mint and
    /// byte-match the committed goldens, with no secret required. If this
    /// test ever needs to change, [`ANON_SALT`] (or the HMAC scheme) changed,
    /// which means EVERY committed golden (`cdo-compiler-anon.json`,
    /// `fixture-compiler-anon.json`) is now keyed differently and MUST be
    /// re-minted via `cargo run --bin mint-goldens` before the change can be
    /// committed. (Pinned under `site:v1` until S9.0d retired that domain.)
    #[test]
    fn fixed_salt_pins_known_id_for_known_input() {
        // Best-effort: only meaningful under the default (no override) key —
        // an exported CDO_ANON_KEY legitimately changes the result without
        // the committed salt itself having changed.
        if std::env::var(ANON_KEY_ENV).is_ok() {
            return;
        }
        // Unchanged scheme: the retired "site:v1" domain still gives the id
        // every L3-minted golden was keyed with.
        assert_eq!(
            anon("site:v1", "Codeunit 50100 PostInvoice").0,
            "dbeef6ec4e976b1c0abb5c59db894de8"
        );
        let id = anon(PAIR_DOMAIN_V1, "Codeunit 50100 PostInvoice");
        assert_eq!(
            id.0, "4e254f2fd04c75a97872ed11ff79775d",
            "anon()'s output for a known (domain, input) pair under the committed \
             fixed salt changed. If this is an intentional salt/scheme bump, \
             update this pin AND re-mint every committed golden under \
             tests/goldens/semantic-edges/ before committing either change."
        );
    }
}
