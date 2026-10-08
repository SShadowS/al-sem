//! Canonical, app-qualified identity for whole-program graph nodes.

use crate::snapshot::AppId;
pub use al_syntax::ir::ObjectKind;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub use crate::program::str_pool::SharedStr;

/// Interned handle for an `AppId` (cheap to copy/compare/sort).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct AppRef(pub u32);

/// Interns `AppId`s by their FULL identity (guid+name+publisher+version) — guid
/// is empty for deps today, so we never key on guid alone.
///
/// `Clone` (T3 Task 5, layered graph split): a [`crate::program::build::DepLayer`]
/// carries its own `AppRegistry` (all apps interned, primary included, for
/// `AppRef` stability) and `assemble_program_graph` clones it into each
/// assembled `ProgramGraph` — cheap relative to re-interning from scratch.
#[derive(Default, Clone)]
pub struct AppRegistry {
    by_key: HashMap<(String, String, String, String), AppRef>,
    apps: Vec<AppId>,
}

impl AppRegistry {
    pub fn intern(&mut self, app: &AppId) -> AppRef {
        let key = (
            app.guid.clone(),
            app.name.clone(),
            app.publisher.clone(),
            app.version.clone(),
        );
        if let Some(&r) = self.by_key.get(&key) {
            return r;
        }
        let r = AppRef(u32::try_from(self.apps.len()).expect("app arena overflow"));
        self.apps.push(app.clone());
        self.by_key.insert(key, r);
        r
    }

    pub fn resolve(&self, r: AppRef) -> &AppId {
        &self.apps[r.0 as usize]
    }

    /// Look up an app ref without panicking if not found (index out of range).
    pub fn try_resolve(&self, r: AppRef) -> Option<&AppId> {
        self.apps.get(r.0 as usize)
    }

    /// Find the first interned app whose `name` matches case-insensitively.
    pub fn find_by_name(&self, name: &str) -> Option<AppRef> {
        self.apps
            .iter()
            .position(|id| id.name.eq_ignore_ascii_case(name))
            .map(|i| AppRef(i as u32))
    }

    /// Look up an interned app by full identity (guid + name + publisher +
    /// version) without mutating the registry.  Returns `None` if the app was
    /// never interned via [`Self::intern`].
    pub fn find(&self, app: &AppId) -> Option<AppRef> {
        let key = (
            app.guid.clone(),
            app.name.clone(),
            app.publisher.clone(),
            app.version.clone(),
        );
        self.by_key.get(&key).copied()
    }
}

/// Object key: prefer the numeric id; fall back to the (lowercased) name for
/// id-less objects (extension objects, or where the IR has no number).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub enum ObjKey {
    Id(i64),
    /// Shared text (engine-switch S10.2, see `crate::program::str_pool`).
    Name(SharedStr),
}

/// Serde "remote" mirror of `al_syntax::ir::ObjectKind` (T3 Task 11 — `LspSnapshot`'s
/// `ItemData { node: RoutineNodeId }` JSON round-trip through `CallHierarchyItem.data`
/// needs `RoutineNodeId`'s whole identity chain to be `Serialize`/`Deserialize`).
/// `al-syntax` deliberately carries NO serde dependency (it is "the only crate that
/// touches tree-sitter," kept minimal on purpose — see its crate doc), so `ObjectKind`
/// itself cannot be derived on directly (that would require adding serde to al-syntax,
/// and neither this crate nor serde owns `ObjectKind`, so a local `impl Serialize for
/// ObjectKind` would violate the orphan rule regardless). Serde's remote-derive idiom
/// mirrors the type's shape locally instead: every variant name below MUST match
/// `ObjectKind`'s real variant list exactly (`crates/al-syntax/src/ir/decl.rs`) or this
/// stops compiling — the derive macro generates the conversion by name.
#[derive(Serialize, Deserialize)]
#[serde(remote = "ObjectKind")]
enum ObjectKindDef {
    Codeunit,
    Table,
    TableExtension,
    Page,
    PageExtension,
    Report,
    ReportExtension,
    Query,
    XmlPort,
    Enum,
    EnumExtension,
    Interface,
    ControlAddIn,
    Entitlement,
    PermissionSet,
    PermissionSetExtension,
    Profile,
    Other,
}

/// Canonical identity of an AL object within one app.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct ObjectNodeId {
    pub app: AppRef,
    #[serde(with = "ObjectKindDef")]
    pub kind: ObjectKind,
    pub key: ObjKey,
}

impl ObjectNodeId {
    /// True when this object was declared with numeric id `n`.
    pub fn id_equals_number(&self, n: i64) -> bool {
        matches!(&self.key, ObjKey::Id(k) if *k == n)
    }
}

/// Canonical identity of a routine within one object. `name_lc` is lowercased
/// (AL identifiers are case-insensitive). `enclosing_member_lc` is the
/// lowercased name of the field/member that a member-trigger is nested in (e.g.
/// the field name for a table-field `OnValidate`); `None` for regular
/// procedures and object-level triggers. This discriminator prevents same-named
/// member triggers on different fields from colliding in maps/sets.
/// `params_count` is the parameter count of the routine, used to distinguish
/// AL overloads (same name, different arity) so each overload maps to a unique
/// node. For SymbolOnly (dep boundary) routines, `params_count` is the real
/// ABI `Parameters[].len()` (Task 1) — arity checking is NOT bypassed for
/// SymbolOnly in resolution (`resolve_in_object` applies the same arity-exact
/// discipline to every tier). The one exception is a SymbolOnly routine whose
/// `Parameters` field was absent/unparseable in `SymbolReference.json`: its
/// arity is genuinely UNKNOWN, so ingestion (`abi_ingest::UNKNOWN_ARITY`)
/// gives it a sentinel `params_count` that can never equal a real call's
/// arity — it exists (for name-only lookups) but never arity-matches.
/// `sig_fp` is a stable FNV-1a fingerprint of the parameter type-text sequence
/// (`0` when `params_count == 0`, for BOTH tiers — a zero-arity routine's
/// `params_count` alone already fully discriminates). Non-zero whenever two
/// routines share the same `name_lc` AND `params_count` but differ in param
/// types: for SymbolOnly ABI routines via `abi_ingest::param_type_fp`; for
/// SOURCE routines via `sig_fp::source_param_sig_fp`
/// (sigfp-and-ambiguous-reclassification plan, Task 2 — before Task 2, SOURCE
/// `sig_fp` was unconditionally `0`; see `node_extract::RoutineNode::
/// source_overload_aliased` for the collision-guard that covered that gap).
/// Together with `name_lc` and `params_count`, extends `RoutineNodeId` to a
/// total discriminator for overloads in EITHER tier.
/// `None < Some(…)` under `Ord`, so object-level triggers sort before field
/// triggers — intentional.
///
/// `sig_fp` STABILITY (Task 2's persistence audit, applicability-param-subtype-recfield
/// plan): `sig_fp` is stable only WITHIN one build of this engine, not ACROSS versions.
/// It is derived from `param_type_fp`'s canonical tuple (outer kind + subtype id + raw
/// subtype name + a degradation tag — see `abi_ingest.rs`), so any fidelity change to
/// that reconstruction (e.g. Task 2's own param/field Subtype fix, which changed which
/// ABI overloads collapse) changes the fingerprint for affected routines. No cache,
/// incremental artifact, or CI baseline was found (grepped) to persist `RoutineNodeId`/
/// `AbiRoutineKey`/`sig_fp`/`param_type_fp` across runs — by construction there is
/// nothing to migrate/version-bump today, but a future consumer that DOES persist a
/// `RoutineNodeId` (a cache keyed on it, an incremental diff, a snapshot) must treat ABI
/// node identity as NOT durable across a fidelity change to the reconstruction logic,
/// and add its own version tag rather than assuming `sig_fp` is forward/backward stable.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct RoutineNodeId {
    pub object: ObjectNodeId,
    /// Shared text (engine-switch S10.2, see `crate::program::str_pool`).
    pub name_lc: SharedStr,
    pub enclosing_member_lc: Option<SharedStr>,
    pub params_count: usize,
    /// Stable fingerprint of the parameter type-text sequence (FNV-1a hash);
    /// `0` when `params_count == 0` in EITHER tier. Non-zero whenever two
    /// routines share the same `name_lc` AND `params_count` but differ in
    /// param types — for SymbolOnly ABI routines (`abi_ingest::
    /// param_type_fp`) and, since Task 2, for SOURCE routines too
    /// (`sig_fp::source_param_sig_fp`). Extends `RoutineNodeId` to a total
    /// discriminator. NOT stable across engine versions — see the
    /// struct-level doc note above.
    ///
    /// `#[serde(with = "sig_fp_as_string")]` (T3 Task 11 review fix-wave):
    /// an FNV-1a hash spans the FULL `u64` range, but JSON's number type is
    /// IEEE-754 double-precision — exactly representable only up to 2^53. A
    /// JavaScript-based LSP client's `JSON.parse(item.data)` (the near-universal
    /// case: VS Code extensions) would silently ROUND `sig_fp` for
    /// essentially every multi-param routine, corrupting the round-tripped
    /// `ItemData` before the follow-up `incomingCalls`/`outgoingCalls`
    /// request ever reaches this engine — a decode-then-lookup miss that
    /// fails closed to an empty result, never a loud error, in every real
    /// editor. Serializing through a decimal string carries the exact value
    /// losslessly regardless of the receiving language's number type.
    ///
    /// The string form applies to HUMAN-READABLE formats only. A `u64` is
    /// exact on a binary format by construction, so the dependency pack
    /// (`crate::program::pack`, postcard) carries a plain varint — see
    /// [`sig_fp_as_string`].
    #[serde(with = "sig_fp_as_string")]
    pub sig_fp: u64,
}

impl crate::program::str_pool::ShareStrings for ObjKey {
    fn share_strings(&mut self, pool: &mut crate::program::str_pool::StrPool) {
        match self {
            ObjKey::Id(_) => {}
            ObjKey::Name(name) => pool.share(name),
        }
    }
}

impl crate::program::str_pool::ShareStrings for ObjectNodeId {
    fn share_strings(&mut self, pool: &mut crate::program::str_pool::StrPool) {
        self.key.share_strings(pool);
    }
}

impl crate::program::str_pool::ShareStrings for RoutineNodeId {
    fn share_strings(&mut self, pool: &mut crate::program::str_pool::StrPool) {
        self.object.share_strings(pool);
        pool.share(&mut self.name_lc);
        pool.share_opt(&mut self.enclosing_member_lc);
    }
}

/// `u64`-as-JSON-string serde helper — see [`RoutineNodeId::sig_fp`]'s doc
/// for why this exists. `RoutineNodeId` is currently the only `u64` field
/// anywhere in this identity chain, so this module is private and narrowly
/// scoped to it; a future `u64` field needing the same treatment should
/// reuse this module rather than duplicating it.
///
/// Gated on `is_human_readable()`, which is TRUE for `serde_json` and FALSE
/// for postcard. The reason to gate rather than apply it unconditionally is
/// cost, and it is not hypothetical: every `RoutineNodeId` in a dependency
/// pack would otherwise pay a `to_string()` on encode and a `String` +
/// `parse::<u64>()` on decode. At the ~127k-routine scale a pack is built for
/// that is ~253k String allocations and integer parses per load — the same
/// order as the `kind_text: String` cost the `Origin` codec exists to avoid,
/// and it would have made the spec §13 measurement pessimistic by an amount
/// that has nothing to do with the format under test.
///
/// The JSON contract is untouched: `is_human_readable()` is true there, so an
/// LSP client still receives (and must still send) the quoted decimal string.
mod sig_fp_as_string {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(v: &u64, s: S) -> Result<S::Ok, S::Error> {
        if s.is_human_readable() {
            return v.to_string().serialize(s);
        }
        v.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        if d.is_human_readable() {
            let s = String::deserialize(d)?;
            return s.parse::<u64>().map_err(serde::de::Error::custom);
        }
        u64::deserialize(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use al_syntax::ir::ObjectKind;

    fn app(name: &str, ver: &str) -> crate::snapshot::AppId {
        crate::snapshot::AppId {
            guid: String::new(),
            name: name.into(),
            publisher: "P".into(),
            version: ver.into(),
        }
    }

    #[test]
    fn app_ref_interns_by_full_identity_even_with_empty_guid() {
        let mut reg = AppRegistry::default();
        let a = reg.intern(&app("Core", "29.0.0.0"));
        let a2 = reg.intern(&app("Core", "29.0.0.0"));
        let b = reg.intern(&app("Core", "28.0.0.0")); // different version
        assert_eq!(a, a2);
        assert_ne!(a, b);
        assert_eq!(reg.resolve(a).name, "Core");
    }

    #[test]
    fn object_node_id_distinguishes_same_name_across_apps() {
        let mut reg = AppRegistry::default();
        let a = reg.intern(&app("AppA", "1.0.0.0"));
        let b = reg.intern(&app("AppB", "1.0.0.0"));
        let na = ObjectNodeId {
            app: a,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Name("Sales-Post".into()),
        };
        let nb = ObjectNodeId {
            app: b,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Name("Sales-Post".into()),
        };
        assert_ne!(
            na, nb,
            "same type+name in different apps must be distinct nodes"
        );
    }

    // ── T3 Task 11 review fix-wave: sig_fp must serialize as a STRING ──────

    #[test]
    fn sig_fp_serializes_as_a_json_string_and_round_trips_past_2_pow_53() {
        let id = RoutineNodeId {
            object: ObjectNodeId {
                app: AppRef(0),
                kind: ObjectKind::Codeunit,
                key: ObjKey::Id(1),
            },
            name_lc: "foo".into(),
            enclosing_member_lc: None,
            params_count: 2,
            // u64::MAX - 1: comfortably past 2^53 (9_007_199_254_740_992) —
            // a JSON NUMBER this large would silently round through a
            // JS-based client's JSON.parse.
            sig_fp: u64::MAX - 1,
        };
        assert!(
            id.sig_fp > (1u64 << 53),
            "fixture assumption: sig_fp must exceed 2^53"
        );

        let json = serde_json::to_value(&id).expect("serialize to Value");
        let sig_fp_value = &json["sig_fp"];
        assert!(
            sig_fp_value.is_string(),
            "sig_fp must serialize as a JSON STRING, not a number \
             (JS JSON.parse silently loses precision past 2^53); got {sig_fp_value:?}"
        );
        assert_eq!(sig_fp_value.as_str().unwrap(), (u64::MAX - 1).to_string());

        let round_tripped: RoutineNodeId =
            serde_json::from_value(json).expect("deserialize from Value");
        assert_eq!(round_tripped, id);

        // Explicit to_string/from_str round trip, per the review's ask.
        let s = serde_json::to_string(&id).expect("to_string");
        assert!(
            s.contains(&format!("\"sig_fp\":\"{}\"", u64::MAX - 1)),
            "serialized JSON must embed sig_fp as a quoted string; got {s}"
        );
        let back: RoutineNodeId = serde_json::from_str(&s).expect("from_str");
        assert_eq!(back, id);
    }

    /// The other half of the `is_human_readable()` gate: on a BINARY format the
    /// decimal-string detour is pure cost (a `to_string` per encode, then a
    /// `String` allocation and a `parse` per decode) and buys nothing, because
    /// a `u64` is exact there. The dependency pack carries ~127k of these, so
    /// this is the difference between a varint and ~253k heap allocations per
    /// load.
    #[test]
    fn sig_fp_is_a_plain_u64_on_a_binary_format() {
        let id = RoutineNodeId {
            object: ObjectNodeId {
                app: AppRef(0),
                kind: ObjectKind::Codeunit,
                key: ObjKey::Id(1),
            },
            name_lc: "foo".into(),
            enclosing_member_lc: None,
            params_count: 2,
            sig_fp: u64::MAX - 1,
        };

        let bytes = postcard::to_stdvec(&id).expect("postcard encode");
        let decimal = (u64::MAX - 1).to_string();
        assert!(
            !bytes
                .windows(decimal.len())
                .any(|w| w == decimal.as_bytes()),
            "the binary encoding must not contain the decimal text {decimal:?} — \
             sig_fp is going over the wire as a string"
        );
        let back: RoutineNodeId = postcard::from_bytes(&bytes).expect("postcard decode");
        assert_eq!(back, id);
    }
}
