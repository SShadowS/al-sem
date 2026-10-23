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

use crate::engine::l5::snapshot::SnapshotIdentityTable;

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
    identities: &SnapshotIdentityTable,
    primary_app_guid: Option<&str>,
) -> SelectorIndexes {
    let is_primary = |id: &str| {
        crate::program::model::role::role_of_stable_id(primary_app_guid, id)
            == crate::program::model::role::Role::Primary
    };
    let mut routine_display_by_id: HashMap<String, String> = HashMap::new();
    let mut display_to_stable_ids: Vec<(String, Vec<String>)> = Vec::new();
    let mut key_pos: HashMap<String, usize> = HashMap::new();

    for i in 0..identities.stable_ids.len() {
        let id = identities.stable_ids.get(i).cloned().unwrap_or_default();
        let display = identities.display_names.get(i).cloned().unwrap_or_default();
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

/// The cascade's matches, in deterministic order. A form-5 match still owes its
/// owner check (#239), so the ids are private: [`narrow_to_owner`] is the only
/// way to read them, and no caller can skip the check and fall back to another
/// object's routine.
pub(crate) struct SelectorMatches {
    ids: Vec<String>,
    owner: Option<OwnerPart>,
}

/// The object part of an `<owner>::<routine>` selector. `typed`: it starts with
/// an object type word (`Codeunit "Sel A"`), so it names a full owner display;
/// otherwise it is a bare object name (`Sel A`, `"Sel A"`). `wants`: its display
/// keys, the AL-unquoted spelling first, then the spelling as written when it
/// differs (an owner whose logical name is itself wrapped in `"`, displayed
/// `Codeunit "Legacy"`). The first spelling that keeps a match wins.
#[derive(Debug, PartialEq)]
struct OwnerPart {
    typed: bool,
    wants: Vec<String>,
}

/// The type words an owner display starts with (the model's `object_type`,
/// `ir_object_type`; pinned by `object_type_words_are_the_model_types`), as
/// display keys.
const OBJECT_TYPE_WORDS: [&str; 14] = [
    "codeunit",
    "table",
    "tableextension",
    "page",
    "pageextension",
    "report",
    "reportextension",
    "query",
    "xmlport",
    "enum",
    "enumextension",
    "interface",
    "controladdin",
    "permissionset",
];

/// One outer `"` pair removed (`"Post ""X"""` -> `Post ""X""`), nothing
/// unescaped: a routine display is the raw source name, `""` kept (only object
/// names are logical, #240). Anything else comes back trimmed.
fn strip_quotes(s: &str) -> &str {
    let s = s.trim();
    s.strip_prefix('"')
        .and_then(|r| r.strip_suffix('"'))
        .unwrap_or(s)
}

/// An AL quoted object name (`"A ""B"""`) -> its logical name (`A "B"`), the
/// spelling owner displays use (#240). Anything else comes back trimmed.
fn unquote(s: &str) -> String {
    let s = s.trim();
    match s.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
        Some(inner) => crate::program::body::scope::unescape_al_identifier(inner),
        None => s.to_string(),
    }
}

/// The byte offset of the last `::` outside AL quotes: `Codeunit X::"A::B"`
/// splits after `X`.
fn last_separator(s: &str) -> Option<usize> {
    let (mut quoted, mut last) = (false, None);
    for (i, c) in s.char_indices() {
        if c == '"' {
            quoted = !quoted;
        } else if !quoted && s[i..].starts_with("::") {
            last = Some(i);
        }
    }
    last
}

/// `None` for an empty object part: `::Run` is the bare routine name `Run`.
/// An unquoted bare name whose first word is a type word (`Page Management`)
/// reads as typed; write it `"Page Management"` to select it by bare name.
fn owner_part(part: &str) -> Option<OwnerPart> {
    let part = part.trim();
    if part.is_empty() {
        return None;
    }
    let (word, rest) = part.split_once(char::is_whitespace).unwrap_or((part, ""));
    let typed = OBJECT_TYPE_WORDS.contains(&word.to_lowercase().as_str());
    let name = if typed { rest } else { part };
    let key = |n: &str| match typed {
        true => normalize_display_key(&format!("{word} {n}")),
        false => normalize_display_key(n),
    };
    let mut wants = vec![key(&unquote(name))];
    let as_written = key(name);
    if as_written != wants[0] {
        wants.push(as_written);
    }
    Some(OwnerPart { typed, wants })
}

/// `resolveSelector` (fingerprint-query.ts) — the 5-form cascade. Returns the
/// matches (read through [`narrow_to_owner`]), plus the name of the form that
/// matched (`""` when unresolved) for diagnostics that report it.
pub(crate) fn resolve_selector(
    selector: &str,
    idx: &SelectorIndexes,
) -> (SelectorMatches, &'static str) {
    let (ids, form, owner) = cascade(selector, idx);
    (SelectorMatches { ids, owner }, form)
}

/// #239: the routines a form-5 selector named before its owner check kept none,
/// each labelled by `label(id, display)`: the hint for an unmatched selector.
/// Empty for every other form.
pub(crate) fn owner_check_candidates(
    selector: &str,
    idx: &SelectorIndexes,
    label: impl Fn(&str, &str) -> String,
) -> Vec<String> {
    let (m, _) = resolve_selector(selector, idx);
    if m.owner.is_none() {
        return Vec::new();
    }
    m.ids
        .iter()
        .map(|id| {
            label(
                id,
                idx.routine_display_by_id.get(id).map_or("", String::as_str),
            )
        })
        .collect()
}

fn cascade(
    selector: &str,
    idx: &SelectorIndexes,
) -> (Vec<String>, &'static str, Option<OwnerPart>) {
    // Form 1: exact StableRoutineId (case-sensitive).
    if idx.routine_display_by_id.contains_key(selector) {
        return (vec![selector.to_string()], "stable-routine-id", None);
    }
    let sep = last_separator(selector);

    // Form 2: full display name (normalized). A selector with no `::` is a
    // routine name, its one outer quote pair removed as in form 5.
    let key = normalize_display_key(match sep {
        None => strip_quotes(selector),
        Some(_) => selector,
    });
    if let Some(full) = display_to_ids(idx, &key)
        && !full.is_empty()
    {
        return (full.clone(), "full-display", None);
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
        return (two, "two-segment", None);
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
        return (one, "one-segment", None);
    }

    // Form 5: object-qualified — routine segment after the last unquoted "::",
    // one outer quote pair removed (`"Post Document"`), with the object part
    // for the owner check.
    if let Some(sep) = sep {
        let routine_key = normalize_display_key(strip_quotes(&selector[sep + 2..]));
        if let Some(qualified) = display_to_ids(idx, &routine_key)
            && !qualified.is_empty()
        {
            let owner = owner_part(&selector[..sep]);
            return (qualified.clone(), "object-qualified", owner);
        }
    }

    (Vec::new(), "", None)
}

/// #78/#239: the matches of [`resolve_selector`]. A form-5 match
/// (`<owner>::<routine>`, as the human headers print it) is kept only when its
/// owner (`owner_of`, a display key) is the named one: the full owner for a typed
/// object part, the owner without its type word for a bare one. When none is,
/// nothing is kept and the caller reports the selector as unmatched.
///
/// shortcut: `_selector` and `_matched_form` are unused (`matches` carries the
/// object part); they stay while `fingerprint_query.rs`'s call passes them.
pub(crate) fn narrow_to_owner(
    _selector: &str,
    _matched_form: &str,
    matches: SelectorMatches,
    owner_of: impl Fn(&str) -> String,
) -> Vec<String> {
    let SelectorMatches { ids, owner } = matches;
    let Some(owner) = owner else {
        return ids;
    };
    let keyed: Vec<(String, String)> = ids
        .into_iter()
        .map(|id| (normalize_display_key(&owner_of(&id)), id))
        .collect();
    for want in &owner.wants {
        let kept: Vec<String> = keyed
            .iter()
            .filter(|(key, _)| match owner.typed {
                true => key == want,
                false => strip_type_word_prefix(key) == Some(want.as_str()),
            })
            .map(|(_, id)| id.clone())
            .collect();
        if !kept.is_empty() {
            return kept;
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #78/#239 (U3): a form-5 match is kept only for the named owner, through
    /// `resolve_selector` + `narrow_to_owner` as every caller runs them. Each
    /// row is a routine displayed `Persist` (the shape that reaches form 5).
    /// Breaks: see the ledger's proof table (#239 attempt 2).
    #[test]
    fn narrow_to_owner_keeps_the_named_owner_only() {
        let owner = |id: &str| {
            match id {
                "a#" => "Codeunit Sel A",
                "b#" => "Codeunit Sel B",
                "c#" => "Table Codeunit Sel A",
                "l#" => "Codeunit \"Legacy\"",
                "m#" => "Codeunit Legacy",
                _ => "Codeunit A \"B\" C",
            }
            .to_string()
        };
        // `(id, routine display)` rows.
        let sel_rows = |selector: &str, rows: &[(&str, &str)]| {
            let idx = build_selector_indexes(&snap_with_identities(rows), None);
            let (m, form) = resolve_selector(selector, &idx);
            narrow_to_owner(selector, form, m, owner)
        };
        let sel = |selector: &str, ids: &[&str]| {
            let rows: Vec<(&str, &str)> = ids.iter().map(|id| (*id, "Persist")).collect();
            sel_rows(selector, &rows)
        };
        // B1: a routine display is the raw name (`""` kept); one outer quote pair
        // goes, nothing is unescaped. B4: the same for a bare routine selector.
        let raw = [("a#", "Post \"\"X\"\""), ("b#", "Persist")];
        assert_eq!(sel_rows("Codeunit Sel A::\"Post \"\"X\"\"\"", &raw), ["a#"]);
        assert_eq!(sel_rows("\"Post \"\"X\"\"\"", &raw), ["a#"]);
        assert_eq!(sel("\"Persist\"", &["a#"]), ["a#"]);
        // B3: the split ignores a `::` inside quotes.
        assert_eq!(
            sel_rows("Codeunit Sel A::\"A::B\"", &[("a#", "A::B")]),
            ["a#"]
        );
        // B2: an owner whose logical name is wrapped in `"` (`"""Legacy"""`), by
        // its AL and its displayed spelling; the unquoted spelling wins first.
        assert_eq!(
            sel("Codeunit \"\"\"Legacy\"\"\"::Persist", &["l#", "m#"]),
            ["l#"]
        );
        assert_eq!(sel("Codeunit \"Legacy\"::Persist", &["l#"]), ["l#"]);
        assert_eq!(sel("Codeunit \"Legacy\"::Persist", &["l#", "m#"]), ["m#"]);
        let ab = ["a#", "b#"];
        // The full owner, case and space folded; the bare name; AL quotes.
        assert_eq!(sel("codeunit  SEL a::Persist", &ab), ["a#"]);
        assert_eq!(sel("Sel A::Persist", &ab), ["a#"]);
        assert_eq!(sel("Codeunit \"Sel A\"::Persist", &ab), ["a#"]);
        // No fallback to another owner.
        assert!(sel("Codeunit Sel C::Persist", &ab).is_empty());
        // CR1: a typed object part never matches a bare name (`c#`'s is
        // `Codeunit Sel A`); a quoted one is a bare name.
        assert_eq!(sel("Codeunit Sel A::Persist", &["a#", "b#", "c#"]), ["a#"]);
        assert!(sel("Codeunit \"Sel A\"::Persist", &["b#", "c#"]).is_empty());
        assert_eq!(sel("\"Codeunit Sel A\"::Persist", &["a#", "c#"]), ["c#"]);
        // CR2: the routine part is unquoted too.
        assert_eq!(sel("Codeunit \"Sel A\"::\"Persist\"", &ab), ["a#"]);
        // CR4: an empty object part is the bare routine name.
        assert_eq!(sel("::Persist", &ab), ab);
        // #240: a logical name holding `"`, in AL and in display spelling.
        assert_eq!(
            sel("Codeunit \"A \"\"B\"\" C\"::Persist", &["a#", "d#"]),
            ["d#"]
        );
        assert_eq!(sel("Codeunit A \"B\" C::Persist", &["a#", "d#"]), ["d#"]);
        // Other forms are not narrowed.
        assert_eq!(sel("Persist", &ab), ab);
    }

    /// #239 B9: `OBJECT_TYPE_WORDS` is exactly the type words the model's owner
    /// displays use (`ir_object_type`). The match has no wildcard, so a new
    /// `ObjectKind` stops the build here.
    #[test]
    fn object_type_words_are_the_model_types() {
        use al_syntax::ir::ObjectKind::{self, *};
        fn all(k: ObjectKind) -> ObjectKind {
            match k {
                Codeunit
                | Table
                | TableExtension
                | Page
                | PageExtension
                | Report
                | ReportExtension
                | Query
                | XmlPort
                | Enum
                | EnumExtension
                | Interface
                | ControlAddIn
                | Entitlement
                | PermissionSet
                | PermissionSetExtension
                | Profile
                | Other => k,
            }
        }
        let kinds = [
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
        ];
        let mut words: Vec<String> = kinds
            .into_iter()
            .filter_map(|k| crate::program::body::ir_walk::ir_object_type(&all(k)))
            .map(str::to_lowercase)
            .collect();
        words.sort();
        let mut list = OBJECT_TYPE_WORDS.map(str::to_string).to_vec();
        list.sort();
        assert_eq!(words, list);
    }

    fn snap_with_identities(rows: &[(&str, &str)]) -> SnapshotIdentityTable {
        let mut ids = SnapshotIdentityTable::default();
        for (id, display) in rows {
            ids.stable_ids.push((*id).into());
            ids.display_names.push((*display).into());
        }
        ids
    }

    // --- #6 resolveSelector cascade ---------------------------------------

    #[test]
    fn selector_form1_exact_stable_id() {
        let snap = snap_with_identities(&[("app:Codeunit:1#abc", "Codeunit \"X\"::Run")]);
        let idx = build_selector_indexes(&snap, None);
        assert_eq!(
            resolve_selector("app:Codeunit:1#abc", &idx).0.ids,
            vec!["app:Codeunit:1#abc".to_string()]
        );
    }

    #[test]
    fn selector_form2_full_display_case_insensitive() {
        let snap = snap_with_identities(&[("app:Codeunit:1#abc", "Codeunit \"X\"::Run")]);
        let idx = build_selector_indexes(&snap, None);
        assert_eq!(
            resolve_selector("codeunit \"x\"::run", &idx).0.ids,
            vec!["app:Codeunit:1#abc".to_string()]
        );
    }

    #[test]
    fn selector_form3_two_segment_strips_typeword() {
        let snap = snap_with_identities(&[("app:Codeunit:1#abc", "Codeunit \"X\"::Run")]);
        let idx = build_selector_indexes(&snap, None);
        // Drop the leading "Codeunit " type-word.
        assert_eq!(
            resolve_selector("\"X\"::Run", &idx).0.ids,
            vec!["app:Codeunit:1#abc".to_string()]
        );
    }

    #[test]
    fn selector_form4_one_segment_routine_name() {
        let snap = snap_with_identities(&[("app:Codeunit:1#abc", "Codeunit \"X\"::Run")]);
        let idx = build_selector_indexes(&snap, None);
        assert_eq!(
            resolve_selector("Run", &idx).0.ids,
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
            resolve_selector("Codeunit \"X\"::OnRun", &idx).0.ids,
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
        let m = resolve_selector("Run", &idx).0.ids;
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
        let m = resolve_selector("Run", &idx).0.ids;
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
            resolve_selector("OnRun", &idx).0.ids,
            vec!["aaaa:Codeunit:1#ws".to_string()]
        );
        assert!(
            resolve_selector("bbbb:Codeunit:2#dep", &idx)
                .0
                .ids
                .is_empty()
        );
        let all = build_selector_indexes(&snap, None);
        assert_eq!(resolve_selector("OnRun", &all).0.ids.len(), 3);
    }

    #[test]
    fn selector_unmatched_returns_empty() {
        let snap = snap_with_identities(&[("app:Codeunit:1#abc", "Codeunit \"X\"::Run")]);
        let idx = build_selector_indexes(&snap, None);
        assert!(resolve_selector("DoesNotExist", &idx).0.ids.is_empty());
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
