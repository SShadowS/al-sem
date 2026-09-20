//! Shared `sig_fp` fingerprint primitives for `RoutineNodeId` overload
//! identity (sigfp-and-ambiguous-reclassification plan, Task 2).
//!
//! Both the ABI tier ([`crate::program::abi_ingest::param_type_fp`]) and the
//! SOURCE tier ([`source_param_sig_fp`] below) fold their own canonical
//! parameter-discriminator tuple through the SAME stable hash primitive
//! ([`fnv1a`]) and the SAME length-delimited encoding
//! ([`write_len_prefixed`]), so the two independently-computed fingerprints
//! never diverge in their underlying collision-resistance properties even
//! though the ABI/source tuples differ in SHAPE: ABI additionally carries a
//! raw `Subtype` id/name/degradation-tag (recovering identity
//! `AbiParameter::type_text` alone can lose — see `abi_ingest::
//! fold_param_discriminator`'s doc). SOURCE has no equivalent raw-JSON
//! Subtype fields to recover: its `Param.ty` type-TEXT is already the
//! fully-qualified VERBATIM source text (array rank / subtype name-or-id /
//! generic args are already IN that text, byte-for-byte between the type
//! clause's first and last token), so a single normalized-text field plus the
//! `var` (by-ref) modifier flag is sufficient — see [`source_param_sig_fp`].

use al_syntax::IdentifierFoldExt;
use al_syntax::ir::{Param, RoutineDecl};

use crate::program::node::{ObjectNodeId, RoutineNodeId};

// ---------------------------------------------------------------------------
// Stable FNV-1a fingerprint (never `DefaultHasher` / a process-random hasher
// — `sig_fp` must be reproducible across runs so every consumer that
// persists or compares it within one run agrees).
// ---------------------------------------------------------------------------

pub(crate) fn fnv1a(data: &str) -> u64 {
    let mut h: u64 = 14695981039346656037;
    for b in data.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    h
}

/// Append `s`'s byte length (decimal) then a `:` separator then `s` itself —
/// a netstring-style LENGTH-DELIMITED encoding. Concatenating multiple
/// variable-length fields naively (e.g. a plain `|`-join) lets one field's
/// content masquerade as an adjacent field's boundary (a type text crafted to
/// contain a `|` could otherwise collide with a differently-shaped tuple);
/// prefixing every field with its own length makes the encoding injective
/// per-field regardless of what bytes the field itself contains.
pub(crate) fn write_len_prefixed(buf: &mut String, s: &str) {
    buf.push_str(&s.len().to_string());
    buf.push(':');
    buf.push_str(s);
}

// ---------------------------------------------------------------------------
// SOURCE-tier sig_fp (Task 2)
// ---------------------------------------------------------------------------

/// Conservative, LEXER-INSENSITIVE-ONLY normalization of a parameter type
/// TEXT: case-fold (`fold_identifier` — simple 1:1 Unicode fold, ASCII-lowercase
/// for ASCII input), then collapse leading/trailing/internal whitespace
/// runs to nothing/a single space (`split_whitespace` + re-join). Deliberately
/// does NOT strip quotes or resolve ID-vs-Name synonyms (`Codeunit "My Cu"`
/// vs `Codeunit 50100` naming the SAME object) — that would need
/// compiler-backed symbol resolution this fold has no access to.
/// Under-normalization here only SPLITS an id (two spellings of the identical
/// overload get distinct `sig_fp`s) — tolerable noise, since a real AL object
/// declares each overload's parameter types with exactly one spelling in its
/// source; over-normalization risks ALIASING two genuinely different
/// overloads onto one id, the cardinal risk this whole plan closes.
pub(crate) fn normalize_type_text(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .fold_identifier()
}

/// The SOURCE-tier overload-identity fingerprint (`RoutineNodeId::sig_fp`) —
/// a length-delimited fold of every parameter's `(normalized type text,
/// by-ref flag)` tuple, using the same [`fnv1a`] + [`write_len_prefixed`]
/// primitives `abi_ingest::param_type_fp` uses for the ABI tier, and the same
/// `params.is_empty() -> 0` convention (a zero-arity routine's `params_count`
/// alone already fully determines the id; ABI parity — see
/// `abi_ingest::param_type_fp`'s identical guard). `by_ref` (the `var`
/// modifier) is folded in as its own component because it is a SEPARATE
/// grammar field (`Param::by_ref`), not embedded in `ty`'s text — unlike
/// array rank / subtype qualifiers, which ARE already part of the verbatim
/// type-clause text.
///
/// **AL overload-identity note on `by_ref` (Task 2 review fix addendum.)**
/// Folding `by_ref` in unconditionally means two otherwise-identical
/// parameter lists differing ONLY in a `var` modifier fingerprint to
/// DIFFERENT `sig_fp`s — this fold is deliberately biased toward
/// OVER-SPLITTING (treating the pair as distinct overloads) rather than
/// UNDER-SPLITTING (aliasing them onto one shared id). That asymmetry is the
/// correct conservative direction for this engine's cardinal invariant: an
/// under-split (false alias) can make a confident `Source` route resolve to
/// the WRONG declaration — a genuine false positive, the exact failure mode
/// the sigfp-and-ambiguous-reclassification plan exists to close. An
/// over-split, at worst, gives two real declarations distinct ids when a
/// looser identity would have merged them; the engine then either resolves
/// each honestly to its own id or, in the pathological case, fails closed to
/// `Unresolved` — never a confidently wrong answer. Over-split never
/// false-aliases; under-split can.
///
/// Open question, noted honestly rather than assumed away: whether the AL
/// compiler actually PERMITS declaring two overloads of the same procedure
/// name/arity/parameter-types differing ONLY by a `var` modifier is not
/// independently verified here (no compiler-backed corpus check was run for
/// this specific case). If AL's own overload resolution does NOT key on
/// `by_ref` — i.e. a var-only-differing pair is a compile-time
/// duplicate-signature conflict in real AL source, not a legal overload —
/// then this fold's extra split is harmless in practice: no genuine AL
/// source can ever produce two survivors differing only by `by_ref` for
/// [`source_param_sig_fp`] to distinguish, so the split costs nothing. If AL
/// DOES treat it as a legal overload, the split is required for correctness.
/// Either way, folding `by_ref` in is the safe choice under the
/// over-split/under-split asymmetry above; this fold's direction was chosen
/// specifically so that open question does not need to be resolved before
/// shipping.
pub(crate) fn source_param_sig_fp(params: &[Param]) -> u64 {
    if params.is_empty() {
        return 0;
    }
    let mut canon = String::new();
    for p in params {
        write_len_prefixed(
            &mut canon,
            &normalize_type_text(p.ty.as_deref().unwrap_or("")),
        );
        write_len_prefixed(&mut canon, if p.by_ref { "1" } else { "0" });
    }
    fnv1a(&canon)
}

/// ONE shared constructor for a SOURCE-tier `RoutineNodeId` (Task 2) — used
/// by every live reconstruction site (`node_extract::extract_nodes`,
/// `resolve::decl_surface::DeclSurface::build`,
/// `resolve::full::resolve_full_program_from_parts`,
/// `resolve::stub::resolve_program`,
/// `resolve::semantic_golden::build_fan_out_site_context`) so a real
/// declaration's identity can never silently diverge between sites. The
/// ORIGINAL Task 2 audit covered 5 sites (`node_extract.rs:452`,
/// `decl_surface.rs:65`, `full.rs:210` [dead code, since deleted — see
/// `full.rs`'s module doc], `full.rs:573`, `stub.rs:68`); a Task 2 review
/// fix then found a 6th LIVE site the audit had missed
/// (`semantic_golden.rs::build_fan_out_site_context`, which independently
/// re-walks the same call sites for `route_applicability`'s fan-out
/// soundness teeth and had hardcoded `sig_fp: 0` too — see the
/// sigfp-and-ambiguous-reclassification plan, Task 2's "review fix" for the
/// full divergence/blast-radius writeup). Post-fix, ALL 5 live construction
/// sites (one of the original 5 was dead code, not a 6th survivor) are
/// unified on this constructor — the 6-site figure names the audit's total
/// reach, not today's live call-site count. `decl` supplies `name` /
/// `enclosing_member` / `params` verbatim from the parsed IR: every field
/// `RoutineNodeId` needs beyond the caller-supplied `object`.
pub fn source_routine_node_id(object: ObjectNodeId, decl: &RoutineDecl) -> RoutineNodeId {
    RoutineNodeId {
        object,
        name_lc: decl.name.fold_identifier(),
        enclosing_member_lc: decl
            .enclosing_member
            .as_ref()
            .map(|(n, _)| n.fold_identifier()),
        params_count: decl.params.len(),
        sig_fp: source_param_sig_fp(&decl.params),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use al_syntax::ir::{Origin, Point, RoutineDecl};

    fn test_origin() -> Origin {
        Origin {
            kind_text: "",
            byte: 0..0,
            start: Point { row: 0, column: 0 },
            end: Point { row: 0, column: 0 },
        }
    }

    fn param(ty: &str, by_ref: bool) -> Param {
        Param {
            name: "x".into(),
            by_ref,
            ty: Some(ty.into()),
            origin: test_origin(),
        }
    }

    /// (d) `params.is_empty()` -> `0`, mirroring ABI's `param_type_fp`
    /// convention (a zero-arity routine's `params_count` alone already fully
    /// discriminates — no fingerprint needed).
    #[test]
    fn empty_params_fingerprint_to_zero() {
        assert_eq!(source_param_sig_fp(&[]), 0);
    }

    /// (b) two param-type-differing single-param overloads get DISTINCT
    /// non-zero fingerprints.
    #[test]
    fn distinct_param_types_never_collide() {
        let int_fp = source_param_sig_fp(&[param("Integer", false)]);
        let text_fp = source_param_sig_fp(&[param("Text", false)]);
        assert_ne!(int_fp, 0);
        assert_ne!(text_fp, 0);
        assert_ne!(int_fp, text_fp);
    }

    /// (c) case/whitespace variants of the SAME type text fingerprint
    /// IDENTICALLY — the conservative lexer-insensitive normalization.
    #[test]
    fn case_and_whitespace_variants_of_same_type_collide() {
        let a = source_param_sig_fp(&[param("Record Customer", false)]);
        let b = source_param_sig_fp(&[param("  record   CUSTOMER  ", false)]);
        assert_eq!(
            a, b,
            "trim + lowercase + internal-whitespace-collapse must treat these as the SAME type"
        );
    }

    /// Round-1 addendum: under-normalization (never strip quotes / resolve
    /// ID-vs-Name) — two DIFFERENT quoted-name spellings that a compiler
    /// would resolve to the same object are NOT unified here (a real object
    /// declares each overload's parameter with exactly one spelling, so this
    /// never over-collapses in practice; it is a conscious conservative
    /// choice, not an oversight).
    #[test]
    fn quoted_object_name_is_never_unquoted() {
        let a = source_param_sig_fp(&[param(r#"Codeunit "My Cu""#, false)]);
        let b = source_param_sig_fp(&[param("Codeunit 50100", false)]);
        assert_ne!(
            a, b,
            "ID-vs-Name synonyms must NOT be unified without compiler backing"
        );
    }

    /// Issue #41 final panel (NF3): the de-collision this issue exists for is
    /// a claim about `RoutineNodeId`, and `RoutineNodeId` gets its member from
    /// HERE — a different function from `engine::ids`'s stable-id path that the
    /// `cli_p1_enclosing_member` tests cover. Nothing pinned this end.
    ///
    /// The precondition is hand-stated rather than asked of production code:
    /// two decls that agree on name, arity and param types and differ ONLY in
    /// `enclosing_member`. Asserted explicitly below, so the test cannot pass
    /// because the two happened to differ for some other reason. Drop
    /// `enclosing_member` from `source_routine_node_id` — the harmonisation the
    /// ABI side's hardcoded `enclosing_member_lc: None` invites — and two
    /// sibling action `modify()` triggers re-collide into one node, which
    /// `program::build`'s run-collapse then folds to a single entry, dropping
    /// one trigger's edges. That is exactly the defect #41 closed.
    #[test]
    fn enclosing_member_discriminates_the_program_routine_node_id() {
        let object = ObjectNodeId {
            app: crate::program::node::AppRef(0),
            kind: al_syntax::ir::ObjectKind::PageExtension,
            key: crate::program::node::ObjKey::Id(50104),
        };
        let decl = |member: Option<&str>| RoutineDecl {
            kind: al_syntax::ir::RoutineKind::Trigger,
            name: "OnAfterAction".into(),
            name_origin: test_origin(),
            params: Vec::new(),
            return_type: None,
            return_name: None,
            locals: Vec::new(),
            attributes: Vec::new(),
            attributes_parsed: Vec::new(),
            access_modifier: None,
            parse_incomplete: false,
            dataitem_source_table: None,
            enclosing_member: member.map(|m| (m.to_string(), test_origin())),
            in_dataset_modify_context: false,
            body: None,
            origin: test_origin(),
        };

        let a = source_routine_node_id(object.clone(), &decl(Some("ActionOne")));
        let b = source_routine_node_id(object.clone(), &decl(Some("ActionTwo")));
        let none = source_routine_node_id(object, &decl(None));

        // Precondition, stated rather than assumed: everything BUT the member
        // is identical, so the member is the only thing that can separate them.
        assert_eq!(a.name_lc, b.name_lc);
        assert_eq!(a.params_count, b.params_count);
        assert_eq!(a.sig_fp, b.sig_fp);
        assert_eq!(a.object, b.object);

        assert_ne!(
            a, b,
            "two routines differing only in enclosing member must be DISTINCT program nodes"
        );
        assert_ne!(
            a, none,
            "a member-less routine must not collide with a member-bearing one"
        );
    }

    /// `var` (by-ref) is folded in as its own tuple component — a parameter
    /// differing ONLY by `var` must fingerprint DIFFERENTLY (never silently
    /// aliased onto the non-var overload's id).
    #[test]
    fn by_ref_flag_distinguishes_otherwise_identical_types() {
        let by_val = source_param_sig_fp(&[param("Record Customer", false)]);
        let by_ref = source_param_sig_fp(&[param("Record Customer", true)]);
        assert_ne!(by_val, by_ref);
    }

    /// Two re-parses of the SAME declaration (identical param list) always
    /// fingerprint identically — a stability sanity check underlying the
    /// true-duplicate collapse path in `build::
    /// dedup_routines_preserving_genuine_overloads`.
    #[test]
    fn identical_param_lists_always_collide() {
        let a = source_param_sig_fp(&[param("Integer", false), param("Text", true)]);
        let b = source_param_sig_fp(&[param("Integer", false), param("Text", true)]);
        assert_eq!(a, b);
    }
}
