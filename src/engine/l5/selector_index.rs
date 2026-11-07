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

use std::collections::{BTreeMap, HashMap};

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
    /// The routine ids of the loaded DEPENDENCY code. Only the exact-id form
    /// selects one (a display name stays the workspace's), so `prove` and
    /// `digest` can start inside a dependency.
    dependency_routine_ids: std::collections::HashSet<String>,
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
    let mut dependency_routine_ids = std::collections::HashSet::new();

    for i in 0..identities.stable_ids.len() {
        let id = identities.stable_ids.get(i).cloned().unwrap_or_default();
        let display = identities.display_names.get(i).cloned().unwrap_or_default();
        if id.is_empty() {
            continue;
        }
        if id.contains(ROUTINE_ID_SEPARATOR) && !is_primary(&id) {
            dependency_routine_ids.insert(id.clone());
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
        dependency_routine_ids,
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
    if idx.routine_display_by_id.contains_key(selector)
        || idx.dependency_routine_ids.contains(selector)
    {
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

    // Form 6 (MCP output plan 3b: one selector grammar for every tool):
    // `Object.Routine`, the spelling `alsem query` takes, read as
    // `Object::Routine` at the last unquoted `.` (an object name may hold
    // dots, `Cust. Ledger Entry`; a routine name holds none unless quoted).
    // Only when nothing above matched, so no selector that resolved before
    // resolves differently now.
    if sep.is_none()
        && let Some(dot) = last_dot(selector)
        && dot > 0
        && dot + 1 < selector.len()
    {
        let as_qualified = format!("{}::{}", &selector[..dot], &selector[dot + 1..]);
        let (ids, form, owner) = cascade(&as_qualified, idx);
        if !ids.is_empty() {
            let form = if form.is_empty() { form } else { "object-dot" };
            return (ids, form, owner);
        }
    }

    (Vec::new(), "", None)
}

/// A member-form selector (output validation V3, §2.1), split:
/// `Object."Field"::Trigger`, `Object.Field::Trigger` (a plain identifier),
/// `Object.action(Name)::OnAction` and its kin (`dataitem`, `control`,
/// `element`); a caption after the member name (`action(Name "Caption")`)
/// is accepted and ignored. The forms the one routine display prints for a
/// member trigger, so a display can be pasted into the next call.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct MemberSelector {
    /// The object part, as written (`Table "CDO E-Mail Job"`).
    pub(crate) owner: String,
    /// The member kind word, lowercase, when the selector names one.
    pub(crate) kind: Option<String>,
    /// The member's logical name (AL quotes removed).
    pub(crate) member: String,
    /// The routine name (one outer quote pair removed).
    pub(crate) routine: String,
}

/// The member kind words a member-form selector takes (`.action(Name)`).
const MEMBER_KIND_WORDS: [&str; 4] = ["action", "dataitem", "control", "element"];

/// The byte offset where each top-level quoted segment of `s` starts, in
/// order (a `""` inside quotes is an escaped quote, not a segment end).
fn quoted_starts(s: &str) -> Vec<(usize, usize)> {
    let b = s.as_bytes();
    let (mut out, mut i) = (Vec::new(), 0usize);
    while i < b.len() {
        if b[i] == b'"' {
            let start = i;
            i += 1;
            while i < b.len() {
                if b[i] == b'"' {
                    if i + 1 < b.len() && b[i + 1] == b'"' {
                        i += 2;
                        continue;
                    }
                    break;
                }
                i += 1;
            }
            out.push((start, i.min(b.len().saturating_sub(1))));
        }
        i += 1;
    }
    out
}

/// A plain AL identifier (`[A-Za-z_][A-Za-z0-9_]*`).
fn is_plain_identifier(s: &str) -> bool {
    s.chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `selector` read as a member form ([`MemberSelector`]); `None` when it is
/// not one. The parameter list, if any, is split off first
/// ([`split_signature`]); a trigger has none.
pub(crate) fn split_member(selector: &str) -> Option<MemberSelector> {
    let sep = last_separator(selector)?;
    let (left, routine) = (selector[..sep].trim(), strip_quotes(&selector[sep + 2..]));
    if routine.is_empty() || left.is_empty() {
        return None;
    }
    let done = |owner: &str, kind: Option<String>, member: String| {
        let owner = owner.trim();
        (!owner.is_empty() && !member.is_empty()).then(|| MemberSelector {
            owner: owner.to_string(),
            kind,
            member,
            routine: routine.to_string(),
        })
    };
    // `.kind(Name …)`: the last unquoted `(` of a text that ends with `)`.
    if left.ends_with(')') {
        let (mut quoted, mut open) = (false, None);
        for (i, c) in left.char_indices() {
            match c {
                '"' => quoted = !quoted,
                '(' if !quoted => open = Some(i),
                _ => {}
            }
        }
        let open = open?;
        let head = &left[..open];
        let dot = head.rfind('.')?;
        let kind = head[dot + 1..].trim().to_ascii_lowercase();
        if !MEMBER_KIND_WORDS.contains(&kind.as_str()) {
            return None;
        }
        let inner = left[open + 1..left.len() - 1].trim();
        let name = if inner.starts_with('"') {
            let &(s, e) = quoted_starts(inner).first()?;
            unquote(&inner[s..=e])
        } else {
            inner.split_whitespace().next()?.to_string()
        };
        return done(&head[..dot], Some(kind), name);
    }
    // `."Field"`: the last quoted segment ends the text and follows a dot.
    if left.ends_with('"') {
        let &(s, e) = quoted_starts(left).last()?;
        if e + 1 != left.len() || s == 0 || left.as_bytes()[s - 1] != b'.' {
            return None;
        }
        return done(&left[..s - 1], None, unquote(&left[s..=e]));
    }
    // `.Field`: a plain identifier after the last unquoted dot.
    let dot = last_dot(left)?;
    let member = &left[dot + 1..];
    if !is_plain_identifier(member) {
        return None;
    }
    done(&left[..dot], None, member.to_string())
}

/// One workspace member trigger as a member-form selector matches it: its
/// stable id, its owner's display (`Table CDO E-Mail Job`), its member's
/// name and kind, and its routine name.
pub(crate) struct MemberRow<'a> {
    pub(crate) id: &'a str,
    pub(crate) owner_display: String,
    pub(crate) member: &'a str,
    pub(crate) member_kind: &'a str,
    pub(crate) routine: &'a str,
}

/// The member triggers a member-form selector names (V3 §2.1), in `rows`
/// order, each stable id once; `None` when `selector` is not a member form.
/// The owner is matched as an `<owner>::<routine>` selector's is (typed:
/// the full owner display; bare: the name; AL quotes optional), the member
/// and routine names case-insensitively, the kind when the selector names
/// one.
pub(crate) fn resolve_member_selector<'a>(
    selector: &str,
    rows: impl IntoIterator<Item = MemberRow<'a>>,
) -> Option<Vec<String>> {
    let m = split_member(selector)?;
    let owner = owner_part(&m.owner)?;
    let mut out: Vec<String> = Vec::new();
    for r in rows {
        if !r.routine.eq_ignore_ascii_case(&m.routine)
            || !r.member.eq_ignore_ascii_case(&m.member)
            || m.kind.as_deref().is_some_and(|k| k != r.member_kind)
        {
            continue;
        }
        let key = normalize_display_key(&r.owner_display);
        let owned = owner.wants.iter().any(|want| match owner.typed {
            true => key == *want,
            false => strip_type_word_prefix(&key) == Some(want.as_str()),
        });
        if owned && !out.iter().any(|x| x == r.id) {
            out.push(r.id.to_string());
        }
    }
    Some(out)
}

/// The selector forms every tool reads, as a failure names them (V3 §2.4).
pub(crate) const SELECTOR_FORMS: &str = "selector forms: Object::Routine, \
     Object::Routine(<parameter types>), Object.\"Field\"::Trigger, \
     Object.action(Name)::OnAction, or a stable id";

/// How far a routine name may be from the asked one to be offered as a
/// nearest name (V3 §2.4): its edit distance (case-insensitive) at most this
/// share of the longer name's length. 0.4 keeps a typo, a missing or extra
/// word of a short name and a dropped suffix (`SendMailTwice` → `SendMail`:
/// 5 of 13) and drops a name that shares only a word (`SendMailTwice` →
/// `Send`: 9 of 13); measured on the fixtures and CDO (the V3 report).
pub(crate) const NEAREST_RATIO: f64 = 0.4;

/// How many nearest names a failure lists at most (V3 §2.4).
pub(crate) const NEAREST_MAX: usize = 5;

/// A nearest name offered for an unmatched selector (V3 §2.4): its display
/// without parameters, how many routines of its object have that name, a
/// selector for the first (complete when it is the only one), and its edit
/// distance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NearName {
    pub display: String,
    pub overloads: usize,
    pub selector: String,
    pub distance: usize,
}

/// What an unmatched selector's answer offers (V3 §2.4): the nearest
/// workspace names, or, when the name exists only in dependency code, where
/// (`(app guid, display)`; a renderer names the app from its loaded apps):
/// selectors are scoped to the workspace app.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectorHints {
    pub nearest: Vec<NearName>,
    pub dependency_only: Vec<(String, String)>,
}

/// Levenshtein distance over chars.
pub(crate) fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let sub = prev[j - 1] + usize::from(a[i - 1] != b[j - 1]);
            cur[j] = sub.min(prev[j] + 1).min(cur[j - 1] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Whether `candidate` is near enough `asked` to be offered ([`NEAREST_RATIO`]).
pub(crate) fn is_near(asked: &str, candidate: &str) -> Option<usize> {
    let (a, c) = (asked.to_lowercase(), candidate.to_lowercase());
    let d = edit_distance(&a, &c);
    let longest = a.chars().count().max(c.chars().count());
    (d as f64 <= NEAREST_RATIO * longest as f64).then_some(d)
}

/// The routine name and object part a selector asks for, whatever its
/// form; `None` for a stable id (no name to be near).
fn asked_name(selector: &str) -> Option<(String, Option<String>)> {
    let (base, _) = split_signature(selector);
    if base.contains(ROUTINE_ID_SEPARATOR) && !base.contains("::") {
        return None;
    }
    if let Some(m) = split_member(base) {
        return Some((m.routine, Some(m.owner)));
    }
    if let Some(sep) = last_separator(base) {
        let owner = base[..sep].trim();
        return Some((
            strip_quotes(&base[sep + 2..]).to_string(),
            (!owner.is_empty()).then(|| owner.to_string()),
        ));
    }
    if let Some(dot) = last_dot(base)
        && dot > 0
        && dot + 1 < base.len()
    {
        return Some((
            strip_quotes(&base[dot + 1..]).to_string(),
            Some(base[..dot].to_string()),
        ));
    }
    Some((strip_quotes(base).to_string(), None))
}

/// The hints for a selector that names no workspace routine (V3 §2.4):
/// the workspace routine names within [`NEAREST_RATIO`] of the asked name
/// (at most [`NEAREST_MAX`], nearest first, then those of the asked object,
/// then by display), or, when no workspace routine has the name and a
/// dependency routine does, those dependency routines.
pub(crate) fn selector_hints(
    names: &crate::engine::l5::routine_rollup::RoutineNames<'_>,
    owners: impl Fn(&str) -> String,
    selector: &str,
) -> SelectorHints {
    use crate::engine::l5::routine_rollup::format_routine;
    let Some((asked, owner)) = asked_name(selector) else {
        return SelectorHints::default();
    };
    if asked.is_empty() {
        return SelectorHints::default();
    }
    let owner = owner.as_deref().and_then(owner_part);
    // Round-2 G40 (n06): a member form (`Page X.action(SendEMailActio)::
    // OnAction`) is near by its member's name among the routines with that
    // trigger name, not by the trigger name every action shares.
    let member_ask = split_member(split_signature(selector).0).map(|m| (m.member, m.routine));
    let model = names.model;
    let mut seen = std::collections::HashSet::new();
    let primary_has = model
        .workspace
        .routines
        .iter()
        .any(|r| names.is_primary(&r.stable_routine_id) && r.name.eq_ignore_ascii_case(&asked));
    if !primary_has {
        let mut dep: Vec<(String, String)> = model
            .workspace
            .routines
            .iter()
            .filter(|r| !names.is_primary(&r.stable_routine_id))
            .filter(|r| r.name.eq_ignore_ascii_case(&asked))
            .filter(|r| seen.insert(r.stable_routine_id.as_str()))
            .map(|r| {
                (
                    r.app_guid.to_ascii_lowercase(),
                    format_routine(&names.parts_of(r), false),
                )
            })
            .collect();
        if !dep.is_empty() {
            dep.sort();
            dep.dedup();
            dep.truncate(NEAREST_MAX);
            return SelectorHints {
                nearest: Vec::new(),
                dependency_only: dep,
            };
        }
    }
    // (display without parameters) -> (distance, same object, count, first id)
    let mut groups: BTreeMap<String, (usize, bool, usize, String)> = BTreeMap::new();
    let mut seen = std::collections::HashSet::new();
    for r in &model.workspace.routines {
        let id = r.stable_routine_id.as_str();
        if !names.is_primary(id) || !seen.insert(id) {
            continue;
        }
        let near = match &member_ask {
            Some((member, trigger)) => match r.enclosing_member.as_deref() {
                Some(m) if r.name.eq_ignore_ascii_case(trigger) => is_near(member, m),
                _ => None,
            },
            None => is_near(&asked, &r.name),
        };
        let Some(d) = near else {
            continue;
        };
        let mut p = names.parts_of(r);
        p.signature = None;
        let display = format_routine(&p, false);
        let same_object = owner.as_ref().is_some_and(|o| {
            let key = normalize_display_key(&owners(id));
            o.wants.iter().any(|w| match o.typed {
                true => key == *w,
                false => strip_type_word_prefix(&key) == Some(w.as_str()),
            })
        });
        let e = groups
            .entry(display)
            .or_insert((d, same_object, 0, id.to_string()));
        e.2 += 1;
    }
    let mut near: Vec<(usize, bool, String, usize, String)> = groups
        .into_iter()
        .map(|(display, (d, same, n, id))| (d, !same, display, n, id))
        .collect();
    near.sort();
    near.truncate(NEAREST_MAX);
    SelectorHints {
        nearest: near
            .into_iter()
            .map(|(distance, _, display, overloads, id)| NearName {
                selector: if overloads == 1 {
                    names.selector(&id)
                } else {
                    display.clone()
                },
                display,
                overloads,
                distance,
            })
            .collect(),
        dependency_only: Vec::new(),
    }
}

/// An app's name from the loaded apps' rows (`appId`, `name`), else its
/// guid.
pub(crate) fn app_name(apps: &[serde_json::Value], guid: &str) -> String {
    apps.iter()
        .find(|a| {
            a["appId"]
                .as_str()
                .is_some_and(|g| g.eq_ignore_ascii_case(guid))
        })
        .and_then(|a| a["name"].as_str())
        .map_or_else(|| guid.to_string(), str::to_string)
}

/// The human lines an unmatched selector's hints print (V3 §2.4), after
/// the failure's first line; `apps` name a dependency app.
pub(crate) fn hint_lines(h: &SelectorHints, apps: &[serde_json::Value]) -> Vec<String> {
    let mut out = Vec::new();
    if !h.dependency_only.is_empty() {
        let each: Vec<String> = h
            .dependency_only
            .iter()
            .map(|(app, d)| format!("{}: {d}", app_name(apps, app)))
            .collect();
        out.push(format!(
            "found only in {}; selectors are scoped to the workspace app (a stable id selects a \
             dependency routine)",
            each.join("; ")
        ));
    } else if h.nearest.is_empty() {
        out.push("nearest names: no close match".to_string());
    } else {
        let each: Vec<String> = h
            .nearest
            .iter()
            .map(|n| match n.overloads {
                1 => n.display.clone(),
                k => format!("{} ({k} overloads)", n.display),
            })
            .collect();
        out.push(format!("nearest names: {}", each.join("; ")));
    }
    out.push(SELECTOR_FORMS.to_string());
    out
}

/// The hints as JSON (V3 §2.4): `nearest: [{display, overloads, selector,
/// distance}]`, `foundOnlyIn: [{appId, display}]`.
pub(crate) fn hints_json(h: &SelectorHints) -> serde_json::Map<String, serde_json::Value> {
    let mut m = serde_json::Map::new();
    m.insert(
        "nearest".into(),
        h.nearest
            .iter()
            .map(|n| {
                serde_json::json!({
                    "display": n.display,
                    "overloads": n.overloads,
                    "selector": n.selector,
                    "distance": n.distance,
                })
            })
            .collect(),
    );
    if !h.dependency_only.is_empty() {
        m.insert(
            "foundOnlyIn".into(),
            h.dependency_only
                .iter()
                .map(|(app, d)| serde_json::json!({"appId": app, "display": d}))
                .collect(),
        );
    }
    m
}

/// An ambiguous selector's candidate as the failure lists it (V3 §2.4): its
/// display with its complete parameter list, its declaration, and the
/// selector that picks it alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmbiguousCandidate {
    pub display: String,
    pub at: String,
    pub selector: String,
    pub types: Vec<String>,
    /// Its stable id (round-2 G40: the candidate array names it).
    pub stable_id: String,
}

impl AmbiguousCandidate {
    /// The parameter list as the one display prints it: `(Integer, var
    /// Text)` ([`signature_text`]'s form).
    #[must_use]
    pub fn signature(&self) -> String {
        format!("({})", self.types.join(", "))
    }
}

impl AmbiguousCandidate {
    /// Of `id`, through the one display.
    pub(crate) fn of(
        names: &crate::engine::l5::routine_rollup::RoutineNames<'_>,
        id: &str,
    ) -> Option<AmbiguousCandidate> {
        let r = names.row(id)?;
        let mut p = names.parts_of(r);
        p.signature = Some(signature_text(&r.parameters));
        let at = &r.source_anchor;
        Some(AmbiguousCandidate {
            display: crate::engine::l5::routine_rollup::format_routine(&p, false),
            at: if at.source_unit_id.is_empty() {
                String::new()
            } else {
                format!(
                    "{}:{}",
                    at.source_unit_id,
                    crate::engine::line_base::shown(at.start_line)
                )
            },
            selector: names.selector(id),
            types: r
                .parameters
                .iter()
                .map(|p| {
                    if p.is_var {
                        format!("var {}", p.type_text)
                    } else {
                        p.type_text.clone()
                    }
                })
                .collect(),
            stable_id: id.to_string(),
        })
    }
}

/// How many candidates an ambiguity lists (and puts in its ready value).
pub(crate) const AMBIGUOUS_SHOWN: usize = 10;

/// An ambiguous selector's failure (V3 §2.4): the first line, one numbered
/// line per candidate (its display with its complete parameter list, where
/// it is declared, and what it adds to the first candidate's parameters),
/// and a ready `routines: [...]` value of the candidates' selectors.
pub(crate) fn ambiguity_lines(
    prefix: &str,
    selector: &str,
    cands: &[AmbiguousCandidate],
) -> Vec<String> {
    // Round-2 G40: what tells the candidates apart, on the failure line.
    let differs = match differs_in(cands) {
        Some(d) => format!(" (differs in: {d})"),
        None => String::new(),
    };
    let mut out = vec![format!(
        "{prefix}'{selector}' names {} routines{differs}",
        cands.len()
    )];
    let shown = &cands[..cands.len().min(AMBIGUOUS_SHOWN)];
    for (i, c) in shown.iter().enumerate() {
        let extra = match cands.first() {
            Some(f) if i > 0 && c.types.len() > f.types.len() && c.types.starts_with(&f.types) => {
                format!(" (+ {})", c.types[f.types.len()..].join(", "))
            }
            _ => String::new(),
        };
        let at = if c.at.is_empty() {
            String::new()
        } else {
            format!(" at {}", c.at)
        };
        out.push(format!("  {}. {}{at}{extra}", i + 1, c.display));
    }
    if cands.len() > shown.len() {
        out.push(format!(
            "  … +{} more (JSON `candidateSelectors` lists every one)",
            cands.len() - shown.len()
        ));
    }
    out
}

/// What tells an ambiguity's candidates apart (round-2 G40), or `None` when
/// nothing short says it: `the object` (one name in two objects), `a
/// trailing Boolean` (one list extends the other by one parameter),
/// `trailing Boolean, Text`, `parameter 3 (Integer or Text)` (same length),
/// `the number of parameters (7, 9, 11)` (each list extends the shorter
/// ones), else `the parameter lists`.
#[must_use]
pub fn differs_in(cands: &[AmbiguousCandidate]) -> Option<String> {
    if cands.len() < 2 {
        return None;
    }
    let object = |c: &AmbiguousCandidate| {
        c.display
            .split_once("::")
            .map_or(String::new(), |(o, _)| o.to_string())
    };
    if cands.iter().any(|c| object(c) != object(&cands[0])) {
        return Some("the object".to_string());
    }
    let mut by_len: Vec<&AmbiguousCandidate> = cands.iter().collect();
    by_len.sort_by_key(|c| c.types.len());
    let nested = by_len
        .windows(2)
        .all(|w| w[1].types.starts_with(&w[0].types) && w[1].types.len() > w[0].types.len());
    if nested {
        if let [a, b] = by_len[..] {
            let extra = &b.types[a.types.len()..];
            return Some(match extra {
                [one] => format!("a trailing {one}"),
                many => format!("trailing {}", many.join(", ")),
            });
        }
        let counts: Vec<String> = by_len.iter().map(|c| c.types.len().to_string()).collect();
        return Some(format!("the number of parameters ({})", counts.join(", ")));
    }
    if let [a, b] = cands
        && a.types.len() == b.types.len()
    {
        let at: Vec<String> = a
            .types
            .iter()
            .zip(&b.types)
            .enumerate()
            .filter(|(_, (x, y))| x != y)
            .map(|(i, (x, y))| format!("parameter {} ({x} or {y})", i + 1))
            .collect();
        if !at.is_empty() {
            return Some(at.join(", "));
        }
    }
    Some("the parameter lists".to_string())
}

/// The complete call an ambiguity offers (round-2 G40), in `surface`'s
/// words: a tool that takes several routines gets one call with every
/// candidate's selector (`complete call: alsem_explain {"routines": [...]}`);
/// a tool that takes one (prove, query effects) gets one call per candidate
/// (`call 1: alsem_prove {"routine": …, "question": …}`). `args` are the
/// call's other arguments as asked (prove's question).
#[must_use]
pub fn complete_call_lines(
    tool: crate::engine::follow_up::Tool,
    cx: crate::engine::follow_up::Cx,
    selectors: &[String],
    args: &[(&str, serde_json::Value)],
) -> Vec<String> {
    use crate::engine::follow_up::{FollowUp, Tool};
    let with_args = |mut f: FollowUp| {
        for (k, v) in args {
            f = f.arg(k, v.clone());
        }
        f
    };
    match tool {
        Tool::Explain | Tool::Digest | Tool::Tests | Tool::Impact => {
            let f = with_args(
                FollowUp::new(tool, cx, "every candidate")
                    .arg("routines", serde_json::Value::from(selectors.to_vec())),
            );
            vec![format!("  complete call: {}", f.text(cx.surface))]
        }
        Tool::Prove | Tool::QueryEffects | Tool::QueryTouches => selectors
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let key = if tool == Tool::QueryTouches {
                    "from"
                } else {
                    "routine"
                };
                let f = with_args(FollowUp::new(tool, cx, "one candidate").arg(key, s.as_str()));
                format!("  call {}: {}", i + 1, f.text(cx.surface))
            })
            .collect(),
    }
}

/// An `Owner::Routine` or `Owner.Routine` selector, split (MCP output plan
/// 3b: one grammar for every tool, so `alsem query` reads the `::` spelling
/// too).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct QualifiedSelector {
    /// The owner's object type word, lowercase, when it starts with one.
    pub(crate) type_word: Option<String>,
    /// The owner's logical name after the type word (AL quotes removed),
    /// lowercase.
    pub(crate) object: String,
    /// The whole owner part read as a name (`Page Management` names an
    /// object whose first word is a type word), lowercase.
    pub(crate) whole: String,
    /// The routine name (one outer quote pair removed), lowercase.
    pub(crate) routine: String,
}

impl QualifiedSelector {
    /// Whether an object of `object_type` named `object_name` declaring a
    /// routine named `routine_name` is the one selected (case-insensitive).
    pub(crate) fn selects(&self, object_type: &str, object_name: &str, routine_name: &str) -> bool {
        if routine_name.to_lowercase() != self.routine {
            return false;
        }
        let name = object_name.to_lowercase();
        let typed = self
            .type_word
            .as_deref()
            .is_some_and(|w| w == object_type.to_lowercase() && name == self.object);
        typed || name == self.whole
    }
}

/// `selector` split at its last unquoted `::`, else its last unquoted `.`;
/// `None` when neither gives two non-empty parts.
pub(crate) fn split_qualified(selector: &str) -> Option<QualifiedSelector> {
    let (owner, routine) = match last_separator(selector) {
        Some(sep) => (&selector[..sep], &selector[sep + 2..]),
        None => {
            let dot = last_dot(selector)?;
            (&selector[..dot], &selector[dot + 1..])
        }
    };
    let (owner, routine) = (owner.trim(), strip_quotes(routine));
    if owner.is_empty() || routine.is_empty() {
        return None;
    }
    let (word, rest) = owner.split_once(char::is_whitespace).unwrap_or((owner, ""));
    let typed = OBJECT_TYPE_WORDS.contains(&word.to_lowercase().as_str()) && !rest.is_empty();
    let name = if typed { rest } else { owner };
    Some(QualifiedSelector {
        type_word: typed.then(|| word.to_lowercase()),
        object: unquote(name).to_lowercase(),
        whole: unquote(owner).to_lowercase(),
        routine: routine.to_lowercase(),
    })
}

/// Output step 1b (fable #7): a selector's trailing parameter list, split
/// off: `Owner::Routine(Integer, Text)` -> (`Owner::Routine`, `["Integer",
/// "Text"]`); `Routine()` names the overload with no parameter. The list is
/// the last unquoted `(`…`)` that ends the selector; its items are split at
/// unquoted `,` or `;`. Every other selector comes back whole, with `None`,
/// so nothing that resolved before resolves differently.
pub(crate) fn split_signature(selector: &str) -> (&str, Option<Vec<String>>) {
    let s = selector.trim_end();
    if !s.ends_with(')') {
        return (selector, None);
    }
    let (mut quoted, mut open) = (false, None);
    for (i, c) in s.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '(' if !quoted => open = Some(i),
            _ => {}
        }
    }
    let Some(open) = open else {
        return (selector, None);
    };
    let base = s[..open].trim_end();
    if base.is_empty() || quoted {
        return (selector, None);
    }
    let inner = &s[open + 1..s.len() - 1];
    let mut items = Vec::new();
    let (mut quoted, mut start) = (false, 0);
    for (i, c) in inner.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ',' | ';' if !quoted => {
                items.push(inner[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    let last = inner[start..].trim();
    if !(items.is_empty() && last.is_empty()) {
        items.push(last.to_string());
    }
    (base, Some(items))
}

/// A parameter type or name as a selector compares it: lowercase, AL quotes
/// dropped, whitespace collapsed, a leading `var` dropped.
fn signature_key(s: &str) -> String {
    let k = normalize_display_key(&s.replace('"', ""));
    k.strip_prefix("var ").map_or(k.clone(), str::to_string)
}

/// Whether `params` are the ones a [`split_signature`] list names, item by
/// item: its type as declared (`Code[20]`, `Record "Sales Header"`), the
/// type without its length (`Code`) or its type word (`Record`), its name,
/// or `name: type`; `var` and AL quotes optional, case-insensitive.
pub(crate) fn signature_matches(
    wanted: &[String],
    params: &[crate::program::model::workspace::ModelParameter],
) -> bool {
    // V3 §2.1: the whole list as [`signature_text`] prints it, compared as
    // one text (case, AL quotes and whitespace folded; `var` optional), so a
    // type whose text holds a comma (`option A,B`, `Dictionary of [Code[10],
    // Text]`) still selects: the item split cannot tell its commas apart.
    let whole = |with_var: bool| -> String {
        let items: Vec<String> = params
            .iter()
            .map(|p| match (with_var, p.is_var) {
                (true, true) => format!("var {}", p.type_text),
                _ => p.type_text.clone(),
            })
            .collect();
        fold_whole(&items.join(","))
    };
    let asked = fold_whole(&wanted.join(","));
    if !params.is_empty() && (asked == whole(true) || asked == whole(false)) {
        return true;
    }
    wanted.len() == params.len()
        && wanted.iter().zip(params).all(|(w, p)| {
            let ty = signature_key(&p.type_text);
            let ty_ok = |want: &str| {
                want == ty
                    || ty.split('[').next().is_some_and(|t| t.trim() == want)
                    || ty.split(' ').next() == Some(want)
            };
            let name = signature_key(&p.name);
            match w.split_once(':') {
                Some((n, t)) => signature_key(n) == name && ty_ok(&signature_key(t)),
                None => {
                    let w = signature_key(w);
                    w == name || ty_ok(&w)
                }
            }
        })
}

/// A parameter list's text as [`signature_matches`] compares it whole:
/// lowercase, AL quotes and every whitespace dropped.
fn fold_whole(s: &str) -> String {
    s.chars()
        .filter(|c| *c != '"' && !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

/// A routine's parameter types as a selector takes them: `(Integer, var
/// RecordRef)`.
pub(crate) fn signature_text(
    params: &[crate::program::model::workspace::ModelParameter],
) -> String {
    let items: Vec<String> = params
        .iter()
        .map(|p| {
            if p.is_var {
                format!("var {}", p.type_text)
            } else {
                p.type_text.clone()
            }
        })
        .collect();
    format!("({})", items.join(", "))
}

/// One ambiguous selector's candidate, as every tool prints it: the
/// selector that names it (`qualified` + its parameter types), where it is
/// declared, and its stable id.
pub(crate) fn candidate_label(
    qualified: &str,
    r: &crate::program::model::workspace::ModelRoutine,
) -> String {
    format!(
        "{qualified}{} at {}:{} (stable id {})",
        signature_text(&r.parameters),
        r.source_anchor.source_unit_id,
        crate::engine::line_base::shown(r.source_anchor.start_line),
        r.stable_routine_id
    )
}

/// The byte offset of the last `.` outside AL quotes.
fn last_dot(s: &str) -> Option<usize> {
    let (mut quoted, mut last) = (false, None);
    for (i, c) in s.char_indices() {
        if c == '"' {
            quoted = !quoted;
        } else if !quoted && c == '.' {
            last = Some(i);
        }
    }
    last
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

    /// MCP output plan 3b (one selector grammar): `Object.Routine`, the
    /// spelling `alsem query` takes, selects what `Object::Routine` does, at
    /// the last unquoted dot (an object name may hold dots); a selector that
    /// already resolved keeps its match (form 6 runs only when nothing did).
    #[test]
    fn selector_form6_object_dot_reads_as_object_qualified() {
        // Routine displays are bare names; the owner check reads the owner.
        let snap = snap_with_identities(&[("a#", "Run"), ("b#", "Run")]);
        let idx = build_selector_indexes(&snap, None);
        let owner = |id: &str| {
            match id {
                "a#" => "Codeunit Cust. Ledger Mgt",
                _ => "Codeunit Other",
            }
            .to_string()
        };
        let sel = |selector: &str| {
            let (m, form) = resolve_selector(selector, &idx);
            narrow_to_owner(selector, form, m, owner)
        };
        assert_eq!(sel("Cust. Ledger Mgt.Run"), ["a#"]);
        assert_eq!(sel("Codeunit \"Cust. Ledger Mgt\".Run"), ["a#"]);
        assert_eq!(sel("Cust. Ledger Mgt::Run"), sel("Cust. Ledger Mgt.Run"));
        assert!(sel("Nope.Run").is_empty());
        // A bare-name match (form 2) is not narrowed by form 6.
        assert_eq!(sel("Run"), ["a#", "b#"]);
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
    /// workspace's alone. G31: a dependency's exact stable id selects it (it
    /// was not selectable at all).
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
        assert_eq!(
            resolve_selector("bbbb:Codeunit:2#dep", &idx).0.ids,
            vec!["bbbb:Codeunit:2#dep".to_string()]
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

    /// V3 §2.1: the member forms the one display prints, split; a caption is
    /// ignored; a selector with no member part is not a member form.
    #[test]
    fn member_forms_split() {
        let m = split_member("Table \"CDO E-Mail Job\".\"Table No.\"::OnValidate").unwrap();
        assert_eq!(
            (
                m.owner.as_str(),
                m.kind,
                m.member.as_str(),
                m.routine.as_str()
            ),
            ("Table \"CDO E-Mail Job\"", None, "Table No.", "OnValidate")
        );
        let m = split_member("Table T.Amount::OnValidate").unwrap();
        assert_eq!((m.owner.as_str(), m.member.as_str()), ("Table T", "Amount"));
        let m = split_member("Page \"P\".action(Export \"Export \"\"All\"\"\")::OnAction").unwrap();
        assert_eq!(
            (m.owner.as_str(), m.kind.as_deref(), m.member.as_str()),
            ("Page \"P\"", Some("action"), "Export")
        );
        let m = split_member("Page P.action(\"My Act\")::OnAction").unwrap();
        assert_eq!(m.member, "My Act");
        assert!(split_member("Codeunit \"Log Mgt\"::Run").is_none());
        assert!(split_member("Codeunit Log::Note(Text)").is_none());
        assert!(split_member("Table Cust. Ledger Entry::OnInsert").is_none());
        assert!(split_member("OnValidate").is_none());
    }

    /// V3 §2.1 (found by the CDO round trip: 37 routines' selectors named
    /// none): a parameter list whose type text holds commas (`option A,B`,
    /// `Dictionary of [Code[10], Text]`), pasted from the display, selects
    /// its overload; a list one type short does not.
    ///
    /// Discrimination (2026-11-06): the whole-list comparison removed from
    /// `signature_matches` fails at "an option list selects"; restored,
    /// passes.
    #[test]
    fn a_parameter_list_with_commas_in_a_type_selects() {
        use crate::program::model::workspace::ModelParameter;
        let p = |ty: &str, var: bool| ModelParameter {
            index: 0,
            name: "P".into(),
            type_text: ty.into(),
            is_var: var,
            is_record: false,
            table_name: None,
        };
        let params = vec![
            p("Record \"CDO File\"", true),
            p("option \" \",\"Attach as one file\",Merge", false),
            p("Dictionary of [Code[10], Text]", false),
        ];
        let shown = format!("Codeunit X::Do{}", signature_text(&params));
        let (_, wanted) = split_signature(&shown);
        assert!(
            signature_matches(&wanted.unwrap(), &params),
            "an option list selects: {shown}"
        );
        let (_, short) = split_signature(
            "Codeunit X::Do(var Record \"CDO File\", option \" \",\"Attach as one file\",Merge)",
        );
        assert!(!signature_matches(&short.unwrap(), &params));
    }

    /// V3 §2.1: a member form keeps only the named owner, member, kind and
    /// routine.
    #[test]
    fn member_selector_resolves_by_owner_member_and_kind() {
        let row =
            |id: &'static str, owner: &str, member: &'static str, kind: &'static str| MemberRow {
                id,
                owner_display: owner.to_string(),
                member,
                member_kind: kind,
                routine: if kind == "action" {
                    "OnAction"
                } else {
                    "OnValidate"
                },
            };
        let rows = || {
            vec![
                row("a#", "Table Job", "Table No.", "field"),
                row("b#", "Table Job", "Status", "field"),
                row("c#", "Table Other", "Table No.", "field"),
                row("d#", "Page Jobs", "Export", "action"),
            ]
        };
        let sel = |s: &str| resolve_member_selector(s, rows()).unwrap();
        assert_eq!(sel("Table Job.\"Table No.\"::OnValidate"), ["a#"]);
        assert_eq!(sel("Job.\"table no.\"::onvalidate"), ["a#"]);
        assert_eq!(sel("Table \"Job\".Status::OnValidate"), ["b#"]);
        assert_eq!(sel("Page Jobs.action(Export \"X\")::OnAction"), ["d#"]);
        assert!(sel("Page Jobs.dataitem(Export)::OnAction").is_empty());
        assert!(sel("Table Job.Nope::OnValidate").is_empty());
        assert!(resolve_member_selector("Table Job::OnValidate", rows()).is_none());
    }

    /// V3 §2.4, a correctness guard on what a failure offers. Hand-stated
    /// model (by assignment): six `SendMail` overloads and a `Send` in the
    /// workspace, `PostDoc` only in a dependency. `SendMailTwice` offers
    /// `SendMail` (6 overloads, within the ratio: 5 of 13) and not `Send` (9
    /// of 13); `PostDoc` is found only in the dependency, and no unrelated
    /// workspace name is offered; a far name offers none.
    ///
    /// Discrimination (2026-11-06): `NEAREST_RATIO` raised to 1.0 fails at
    /// "a name sharing one word is not near" (`Send` offered); restored,
    /// passes. The dependency-only branch removed (`if !primary_has` →
    /// `if false`) fails at "found only in the dependency"; restored,
    /// passes.
    #[test]
    fn hints_offer_only_near_names_and_name_a_dependency_only_routine() {
        use crate::engine::l5::routine_rollup::RoutineNames;
        use crate::engine::l5::test_support::{empty_resolved, routine};
        use crate::program::model::workspace::{ModelObject, ModelParameter};
        let mut m = empty_resolved();
        m.primary_app = Some(crate::engine::gate::app_attribution::App {
            app_guid: "ws".into(),
            publisher: String::new(),
            name: "Workspace".into(),
            version: String::new(),
        });
        let object = |id: &str, app: &str, name: &str, number: i64| ModelObject {
            id: id.into(),
            app_guid: app.into(),
            object_type: "Codeunit".into(),
            object_number: number,
            object_key: number.to_string(),
            name: name.into(),
            source_table_name: None,
            extends_target_name: None,
            implements_interfaces: None,
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
            run_actions: None,
        };
        m.workspace.objects = vec![
            object("ws/Codeunit/1", "ws", "Send Mail Mgt", 1),
            object("dep/Codeunit/2", "dep", "Posting", 2),
        ];
        let mut rows = Vec::new();
        for k in 0..6u32 {
            let mut r = routine("SendMail", "procedure");
            r.stable_routine_id = format!("ws:SendMail#{k}");
            r.object_id = "ws/Codeunit/1".into();
            r.app_guid = "ws".into();
            r.parameters = (0..=k)
                .map(|i| ModelParameter {
                    index: i,
                    name: format!("P{i}"),
                    type_text: "Text".into(),
                    is_var: false,
                    is_record: false,
                    table_name: None,
                })
                .collect();
            rows.push(r);
        }
        let mut send = routine("Send", "procedure");
        send.stable_routine_id = "ws:Send".into();
        send.object_id = "ws/Codeunit/1".into();
        send.app_guid = "ws".into();
        rows.push(send);
        let mut dep = routine("PostDoc", "procedure");
        dep.stable_routine_id = "dep:PostDoc".into();
        dep.object_id = "dep/Codeunit/2".into();
        dep.app_guid = "dep".into();
        dep.object_number = 2;
        rows.push(dep);
        // Round-2 G40 (n06): three page actions, each an `OnAction` (by
        // assignment), in an order where the nearest action is not first.
        m.workspace
            .objects
            .push(object("ws/Page/3", "ws", "Posted Actions", 3));
        for (k, action) in ["CDOEMailAction", "PrintPDFAction", "SendEMailAction"]
            .iter()
            .enumerate()
        {
            let mut r = routine("OnAction", "trigger");
            r.stable_routine_id = format!("ws:OnAction#{k}");
            r.object_id = "ws/Page/3".into();
            r.object_type = "Page".into();
            r.app_guid = "ws".into();
            r.enclosing_member = Some((*action).into());
            rows.push(r);
        }
        m.workspace.routines = rows;
        let names = RoutineNames::new(&m, &[]);
        let owners = |_: &str| "Codeunit Send Mail Mgt".to_string();
        let h = selector_hints(&names, owners, "SendMailTwice");
        let offered: Vec<&str> = h.nearest.iter().map(|n| n.display.as_str()).collect();
        assert_eq!(
            offered,
            ["Codeunit \"Send Mail Mgt\"::SendMail"],
            "a name sharing one word is not near: {offered:?}"
        );
        assert_eq!(h.nearest[0].overloads, 6);
        let h = selector_hints(&names, owners, "PostDoc");
        assert_eq!(
            h.dependency_only,
            [(
                "dep".to_string(),
                "Codeunit Posting (2)::PostDoc".to_string()
            )]
            .into_iter()
            .collect::<Vec<_>>(),
            "found only in the dependency"
        );
        assert!(h.nearest.is_empty(), "no unrelated workspace name");
        let lines = hint_lines(
            &h,
            &[serde_json::json!({"appId": "dep", "name": "Base App"})],
        );
        assert!(lines[0].starts_with("found only in Base App: Codeunit Posting (2)::PostDoc"));
        let h = selector_hints(&names, owners, "Zebra");
        assert!(h.nearest.is_empty() && h.dependency_only.is_empty());
        assert_eq!(hint_lines(&h, &[])[0], "nearest names: no close match");
        // Round-2 G40 (n06): a member form is near by its member's name,
        // nearest first (`SendEMailActio` → `SendEMailAction`, distance 1).
        let h = selector_hints(
            &names,
            |_: &str| "Page Posted Actions".to_string(),
            "Page \"Posted Actions\".action(SendEMailActio)::OnAction",
        );
        let offered: Vec<&str> = h.nearest.iter().map(|n| n.display.as_str()).collect();
        assert_eq!(
            offered.first().copied(),
            Some("Page \"Posted Actions\".action(SendEMailAction)::OnAction"),
            "the nearest action first: {offered:?}"
        );
        assert_eq!(h.nearest[0].distance, 1, "{offered:?}");
        assert!(
            !offered.iter().any(|d| d.contains("PrintPDFAction")),
            "a far action is not offered: {offered:?}"
        );
    }

    /// Round-2 G40: what tells an ambiguity's candidates apart, a
    /// classification read off the hand-stated candidates.
    #[test]
    fn differs_in_names_what_tells_candidates_apart() {
        let c = |display: &str, types: &[&str]| AmbiguousCandidate {
            display: display.to_string(),
            at: String::new(),
            selector: String::new(),
            types: types.iter().map(|t| t.to_string()).collect(),
            stable_id: String::new(),
        };
        let nine = ["Integer"; 9];
        let mut ten = nine.to_vec();
        ten.push("Boolean");
        assert_eq!(
            differs_in(&[c("C::P(…)", &ten), c("C::P(…)", &nine)]).as_deref(),
            Some("a trailing Boolean")
        );
        assert_eq!(
            differs_in(&[c("C::P(a)", &["Integer"]), c("C::P(b)", &["Text"])]).as_deref(),
            Some("parameter 1 (Integer or Text)")
        );
        assert_eq!(
            differs_in(&[c("A::P(a)", &["Integer"]), c("B::P(a)", &["Integer"])]).as_deref(),
            Some("the object")
        );
        assert_eq!(
            differs_in(&[
                c("C::P", &["A"]),
                c("C::P", &["A", "B"]),
                c("C::P", &["A", "B", "C"])
            ])
            .as_deref(),
            Some("the number of parameters (1, 2, 3)")
        );
        assert_eq!(
            differs_in(&[
                c("C::P", &["A"]),
                c("C::P", &["B", "C"]),
                c("C::P", &["A", "B", "C"])
            ])
            .as_deref(),
            Some("the parameter lists")
        );
        assert_eq!(differs_in(&[c("C::P", &["A"])]), None);
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
