//! Deterministic routine-selector resolution, shared by the DIGEST CLI's
//! changed-roots matching (`digest_cli::resolve_changed_roots`) and the
//! FINGERPRINT QUERY's selector resolution (`fingerprint_query::fingerprint_query`).
//!
//! Ports `resolveSelector` (fingerprint-query.ts) + `normalizeDisplayKey` /
//! `displayToStableIds` (indexes.ts).
//!
//! This used to be two independently-maintained copies. The `fingerprint_query`
//! copy rebuilt its bucket index by iterating a `HashMap<String, String>`
//! (`routine_display_by_id`) instead of the identity table's source `Vec` —
//! both the bucket ORDER and the per-bucket id order were process-random, so
//! `SelectorAmbiguous.candidates` (rendered verbatim, truncated at
//! `MAX_AMBIGUOUS_CANDIDATES`) reordered run-to-run and the displayed SET could
//! change past the truncation point. `digest_cli`'s copy was already correct
//! (built from `snap.identities.stable_ids`, a deterministic `Vec`). Both call
//! sites now share this ONE implementation so they cannot drift apart again.

use std::collections::HashMap;

use crate::engine::l5::snapshot::CapabilitySnapshot;

const ROUTINE_ID_SEPARATOR: char = '#';

/// `normalizeDisplayKey` (indexes.ts) — lowercase, trim, collapse internal whitespace.
pub(crate) fn normalize_display_key(s: &str) -> String {
    let trimmed = s.trim().to_lowercase();
    // Collapse runs of ASCII/Unicode whitespace into a single space (JS \s+).
    let mut out = String::with_capacity(trimmed.len());
    let mut prev_ws = false;
    for c in trimmed.chars() {
        if c.is_whitespace() {
            if !prev_ws {
                out.push(' ');
            }
            prev_ws = true;
        } else {
            out.push(c);
            prev_ws = false;
        }
    }
    out
}

/// Strip a leading type-word + whitespace prefix (`/^\w+\s+/`), returning None when
/// the line doesn't match (mirrors `display.replace(typeWordPrefix, "")` checked via
/// `stripped !== display`).
fn strip_type_word_prefix(display: &str) -> Option<&str> {
    let bytes = display.as_bytes();
    let mut i = 0usize;
    // \w+ : [A-Za-z0-9_]
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
        i += 1;
    }
    if i == 0 {
        return None;
    }
    let word_end = i;
    // \s+
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i == word_end {
        return None; // no whitespace after the word → no match
    }
    Some(&display[i..])
}

/// Ordered selector indexes: preserves the identity-table insertion order so the
/// two-segment / one-segment loops iterate exactly like the TS Map, and so
/// ambiguous-candidate order is deterministic. Built once per query from the
/// identity table's source `Vec` — never from a `HashMap` iteration.
pub(crate) struct SelectorIndexes {
    /// stableId → display (routine-only).
    pub(crate) routine_display_by_id: HashMap<String, String>,
    /// normalizeDisplayKey(display) → [stableId...], buckets in insertion order;
    /// keys also in first-insertion order (Vec of (key, ids)).
    display_to_stable_ids: Vec<(String, Vec<String>)>,
}

/// `primary_app_guid`: keep only that app's routines (a stable id starts with
/// `<appGuid>:`). A cross-app snapshot (`alsem digest`, #66) also holds dependency
/// routines; without this a bare `OnRun` would be ambiguous with every
/// dependency's. `None` keeps every routine.
pub(crate) fn build_selector_indexes(
    snap: &CapabilitySnapshot,
    primary_app_guid: Option<&str>,
) -> SelectorIndexes {
    let is_primary = |id: &str| {
        crate::program::model::role::role_of_stable_id(primary_app_guid, id)
            == crate::program::model::role::Role::Primary
    };
    let mut routine_display_by_id: HashMap<String, String> = HashMap::new();
    let mut display_to_stable_ids: Vec<(String, Vec<String>)> = Vec::new();
    let mut key_pos: HashMap<String, usize> = HashMap::new();

    for i in 0..snap.identities.stable_ids.len() {
        let id = snap
            .identities
            .stable_ids
            .get(i)
            .cloned()
            .unwrap_or_default();
        let display = snap
            .identities
            .display_names
            .get(i)
            .cloned()
            .unwrap_or_default();
        if id.is_empty() {
            continue;
        }
        if id.contains(ROUTINE_ID_SEPARATOR) && is_primary(&id) {
            routine_display_by_id.insert(id.clone(), display.clone());
            let key = normalize_display_key(&display);
            if let Some(&pos) = key_pos.get(&key) {
                display_to_stable_ids[pos].1.push(id);
            } else {
                key_pos.insert(key.clone(), display_to_stable_ids.len());
                display_to_stable_ids.push((key, vec![id]));
            }
        }
    }

    SelectorIndexes {
        routine_display_by_id,
        display_to_stable_ids,
    }
}

fn display_to_ids<'a>(idx: &'a SelectorIndexes, key: &str) -> Option<&'a Vec<String>> {
    idx.display_to_stable_ids
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
}

/// `resolveSelector` (fingerprint-query.ts) — the 5-form cascade. Returns the
/// matched stable IDs in deterministic order, plus the name of the form that
/// matched (`""` when unresolved) for diagnostics that report it.
pub(crate) fn resolve_selector(
    selector: &str,
    idx: &SelectorIndexes,
) -> (Vec<String>, &'static str) {
    // Form 1: exact StableRoutineId (case-sensitive).
    if idx.routine_display_by_id.contains_key(selector) {
        return (vec![selector.to_string()], "stable-routine-id");
    }

    // Form 2: full display name (normalized).
    let key = normalize_display_key(selector);
    if let Some(full) = display_to_ids(idx, &key)
        && !full.is_empty()
    {
        return (full.clone(), "full-display");
    }

    // Form 3: two-segment — strip leading type-word from the (already-normalized)
    // bucket KEY, compare to `key`. Matches TS, which iterates over the map keys.
    let mut two: Vec<String> = Vec::new();
    for (bucket_key, ids) in idx.display_to_stable_ids.iter() {
        if let Some(stripped) = strip_type_word_prefix(bucket_key)
            && stripped == key
        {
            two.extend(ids.iter().cloned());
        }
    }
    if !two.is_empty() {
        return (two, "two-segment");
    }

    // Form 4: one-segment — routine name after the last "::" in the bucket KEY.
    let mut one: Vec<String> = Vec::new();
    for (bucket_key, ids) in idx.display_to_stable_ids.iter() {
        let last = match bucket_key.rfind("::") {
            Some(sep) => &bucket_key[sep + 2..],
            None => bucket_key.as_str(),
        };
        if normalize_display_key(last) == key {
            one.extend(ids.iter().cloned());
        }
    }
    if !one.is_empty() {
        return (one, "one-segment");
    }

    // Form 5: object-qualified — routine segment after the LAST "::".
    if let Some(sep) = selector.rfind("::") {
        let routine_key = normalize_display_key(&selector[sep + 2..]);
        if let Some(qualified) = display_to_ids(idx, &routine_key)
            && !qualified.is_empty()
        {
            return (qualified.clone(), "object-qualified");
        }
    }

    (Vec::new(), "")
}

/// #78/#239: an object-qualified selector (`<owner>::<routine>`, as the human
/// headers print it) keeps only the matches whose owner (`owner_of`) equals its
/// object part, compared as display keys with `"` removed (so the AL spelling
/// `Codeunit "X"` works). The full owner (`Codeunit X`) wins; only when no
/// match has it does the bare object name (`X`) count. When neither fits, no
/// match is kept, so the caller reports the selector as unmatched.
pub(crate) fn narrow_to_owner(
    selector: &str,
    matched_form: &str,
    matches: Vec<String>,
    owner_of: impl Fn(&str) -> String,
) -> Vec<String> {
    let Some(sep) = selector.rfind("::") else {
        return matches;
    };
    if matched_form != "object-qualified" {
        return matches;
    }
    let want = normalize_display_key(&selector[..sep].replace('"', ""));
    let keyed: Vec<(String, String)> = matches
        .into_iter()
        .map(|id| (normalize_display_key(&owner_of(&id)), id))
        .collect();
    let full: Vec<String> = keyed
        .iter()
        .filter(|(key, _)| *key == want)
        .map(|(_, id)| id.clone())
        .collect();
    if !full.is_empty() {
        return full;
    }
    keyed
        .into_iter()
        .filter(|(key, _)| strip_type_word_prefix(key) == Some(want.as_str()))
        .map(|(_, id)| id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #78/#239: the owner narrows only an object-qualified match: full owner
    /// first, then bare name, quotes removed, and nothing when no owner fits.
    ///
    /// Mutations: restore the fallback -> the `[]` assert reads `[a, b]`;
    /// one-pass OR -> the precedence assert reads `[a, c]`; drop the `"`
    /// removal -> the quote assert reads `[]`.
    #[test]
    fn narrow_to_owner_keeps_the_named_owner_only() {
        let v = |xs: &[&str]| xs.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let owner = |id: &str| match id {
            "a" => "Codeunit Sel A".to_string(),
            "b" => "Codeunit Sel B".to_string(),
            _ => "Table Codeunit Sel A".to_string(),
        };
        let q = "object-qualified";
        let ab = || v(&["a", "b"]);
        assert_eq!(
            narrow_to_owner("codeunit  SEL a::Persist", q, ab(), owner),
            v(&["a"])
        );
        assert_eq!(narrow_to_owner("Sel A::Persist", q, ab(), owner), v(&["a"]));
        assert_eq!(
            narrow_to_owner("Codeunit \"Sel A\"::Persist", q, ab(), owner),
            v(&["a"])
        );
        assert_eq!(
            narrow_to_owner("Codeunit Sel A::Persist", q, v(&["a", "b", "c"]), owner),
            v(&["a"])
        );
        assert_eq!(
            narrow_to_owner("Codeunit Sel C::Persist", q, ab(), owner),
            Vec::<String>::new()
        );
        assert_eq!(
            narrow_to_owner("Sel A::Persist", "full-display", ab(), owner),
            ab()
        );
    }
    use crate::engine::l5::snapshot::SnapshotIdentityTable;

    fn snap_with_identities(rows: &[(&str, &str)]) -> CapabilitySnapshot {
        let mut ids = SnapshotIdentityTable {
            stable_ids: Vec::new(),
            display_names: Vec::new(),
        };
        for (id, display) in rows {
            ids.stable_ids.push((*id).into());
            ids.display_names.push((*display).into());
        }
        CapabilitySnapshot {
            identities: ids,
            capability_facts: Vec::new(),
            typed_edges: Vec::new(),
            operation_index: Vec::new(),
            callsite_index: Vec::new(),
            callsite_resolutions: Vec::new(),
            analysis_gaps: Vec::new(),
            coverage: Vec::new(),
            event_declarations: Vec::new(),
            root_classifications: Vec::new(),
            routine_order_frames: None,
            pd_solution: Default::default(),
        }
    }

    // --- #6 resolveSelector cascade ---------------------------------------

    #[test]
    fn selector_form1_exact_stable_id() {
        let snap = snap_with_identities(&[("app:Codeunit:1#abc", "Codeunit \"X\"::Run")]);
        let idx = build_selector_indexes(&snap, None);
        assert_eq!(
            resolve_selector("app:Codeunit:1#abc", &idx).0,
            vec!["app:Codeunit:1#abc".to_string()]
        );
    }

    #[test]
    fn selector_form2_full_display_case_insensitive() {
        let snap = snap_with_identities(&[("app:Codeunit:1#abc", "Codeunit \"X\"::Run")]);
        let idx = build_selector_indexes(&snap, None);
        assert_eq!(
            resolve_selector("codeunit \"x\"::run", &idx).0,
            vec!["app:Codeunit:1#abc".to_string()]
        );
    }

    #[test]
    fn selector_form3_two_segment_strips_typeword() {
        let snap = snap_with_identities(&[("app:Codeunit:1#abc", "Codeunit \"X\"::Run")]);
        let idx = build_selector_indexes(&snap, None);
        // Drop the leading "Codeunit " type-word.
        assert_eq!(
            resolve_selector("\"X\"::Run", &idx).0,
            vec!["app:Codeunit:1#abc".to_string()]
        );
    }

    #[test]
    fn selector_form4_one_segment_routine_name() {
        let snap = snap_with_identities(&[("app:Codeunit:1#abc", "Codeunit \"X\"::Run")]);
        let idx = build_selector_indexes(&snap, None);
        assert_eq!(
            resolve_selector("Run", &idx).0,
            vec!["app:Codeunit:1#abc".to_string()]
        );
    }

    #[test]
    fn selector_form5_object_qualified() {
        // Form 5 fires when the FULL routine index has a bucket keyed by the bare
        // routine name (e.g. a trigger routine whose display IS just "OnRun"), and the
        // selector is object-qualified ("Obj::OnRun"): the segment after the last "::"
        // is looked up directly. Here the identity display is the bare "OnRun".
        let snap = snap_with_identities(&[("app:Codeunit:1#abc", "OnRun")]);
        let idx = build_selector_indexes(&snap, None);
        assert_eq!(
            resolve_selector("Codeunit \"X\"::OnRun", &idx).0,
            vec!["app:Codeunit:1#abc".to_string()]
        );
    }

    #[test]
    fn selector_ambiguous_is_deterministic_in_identity_order() {
        // Two routines share the bare name "Run" → one-segment form returns BOTH,
        // in identity-table insertion order (deterministic, not HashMap order).
        let snap = snap_with_identities(&[
            ("app:Codeunit:1#aaa", "Codeunit \"A\"::Run"),
            ("app:Codeunit:2#bbb", "Codeunit \"B\"::Run"),
        ]);
        let idx = build_selector_indexes(&snap, None);
        let m = resolve_selector("Run", &idx).0;
        assert_eq!(
            m,
            vec![
                "app:Codeunit:1#aaa".to_string(),
                "app:Codeunit:2#bbb".to_string()
            ],
            "ambiguous matches must be in deterministic identity order"
        );
    }

    #[test]
    fn selector_ambiguous_stays_deterministic_past_two_candidates() {
        // Regression for the fingerprint_query HashMap-iteration bug: with 4+
        // same-display routines, both bucket order AND per-bucket push order used
        // to depend on HashMap iteration. Assert against the ONE known-correct
        // (identity-table insertion) order — a random order would fail this
        // assertion on nearly every run.
        let snap = snap_with_identities(&[
            ("app:Codeunit:1#aaa", "Codeunit \"A\"::Run"),
            ("app:Codeunit:2#bbb", "Codeunit \"B\"::Run"),
            ("app:Codeunit:3#ccc", "Codeunit \"C\"::Run"),
            ("app:Codeunit:4#ddd", "Codeunit \"D\"::Run"),
        ]);
        let idx = build_selector_indexes(&snap, None);
        let m = resolve_selector("Run", &idx).0;
        assert_eq!(
            m,
            vec![
                "app:Codeunit:1#aaa".to_string(),
                "app:Codeunit:2#bbb".to_string(),
                "app:Codeunit:3#ccc".to_string(),
                "app:Codeunit:4#ddd".to_string(),
            ],
            "ambiguous matches must be in deterministic identity order"
        );
    }

    /// #66: a cross-app snapshot holds dependency routines too. With the primary
    /// app given, a bare name shared with a dependency routine resolves to the
    /// workspace's alone; a dependency stable id is not selectable at all.
    #[test]
    fn selector_primary_filter_drops_dependency_routines() {
        let snap = snap_with_identities(&[
            ("aaaa:Codeunit:1#ws", "Codeunit \"W\"::OnRun"),
            ("bbbb:Codeunit:2#dep", "Codeunit \"D\"::OnRun"),
            ("aaaab:Codeunit:3#lookalike", "Codeunit \"L\"::OnRun"),
        ]);
        let idx = build_selector_indexes(&snap, Some("AAAA"));
        assert_eq!(
            resolve_selector("OnRun", &idx).0,
            vec!["aaaa:Codeunit:1#ws".to_string()]
        );
        assert!(resolve_selector("bbbb:Codeunit:2#dep", &idx).0.is_empty());
        let all = build_selector_indexes(&snap, None);
        assert_eq!(resolve_selector("OnRun", &all).0.len(), 3);
    }

    #[test]
    fn selector_unmatched_returns_empty() {
        let snap = snap_with_identities(&[("app:Codeunit:1#abc", "Codeunit \"X\"::Run")]);
        let idx = build_selector_indexes(&snap, None);
        assert!(resolve_selector("DoesNotExist", &idx).0.is_empty());
    }

    #[test]
    fn normalize_display_key_collapses_whitespace() {
        assert_eq!(normalize_display_key("  Foo   Bar  "), "foo bar");
        assert_eq!(
            normalize_display_key("Codeunit\t\"X\"::Run"),
            "codeunit \"x\"::run"
        );
    }
}
