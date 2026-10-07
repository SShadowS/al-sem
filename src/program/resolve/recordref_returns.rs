//! Versioned `RecordRef`/`FieldRef`/`KeyRef` typed-return table (Task 4,
//! chain-tables plan).
//!
//! Sibling of [`crate::program::resolve::framework_returns`] — same fail-closed
//! `(kind, member_lc, is_method, arity) -> Option<kind>` shape, DISTINCT family:
//! this table answers "what does `<RecordRef|FieldRef|KeyRef>.<member>(...)`
//! ITSELF evaluate to?" for the three "Ref" handle types, which are a separate
//! branch of [`crate::program::resolve::receiver::ReceiverType`]
//! (`RecordRef`/`FieldRef`/`KeyRef`, unit variants — never wrapped in
//! `Framework(FrameworkKind)`) and therefore need their own lookup rather than
//! reusing [`framework_return_kind`](crate::program::resolve::framework_returns::framework_return_kind).
//!
//! # Type-stable HANDLES only (round-1 finding I4)
//!
//! `RecordRef`/`FieldRef`/`KeyRef` expose several members that return a
//! FURTHER handle of one of these three kinds — `RecordRef.Field`/
//! `.FieldIndex` and `KeyRef.FieldIndex` all yield a `FieldRef`;
//! `RecordRef.KeyIndex` yields a `KeyRef` — and this table exists to type
//! those so a chained call on the result (`RecRef.KeyIndex(1).FieldIndex(1)`)
//! resolves. It deliberately excludes two categories that would NOT be
//! type-stable:
//! - **Scalar accessors** (`FieldCount`, `KeyCount`) — these return `Integer`,
//!   never a chainable handle; a chain rooted at one of them (nonsensical AL
//!   in the first place: `RecRef.FieldCount().X()`) correctly SCALAR-DECLINES
//!   via a table miss, same as any other unlisted member.
//! - **Variant-like LEAF members** (`FieldRef.Value`) — `Value` returns the
//!   field's actual DATA, whose real type cannot be known without a
//!   table-field type index this engine does not model yet. `Value` stays a
//!   pure LEAF (resolvable as a terminal [`crate::program::resolve::member_catalog`]
//!   membership hit — `FIELDREF` already lists it), NEVER a chainable
//!   receiver: a chained `.X()` off it (`RecRef.Field(1).Value().X()`) MUST
//!   decline. Adding a `(FieldRef, "value", ..)` row here — even a
//!   deliberately-wrong one — would be exactly the kind of fabricated
//!   chainable-anything entry this table's fail-closed contract forbids.
//!
//! `FieldRef.Record()` / `KeyRef.Record()` (both real, MS-Learn-documented
//! zero-arg methods returning `RecordRef`) are ALSO deliberately OMITTED:
//! validated but out of THIS task's reviewed scope (round-1 I4's enumerated
//! handle set is exactly `Field`/`FieldIndex` → `FieldRef`, `KeyIndex` →
//! `KeyRef`) and unexercised by any real corpus site — see
//! [`recordref_family_return_kind`]'s tests for the explicit regression pin
//! that a `.Record()` chain stays declined today.
//!
//! # Clean-room note
//!
//! Fresh design over this engine's own `ReceiverType`/`member_catalog`
//! lattice — NOT a port of any L3 table (L3 has no equivalent RecordRef-family
//! chain-typing mechanism to port from).
//!
//! # Per-entry validation
//!
//! Every entry below is verified against the **primary source** — the
//! `methods-auto` reference tables at
//! `learn.microsoft.com/.../dev-itpro/developer/methods-auto/<type>/<type>-data-type`
//! (RecordRef/FieldRef/KeyRef pages, fetched 2026-07-02, each stating
//! `"Available or changed with runtime version 1.0"`) — cross-checked for
//! member EXISTENCE against the independently-generated
//! [`crate::program::resolve::member_catalog`] phf sets
//! (`ms-dynamics-smb.al-18.0.2293710`).

use crate::program::resolve::receiver::ReceiverType;

// ---------------------------------------------------------------------------
// Supported-runtime pin
// ---------------------------------------------------------------------------

/// The minimum AL/BC runtime version every table entry below is validated
/// against, as `(major, minor)`. See
/// [`crate::program::resolve::framework_returns::MIN_SUPPORTED_RUNTIME`] for
/// the full policy rationale — identical here: every entry is documented
/// `"Available or changed with runtime version 1.0"`, so no dynamic
/// per-workspace runtime gate is wired in today.
pub const MIN_SUPPORTED_RUNTIME: (u32, u32) = (1, 0);

// ---------------------------------------------------------------------------
// RecordRefFamilyKind
// ---------------------------------------------------------------------------

/// The three "Ref" handle kinds this table dispatches on — a narrower,
/// OWNED mirror of [`ReceiverType`]'s three unit `*Ref` variants, used as
/// both this table's lookup key and its return value so
/// [`recordref_family_return_kind`] never has to reason about the other
/// (non-Ref) `ReceiverType` variants at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordRefFamilyKind {
    RecordRef,
    FieldRef,
    KeyRef,
}

impl RecordRefFamilyKind {
    /// Classify a [`ReceiverType`] into its `RecordRefFamilyKind`, or `None`
    /// for every other variant (Object/Record/Framework/…) — the guard
    /// [`crate::program::resolve::receiver::infer_compound_member_receiver`]
    /// uses to decide whether this table's arm engages at all.
    pub fn from_receiver_type(rt: &ReceiverType) -> Option<Self> {
        match rt {
            ReceiverType::RecordRef => Some(Self::RecordRef),
            ReceiverType::FieldRef => Some(Self::FieldRef),
            ReceiverType::KeyRef => Some(Self::KeyRef),
            _ => None,
        }
    }

    /// The inverse of [`Self::from_receiver_type`] — wraps this kind back
    /// into its `ReceiverType` unit variant.
    pub fn to_receiver_type(self) -> ReceiverType {
        match self {
            Self::RecordRef => ReceiverType::RecordRef,
            Self::FieldRef => ReceiverType::FieldRef,
            Self::KeyRef => ReceiverType::KeyRef,
        }
    }
}

// ---------------------------------------------------------------------------
// recordref_family_return_kind
// ---------------------------------------------------------------------------

/// Look up the [`RecordRefFamilyKind`] returned by accessing `member_lc` on a
/// `base` receiver of one of the three "Ref" handle kinds, given its AL
/// syntactic FORM (`is_method`) and `arity`.
///
/// `None` — fail closed — for: an unlisted `(base, member_lc)` pair (table
/// miss, including the deliberately-excluded scalar/variant-like members
/// documented on this module); the right member but the wrong FORM; or the
/// right member/form but a mismatched arity. Mirrors
/// [`crate::program::resolve::framework_returns::framework_return_kind`]'s
/// contract exactly — same fail-closed mechanism, distinct family.
pub fn recordref_family_return_kind(
    base: &RecordRefFamilyKind,
    member_lc: &str,
    is_method: bool,
    arity: usize,
) -> Option<RecordRefFamilyKind> {
    use RecordRefFamilyKind::*;
    match (base, member_lc, is_method, arity) {
        // ---------------------------------------------------------------
        // RecordRef.Field(Integer) / Field(Text) — both real overloads
        // (methods-auto/recordref: `Field(Integer)` and `Field(Text)`),
        // both arity 1, both returning `FieldRef` — no return-kind
        // ambiguity across the overload pair, only the arg TYPE differs
        // (this table does not distinguish arg type, only arity, per
        // `framework_return_kind`'s own established convention).
        // ---------------------------------------------------------------
        (RecordRef, "field", true, 1) => Some(FieldRef),

        // ---------------------------------------------------------------
        // RecordRef.FieldIndex(Integer) — methods-auto/recordref: single
        // arity-1 overload, returns `FieldRef`. Real CDO site (Task 4
        // gate adjudication): `Codeunit 6175399 "CDO Data Delete
        // Handler"`, `SourceRecRef.KeyIndex(1).FieldIndex(1)`.
        // ---------------------------------------------------------------
        (RecordRef, "fieldindex", true, 1) => Some(FieldRef),

        // ---------------------------------------------------------------
        // RecordRef.KeyIndex(Integer) — methods-auto/recordref: single
        // arity-1 overload, returns `KeyRef`. Real CDO sites (Task 4 gate
        // adjudication): `Codeunit 6175399 "CDO Data Delete Handler"`,
        // `SourceRecRef.KeyIndex(1).FieldCount` and
        // `SourceRecRef.KeyIndex(1).FieldIndex(1)`.
        // ---------------------------------------------------------------
        (RecordRef, "keyindex", true, 1) => Some(KeyRef),

        // ---------------------------------------------------------------
        // KeyRef.FieldIndex(Integer) — methods-auto/keyref: single arity-1
        // overload, returns `FieldRef`. Real CDO site (Task 4 gate
        // adjudication): `Codeunit 6175310 "CDO Subscribers"`,
        // `KeyRef.FieldIndex(1).Value`.
        // ---------------------------------------------------------------
        (KeyRef, "fieldindex", true, 1) => Some(FieldRef),

        // ---------------------------------------------------------------
        // FieldRef.Record() / KeyRef.Record() — methods-auto/fieldref,
        // /keyref: zero-arg, returns the owning `RecordRef`. S9.0e: 43 real
        // sites in Base Application (`SourceFieldRef.Record().Number()`).
        // ---------------------------------------------------------------
        (FieldRef | KeyRef, "record", true, 0) => Some(RecordRef),

        // Deliberately NOT tabled (see module doc): RecordRef/KeyRef's
        // `FieldCount`/`KeyCount` (scalar `Integer` return — no chainable
        // handle), FieldRef's `Value` (variant-like LEAF, never
        // chainable). (`Record()` used to be listed here as unexercised; it is
        // tabled above since S9.0e.)
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recordref_field_and_fieldindex_resolve_fieldref() {
        assert_eq!(
            recordref_family_return_kind(&RecordRefFamilyKind::RecordRef, "field", true, 1),
            Some(RecordRefFamilyKind::FieldRef)
        );
        assert_eq!(
            recordref_family_return_kind(&RecordRefFamilyKind::RecordRef, "fieldindex", true, 1),
            Some(RecordRefFamilyKind::FieldRef)
        );
    }

    #[test]
    fn recordref_keyindex_resolves_keyref() {
        assert_eq!(
            recordref_family_return_kind(&RecordRefFamilyKind::RecordRef, "keyindex", true, 1),
            Some(RecordRefFamilyKind::KeyRef)
        );
    }

    #[test]
    fn keyref_fieldindex_resolves_fieldref() {
        assert_eq!(
            recordref_family_return_kind(&RecordRefFamilyKind::KeyRef, "fieldindex", true, 1),
            Some(RecordRefFamilyKind::FieldRef)
        );
    }

    /// round-1 finding I4: `FieldCount`/`KeyCount` return a scalar `Integer`
    /// — never tabled, so a chain rooted at either scalar-declines via a
    /// table miss, regardless of form/arity.
    #[test]
    fn fieldcount_and_keycount_scalar_decline() {
        assert_eq!(
            recordref_family_return_kind(&RecordRefFamilyKind::RecordRef, "fieldcount", true, 0),
            None
        );
        assert_eq!(
            recordref_family_return_kind(&RecordRefFamilyKind::RecordRef, "keycount", true, 0),
            None
        );
        assert_eq!(
            recordref_family_return_kind(&RecordRefFamilyKind::KeyRef, "fieldcount", true, 0),
            None
        );
    }

    /// round-1 finding I4 (the hardened rule this table exists to enforce):
    /// `FieldRef.Value` is variant-like LEAF data — NEVER a chainable entry,
    /// in EITHER AL syntactic form (bare property or zero-arg method call —
    /// both forms are real, observed in production AL source).
    #[test]
    fn fieldref_value_never_chainable_in_any_form() {
        assert_eq!(
            recordref_family_return_kind(&RecordRefFamilyKind::FieldRef, "value", true, 0),
            None
        );
        assert_eq!(
            recordref_family_return_kind(&RecordRefFamilyKind::FieldRef, "value", false, 0),
            None
        );
        // Even the setter-arity form (`FieldRef.Value(NewValue)`) must not
        // be mistaken for a getter-chain opportunity.
        assert_eq!(
            recordref_family_return_kind(&RecordRefFamilyKind::FieldRef, "value", true, 1),
            None
        );
    }

    /// `FieldRef.Record()` / `KeyRef.Record()` (MS Learn: zero-arg, returns
    /// `RecordRef`) were held back until a real site needed them; S9.0e found
    /// 43 in Base Application (`SourceFieldRef.Record().Number()`). The arity-1
    /// form does not exist.
    #[test]
    fn fieldref_and_keyref_record_return_recordref() {
        assert_eq!(
            recordref_family_return_kind(&RecordRefFamilyKind::FieldRef, "record", true, 0),
            Some(RecordRefFamilyKind::RecordRef)
        );
        assert_eq!(
            recordref_family_return_kind(&RecordRefFamilyKind::KeyRef, "record", true, 0),
            Some(RecordRefFamilyKind::RecordRef)
        );
        assert_eq!(
            recordref_family_return_kind(&RecordRefFamilyKind::FieldRef, "record", true, 1),
            None
        );
    }

    #[test]
    fn wrong_form_never_matches() {
        assert_eq!(
            recordref_family_return_kind(&RecordRefFamilyKind::RecordRef, "keyindex", false, 1),
            None
        );
    }

    #[test]
    fn wrong_arity_declines() {
        assert_eq!(
            recordref_family_return_kind(&RecordRefFamilyKind::RecordRef, "keyindex", true, 2),
            None
        );
        assert_eq!(
            recordref_family_return_kind(&RecordRefFamilyKind::RecordRef, "keyindex", true, 0),
            None
        );
    }

    #[test]
    fn unlisted_member_declines() {
        assert_eq!(
            recordref_family_return_kind(&RecordRefFamilyKind::RecordRef, "notamember", true, 0),
            None
        );
    }

    #[test]
    fn from_receiver_type_classifies_exactly_the_three_ref_variants() {
        assert_eq!(
            RecordRefFamilyKind::from_receiver_type(&ReceiverType::RecordRef),
            Some(RecordRefFamilyKind::RecordRef)
        );
        assert_eq!(
            RecordRefFamilyKind::from_receiver_type(&ReceiverType::FieldRef),
            Some(RecordRefFamilyKind::FieldRef)
        );
        assert_eq!(
            RecordRefFamilyKind::from_receiver_type(&ReceiverType::KeyRef),
            Some(RecordRefFamilyKind::KeyRef)
        );
        assert_eq!(
            RecordRefFamilyKind::from_receiver_type(&ReceiverType::Primitive),
            None
        );
        assert_eq!(
            RecordRefFamilyKind::from_receiver_type(&ReceiverType::Unknown),
            None
        );
    }

    #[test]
    fn to_receiver_type_round_trips() {
        for kind in [
            RecordRefFamilyKind::RecordRef,
            RecordRefFamilyKind::FieldRef,
            RecordRefFamilyKind::KeyRef,
        ] {
            assert_eq!(
                RecordRefFamilyKind::from_receiver_type(&kind.to_receiver_type()),
                Some(kind)
            );
        }
    }
}
