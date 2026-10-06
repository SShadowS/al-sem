//! `toConfidence` — port of al-sem `src/detectors/confidence.ts`.
//!
//! Maps a list of uncertainties to a [`FindingConfidence`]. Any uncertainty caps
//! `level` at `possible`. Uncertainty kinds that are valid `cappedBy` values
//! (directly or via the alias map) are listed in `cappedBy`; the others still cap
//! the level but appear only in `evidence`. `base_level` is never raised.
//!
//! For the R4-A wave (d4) the only call is `to_confidence(&[], "likely")` →
//! `{ level: "likely", evidence: [] }`. The full uncertainty-mapping path is
//! ported for fidelity even though no ported detector exercises it yet.

use std::sync::Arc;

use crate::engine::l4::summary::{Uncertainty, uncertainty_at};
use crate::engine::l5::finding::{ConfidenceEvidence, FindingConfidence};

/// The `Uncertainty` kinds that are also valid `cappedBy` values
/// (`VALID_CAPPED_BY` in confidence.ts).
const VALID_CAPPED_BY: &[&str] = &[
    "unresolved-call",
    "opaque-callee",
    "dynamic-dispatch",
    "parse-incomplete",
    "version-mismatch",
];

/// A minimal Uncertainty for the confidence mapper. al-sem's `Uncertainty` is a
/// discriminated union carrying a `kind` plus one id field (callsiteId /
/// operationId / routineId). The mapper only reads `kind` (for cappedBy) and the
/// id (for the evidence note) — this carries exactly that subset.
///
/// **Both fields are `Arc<str>`, and the note is materialised HERE rather than
/// in the mapper.** `d1`'s cohort path turns ~3.1k distinct uncertainties into
/// 7.4M evidence records on Base App 8020; interning the note at the *value*
/// means [`crate::engine::l5::d1_cohort::UncertaintyTable`] can hold one
/// `UncertaintyLite` per distinct uncertainty and hand out clones, so a record
/// costs two refcount bumps and no allocation. The four low-volume producers
/// (d2/d3/d46/d48) call [`Self::new`] per record, which is one allocation
/// *fewer* than the `{kind: String, at: String}` pair it replaces.
///
/// **Both fields are PRIVATE, and that is load-bearing rather than tidiness.**
/// Before the note existed as a field, [`to_confidence`] derived it with a
/// single `format!`, so no caller could produce a note that did not have the
/// `"<kind> at <id>"` shape — the invariant was structural. Moving the `format!`
/// up to [`Self::new`] kept the single production site but would, with a `pub`
/// field, let any caller write `UncertaintyLite { note: <anything> }` and have
/// that text flow unmodified into `ConfidenceEvidence.note` → `StableEvidence`
/// → every golden. Keeping the fields private restores the guarantee: the two
/// constructors below are the only way to obtain one, so the format is again
/// enforced by the type rather than by a comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UncertaintyLite {
    kind: Arc<str>,
    /// The evidence note text, `"<kind> at <id>"` — the exact `String` that
    /// reaches `StableEvidence.note`, built once per distinct uncertainty and
    /// shared by every [`ConfidenceEvidence`] this uncertainty produces.
    note: Arc<str>,
}

impl UncertaintyLite {
    /// ⟨census⟩ Heap bytes of this lite's two shared strings. Diagnostic only —
    /// charged once per DISTINCT uncertainty, which is the point of interning it.
    pub(crate) fn census_heap_bytes(&self) -> u64 {
        self.kind.len() as u64 + self.note.len() as u64 + 32
    }

    /// Build from a `kind` and the descriptive id it is reported *at*
    /// (callsiteId | operationId | routineId).
    ///
    /// The `format!` is the ONE place the evidence note text is produced; it was
    /// previously inlined in [`to_confidence`] and run once per evidence record
    /// instead of once per distinct uncertainty. Same format string, same
    /// arguments, same bytes.
    pub fn new(kind: &str, at: &str) -> Self {
        Self {
            kind: Arc::from(kind),
            note: Arc::from(format!("{kind} at {at}")),
        }
    }

    /// Build from a full [`Uncertainty`], taking the id under
    /// [`uncertainty_at`]'s `callsiteId → operationId → routineId` precedence —
    /// the same precedence
    /// [`crate::engine::l4::summary::uncertainty_key`] de-dups by.
    pub fn of(u: &Uncertainty) -> Self {
        Self::new(&u.kind, uncertainty_at(u))
    }

    /// The uncertainty's `kind` — read by [`to_confidence`] for the `cappedBy`
    /// mapping. An accessor rather than a `pub` field so the note-format
    /// invariant above stays structural.
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// The materialised evidence note. Read by [`to_confidence`] only, which
    /// clones the handle into every record it emits.
    pub fn note(&self) -> &Arc<str> {
        &self.note
    }
}

/// Map a new resolver-upgrade uncertainty kind onto an existing `cappedBy` value
/// (`UNCERTAINTY_TO_CAPPED_BY` alias map in confidence.ts).
fn alias_capped_by(kind: &str) -> Option<&'static str> {
    match kind {
        "ambiguous-overload" => Some("unresolved-call"),
        "member-not-found" => Some("unresolved-call"),
        "external-target" => Some("opaque-callee"),
        // #22: a symbol-only routine's own body is unseen, like an opaque callee's.
        "opaque-body" => Some("opaque-callee"),
        "interface-open-world" => Some("dynamic-dispatch"),
        _ => None,
    }
}

/// `toCappedByKind` — alias mapping first, then identity against
/// `VALID_CAPPED_BY`.
///
/// Returns the `&'static str` *from* those tables rather than a fresh `String`:
/// the answer is one of a five-value closed set either way, and a finding with
/// hundreds of uncertainties (max measured: 893) would otherwise allocate one
/// `String` per uncertainty only for the `BTreeSet` to drop all but the distinct
/// few. Byte-identical — the identity arm returns the table entry that compared
/// equal to `kind`.
fn to_capped_by_kind(kind: &str) -> Option<&'static str> {
    if let Some(alias) = alias_capped_by(kind) {
        return Some(alias);
    }
    VALID_CAPPED_BY.iter().copied().find(|&v| v == kind)
}

/// Port of `toConfidence`. Empty `uncertainties` ⇒ `{ level: base_level,
/// evidence: [] }`. Otherwise level is capped at `possible`, `cappedBy` carries
/// the sorted valid-mapped kinds (absent when none mapped), and `evidence`
/// carries `"<kind> at <id>"` notes in input order.
pub fn to_confidence(uncertainties: &[UncertaintyLite], base_level: &str) -> FindingConfidence {
    if uncertainties.is_empty() {
        return FindingConfidence {
            level: base_level.to_string(),
            capped_by: None,
            evidence: Vec::new(),
        };
    }

    // Sorted, de-duped capped-by set (al-sem `new Set(...).sort()`). `&str`'s
    // `Ord` is byte order, exactly as `String`'s is, so the emitted order is
    // unchanged by holding borrows here.
    let mut capped_by_set: std::collections::BTreeSet<&'static str> =
        std::collections::BTreeSet::new();
    for u in uncertainties {
        if let Some(mapped) = to_capped_by_kind(u.kind()) {
            capped_by_set.insert(mapped);
        }
    }
    let capped_by = if capped_by_set.is_empty() {
        None
    } else {
        Some(capped_by_set.into_iter().map(str::to_string).collect())
    };

    // Each record shares the uncertainty's already-materialised note: one
    // refcount bump, no allocation, and no stored `source` (there is one source
    // value in the engine and the projection re-materialises it). See
    // [`ConfidenceEvidence`]'s doc for why both matter at 7.4M records.
    let evidence = uncertainties
        .iter()
        .map(|u| ConfidenceEvidence {
            note: Some(Arc::clone(u.note())),
        })
        .collect();

    FindingConfidence {
        level: "possible".to_string(),
        capped_by,
        evidence,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_uncertainties_keeps_base_level_and_empty_evidence() {
        let c = to_confidence(&[], "likely");
        assert_eq!(c.level, "likely");
        assert!(c.capped_by.is_none());
        assert!(c.evidence.is_empty());
    }

    #[test]
    fn any_uncertainty_caps_to_possible() {
        let c = to_confidence(
            &[UncertaintyLite::new("interface-dispatch", "r/cs0")],
            "likely",
        );
        assert_eq!(c.level, "possible");
        // interface-dispatch is NOT a valid cappedBy → only evidence.
        assert!(c.capped_by.is_none());
        assert_eq!(c.evidence.len(), 1);
        assert_eq!(
            c.evidence[0].note.as_deref(),
            Some("interface-dispatch at r/cs0")
        );
    }

    #[test]
    fn alias_and_identity_capped_by_sorted_unique() {
        let c = to_confidence(
            &[
                UncertaintyLite::new("external-target", "r/cs1"),
                UncertaintyLite::new("parse-incomplete", "r"),
                UncertaintyLite::new("external-target", "r/cs2"),
            ],
            "confirmed",
        );
        assert_eq!(c.level, "possible");
        assert_eq!(
            c.capped_by,
            Some(vec![
                "opaque-callee".to_string(),
                "parse-incomplete".to_string()
            ])
        );
        assert_eq!(c.evidence.len(), 3);
    }

    /// #22: a symbol-only routine's `opaque-body` caps like an opaque callee,
    /// rather than reaching a finding only as uncapped evidence.
    #[test]
    fn opaque_body_caps_as_opaque_callee() {
        let c = to_confidence(&[UncertaintyLite::new("opaque-body", "r")], "likely");
        assert_eq!(c.level, "possible");
        assert_eq!(c.capped_by, Some(vec!["opaque-callee".to_string()]));
    }

    /// `UncertaintyLite::of` must take the id under the SAME
    /// `callsiteId → operationId → routineId` precedence
    /// `uncertainty_key` de-dups by, including the `""` fallback when all three
    /// are absent — that identity is what licenses `d1` carrying its cohort
    /// unions as ids into a table keyed by `uncertainty_key`.
    #[test]
    fn lite_of_uncertainty_matches_the_uncertainty_key_precedence() {
        use crate::engine::l4::summary::{Uncertainty, uncertainty_key};
        let mk = |cs: Option<&str>, op: Option<&str>, rt: Option<&str>| Uncertainty {
            kind: "opaque-callee".to_string(),
            callsite_id: cs.map(str::to_string),
            operation_id: op.map(str::to_string),
            routine_id: rt.map(str::to_string),
            interface_name: None,
        };
        for u in [
            mk(Some("r/cs0"), Some("r/op0"), Some("r")), // callsite wins
            mk(None, Some("r/op0"), Some("r")),          // then operation
            mk(None, None, Some("r")),                   // then routine
            mk(None, None, None),                        // then ""
        ] {
            let lite = UncertaintyLite::of(&u);
            let (k, at) = uncertainty_key(&u)
                .split_once('|')
                .map(|(k, at)| (k.to_string(), at.to_string()))
                .expect("uncertainty_key is kind|at");
            assert_eq!(lite.kind(), k);
            assert_eq!(&**lite.note(), format!("{k} at {at}"));
        }
    }

    /// `ConfidenceEvidence` is the 7.4M-record type; every word of it is 56.6 MiB
    /// of live heap on Base App 8020. This pins the representation the saving
    /// rests on — two words, i.e. an `Option<Arc<str>>` and NOTHING else. A
    /// future edit that puts a `source` (or any other field) back fails here
    /// rather than silently costing 113 MiB.
    #[test]
    fn confidence_evidence_stays_two_words_wide() {
        assert_eq!(
            std::mem::size_of::<ConfidenceEvidence>(),
            2 * std::mem::size_of::<usize>(),
            "ConfidenceEvidence must stay just an Option<Arc<str>> — see its doc"
        );
    }
}
