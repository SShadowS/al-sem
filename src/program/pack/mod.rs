//! Dependency pack codec — one app's extraction output, to bytes and back.
//!
//! Persistence only. No engine wiring lives here: obtaining a pack, keying it
//! and loading it into `build_dep_layer` is spec step 5. This module exists so
//! the format's round-trip cost can be measured before that work starts, per
//! `docs/superpowers/specs/2026-08-07-dependency-pack-cache-design.md` §13.
//!
//! Kept separate from `node_extract.rs` deliberately: extraction logic and the
//! persistence format have different reasons to change, and the spec's
//! EXTRACTION_FINGERPRINT closure (§8) names this module in its own right.
//!
//! # Wire layout
//!
//! ```text
//! postcard(PackHeader { schema, self_hash })  ‖  postcard(DepPack)
//! ```
//!
//! The hash covers the BODY BYTES, so [`decode`] verifies with one blake3 pass
//! over a slice it already holds — it never re-encodes. An earlier revision
//! stored `self_hash` as a plain field and re-serialized the decoded pack to
//! check it, which cost 44 % of decode in a DEBUG build (33 % in release — see
//! CHANGELOG's `[Unreleased]` entry for the release figure) and would have
//! made the measurement in spec §13 unattributable: near the abandon
//! threshold we could not have told a slow format from a re-encode we chose
//! to pay. The hash VALUE is unchanged by the envelope (same fields, same
//! order, same bytes), and so is this module's public interface.
//!
//! Hashing the encoded bytes changes WHICH guard catches a corruption
//! postcard itself rejects anyway: the hash now fires first, instead of
//! never being reached. On all evidence gathered, both orderings reject the
//! same corruption set — what moved is which check catches it, not the
//! set's size (see CHANGELOG's `[Unreleased]` entry for the controlled
//! before/after).
//!
//! # Two things a reader should know
//!
//! 1. **Node ids carry an `AppRef`, and step 5 must re-intern it on load.** The
//!    app-identity HEADER on [`DepPack`] is symbolic, but the payload is not —
//!    see [`DepPack::app_guid`].
//! 2. **Nothing here writes to a cache directory, and nothing should yet.** The
//!    key (spec §7) and the behavioural canary (§8) are what make a real cache
//!    directory safe; until they exist, a pack on disk under a wrong key is a
//!    stale pack served as fresh. `encode`/`decode` are pure byte functions —
//!    keep them that way.

use serde::{Deserialize, Serialize};

use al_syntax::ir::{Origin, Point};
use al_syntax::raw::RawKind;

use crate::program::node::RoutineNodeId;
use crate::program::node_extract::{ObjectNode, RoutineNode};
use crate::program::resolve::decl_surface::RoutineMeta;

/// Bump when `DepPack`'s or `PackedFile`'s shape changes. Old packs then fail
/// the check in [`decode`] and are recomputed. Never migrate in place.
///
/// CAVEAT: this is a SECOND line of defence, not the mechanism that rejects an
/// old pack. postcard is not self-describing, so a genuinely different-shaped
/// body usually fails `from_bytes` before any field is validated. The schema
/// lives in the HEADER precisely so it is read before the body is parsed at
/// all — but a shape change within a header-compatible layout is still caught
/// by the body parse or the hash, not by this check. Every path is a miss, so
/// this is not a soundness hole; it is only not the whole story.
/// 2: parameter names + `temporary` in `AbiParamRetained` / `ParamMeta`
/// (engine-switch S2b.5).
/// 3: `ParsedSubscriberArgs::publisher_id` (engine-switch S4.3b).
/// 4: `ObjectNode::protected_vars` (engine-switch S9.0e).
/// 5: `ObjectNode::query_columns` (engine-switch S9.0e).
/// 6: `RoutineNode::preproc_context` (engine-switch S9.0e).
pub const PACK_SCHEMA: u32 = 6;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackedFile {
    pub virtual_path: String,
    /// `AlFile::parse_status == ParseStatus::Recovered` for this file. Stored
    /// rather than recomputed: on a pack hit the file is never parsed, and
    /// `snapshot::parse::recovered_file_paths` is the load-bearing
    /// absence-proof diagnostic that must still see it (spec §11.2).
    pub parse_status_recovered: bool,
    pub objects: Vec<ObjectNode>,
    pub routines: Vec<RoutineNode>,
    /// Per-routine decl metadata, keyed as `DeclSurface` keys it. NOT derivable
    /// on a pack hit: `RoutineMeta::from_decl` consumes a `RoutineDecl`, and the
    /// `ParsedUnit`s those come from are exactly what a hit avoids building.
    /// Nothing in `RoutineNode` can reconstruct `virtual_path`, the two
    /// `Origin`s, or per-param `ty`/`by_ref` (spec §4).
    pub routine_meta: Vec<(RoutineNodeId, RoutineMeta)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DepPack {
    pub schema: u32,
    /// The app-identity HEADER, stored SYMBOLICALLY: guid/name/publisher/version,
    /// never an `AppRef`. `AppRef` is a per-run interning index handed out in
    /// encounter order by `AppRegistry::intern` (`src/program/node.rs`), so a
    /// persisted one is meaningless in the next run.
    ///
    /// **The PAYLOAD is a different matter, and this is load-bearing for step 5.**
    /// `ObjectNodeId.app` IS an `AppRef`, `RoutineNodeId` embeds an
    /// `ObjectNodeId`, and every packed routine carries three of them
    /// (`ObjectNode.id`, `RoutineNode.id`, and the `routine_meta` key). Those
    /// ids are persisted as-is, so **a loader MUST re-intern every `AppRef`
    /// against the current run's `AppRegistry`** before the nodes join a graph.
    /// Skipping that yields silently-wrong graphs rather than errors.
    ///
    /// The re-intern deliberately does NOT happen inside [`decode`], and this
    /// module exposes no surface for it: spec §13 requires the gate to MEASURE
    /// that cost as part of the load, so burying it in the codec would hide it
    /// from the very number it is meant to inform. Implementing it is step 5.
    pub app_guid: String,
    pub app_name: String,
    pub app_publisher: String,
    pub app_version: String,
    /// Per-file contributions in extraction order, pre-dedup. Order is
    /// load-bearing: `dedup_routines_preserving_genuine_overloads` keeps the
    /// first occurrence per key, so a reordered pack changes which survivor
    /// wins (spec §12).
    pub files: Vec<PackedFile>,
    /// blake3 over the postcard encoding of every field above, hex.
    ///
    /// `#[serde(skip)]` is what makes the envelope correct, not an oversight:
    /// the encoded `DepPack` IS the body the hash covers, so the hash cannot
    /// also be inside it. [`encode`] writes this value into the header and
    /// [`decode`] restores it from there, so a round-trip is lossless.
    ///
    /// The consequence a reader must know: serializing a `DepPack` through any
    /// OTHER format (a `serde_json` debug dump, say) silently omits this field.
    /// The upside is that a field added to this struct is covered by the hash
    /// automatically — only an explicit `skip` can exclude one.
    #[serde(skip)]
    pub self_hash: String,
}

#[derive(Debug)]
pub enum PackError {
    Codec(String),
    SchemaMismatch { found: u32, expected: u32 },
    SelfHashMismatch,
}

impl std::fmt::Display for PackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackError::Codec(e) => write!(f, "pack codec error: {e}"),
            PackError::SchemaMismatch { found, expected } => {
                write!(f, "pack schema {found}, expected {expected}")
            }
            PackError::SelfHashMismatch => write!(f, "pack self-hash mismatch"),
        }
    }
}

impl std::error::Error for PackError {}

// ---------------------------------------------------------------------------
// Origin wire codec
// ---------------------------------------------------------------------------

/// The wire shape of an [`Origin`].
///
/// `Origin` itself stays exactly as the IR defines it: its `kind_text` is a
/// `&'static str` fed verbatim to anchor `syntax_kind` for parity, so widening
/// it to `String` (≈2 heap allocations per routine, forever, to carry a value
/// from a closed 389-member set) or dropping it (an unaudited lossy field) were
/// both rejected. `&'static str` cannot `Deserialize`; a `RawKind` can, and
/// [`RawKind::as_str`] hands back a `&'static str` on the way out — so decoding
/// an `Origin` allocates NOTHING on the binary path, where the wire carries a
/// varint. On a human-readable format the wire carries the kind STRING instead
/// (`raw_kind_wire`) — that path is for debugging, not for the gate, and
/// stability matters there more than allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackedOrigin {
    /// `kind_text` as a closed-set discriminant: a varint index on a binary
    /// format, the grammar kind string on a human-readable one — see
    /// `raw_kind_wire`.
    #[serde(with = "raw_kind_wire")]
    pub kind: RawKind,
    pub byte_start: usize,
    pub byte_end: usize,
    pub start: (u32, u32),
    pub end: (u32, u32),
}

impl PackedOrigin {
    /// `None` when `kind_text` is not a named grammar kind — the app is then
    /// not packed. Cold path, fail-closed, never a panic. (`RawNode::kind_str`
    /// returns the raw kind of ANY node, named or anonymous, and nothing at the
    /// type level restricts which nodes an `Origin` is minted from — which is
    /// why this is fallible rather than a `RawKind` field in the IR.)
    #[must_use]
    pub fn try_from_origin(o: &Origin) -> Option<Self> {
        Some(PackedOrigin {
            kind: RawKind::try_from_raw(o.kind_text)?,
            byte_start: o.byte.start,
            byte_end: o.byte.end,
            start: (o.start.row, o.start.column),
            end: (o.end.row, o.end.column),
        })
    }

    /// Infallible: every `RawKind` has a canonical kind string.
    #[must_use]
    pub fn into_origin(self) -> Origin {
        Origin {
            kind_text: self.kind.as_str(),
            byte: self.byte_start..self.byte_end,
            start: Point {
                row: self.start.0,
                column: self.start.1,
            },
            end: Point {
                row: self.end.0,
                column: self.end.1,
            },
        }
    }
}

/// `RawKind` as a varint index into [`RawKind::ALL`] on a BINARY format, and as
/// its grammar kind string on a human-readable one.
///
/// The index is POSITIONAL and therefore not stable across grammar revisions — a
/// kind added mid-alphabet shifts every later index. That is sound for a pack
/// only because `crates/` is inside the pack fingerprint closure (spec §8), so
/// regenerating the raw vocabulary invalidates every pack. Nothing gives a
/// human-readable dump that protection, so it gets the STRING, which is stable
/// by construction. An out-of-range index decodes to an error, never to a wrong
/// kind.
mod raw_kind_wire {
    use al_syntax::raw::RawKind;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    // `&RawKind` rather than `RawKind` because serde's `with` module requires
    // this exact signature.
    pub fn serialize<S: Serializer>(k: &RawKind, s: S) -> Result<S::Ok, S::Error> {
        if s.is_human_readable() {
            return k.as_str().serialize(s);
        }
        // `RawKind` is fieldless with implicit discriminants, so `as u32` is
        // its declaration index; `RawKind::ALL[k as usize] == k` is asserted
        // exhaustively in al-syntax's own `raw` tests.
        (*k as u32).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<RawKind, D::Error> {
        if d.is_human_readable() {
            let s = String::deserialize(d)?;
            return RawKind::try_from_raw(&s)
                .ok_or_else(|| serde::de::Error::custom(format!("unknown raw kind {s:?}")));
        }
        let i = u32::deserialize(d)?;
        // A STATIC message, not a `format!`: on the binary path the format is
        // postcard, whose `de::Error::custom` takes `_msg` and DISCARDS it
        // (`postcard-1.1.3/src/error.rs`), so an interpolated index would be
        // built and thrown away. The human-readable arm above keeps its detail
        // because `serde_json` does preserve it.
        RawKind::ALL
            .get(i as usize)
            .copied()
            .ok_or_else(|| serde::de::Error::custom("raw kind index out of range"))
    }
}

/// `Origin` through [`PackedOrigin`], for `#[serde(with = ...)]` on any field
/// holding one. Fallible on encode (an unencodable kind means the app is not
/// packed), infallible on decode.
pub mod origin_wire {
    use super::PackedOrigin;
    use al_syntax::ir::Origin;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(o: &Origin, s: S) -> Result<S::Ok, S::Error> {
        match PackedOrigin::try_from_origin(o) {
            Some(p) => p.serialize(s),
            None => Err(serde::ser::Error::custom(format!(
                "origin kind {:?} is not a named grammar kind",
                o.kind_text
            ))),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Origin, D::Error> {
        Ok(PackedOrigin::deserialize(d)?.into_origin())
    }
}

// ---------------------------------------------------------------------------
// Codec
// ---------------------------------------------------------------------------

/// The prefix in front of the body, read before the body is parsed, so an
/// incompatible format and a corrupt payload are both rejected without ever
/// decoding the payload.
///
/// NOT fixed-size: `schema` is a postcard varint (1 byte at `PACK_SCHEMA` = 1,
/// 2 bytes from 128), and `self_hash` is a length-prefixed string. Nothing may
/// index into the file at a hardcoded offset.
///
/// **The header is OUTSIDE the hash**, which is why `schema` also stays on
/// [`DepPack`] — see [`decode`] for the invariant that ties the two together.
#[derive(Debug, Serialize, Deserialize)]
struct PackHeader<'a> {
    schema: u32,
    /// blake3 of the body bytes that follow, hex — the value of
    /// [`DepPack::self_hash`].
    #[serde(borrow)]
    self_hash: &'a str,
}

/// blake3 over the postcard encoding of every `DepPack` field except
/// `self_hash` — i.e. over exactly the bytes [`encode`] writes as the body.
/// That identity is what lets [`decode`] verify the bytes it already holds
/// instead of re-encoding what it decoded.
///
/// `self_hash` is excluded structurally (`#[serde(skip)]` on the field), not by
/// a hand-maintained field list, so a field added to `DepPack` is hashed
/// automatically.
///
/// Returns the hash of an empty input if the pack contains an unencodable
/// `Origin` kind; that pack cannot be [`encode`]d either, so the failure is
/// still surfaced — loudly and with the offending kind named — there.
#[must_use]
pub fn compute_self_hash(pack: &DepPack) -> String {
    let bytes = postcard::to_stdvec(pack).unwrap_or_default();
    blake3::hash(&bytes).to_hex().to_string()
}

/// Serialize a pack. The caller must have set `self_hash` via
/// [`compute_self_hash`] first.
pub fn encode(pack: &DepPack) -> Result<Vec<u8>, PackError> {
    let header = PackHeader {
        schema: pack.schema,
        self_hash: &pack.self_hash,
    };
    let out = postcard::to_extend(&header, Vec::new()).map_err(map_ser_err(pack))?;
    // `to_extend` appends the body straight into the header's buffer, so the
    // envelope costs no extra copy of the payload.
    postcard::to_extend(pack, out).map_err(map_ser_err(pack))
}

/// postcard DISCARDS `ser::Error::custom` messages (its `Error::custom` takes
/// `_msg`), so the only way to say WHICH origin failed is to go find it. Done
/// on the failure path only, so the diagnostic costs nothing when encoding
/// succeeds.
fn map_ser_err(pack: &DepPack) -> impl Fn(postcard::Error) -> PackError + '_ {
    move |e| match e {
        postcard::Error::SerdeSerCustom => PackError::Codec(unencodable_origin_report(pack)),
        other => PackError::Codec(other.to_string()),
    }
}

/// Name the first `Origin` whose kind cannot be encoded. Failure path only.
fn unencodable_origin_report(pack: &DepPack) -> String {
    for file in &pack.files {
        for (_, meta) in &file.routine_meta {
            for (which, origin) in [("origin", &meta.origin), ("name_origin", &meta.name_origin)] {
                if RawKind::try_from_raw(origin.kind_text).is_none() {
                    return format!(
                        "{}: routine {} {which} kind {:?} is not a named grammar kind, so this \
                         app is not packable",
                        file.virtual_path, meta.name, origin.kind_text
                    );
                }
            }
        }
    }
    "serde rejected the pack, but no unencodable Origin kind was found".to_string()
}

/// Deserialize a pack, checking schema then integrity.
///
/// Every abnormal state is an `Err`, never a partial `Ok` — the consumer in
/// spec step 5 treats any `Err` as a cache miss and recomputes.
///
/// # Why `schema` is on the wire twice, and why both copies are checked
///
/// The HEADER copy exists so an incompatible format is rejected before the
/// body is parsed or even hashed — that is what a header is for. But the
/// header is outside the hash, so that copy is UNAUTHENTICATED. The BODY copy
/// is inside the hash, so it is the authoritative one.
///
/// Dropping the body copy (header-only, one byte cheaper) would leave the only
/// schema on the wire unauthenticated, and that has a concrete window once a
/// second version exists: a flip turning a future pack's header from `2` into
/// `1` passes the header check, passes the body hash (which does not cover the
/// header), and gets a v2 body parsed as v1. Comparing the two closes it.
///
/// The redundancy is therefore an ENFORCED invariant rather than an assumption,
/// which is the opposite of a maintenance hazard: `encode` always writes the
/// two equal, and a pack where they disagree was written by a broken writer or
/// had its header edited. Either way it is a miss.
pub fn decode(bytes: &[u8]) -> Result<DepPack, PackError> {
    let (header, body) = postcard::take_from_bytes::<PackHeader>(bytes)
        .map_err(|e| PackError::Codec(e.to_string()))?;
    if header.schema != PACK_SCHEMA {
        return Err(PackError::SchemaMismatch {
            found: header.schema,
            expected: PACK_SCHEMA,
        });
    }
    // One blake3 pass over a slice already in hand. `to_hex()` returns a
    // stack `ArrayString`, so this comparison allocates nothing.
    if blake3::hash(body).to_hex().as_str() != header.self_hash {
        return Err(PackError::SelfHashMismatch);
    }
    let mut pack: DepPack =
        postcard::from_bytes(body).map_err(|e| PackError::Codec(e.to_string()))?;
    if pack.schema != header.schema {
        return Err(PackError::Codec(format!(
            "header schema {} disagrees with body schema {} — the pack was written by a \
             broken writer, or its header was edited",
            header.schema, pack.schema
        )));
    }
    // `self_hash` is `#[serde(skip)]` (it is the hash OF the body), so it
    // arrives defaulted and is restored from the header.
    pack.self_hash = header.self_hash.to_string();
    Ok(pack)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::node::{ObjKey, ObjectNodeId};
    use crate::program::node_extract::test_fixtures::{
        fully_populated_object_node, fully_populated_routine_node,
    };
    use crate::program::resolve::decl_surface::ParamMeta;
    use al_syntax::ir::ObjectKind;

    fn origin(kind_text: &'static str, lo: usize, hi: usize) -> Origin {
        Origin {
            kind_text,
            byte: lo..hi,
            start: Point { row: 3, column: 4 },
            end: Point { row: 5, column: 6 },
        }
    }

    fn sample_routine_id() -> RoutineNodeId {
        RoutineNodeId {
            object: ObjectNodeId {
                app: crate::program::node::AppRef(0),
                kind: ObjectKind::Codeunit,
                key: ObjKey::Id(80),
            },
            name_lc: "postsalesdocument".into(),
            enclosing_member_lc: Some("no.".into()),
            // 2^53 + 1 — outside JSON's exactly-representable integer range, so
            // this also keeps `RoutineNodeId`'s decimal-string `sig_fp` codec
            // under a value that would round if it were ever treated as a
            // double.
            params_count: 2,
            sig_fp: 9_007_199_254_740_993,
        }
    }

    /// A fully-populated `RoutineMeta`: both `Origin`s and a non-empty
    /// `params` with a `Some(ty)` and a `None` ty. An empty fixture here would
    /// make the entire `Origin` codec — the substance of this module —
    /// round-trip vacuously.
    fn sample_routine_meta() -> RoutineMeta {
        RoutineMeta {
            name: "PostSalesDocument".into(),
            enclosing_member: Some("No.".into()),
            parse_incomplete: true,
            params: vec![
                ParamMeta {
                    name: "SalesHeader".into(),
                    ty: Some("Record \"Sales Header\" temporary".into()),
                    by_ref: true,
                },
                ParamMeta {
                    name: String::new(),
                    ty: None,
                    by_ref: false,
                },
            ],
            origin: origin("procedure", 100, 240),
            name_origin: origin("identifier", 110, 127),
            virtual_path: "src/Sales/SalesPost.Codeunit.al".into(),
        }
    }

    fn sample_pack() -> DepPack {
        DepPack {
            schema: PACK_SCHEMA,
            app_guid: "437dbf0e-84ff-417a-965d-ed2bb9650972".to_string(),
            app_name: "Base Application".to_string(),
            app_publisher: "Microsoft".to_string(),
            app_version: "28.2.0.0".to_string(),
            files: vec![PackedFile {
                virtual_path: "src/Sales/SalesPost.Codeunit.al".to_string(),
                parse_status_recovered: true,
                // Fix round 1 (#M-2): real nodes, not `vec![]`. These are the
                // bulk of what the spec §13 gate prices, and postcard is not
                // self-describing — Task 4's JSON round trip is not evidence
                // about the binary one. Shared with `node_extract`'s own tests
                // so both exercise the identical maximal node.
                objects: vec![fully_populated_object_node()],
                routines: vec![fully_populated_routine_node()],
                routine_meta: vec![(sample_routine_id(), sample_routine_meta())],
            }],
            self_hash: String::new(),
        }
    }

    #[test]
    fn encode_decode_round_trips() {
        let mut pack = sample_pack();
        pack.self_hash = compute_self_hash(&pack);
        let bytes = encode(&pack).expect("encode");
        let back = decode(&bytes).expect("decode");
        assert_eq!(back.app_guid, pack.app_guid);
        assert_eq!(back.files.len(), 1);
        assert_eq!(back.files[0].virtual_path, pack.files[0].virtual_path);
        assert!(back.files[0].parse_status_recovered);
        // Total, not field-by-field: a field added to `DepPack`/`PackedFile`/
        // `RoutineMeta` later is covered without editing this assertion.
        assert_eq!(back, pack);
    }

    /// The `Origin` codec specifically: a decoded `kind_text` must be the same
    /// `&'static str` the IR would have produced, and every position field must
    /// survive. `assert_eq!(back, pack)` above covers this too, but this test
    /// says WHICH property is load-bearing, and fails with a readable message.
    #[test]
    fn origin_round_trips_through_the_raw_kind_discriminant() {
        let o = origin("trigger_declaration", 7, 91);
        let packed = PackedOrigin::try_from_origin(&o).expect("named kind must encode");
        assert_eq!(packed.kind, RawKind::TriggerDeclaration);
        let back = packed.into_origin();
        assert_eq!(back, o);
        // The decoded string is the grammar's own `&'static str`, not a copy —
        // this is the property that makes decode allocation-free.
        assert_eq!(back.kind_text, RawKind::TriggerDeclaration.as_str());
    }

    /// Fix round 1 (#M-8): the positional index is only safe where the pack
    /// fingerprint (spec §8) invalidates it on a grammar revision. A
    /// human-readable dump has no such protection, so it must carry the
    /// grammar STRING — stable by construction.
    #[test]
    fn a_human_readable_origin_carries_the_kind_string_not_the_index() {
        let meta = sample_routine_meta();
        let json = serde_json::to_string(&meta).expect("serialize to JSON");
        assert!(
            json.contains(r#""kind":"procedure""#),
            "a JSON Origin must carry the grammar kind string, not a \
             grammar-revision-unstable index; got: {json}"
        );
        assert!(
            json.contains(r#""kind":"identifier""#),
            "name_origin's kind must be a string too; got: {json}"
        );
        let back: RoutineMeta = serde_json::from_str(&json).expect("deserialize from JSON");
        assert_eq!(back, meta);
    }

    /// Hand-stated precondition: an `Origin` whose `kind_text` is not a named
    /// grammar kind. Production cannot currently produce one (see
    /// `every_decl_origin_kind_is_a_named_grammar_kind`), which is exactly why
    /// the fixture is constructed literally rather than obtained from a parse.
    #[test]
    fn an_unencodable_origin_kind_fails_the_pack_instead_of_panicking() {
        assert_eq!(
            PackedOrigin::try_from_origin(&origin("definitely_not_a_real_kind", 0, 1)),
            None
        );

        let mut pack = sample_pack();
        pack.files[0].routine_meta[0].1.origin = origin("definitely_not_a_real_kind", 0, 1);
        pack.self_hash = compute_self_hash(&pack);
        match encode(&pack) {
            Err(PackError::Codec(msg)) => {
                // Fail-closed AND diagnosable: the message must name the kind,
                // or an unpackable app becomes a mystery cache-miss rate.
                assert!(
                    msg.contains("definitely_not_a_real_kind"),
                    "codec error must name the offending kind, got: {msg}"
                );
                assert!(
                    msg.contains("PostSalesDocument"),
                    "codec error must name the offending routine, got: {msg}"
                );
            }
            other => panic!("expected PackError::Codec, got {other:?}"),
        }
    }

    /// The WHOLE single-bit corruption population, partitioned by which guard
    /// rejects it — and `leaked` asserted to be zero.
    ///
    /// This is stronger than sampling one flip: it says that of every reachable
    /// single-bit corruption, NONE decodes to a wrong pack, and it reports how
    /// the work is divided so the self-hash's contribution is a measured number
    /// rather than an assumption.
    ///
    /// Under the pre-envelope layout the hash was checked AFTER the body was
    /// decoded, so a corruption postcard rejected never reached it and `codec`
    /// carried a real share. (`identical` was 0 in both layouts.) Hashing the
    /// encoded body moved that work: the hash now runs FIRST and catches
    /// everything in the body.
    ///
    /// STATED LIMIT: this test cannot discriminate on WHICH slice is hashed.
    /// Hashing the wrong slice (`&body[..0]`, say) makes every flip fail the
    /// hash, so the partition still shows `leaked == 0` and `hash > 0` and this
    /// test stays green. `encode_decode_round_trips` is the test that catches
    /// it — an honest pack stops decoding — and the probe that proves so breaks
    /// exactly that (fix round 1, R2).
    #[test]
    fn every_single_bit_corruption_is_rejected_and_the_hash_catches_most() {
        let mut pack = sample_pack();
        pack.self_hash = compute_self_hash(&pack);
        let bytes = encode(&pack).expect("encode");

        let (mut hash, mut schema, mut codec, mut identical) = (0usize, 0usize, 0usize, 0usize);
        let mut leaked: Vec<(usize, u32)> = Vec::new();
        for bit in 0..8u32 {
            for i in 0..bytes.len() {
                let mut c = bytes.clone();
                c[i] ^= 1u8 << bit;
                match decode(&c) {
                    Err(PackError::SelfHashMismatch) => hash += 1,
                    Err(PackError::SchemaMismatch { .. }) => schema += 1,
                    Err(PackError::Codec(_)) => codec += 1,
                    // A flip that still yields the identical pack corrupted
                    // nothing observable, so accepting it is correct.
                    Ok(p) if p == pack => identical += 1,
                    Ok(_) => leaked.push((i, bit)),
                }
            }
        }
        let total = bytes.len() * 8;
        assert!(
            leaked.is_empty(),
            "corrupted packs decoded to a DIFFERENT pack with no guard firing, at \
             (byte, bit): {leaked:?}"
        );
        // NOTE: this sum is a structural invariant of the LOOP, not a property
        // of the code under test — the loop runs exactly `total` times and each
        // arm increments exactly one counter, so it holds by construction and
        // no change to `decode` can break it. It is kept (unlike the `ALL.len()`
        // assertion, which a compile error already covered) because it DOES
        // catch a future edit here that adds a match arm counting nothing, which
        // would silently shrink the population without shrinking `total`.
        assert_eq!(hash + schema + codec + identical + leaked.len(), total);
        assert!(
            hash > 0,
            "no flip was caught by the self-hash, so this test proves nothing about it"
        );
        println!(
            "corruption partition over {total} single-bit flips ({} bytes): \
             self_hash={hash} schema={schema} codec={codec} identical={identical} leaked={}",
            bytes.len(),
            leaked.len()
        );
    }

    /// STATED LIMIT: this test cannot discriminate the integrity check.
    /// Deleting the `self_hash` comparison from [`decode`] leaves it GREEN,
    /// because its `^= 0xFF` flip lands in UTF-8 string content and postcard's
    /// own decoder rejects it — the test then passes through the `Codec` arm.
    /// That is a real property of the code, not a broken test, so the test is
    /// kept as the brief wrote it and the guard is pinned by
    /// `every_single_bit_corruption_is_rejected_and_the_hash_catches_most`
    /// instead, which enumerates the whole population and dies loudly (with
    /// hundreds of leaks) when the check is removed.
    ///
    /// This limit was measured, not assumed, in both layouts: pre-envelope
    /// (fix round 1, probe P1) and post-envelope (probe S3).
    #[test]
    fn a_corrupted_payload_is_rejected_not_misread() {
        let mut pack = sample_pack();
        pack.self_hash = compute_self_hash(&pack);
        let mut bytes = encode(&pack).expect("encode");
        // Flip one byte deep in the payload, past the schema prefix.
        let idx = bytes.len() / 2;
        bytes[idx] ^= 0xFF;
        match decode(&bytes) {
            Err(PackError::SelfHashMismatch) | Err(PackError::Codec(_)) => {}
            Err(other) => panic!("unexpected error: {other:?}"),
            Ok(_) => panic!("a corrupted pack decoded successfully — integrity check is absent"),
        }
    }

    #[test]
    fn a_pack_from_a_different_schema_is_rejected() {
        let mut pack = sample_pack();
        pack.schema = PACK_SCHEMA + 1;
        pack.self_hash = compute_self_hash(&pack);
        let bytes = encode(&pack).expect("encode");
        match decode(&bytes) {
            Err(PackError::SchemaMismatch { found, expected }) => {
                assert_eq!(found, PACK_SCHEMA + 1);
                assert_eq!(expected, PACK_SCHEMA);
            }
            other => panic!("expected SchemaMismatch, got {other:?}"),
        }
    }

    /// `schema` is on the wire twice — once in the header (fast, unhashed) and
    /// once in the body (authenticated). `decode` must compare them, or the
    /// only copy anyone checked would be the one the hash does not cover.
    ///
    /// Hand-stated precondition: `encode` cannot produce a pack whose two
    /// copies disagree, so the fixture is FORGED — encode a schema-2 pack, then
    /// patch the header's leading varint back to 1. That is exactly the
    /// bit-flip this check exists for: it passes the header check, and passes
    /// the body hash, because the hash does not cover the header.
    #[test]
    fn a_header_whose_schema_disagrees_with_the_body_is_rejected() {
        let mut pack = sample_pack();
        pack.schema = PACK_SCHEMA + 1;
        pack.self_hash = compute_self_hash(&pack);
        let mut bytes = encode(&pack).expect("encode");

        // Precondition, asserted rather than assumed: byte 0 is the header's
        // `schema` varint. If the layout changes this fails loudly instead of
        // silently testing nothing.
        assert_eq!(
            bytes[0],
            u8::try_from(PACK_SCHEMA + 1).unwrap(),
            "fixture assumption: byte 0 is the header schema varint"
        );
        bytes[0] = u8::try_from(PACK_SCHEMA).unwrap();

        match decode(&bytes) {
            Err(PackError::Codec(msg)) => {
                assert!(
                    msg.contains("header schema") && msg.contains("body schema"),
                    "the error must say the two copies disagree, got: {msg}"
                );
            }
            other => panic!(
                "a pack whose header says {PACK_SCHEMA} and whose body says {} must be \
                 rejected, got {other:?}",
                PACK_SCHEMA + 1
            ),
        }
    }

    /// `self_hash` is excluded structurally by `#[serde(skip)]`, so a field
    /// added to `DepPack` is hashed automatically; this test makes a field
    /// that IS hashed observably load-bearing.
    #[test]
    fn the_self_hash_covers_the_body_fields() {
        let base = sample_pack();
        let h = compute_self_hash(&base);
        for mutate in [
            (|p: &mut DepPack| p.app_guid.push('x')) as fn(&mut DepPack),
            |p: &mut DepPack| p.app_version.push('x'),
            |p: &mut DepPack| p.files[0].virtual_path.push('x'),
            |p: &mut DepPack| p.files[0].parse_status_recovered = false,
            |p: &mut DepPack| p.files[0].routine_meta[0].1.name.push('x'),
            |p: &mut DepPack| p.files[0].routine_meta[0].1.origin.byte.end += 1,
            |p: &mut DepPack| p.files[0].routine_meta[0].1.params[0].by_ref = false,
            |p: &mut DepPack| p.files[0].routine_meta[0].0.sig_fp += 1,
        ] {
            let mut p = base.clone();
            mutate(&mut p);
            assert_ne!(compute_self_hash(&p), h, "a body change must move the hash");
        }
        // `self_hash` itself is NOT part of its own input.
        let mut p = base.clone();
        p.self_hash = "not the real hash".into();
        assert_eq!(compute_self_hash(&p), h);
    }

    /// Every `.al` file under the two committed fixture corpora.
    fn fixture_al_files() -> Vec<std::path::PathBuf> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
        let mut out: Vec<std::path::PathBuf> = ["r0-corpus", "fixtures"]
            .iter()
            .flat_map(|d| walkdir::WalkDir::new(root.join(d)).into_iter().flatten())
            .filter(|e| e.file_type().is_file())
            .map(walkdir::DirEntry::into_path)
            .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("al")))
            .collect();
        out.sort();
        out
    }

    #[test]
    fn every_decl_origin_kind_is_a_named_grammar_kind() {
        // Lower a real fixture corpus and assert both Origins on every routine decl
        // survive try_from_raw. If this ever fails, the encoder's None arm is live
        // and that app silently stops being packable -- which is safe, but we want
        // to KNOW, not discover it as a mystery cache-miss rate.
        let files = fixture_al_files();
        assert!(
            !files.is_empty(),
            "no .al fixtures found -- the corpus walk is broken, not the corpus"
        );
        let mut checked = 0usize;
        for path in files {
            let file = al_syntax::parse(&std::fs::read_to_string(&path).unwrap());
            for obj in &file.objects {
                for r in &obj.routines {
                    assert!(
                        RawKind::try_from_raw(r.origin.kind_text).is_some(),
                        "{}: routine {} origin kind {:?} is not a named grammar kind",
                        path.display(),
                        r.name,
                        r.origin.kind_text
                    );
                    assert!(
                        RawKind::try_from_raw(r.name_origin.kind_text).is_some(),
                        "{}: routine {} name_origin kind {:?} is not a named grammar kind",
                        path.display(),
                        r.name,
                        r.name_origin.kind_text
                    );
                    checked += 1;
                }
            }
        }
        assert!(
            checked > 0,
            "the corpus produced no routines -- the test proved nothing"
        );
        // Printed so the count is an observed number in the run log, not a
        // claim: `cargo test -- --nocapture`.
        println!("every_decl_origin_kind_is_a_named_grammar_kind: checked {checked} routines");
    }
}
