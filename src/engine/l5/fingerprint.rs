//! `fingerprintOf` — port of al-sem `src/projection/finding-fingerprint.ts`.
//!
//! A stable edit-survival key. CRITICAL: it hashes the INTERNAL `rootCauseKey` +
//! INTERNAL `affectedTables` — the finding BEFORE the stable projection. So the
//! detector computes it over internal ids, and the projection copies it verbatim.
//!
//!   sha256( detector | objectType/objectKey | routineName |
//!           affectedTables.join(",") | rootCauseKey )  → first 16 hex chars
//!
//! `objectType/objectKey` (the key is the number, or `@name` for a numberless
//! object) is the OWNING object of the finding's
//! `primaryLocation.enclosingRoutineId` (empty when unresolved); `routineName` is
//! that routine's name (empty when unresolved). Uses plain UTF-8 sha256 (Node's
//! `createHash("sha256").update(str)`), NOT the UTF-16 length-prefixed framing.
//!
//! ## Routine-id stabilization (edit-stable fingerprint)
//!
//! The `rootCauseKey` embeds INTERNAL routine ids — directly, or as the PREFIX of
//! operation/callsite/loop ids (`${routineId}/suffix`). For SOURCE routines that
//! internal id is `${modelInstanceId}/${keyHash}`; for DEPENDENCY routines it is
//! `dep:<artifactKey>/<keyHash>`. Either form is unstable across edits/cache bumps:
//! `modelInstanceId` is content-derived (shifts when any workspace `.al` file is
//! added/removed/renamed) in the content-MII gate path, and `<artifactKey>` is
//! cache-derived (drifts on every cache bump). Hashing either verbatim makes the
//! fingerprint unstable for an identical logical finding.
//!
//! To match al-sem's NEW stabilized fingerprint (`src/projection/finding-fingerprint.ts`),
//! we build a map from EVERY routine's internal id → its `StableRoutineId`
//! (`${appGuid}:${objectType}:${objectKey}#${64 lowercase hex}` — the hex is the
//! `normalizedSignatureHash` for a member-less routine and its enclosing-member fold
//! for a member trigger, see `ids::to_stable_routine_id_from_parts`) — for ALL
//! routines, SOURCE and DEPENDENCY alike — and substitute every internal routine-id
//! occurrence in the `rootCauseKey` before hashing. Because operation/callsite/loop
//! ids carry the routine id as a prefix, replacing the routine-id substring preserves
//! the `/suffix`. The result is `modelInstanceId`- and cache-INDEPENDENT (edit-stable).
//!
//! Routines whose `normalized_signature_hash` is empty are skipped (no stable form to
//! swap to — `to_stable_routine_id_from_parts` would emit a degenerate trailing `#`).
//!
//! ⟨task 4⟩ **This is where the member discriminator on the STABLE id pays off.**
//! Task 3 gave two sibling `trigger OnAction()` bodies distinct INTERNAL ids, but
//! both still substituted to one stable id here, so their findings hashed to one
//! fingerprint and a single baseline entry suppressed both. Folding the enclosing
//! member into the stable id closes that (measured on DO: duplicate-fingerprint
//! excess 7 → 3, the residual 3 being `d55`'s own callsite-blind `rootCauseKey`,
//! not an id collision).
//! For the r0-pinned (r4 differential) path the engine pins `modelInstanceId = "r0"`,
//! so source ids are already reproducible; the substitution still applies and both
//! paths stabilize to the SAME `StableRoutineId`. This unifies the prior dep-only
//! special case into one all-routines map.

use crate::engine::ids::{is_lower_hex, sha256_hex};
use crate::engine::l5::finding::Finding;
use crate::program::model::workspace::{ModelObject, ModelRoutine};
use std::collections::HashMap;

/// Per-model id indexes for the fingerprint (routine-by-internal-id +
/// object-by-internal-id + all-routines stabilization map). Built once per run.
pub struct FingerprintIndex<'a> {
    routines_by_id: HashMap<&'a str, &'a ModelRoutine>,
    objects_by_id: HashMap<&'a str, &'a ModelObject>,
    /// EVERY routine's internal id → stable routine id (source AND dep). Used to
    /// substitute internal routine-id occurrences in the `rootCauseKey` before
    /// hashing, making the fingerprint edit-/cache-independent. EMPTY only for an
    /// empty model → `fingerprint_of` is then a no-op. Borrows the model's strings.
    stable_by_id: HashMap<&'a str, &'a str>,
    /// The distinct `"{modelInstanceId}/"` prefixes present in `stable_by_id`'s
    /// keys (each key up to and including its first `/`), longest-first. Internal
    /// RoutineIds have the fixed shape `"{modelInstanceId}/{64 lowercase hex}"`
    /// (`engine/ids.rs:192`), so `substitute_stable_ids` finds candidate id
    /// occurrences by scanning for these prefixes instead of trying every id at
    /// every byte — turning the per-finding O(F·(R log R + L·R)) scan into O(L).
    model_instance_prefixes: Vec<String>,
}

impl FingerprintIndex<'_> {
    /// The retained census's shallow walk (#160 §4.1.5).
    pub(crate) fn census_tally(&self) -> crate::engine::census::Tally {
        let mut t = crate::engine::census::Tally::default();
        t.buf::<(&str, &ModelRoutine)>(self.routines_by_id.len())
            .buf::<(&str, &ModelObject)>(self.objects_by_id.len())
            .buf::<(&str, &str)>(self.stable_by_id.len())
            .buf::<String>(self.model_instance_prefixes.len())
            .strs(&self.model_instance_prefixes);
        t
    }
}

impl<'a> FingerprintIndex<'a> {
    /// Build the fingerprint index. Maps EVERY routine's internal id to its stable
    /// id (source AND dependency) so that all internal routine-id occurrences in a
    /// `rootCauseKey` are stabilized before hashing. Routines whose
    /// `normalized_signature_hash` is empty are skipped (no stable form). This is
    /// the single, unified path — there is no longer a dep-only variant.
    pub fn build(routines: &'a [ModelRoutine], objects: &'a [ModelObject]) -> Self {
        let routines_by_id: HashMap<&str, &ModelRoutine> =
            routines.iter().map(|r| (r.id.as_str(), r)).collect();
        let objects_by_id: HashMap<&str, &ModelObject> =
            objects.iter().map(|o| (o.id.as_str(), o)).collect();

        // Map every routine's INTERNAL id → its modelInstanceId-/cache-independent
        // StableRoutineId. Skip routines with an empty normalized_signature_hash
        // (mirrors al-sem skipping `normalizedSignatureHash === ""`).
        // ⟨final-branch-review-l3.md M-7⟩ The "degenerate trailing `#`" rationale
        // is only half-true: it holds for a MEMBER-LESS routine (empty hash → an
        // empty six-part join, a real degenerate id with nothing to swap to), but
        // NOT for a member-trigger routine — `sha256_of_strings(["", member])`
        // still folds to 64 real hex, so there is nothing degenerate to avoid
        // there. Unreachable either way in production (assembly always computes a
        // non-empty `normalized_signature_hash`), so this skip is conservative
        // rather than wrong, and the comment above states only the case where the
        // rationale actually applies.
        let stable_by_id: HashMap<&str, &str> = routines
            .iter()
            .filter(|r| !r.normalized_signature_hash.is_empty())
            .map(|r| (r.id.as_str(), r.stable_routine_id.as_str()))
            .collect();

        // Distinct `"{modelInstanceId}/"` prefixes (key up to and including its
        // first `/`). Deduplicated via BTreeSet, then sorted longest-first so a
        // longer prefix shadows a shorter one that is its own prefix (mirrors the
        // old scan's longest-key-first shadowing). In a real run every id shares
        // one modelInstanceId so this is a single-element vec.
        let model_instance_prefixes: Vec<String> = {
            let mut v: Vec<String> = stable_by_id
                .keys()
                .filter_map(|k| k.find('/').map(|i| k[..=i].to_string()))
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            v.sort_by(|a, b| b.len().cmp(&a.len()).then(a.cmp(b)));
            v
        };

        FingerprintIndex {
            routines_by_id,
            objects_by_id,
            stable_by_id,
            model_instance_prefixes,
        }
    }

    /// The model routine with internal id `id`.
    pub fn routine(&self, id: &str) -> Option<&'a ModelRoutine> {
        self.routines_by_id.get(id).copied()
    }

    /// The model object with internal id `id`.
    pub fn object(&self, id: &str) -> Option<&'a ModelObject> {
        self.objects_by_id.get(id).copied()
    }

    /// Compute the finding's fingerprint. Internal routine ids in `rootCauseKey`
    /// (directly, or as the prefix of operation/callsite/loop ids) are replaced with
    /// their stable ids before hashing (mirrors al-sem's stabilizing substitution —
    /// see module docs). Returns the first 16 hex chars of `sha256(parts.join("|"))`.
    pub fn fingerprint_of(&self, finding: &Finding) -> String {
        let (routine, obj_part) = self.anchor_of(finding);
        let routine_name = routine.map(|r| r.name.clone()).unwrap_or_default();
        let key = self.stabilize(&finding.root_cause_key);
        hash_parts(finding, obj_part, routine_name, key)
    }

    /// #247: the routine-independent key. The fingerprint's parts with `routineName`
    /// blanked and, when the anchor is a routine, every occurrence of the anchor's
    /// internal or stable id in `rootCauseKey` (with the one `/op3`-style site-id
    /// segment minted on it) replaced by `~`. So the same problem moved to another routine of the
    /// same object keeps this key. Not unique: several findings can share one. When
    /// the anchor is not a routine (d64's object id, an unresolved id) it equals
    /// `fingerprint_of`.
    ///
    /// #252: when the detector states a [`LooseSubject`](crate::engine::l5::finding::LooseSubject),
    /// its key replaces the stripped `rootCauseKey` and is hashed as given (after
    /// `stabilize`, never stripped: it holds only what the detector chose, so an id
    /// in it is meant to be there), and its `object_id`, when set, replaces the
    /// anchor's object part. `None` is the #247 path above, unchanged.
    pub fn loose_fingerprint_of(&self, finding: &Finding) -> String {
        let (routine, anchor_obj) = self.anchor_of(finding);
        let (obj_part, key) = match (&finding.loose_subject, routine) {
            (Some(s), _) => (
                s.object_id
                    .as_deref()
                    .map_or(anchor_obj, |id| self.object_part(id)),
                s.key.clone(),
            ),
            (None, Some(r)) => (
                anchor_obj,
                strip_anchor(
                    &finding.root_cause_key,
                    &[r.id.as_str(), r.stable_routine_id.as_str()],
                ),
            ),
            (None, None) => (anchor_obj, finding.root_cause_key.clone()),
        };
        let key = self.stabilize(&key);
        hash_parts(finding, obj_part, String::new(), key)
    }

    /// The `objectType/objectKey` part of the object with internal id `id`, or `id`
    /// itself when the index does not hold it (internal object ids are
    /// `{appGuid}/{type}/{key}`, so still stable).
    fn object_part(&self, id: &str) -> String {
        self.objects_by_id
            .get(id)
            .map_or_else(|| id.to_string(), |o| object_type_key(o))
    }

    /// The anchor routine (`primaryLocation.enclosingRoutineId`, when it is one) and
    /// the `objectType/objectKey` part of its owning object (empty when unresolved).
    fn anchor_of(&self, finding: &Finding) -> (Option<&'a ModelRoutine>, String) {
        let enclosing = finding.primary_location.enclosing_routine_id.as_str();
        let routine = self.routines_by_id.get(enclosing).copied();
        // Object-level finding convention (d64 — the first detector to anchor a
        // finding directly on an object rather than a routine, e.g. a
        // declarative page with no routines at all): `enclosing_routine_id` IS
        // the object's own internal id when there is no routine to anchor on.
        // Fall back to a direct object lookup so `obj_part` still resolves;
        // `routine_name` correctly stays empty below (there is no routine).
        // Behavior-preserving for every routine-anchored finding: the `or_else`
        // only runs when the routine branch already missed.
        let obj = routine
            .and_then(|r| self.objects_by_id.get(r.object_id.as_str()))
            .or_else(|| self.objects_by_id.get(enclosing));

        let obj_part = obj.map_or_else(String::new, |o| object_type_key(o));
        (routine, obj_part)
    }

    /// Stabilize every internal routine id embedded in a rootCauseKey (mirrors
    /// al-sem's stabilizing reduce). For an empty model stable_by_id is empty, so
    /// the key is used verbatim.
    fn stabilize(&self, key: &str) -> String {
        if self.stable_by_id.is_empty() {
            key.to_string()
        } else {
            substitute_stable_ids(key, &self.stable_by_id, &self.model_instance_prefixes)
        }
    }
}

/// The object KEY part: the number for a numbered object (the same bytes as before
/// #83), `@name` for one without, so two numberless objects of one kind no longer
/// share an object part.
fn object_type_key(o: &ModelObject) -> String {
    format!("{}/{}", o.object_type, o.object_key)
}

/// `sha256(detector | obj_part | routine_name | affectedTables | key)[..16]`.
fn hash_parts(finding: &Finding, obj_part: String, routine_name: String, key: String) -> String {
    let parts = [
        finding.detector.clone(),
        obj_part,
        routine_name,
        finding.affected_tables.join(","),
        key,
    ];
    let joined = parts.join("|");
    sha256_hex(&joined)[..16].to_string()
}

/// The site-id tags minted on a routine id as `{routineId}/{tag}{n}`, ONE segment
/// deep: `op`/`cs`/`loop`/`u` in `program/body/ir_walk.rs` (each minting site points
/// back here). A detector's own `/p{N}` (d19, d32: a parameter index) is not one of
/// them: a parameter belongs to its routine, so it stays in the loose key.
const ROUTINE_SITE_TAGS: [&str; 4] = ["op", "cs", "loop", "u"];

/// Replace every occurrence of any of `ids` in `key`, plus the ONE site-id segment
/// minted on it (`/op3`, `/cs1`, `/loop0`, `/u2`) when present, with `~`.
fn strip_anchor(key: &str, ids: &[&str]) -> String {
    let mut out = String::with_capacity(key.len());
    let mut pos = 0usize;
    'outer: while pos < key.len() {
        for id in ids.iter().filter(|id| !id.is_empty()) {
            if key[pos..].starts_with(id) {
                out.push('~');
                pos += id.len();
                pos += site_segment_len(&key[pos..]).unwrap_or(0);
                continue 'outer;
            }
        }
        let ch = key[pos..].chars().next().expect("non-empty slice");
        out.push(ch);
        pos += ch.len_utf8();
    }
    out
}

/// The byte length of a leading `/{tag}<digits>` site-id segment of `rest`. The
/// digits must end the key or be followed by a key separator (`/`, or `|` as in
/// d43's `{caller}/cs1|…`), so a variable named `p1_old` or `op2x` is not one.
fn site_segment_len(rest: &str) -> Option<usize> {
    let after_slash = rest.strip_prefix('/')?;
    ROUTINE_SITE_TAGS.iter().find_map(|tag| {
        let after = after_slash.strip_prefix(tag)?;
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        let ends = matches!(after.as_bytes().get(digits), None | Some(b'/' | b'|'));
        (digits > 0 && ends).then_some(1 + tag.len() + digits)
    })
}

/// Replace every stable_by_id KEY occurring in `key` with its value, scanning
/// structurally instead of trying every id at every byte. Ids have the fixed
/// shape "{modelInstanceId}/{64 lowercase hex}" (engine/ids.rs:192), so we find
/// candidate occurrences by scanning for each known "{mid}/" prefix and
/// checking the following 64 bytes for lowercase-hex; the candidate substring
/// is then a single HashMap probe. A candidate that misses the map is copied
/// verbatim (identical to the old scan's behavior for empty-hash routines).
///
/// This is byte-for-byte equivalent to the original try-every-id scan for every
/// id the engine emits (all ids share one modelInstanceId, so all keys have the
/// same length and at most one can match at any position) — pinned by the
/// `structural_substitution_matches_scan_oracle` equivalence test.
fn substitute_stable_ids(
    key: &str,
    stable_by_id: &HashMap<&str, &str>,
    prefixes: &[String],
) -> String {
    let bytes = key.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len);
    let mut pos = 0usize;
    'outer: while pos < len {
        for p in prefixes {
            let plen = p.len();
            // Bounds check (`pos + plen + 64 <= len`) short-circuits BEFORE the hex
            // slice, so the slice below never panics. Only lowercase a-f is hex here
            // (real ids are lowercase — `is_ascii_hexdigit` would wrongly accept A-F).
            if key[pos..].starts_with(p.as_str())
                && pos + plen + 64 <= len
                && bytes[pos + plen..pos + plen + 64]
                    .iter()
                    .all(|b| is_lower_hex(*b))
            {
                let candidate = &key[pos..pos + plen + 64];
                if let Some(stable) = stable_by_id.get(candidate) {
                    out.push_str(stable);
                    pos += plen + 64;
                    continue 'outer;
                }
            }
        }
        // No id starts here — copy one Unicode scalar value (char-boundary-safe;
        // ids are ASCII, so this is 1 byte in practice, but rootCauseKeys may
        // carry non-ASCII object names).
        let ch = key[pos..]
            .chars()
            .next()
            .expect("valid UTF-8 non-empty slice");
        out.push(ch);
        pos += ch.len_utf8();
    }
    out
}

// ---------------------------------------------------------------------------
// Native oracles — #[cfg(test)]
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::ids::sha256_hex;
    use crate::engine::l5::finding::{Evidence, FindingConfidence, LooseSubject, SourceAnchor};
    use crate::program::model::workspace::{ModelRoutine, RoutineVariables};

    fn dummy_anchor(enclosing: &str) -> SourceAnchor {
        SourceAnchor {
            source_unit_id: "ws:test.al".to_string(),
            start_line: 1,
            start_column: 0,
            end_line: 1,
            end_column: 10,
            enclosing_routine_id: enclosing.to_string(),
            syntax_kind: "procedure".to_string(),
            normalized_text_hash: None,
            leading_context_hash: None,
            trailing_context_hash: None,
        }
    }

    fn dummy_finding(detector: &str, root_cause_key: &str, enclosing: &str) -> Finding {
        Finding {
            id: root_cause_key.to_string(),
            root_cause_key: root_cause_key.to_string(),
            detector: detector.to_string(),
            title: "Test finding".into(),
            root_cause: "root cause".to_string(),
            severity: "info".to_string(),
            confidence: FindingConfidence {
                level: "possible".to_string(),
                capped_by: None,
                evidence: Vec::new(),
            },
            primary_location: dummy_anchor(enclosing),
            evidence_path: Vec::new(),
            additional_paths: None,
            affected_objects: Vec::new(),
            affected_tables: Vec::new(),
            fix_options: Vec::new(),
            provenance: vec![Evidence {
                source: "tree-sitter",
                note: None,
            }],
            actionable_anchor: None,
            fingerprint: None,
            event_kind: None,
            cross_extension_subscribers: None,
            cohort_contexts: None,
            loose_subject: None,
        }
    }

    fn minimal_routine(id: &str, object_id: &str, name: &str) -> ModelRoutine {
        ModelRoutine {
            id: id.to_string(),
            stable_routine_id: format!("stable::{id}"),
            object_id: object_id.to_string(),
            object_type: "Codeunit".to_string(),
            name: name.to_string(),
            kind: "procedure".to_string(),
            attributes_parsed: Vec::new(),
            app_guid: "app".to_string(),
            object_number: 1,
            // Non-empty so the routine is included in the all-routines stable map
            // (routines with an empty hash are skipped — no stable form).
            normalized_signature_hash: "sig".to_string(),
            body_available: true,
            parse_incomplete: false,
            record_variables: Vec::new(),
            record_operations: Vec::new(),
            field_accesses: Vec::new(),
            variables: RoutineVariables::default(),
            parameters: Vec::new(),
            access_modifier: None,
            return_type: None,
            call_sites: Vec::new(),
            operation_sites: Vec::new(),
            statement_tree: None,
            loops: Vec::new(),
            file: None,
            site_exprs: Default::default(),
            source_anchor: crate::program::body::features::PAnchor {
                source_unit_id: "ws:test.al".to_string(),
                start_line: 0,
                start_column: 0,
                end_line: 0,
                end_column: 0,
                syntax_kind: "procedure".to_string(),
            },
            declaration_start_line: 0,
            identifier_references: Vec::new(),
            unreachable_statements: Vec::new(),
            has_branching: false,
            var_assignments: Vec::new(),
            condition_references: Vec::new(),
            enclosing_member: None,
            originating_object: None,
            enclosing_member_range: None,
            entry_temp_guard_receiver: None,
        }
    }

    fn minimal_object(id: &str, object_type: &str, object_number: i64) -> ModelObject {
        ModelObject {
            id: id.to_string(),
            app_guid: "app".to_string(),
            object_type: object_type.to_string(),
            object_number,
            object_key: object_number.to_string(),
            name: format!("Obj{object_number}"),
            source_table_name: None,
            extends_target_name: None,
            implements_interfaces: Some(Vec::new()),
            object_subtype: None,
            page_type: None,
            inherent_commit_behavior: None,
            source_table_temporary: None,
            page_controls: Vec::new(),
            single_instance: None,
            editable: None,
            insert_allowed: None,
            modify_allowed: None,
            delete_allowed: None,
            source_anchor: None,
            declaration_only_routines: Vec::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Oracle 1 — a rootCauseKey that embeds NO routine id is hashed verbatim
    // (the only routine's id is not a substring of the key → no substitution).
    // -----------------------------------------------------------------------
    #[test]
    fn fingerprint_with_no_embedded_routine_id_is_sha256_of_parts() {
        let r = minimal_routine("r1", "obj/Codeunit/50100", "MyProc");
        let o = minimal_object("obj/Codeunit/50100", "Codeunit", 50100);
        let routines = vec![r];
        let objects = vec![o];
        let idx = FingerprintIndex::build(&routines, &objects);

        let finding = dummy_finding("d17-min-version-drift", "d17/some-guid", "r1");
        let fp = idx.fingerprint_of(&finding);

        // Verify it is 16 hex chars.
        assert_eq!(fp.len(), 16, "fingerprint must be 16 hex chars: {fp}");
        assert!(
            fp.chars().all(|c| c.is_ascii_hexdigit()),
            "must be hex: {fp}"
        );

        // Reproducibility: calling again yields the same value.
        let fp2 = idx.fingerprint_of(&finding);
        assert_eq!(fp, fp2, "fingerprint must be deterministic");

        // "r1" is not a substring of "d17/some-guid", so the key is verbatim:
        // sha256("d17-min-version-drift|Codeunit/50100|MyProc||d17/some-guid")
        let expected_parts = "d17-min-version-drift|Codeunit/50100|MyProc||d17/some-guid";
        let expected = &sha256_hex(expected_parts)[..16];
        assert_eq!(
            fp, expected,
            "fingerprint must match manual sha256 of parts"
        );
    }

    // #83: two numberless objects of one kind (object_number 0) must not share a
    // fingerprint object part. The precondition is hand-stated: same kind, same
    // number, different object_key.
    #[test]
    fn numberless_objects_of_one_kind_fingerprint_differently() {
        let fp_of = |key: &str| {
            let obj_id = format!("app/Interface/{key}");
            let r = minimal_routine("r1", &obj_id, "Proc");
            let mut o = minimal_object(&obj_id, "Interface", 0);
            o.object_key = key.to_string();
            let (routines, objects) = (vec![r], vec![o]);
            let idx = FingerprintIndex::build(&routines, &objects);
            idx.fingerprint_of(&dummy_finding("d17-x", "d17/key", "r1"))
        };
        assert_ne!(fp_of("@ifoo"), fp_of("@ibar"));
    }

    // -----------------------------------------------------------------------
    // Oracle 2 — a DEP routine id embedded in rootCauseKey is replaced with its
    // stable id before hashing (the cross-app case, now via the unified build).
    // -----------------------------------------------------------------------
    #[test]
    fn dep_id_in_root_cause_key_is_substituted_with_stable_id() {
        // Internal RoutineIds have the fixed shape "{modelInstanceId}/{64 hex}"
        // (engine/ids.rs:192) — the structural substitution keys on exactly that
        // shape, so the fixture id must be real-shaped (a bare "r0/dep:..." would
        // never be a real key and so would never be substituted, old scan or new).
        let internal_dep_id = format!("r0/{}", "0123456789abcdef".repeat(4));
        let stable_dep_id =
            "app:Codeunit:99#aabbccdd0011223344556677889900aabbccdd0011223344556677889900aabb";

        let mut r_dep = minimal_routine(&internal_dep_id, "dep/Codeunit/99", "DepProc");
        r_dep.stable_routine_id = stable_dep_id.to_string();
        let o = minimal_object("dep/Codeunit/99", "Codeunit", 99);

        let mut r_ws = minimal_routine("ws_routine", "ws/Codeunit/50100", "WSProc");
        r_ws.stable_routine_id = "ws_stable".to_string();
        let o_ws = minimal_object("ws/Codeunit/50100", "Codeunit", 50100);

        let routines2 = vec![r_dep.clone(), r_ws.clone()];
        let objects2 = vec![o.clone(), o_ws.clone()];
        let idx = FingerprintIndex::build(&routines2, &objects2);

        // rootCauseKey embeds the internal dep id (as d16 does)
        let root_cause_key = format!("d16/{internal_dep_id}");
        let finding = dummy_finding("d16-obsolete-routine-call", &root_cause_key, "ws_routine");
        let fp = idx.fingerprint_of(&finding);

        // Build the expected fingerprint with the dep id substituted.
        let stable_key = format!("d16/{stable_dep_id}");
        let expected_parts =
            format!("d16-obsolete-routine-call|Codeunit/50100|WSProc||{stable_key}");
        let expected = &sha256_hex(&expected_parts)[..16];
        assert_eq!(
            fp, expected,
            "dep id must be substituted with stable id in fingerprint"
        );
    }

    // -----------------------------------------------------------------------
    // Oracle 3 — a SOURCE routine id embedded in rootCauseKey is ALSO replaced
    // with its stable id (the edit-stable fix: ALL routine ids stabilize).
    // -----------------------------------------------------------------------
    #[test]
    fn source_id_in_root_cause_key_is_substituted_with_stable_id() {
        // Real-shaped ids ("{modelInstanceId}/{64 hex}", engine/ids.rs:192) — the
        // structural substitution keys on that shape (see oracle 2's note).
        let source_id = format!("r0/{}", "fedcba9876543210".repeat(4));
        let source_stable = "app:Codeunit:1#ws_stable_hash";
        let dep_id = format!("r0/{}", "0011223344556677".repeat(4));

        let mut r_dep = minimal_routine(&dep_id, "dep/Codeunit/99", "DepProc");
        r_dep.stable_routine_id = "dep_stable_id".to_string();

        let mut r_ws = minimal_routine(&source_id, "ws/Codeunit/1", "WsProc");
        r_ws.stable_routine_id = source_stable.to_string();
        let o_ws = minimal_object("ws/Codeunit/1", "Codeunit", 1);
        let o_dep = minimal_object("dep/Codeunit/99", "Codeunit", 99);

        let routines3 = vec![r_dep.clone(), r_ws.clone()];
        let objects3 = vec![o_dep.clone(), o_ws.clone()];
        let idx = FingerprintIndex::build(&routines3, &objects3);

        // rootCauseKey embeds a SOURCE id → now stabilized (edit-survival fix).
        let root_cause_key = format!("d16/{source_id}");
        let finding = dummy_finding("d16-obsolete-routine-call", &root_cause_key, &source_id);
        let fp = idx.fingerprint_of(&finding);

        // The source id is replaced with its stable form before hashing.
        let stable_key = format!("d16/{source_stable}");
        let expected_parts = format!("d16-obsolete-routine-call|Codeunit/1|WsProc||{stable_key}");
        let expected = &sha256_hex(&expected_parts)[..16];
        assert_eq!(
            fp, expected,
            "source id must be substituted with stable id (edit-stable fix)"
        );
    }

    // -----------------------------------------------------------------------
    // Oracle 4 — routines with an empty normalized_signature_hash are skipped
    // (no stable form): their internal id stays verbatim in the rootCauseKey.
    // -----------------------------------------------------------------------
    #[test]
    fn routine_with_empty_sig_hash_is_not_substituted() {
        let id = "r0/ws:src/x.al/h";
        let mut r = minimal_routine(id, "ws/Codeunit/1", "WsProc");
        r.normalized_signature_hash = String::new(); // skipped
        r.stable_routine_id = "app:Codeunit:1#".to_string();
        let o = minimal_object("ws/Codeunit/1", "Codeunit", 1);

        let routines = vec![r];
        let objects = vec![o];
        let idx = FingerprintIndex::build(&routines, &objects);

        let root_cause_key = format!("d16/{id}");
        let finding = dummy_finding("d16-obsolete-routine-call", &root_cause_key, id);
        let fp = idx.fingerprint_of(&finding);

        // No stable form → key used verbatim.
        let expected_parts = format!("d16-obsolete-routine-call|Codeunit/1|WsProc||d16/{id}");
        let expected = &sha256_hex(&expected_parts)[..16];
        assert_eq!(
            fp, expected,
            "routine with empty sig hash must not be substituted"
        );
    }

    // -----------------------------------------------------------------------
    // Equivalence oracle — the structural `substitute_stable_ids` must produce
    // byte-identical output to the ORIGINAL try-every-id-at-every-byte scan for
    // every id shape the real engine emits. `oracle_substitute` is a verbatim
    // copy of the pre-change scan (fingerprint.rs :118-152, the old `else`
    // branch body) so this test pins the new fast path to the old slow one.
    // -----------------------------------------------------------------------

    /// The ORIGINAL substitution scan (pre-change), extracted verbatim as a
    /// free oracle: sort ids longest-first, then at each byte try every id via
    /// `starts_with`. Kept ONLY as the equivalence reference for the test below.
    fn oracle_substitute(key: &str, stable_by_id: &HashMap<String, String>) -> String {
        let mut sorted_entries: Vec<(&String, &String)> = stable_by_id.iter().collect();
        sorted_entries.sort_by(|a, b| {
            b.0.len()
                .cmp(&a.0.len())
                .then_with(|| a.0.as_str().cmp(b.0.as_str()))
        });

        let len = key.len(); // byte length
        let mut out = String::with_capacity(len);
        let mut pos = 0usize;
        'outer: while pos < len {
            for (k, v) in &sorted_entries {
                if key[pos..].starts_with(k.as_str()) {
                    out.push_str(v.as_str());
                    pos += k.len();
                    continue 'outer;
                }
            }
            let ch = key[pos..]
                .chars()
                .next()
                .expect("valid UTF-8 non-empty slice");
            out.push(ch);
            pos += ch.len_utf8();
        }
        out
    }

    #[test]
    fn structural_substitution_matches_scan_oracle() {
        // Build a stable_by_id with ids of the REAL shape: "{mid}/{64-hex}".
        let mid = "r0"; // also test a 16-char mid in a second map
        let mk = |h: &str| format!("{mid}/{}", h.repeat(64)); // "{mid}/{64 hex}"
        let id_a = mk("a");
        let id_b = mk("b");
        let id_absent = mk("c"); // id-shaped but NOT in the map (empty-hash routine)
        let mut map: HashMap<String, String> = HashMap::new();
        map.insert(id_a.clone(), "STABLE_A".to_string());
        map.insert(id_b.clone(), "STABLE_B".to_string());

        let keys = [
            format!("op/{id_a}/cs1"),                // embedded with suffix
            format!("{id_a}|{id_b}"),                // two ids
            format!("{id_absent}/op3"),              // id-shaped, absent -> verbatim
            format!("prefix {id_a}{id_b} adjacent"), // adjacent ids
            "no ids at all".to_string(),
            format!("{mid}/short-not-hex"), // prefix but not 64-hex
            id_a.clone(),                   // exact
        ];
        let bmap: HashMap<&str, &str> = map.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        for k in &keys {
            assert_eq!(
                substitute_stable_ids(k, &bmap, &["r0/".to_string()]),
                oracle_substitute(k, &map),
                "divergence on key: {k}"
            );
        }

        // Second map with a REAL 16-hex modelInstanceId (production shape) to
        // exercise a longer prefix than "r0/".
        let mid16 = "a764b56fa105f014";
        let mk16 = |h: &str| format!("{mid16}/{}", h.repeat(64));
        let id_p = mk16("d");
        let id_q = mk16("e");
        let mut map16: HashMap<String, String> = HashMap::new();
        map16.insert(id_p.clone(), "STABLE_P".to_string());
        map16.insert(id_q.clone(), "STABLE_Q".to_string());
        let prefixes16 = [format!("{mid16}/")];
        let keys16 = [
            format!("d19/{id_p}/loop2"),
            format!("{id_p}{id_q}"),
            format!("{mid16}/short"),
            "plain".to_string(),
        ];
        let bmap16: HashMap<&str, &str> = map16
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        for k in &keys16 {
            assert_eq!(
                substitute_stable_ids(k, &bmap16, &prefixes16),
                oracle_substitute(k, &map16),
                "divergence (16-hex mid) on key: {k}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // #247 looseFingerprint
    // -----------------------------------------------------------------------

    /// Two routines `rA`/`rB` of one codeunit, with stable ids `S#A`/`S#B` that do
    /// NOT contain the internal ids (else stripping the internal id alone would also
    /// erase the stable form and hide a missing stable-id strip).
    fn two_routine_model() -> (Vec<ModelRoutine>, Vec<ModelObject>) {
        let obj = "obj/Codeunit/50100";
        let mut a = minimal_routine("rA", obj, "First");
        let mut b = minimal_routine("rB", obj, "Second");
        a.stable_routine_id = "S#A".to_string();
        b.stable_routine_id = "S#B".to_string();
        (vec![a, b], vec![minimal_object(obj, "Codeunit", 50100)])
    }

    /// T3: the detector part. Equal key, tables and anchor; only the detector
    /// differs. On the production path the key's `dNN/` prefix already differs, so
    /// only a hand-made pair can show the part is hashed. Break: drop `detector`
    /// from the loose parts.
    #[test]
    fn loose_fingerprint_hashes_the_detector() {
        let (routines, objects) = two_routine_model();
        let idx = FingerprintIndex::build(&routines, &objects);
        let a = dummy_finding("d11-modify-without-get", "x/rA/op0", "rA");
        let b = dummy_finding("d21-other", "x/rA/op0", "rA");
        assert_ne!(idx.loose_fingerprint_of(&a), idx.loose_fingerprint_of(&b));
    }

    /// The anchor id and the ONE site-id segment minted on it go; the rest of the
    /// key stays. Keys are the detector `rootCauseKey` layouts (`rA` = the anchor's
    /// internal id, `S#A` its stable id). Since #252 this is only the fallback path
    /// (`loose_subject: None`); a detector that states a subject no longer reaches it.
    #[test]
    fn strip_anchor_shapes() {
        let ids = ["rA", "S#A"];
        // d1/d10/d11: `{rid}/{opId}`.
        assert_eq!(strip_anchor("d11/rA/rA/op12", &ids), "d11/~/~");
        // d18: `{rid}/{loopId}/{recVar}/{field}`.
        assert_eq!(
            strip_anchor("d18/rA/rA/loop0/rec/field", &ids),
            "d18/~/~/rec/field"
        );
        // d60's key layout `{rid}/{loopId}/{var}` — a variable named like a site id
        // after the loop segment stays (#247 CR1).
        assert_eq!(strip_anchor("d60/rA/rA/loop0/p1", &ids), "d60/~/~/p1");
        assert_eq!(strip_anchor("d60/rA/rA/loop0/op2", &ids), "d60/~/~/op2");
        // A segment must end at `/`, `|` or the key's end.
        assert_eq!(strip_anchor("d57/rA/p1_old", &ids), "d57/~/p1_old");
        assert_eq!(strip_anchor("d57/rA/op1_old", &ids), "d57/~/op1_old");
        assert_eq!(strip_anchor("d57/rA/op2x", &ids), "d57/~/op2x");
        // d19: the parameter position `/p{N}` is kept (#247 CR2).
        assert_eq!(strip_anchor("d19/rA/p2", &ids), "d19/~/p2");
        // d43: `|`-separated, the anchor's call site ends at `|`.
        assert_eq!(strip_anchor("d43/ev|rA/cs1|rB|t", &ids), "d43/ev|~|rB|t");
        // d16: the callee id is the subject and stays.
        assert_eq!(strip_anchor("d16/rA/rA/cs1/rB", &ids), "d16/~/~/rB");
        // d47: the anchor's stable id.
        assert_eq!(strip_anchor("d47/S#A", &ids), "d47/~");
        // No anchor in the key: unchanged.
        assert_eq!(strip_anchor("d2/event", &ids), "d2/event");
    }

    /// #247 CR1/CR8: on the fallback path (`loose_subject: None`), two findings with
    /// d60's key layout on variables `p1` and `p2` keep different loose keys. Since
    /// #252 d60 states its own subject; this pins the strip, not d60.
    #[test]
    fn loose_fingerprint_keeps_the_variable_after_the_site_id() {
        let (routines, objects) = two_routine_model();
        let idx = FingerprintIndex::build(&routines, &objects);
        let a = dummy_finding("d60", "d60/rA/rA/loop0/p1", "rA");
        let b = dummy_finding("d60", "d60/rA/rA/loop0/p2", "rA");
        assert_ne!(idx.loose_fingerprint_of(&a), idx.loose_fingerprint_of(&b));
    }

    /// On the fallback path (`loose_subject: None`): moving a finding between routines
    /// of one object keeps the loose key (internal and stable id forms) and changes
    /// the fingerprint; a callee id is kept.
    #[test]
    fn loose_fingerprint_ignores_the_anchor_routine() {
        let (routines, objects) = two_routine_model();
        let idx = FingerprintIndex::build(&routines, &objects);
        let pairs = [
            ("d1/rA/rA/op3", "rA", "d1/rB/rB/op7", "rB"),
            ("d18/rA/rA/loop0/r/f", "rA", "d18/rB/rB/loop2/r/f", "rB"),
            ("d47/S#A", "rA", "d47/S#B", "rB"),
        ];
        for (ka, aa, kb, ab) in pairs {
            let a = dummy_finding("dX", ka, aa);
            let b = dummy_finding("dX", kb, ab);
            assert_ne!(idx.fingerprint_of(&a), idx.fingerprint_of(&b), "{ka}");
            assert_eq!(
                idx.loose_fingerprint_of(&a),
                idx.loose_fingerprint_of(&b),
                "{ka}"
            );
        }
        // d16: the same caller anchor, different callees → different loose keys.
        let a = dummy_finding("d16", "d16/rA/rA/cs1/rB", "rA");
        let b = dummy_finding("d16", "d16/rA/rA/cs1/rA", "rA");
        assert_ne!(idx.loose_fingerprint_of(&a), idx.loose_fingerprint_of(&b));
    }

    // -----------------------------------------------------------------------
    // #252 typed loose subjects
    // -----------------------------------------------------------------------

    fn with_subject(mut f: Finding, object_id: Option<&str>, key: &str) -> Finding {
        f.loose_subject = Some(LooseSubject {
            object_id: object_id.map(str::to_string),
            key: key.to_string(),
        });
        f
    }

    /// U1: a subject key is hashed verbatim, never stripped. A key holding the
    /// anchor's own id (the d16 self-call shape) keeps it, so it differs from the
    /// key the strip would have made of it. Break: strip the subject key.
    #[test]
    fn loose_subject_key_is_not_stripped() {
        let (routines, objects) = two_routine_model();
        let idx = FingerprintIndex::build(&routines, &objects);
        let own = with_subject(dummy_finding("d16", "k", "rA"), None, "d16/rA");
        let tilde = with_subject(dummy_finding("d16", "k", "rA"), None, "d16/~");
        assert_ne!(
            idx.loose_fingerprint_of(&own),
            idx.loose_fingerprint_of(&tilde)
        );
    }

    /// U4: a subject `object_id` the index does not hold is hashed verbatim, not
    /// dropped, so two unknown objects keep different loose keys. Break: map an
    /// unknown id to `""`.
    #[test]
    fn loose_subject_unknown_object_is_hashed_verbatim() {
        let (routines, objects) = two_routine_model();
        let idx = FingerprintIndex::build(&routines, &objects);
        let a = with_subject(dummy_finding("d7", "k", "rA"), Some("x/A"), "d7/e");
        let b = with_subject(dummy_finding("d7", "k", "rA"), Some("x/B"), "d7/e");
        assert_ne!(idx.loose_fingerprint_of(&a), idx.loose_fingerprint_of(&b));
        // The ordinary fingerprint never reads the subject.
        assert_eq!(idx.fingerprint_of(&a), idx.fingerprint_of(&b));
    }
}
