//! **The impact answer** (agent-tasks design §8.5): human text or
//! `al-sem.impact/1` JSON, from the comparison tuple and the parts. The CLI
//! and the MCP tool call this one function on the same parts.
//!
//! Honesty rules (§8.6), each a line of the human text:
//! - the input accounting comes first, always: every line the engine could
//!   not map is counted (and its first items named) before any conclusion;
//! - without a base: "current effects; no before/after", and nothing is
//!   called added or deleted;
//! - the reverse dependents are static may-paths, with the walk's coverage.
//!
//! Size: the human default shows the first `max_roots` routines (default
//! 20), ranked "reached from a user or a job first, then by ancestors", each
//! in a few lines with the first item of each list; every count stays whole
//! and the fold says how many it hides. `detail` shows more of each list.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::compare::Comparison;
use super::run::TestsOutcome;
use crate::engine::l5::routine_rollup::{plural, thousands};

/// The schema name and version.
pub const SCHEMA: &str = "al-sem.impact";
pub const VERSION: u32 = 1;

/// How to render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderOpts {
    pub json: bool,
    /// Routines shown in the human text (the JSON holds every one).
    pub max_roots: usize,
    pub detail: bool,
    /// The human text's size budget in bytes: fewer than `max_roots`
    /// routines are shown when their blocks would pass it, and the fold says
    /// so. `None` (a `--max-roots` or `--detail` the caller asked for): no
    /// budget.
    pub budget: Option<usize>,
    /// `--surface` (output validation §6.1).
    pub surface: crate::engine::follow_up::Surface,
    /// `--single-app` (the header's universe).
    pub single_app: bool,
}

impl Default for RenderOpts {
    fn default() -> Self {
        RenderOpts::new(false, None, false)
    }
}

impl RenderOpts {
    /// The options of a request: `max_roots` as asked (default
    /// [`DEFAULT_MAX_ROOTS`]); the size budget [`HUMAN_BUDGET`] unless the
    /// caller asked for a routine count or for everything (`detail`).
    #[must_use]
    pub fn new(json: bool, max_roots: Option<usize>, detail: bool) -> RenderOpts {
        RenderOpts {
            json,
            max_roots: max_roots.unwrap_or(DEFAULT_MAX_ROOTS),
            detail,
            budget: (max_roots.is_none() && !detail).then_some(HUMAN_BUDGET),
            surface: crate::engine::follow_up::Surface::Cli,
            single_app: false,
        }
    }

    /// `--surface` (output validation §6.1).
    #[must_use]
    pub fn with_surface(mut self, surface: crate::engine::follow_up::Surface) -> RenderOpts {
        self.surface = surface;
        self
    }

    /// `--single-app`.
    #[must_use]
    pub fn with_single_app(mut self, single_app: bool) -> RenderOpts {
        self.single_app = single_app;
        self
    }

    /// `input` in the surface's words.
    fn input(&self, input: crate::engine::follow_up::Input) -> &'static str {
        input.on(self.surface)
    }
}

/// `--max-roots`' default.
pub const DEFAULT_MAX_ROOTS: usize = 20;

/// The default human answer's size budget: under the MCP tool's 40 KiB
/// inline cap (`mcp::tools::MCP_DEFAULT_INLINE_BYTES`) with room to spare,
/// so the default answer is returned inline.
pub const HUMAN_BUDGET: usize = 38 * 1024;

/// The rendered answer: stdout, the exit code, and the summary an MCP
/// answer carries inline when the text goes to a file.
pub struct Rendered {
    pub text: String,
    pub exit: u8,
    /// The envelope summary of a file answer (output validation §5.2): the
    /// answer, the outcome, the accounting and the first five routines with
    /// the tests line (`text`), every changed routine as an index row.
    pub summary: Value,
    /// What the exit code means (§6.3): `answered`, `no-routine-changed`
    /// (complete accounting, no routine) or `input-not-mapped`.
    pub outcome: &'static str,
}

/// The impact outcome (§6.3, §1 F24): no routine with exit 0 is
/// `no-routine-changed` (the accounting is complete: exit 2 when input did
/// not map), with exit 2 `input-not-mapped`.
fn impact_outcome(roots: usize, exit: u8) -> &'static str {
    match (roots, exit) {
        (0, 0) => "no-routine-changed",
        (0, _) => "input-not-mapped",
        _ => "answered",
    }
}

/// The summary (§5.2) around `text`, with every routine as an index row.
fn impact_summary(
    header: &crate::engine::result_header::Header,
    outcome: &str,
    text: String,
    roots: &[Value],
) -> Value {
    let row = |r: &Value| {
        json!({"routine": named(r), "stableId": r["stableId"], "status": r["status"],
               "testStatus": r["tests"]["status"].as_str()
                   .map(crate::engine::vocab::test_status),
               // Round-2 r2 §2.1 (additive): the reach is classified by
               // relation (W2); its populations are in the routine's JSON.
               "relationRecorded": r["up"]["relationRecorded"].as_bool().unwrap_or(false)})
    };
    crate::engine::result_index::with_index(
        json!({
            "kind": "impact",
            "answer": header.answer,
            "analysis": header.analysis.human(),
            "outcome": outcome,
            "routinesTotal": roots.len(),
            "routines": roots.iter().take(5).map(row).collect::<Vec<_>>(),
            "text": text,
        }),
        "routines",
        "stableId",
        roots.iter().map(row).collect(),
    )
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn u(v: &Value, k: &str) -> usize {
    v.get(k).and_then(Value::as_u64).unwrap_or(0) as usize
}

/// A routine's display (V3 §2.1: the one display prints an overloaded
/// name's parameter types, so two overloads never print alike; the short
/// `handle` stays in JSON only).
fn named(v: &Value) -> String {
    s(v, "display").to_string()
}

fn short(c: &str) -> String {
    c.chars().take(10).collect()
}

/// The comparison line.
fn comparison_line(cmp: &Comparison, files: usize) -> String {
    let head = cmp.head_commit.as_deref().map(short);
    let run = match &cmp.inputs_stamp {
        Some(st) => format!("revision {st}"),
        None => "one-shot run (no resident revision)".to_string(),
    };
    let what = match cmp.source {
        "base" => format!(
            "impact of the working tree against comparison base {} (the merge base of {} {} and HEAD {}); {}",
            cmp.comparison_base
                .as_deref()
                .map(short)
                .unwrap_or_default(),
            cmp.requested_ref.as_deref().unwrap_or(""),
            cmp.resolved_commit
                .as_deref()
                .map(short)
                .unwrap_or_default(),
            head.unwrap_or_default(),
            plural(files, "file", "files")
        ),
        "diff" => format!(
            "impact of a diff ({}); HEAD {}",
            plural(files, "file", "files"),
            head.as_deref().unwrap_or("not a git repository")
        ),
        "files" => "impact of the named files (every routine in them)".into(),
        _ => "impact of the named routines".into(),
    };
    let hash = cmp
        .diff_hash
        .as_deref()
        .map(|h| format!("; diff {}", &h[..h.len().min(19)]))
        .unwrap_or_default();
    format!("{what}{hash}; {run}")
}

/// The accounting's bucket clause (`routine bodies 10,880, …`).
/// What the changed object-header lines changed (output validation §9.4,
/// F62): `object headers: 25 files, line 1 only — a byte-order mark removed
/// (no declaration changed)`. `None` when no file changed a header line.
fn object_header_line(acc: &Value) -> Option<String> {
    let changes: Vec<&Value> = acc["files"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|f| &f["objectHeaderChange"])
        .filter(|h| h.is_object())
        .collect();
    if changes.is_empty() {
        return None;
    }
    let lines: std::collections::BTreeSet<i64> = changes
        .iter()
        .flat_map(|h| h["lines"].as_array().into_iter().flatten())
        .filter_map(Value::as_i64)
        .collect();
    let at = match (lines.first(), lines.last()) {
        (Some(a), Some(b)) if a == b => format!("line {a} only"),
        (Some(a), Some(b)) if (b - a + 1) as usize == lines.len() => format!("lines {a}-{b}"),
        _ => format!(
            "lines {}",
            lines
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    let words = |k: &str| match k {
        "byte-order-mark-removed" => "a byte-order mark removed",
        "byte-order-mark-added" => "a byte-order mark added",
        "whitespace" => "whitespace only, outside quoted names and values",
        _ => "the declaration text changed",
    };
    let mut by: BTreeMap<&str, usize> = BTreeMap::new();
    for h in &changes {
        *by.entry(h["kind"].as_str().unwrap_or("text")).or_default() += 1;
    }
    let unchanged = !by.contains_key("text");
    let what = if by.len() == 1 {
        words(by.keys().next().expect("one kind")).to_string()
    } else {
        by.iter()
            .map(|(k, n)| format!("{} in {}", words(k), plural(*n, "file", "files")))
            .collect::<Vec<_>>()
            .join("; ")
    };
    Some(format!(
        "  object headers: {}, {at} — {what}{}",
        plural(changes.len(), "file", "files"),
        if unchanged {
            " (no declaration changed)"
        } else {
            ""
        }
    ))
}

fn buckets_clause(t: &Value) -> String {
    let mut parts = vec![
        format!("routine bodies {}", thousands(u(t, "routineBody"))),
        format!("signatures {}", thousands(u(t, "routineSignature"))),
    ];
    let decl = &t["objectDeclaration"];
    let kinds: Vec<String> = decl["byKind"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, n)| n.as_u64().unwrap_or(0) > 0)
        .map(|(k, n)| {
            format!(
                "{} {}",
                kind_human(k),
                thousands(n.as_u64().unwrap_or(0) as usize)
            )
        })
        .collect();
    parts.push(format!(
        "object declarations {}{}",
        thousands(u(decl, "total")),
        if kinds.is_empty() {
            String::new()
        } else {
            format!(" ({})", kinds.join(", "))
        }
    ));
    parts.push(format!(
        "outside objects {}",
        thousands(u(t, "outsideObject"))
    ));
    if u(t, "removedRoutineText") > 0 {
        parts.push(format!(
            "removed routine text {} (not called deleted: that needs a base)",
            thousands(u(t, "removedRoutineText"))
        ));
    }
    if u(t, "routineNotInModel") > 0 {
        parts.push(format!(
            "routine text the model has no routine for {}",
            thousands(u(t, "routineNotInModel"))
        ));
    }
    let un = &t["unmapped"];
    let reasons: Vec<String> = un["byReason"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, n)| format!("{} {}", unmapped_human(k), n))
        .collect();
    parts.push(format!(
        "unmapped {}{}",
        thousands(u(un, "total")),
        if reasons.is_empty() {
            String::new()
        } else {
            format!(" ({})", reasons.join(", "))
        }
    ));
    parts.join(", ")
}

fn kind_human(k: &str) -> &str {
    match k {
        "field" => "fields",
        "key" => "keys",
        "fieldgroup" => "field groups",
        "global-var" => "global vars",
        "property" => "properties",
        "action" => "actions",
        "trigger-decl" => "trigger declarations",
        "enum-value" => "enum values",
        "object-header" => "object headers",
        "comment-or-blank" => "comments or blank",
        other => other,
    }
}

fn unmapped_human(k: &str) -> &str {
    match k {
        "not-in-workspace" => "not in the workspace",
        "ambiguous-path" => "path names two workspace files",
        "not-al" => "not AL",
        "deleted-file" => "deleted files, need a base",
        "diff-parse-error" => "hunk header rejected",
        "past-end-of-file" => "past the end of the analyzed file",
        other => other,
    }
}

/// A hunk's or a file's buckets, short (`field 10, key 2`).
fn buckets_short(b: &Value) -> String {
    let mut parts = Vec::new();
    for (k, label) in [("routineBody", "body"), ("routineSignature", "signature")] {
        if u(b, k) > 0 {
            parts.push(format!("{label} {}", u(b, k)));
        }
    }
    for (k, n) in b["objectDeclaration"]["byKind"]
        .as_object()
        .into_iter()
        .flatten()
    {
        let n = n.as_u64().unwrap_or(0);
        if n > 0 {
            parts.push(format!("{k} {n}"));
        }
    }
    for (k, label) in [
        ("outsideObject", "outside objects"),
        ("removedRoutineText", "removed routine text"),
        ("routineNotInModel", "routine not in model"),
    ] {
        if u(b, k) > 0 {
            parts.push(format!("{label} {}", u(b, k)));
        }
    }
    if u(&b["unmapped"], "total") > 0 {
        parts.push(format!("unmapped {}", u(&b["unmapped"], "total")));
    }
    parts.join(", ")
}

/// The effects clause of one root (its `RollupTotals` JSON).
fn effects_clause(e: &Value) -> String {
    if e.is_null() {
        return "effects: not computed for this routine".into();
    }
    let w = &e["mayWriteTables"];
    let mut parts = Vec::new();
    let writes = u(w, "count");
    let unresolved = u(e, "unresolvedTableWrites");
    if writes == 0 && unresolved == 0 {
        // §8.1 negative certainty: the routine's summary does not carry its
        // cone's completeness here, so the absence states its search.
        parts.push(format!(
            "writes that may persist: {}",
            crate::engine::vocab::none_observed(
                "the cone through every resolved call",
                "calls not resolved"
            )
        ));
    } else {
        let mut x = format!(
            "may write {} ({} owned by this app, {} by dependencies)",
            plural(writes, "table", "tables"),
            u(w, "ownedByApp"),
            u(w, "byDependencies")
        );
        if unresolved > 0 {
            x.push_str(&format!(
                " and {} on a table not resolved",
                plural(unresolved, "write", "writes")
            ));
        }
        parts.push(x);
    }
    let cd = u(e, "callerDependentTables");
    if cd > 0 {
        parts.push(format!(
            "{} whose temporary state the caller decides",
            plural(cd, "table", "tables")
        ));
    }
    let c = &e["commits"];
    let at_least = if c["atLeast"] == json!(true) {
        "at least "
    } else {
        ""
    };
    // §8.1 negative certainty for an absence (the JSON keeps its answer).
    let answer = match s(c, "answer") {
        "none" => crate::engine::vocab::NO_EVERY_CANDIDATE_DECIDED.to_string(),
        "no observed" => crate::engine::vocab::none_observed(
            "the cone through every resolved call",
            "calls not resolved",
        ),
        a => a.to_string(),
    };
    parts.push(format!(
        "commits: {answer} ({at_least}{})",
        plural(u(c, "ownSites"), "own commit site", "own commit sites")
    ));
    let kinds = &e["kinds"];
    for (k, label) in [
        ("ui", "UI"),
        ("errors", "errors"),
        ("http", "HTTP"),
        ("backgroundTasks", "background tasks"),
    ] {
        let n = u(kinds, k);
        if n > 0 {
            parts.push(format!("{label} {}", thousands(n)));
        }
    }
    let ev = u(&e["events"], "distinct");
    if ev > 0 {
        parts.push(plural(ev, "event", "events"));
    }
    // Round-2 n09: the effects' completeness, as explain states it.
    if let Some(a) = effects_analysis(e) {
        parts.push(a.human());
    }
    format!("effects: {}", parts.join("; "))
}

/// A root's effects completeness (`effects.analysis`, from its digest
/// entry), or `None` when complete or not stated.
fn effects_analysis(e: &Value) -> Option<crate::engine::result_header::Analysis> {
    use crate::engine::result_header::Analysis;
    let a = &e["analysis"];
    let reasons: Vec<String> = a["reasons"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r.as_str().map(str::to_string))
        .collect();
    match s(a, "status") {
        "partial" => Some(Analysis::Partial(reasons)),
        "notAnalyzed" => Some(Analysis::NotAnalyzed(reasons.join("; "))),
        _ => None,
    }
}

/// An event that raises the routine itself, with its tier and its relation
/// (an event dispatch is code; round-2 r2 §5: impact mixes relations, so
/// every item prints both axes): `[via-dependency, code] Table Customer
/// (18)::OnAfterDeleteEvent (1 hop)`.
fn node_short(n: &Value) -> String {
    format!(
        "[{}, {}] {} ({})",
        crate::engine::vocab::tier_key(s(n, "tier")),
        crate::engine::vocab::RELATION_CODE,
        s(n, "display"),
        plural(u(n, "hops"), "hop", "hops")
    )
}

/// A classified witness (round-2 r2 §2.1, W2): `[<tier>, <relation>]`, its
/// display (a page with the action an action-run site stands for), what
/// kind of entry it is, its hops, and whether a candidate's path also needs
/// UI navigation: `[via-dependency, UI navigation] Page "Sales Invoice"
/// (page action, 3 hops)`.
fn witness_text(w: &Value) -> String {
    let mut what = s(w, "display").to_string();
    if let Some(a) = w["action"].as_str() {
        what = format!("{what} action {}", al_name(a));
    }
    let kind = w["entry"].as_str().or(w["kind"].as_str());
    let mut tail = match kind {
        Some(k) if w["kind"].is_null() => format!("{k}, "),
        _ => String::new(),
    };
    tail.push_str(&plural(u(w, "hops"), "hop", "hops"));
    if w["needsNavigation"] == json!(true) {
        tail.push_str("; the path also needs UI navigation");
    }
    format!("[{}, {}] {what} ({tail})", s(w, "tier"), s(w, "relation"))
}

/// A name as AL writes it: quoted only when it needs quotes.
fn al_name(n: &str) -> String {
    if !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        n.to_string()
    } else {
        format!("\"{n}\"")
    }
}

/// The direct callers and event publishers a routine lists with `detail`
/// (output validation §7.6, F46: an evidence budget, about 15 lines per
/// routine, not ten items per list).
const DETAIL_ITEMS: usize = 3;

/// The share of the program's entries at which a population's list folds
/// (§3.2: start at 50%).
const FOLD_SHARE_PERCENT: usize = 50;

/// The share of the program's entries at which a population is
/// `program-wide` and its counts by kind fold too (round-2 r2 §2.1: 90%, not
/// equality, since the changed routine is left out and the counts move with
/// every correctness fix).
const WIDE_SHARE_PERCENT: usize = 90;

/// A population's share of the program's canonical entries, in percent:
/// `None` for a limited walk (it claims no share) or a walk with no
/// classified counts. `key` is a `programShare` key (`code`, `all`, …).
pub(crate) fn population_share(up: &Value, key: &str) -> Option<usize> {
    if !up["limited"].is_null() {
        return None;
    }
    let sh = &up["programShare"][key];
    let program = sh["program"].as_u64()?;
    let reached = sh["reached"].as_u64()?;
    (reached * 100)
        .checked_div(program)
        .map(|p| p.min(100) as usize)
}

/// The share clause of a population line: `program-wide` from 90% (its
/// counts by kind folded), `broad reach` from 50%, the share below, or no
/// entry at all; a limited walk states its limit instead of any share.
fn share_clause(up: &Value, key: &str, detail: bool, detail_in: &str) -> (String, bool) {
    if let Some(l) = up["limited"].as_str() {
        return (format!("; analysis partial (limit: {l})"), false);
    }
    let sh = &up["programShare"][key];
    let (n, total) = (u(sh, "reached"), u(sh, "program"));
    match population_share(up, key) {
        Some(p) if p >= WIDE_SHARE_PERCENT => (
            format!(
                "; program-wide: {p}% of the program's entries ({} of {}){}",
                thousands(n),
                thousands(total),
                if detail {
                    String::new()
                } else {
                    format!("; counts by kind folded ({detail_in} prints them)")
                }
            ),
            !detail,
        ),
        Some(p) if p >= FOLD_SHARE_PERCENT => (
            format!(
                "; broad reach ({p}% of the program's entries, {} of {}{})",
                thousands(n),
                thousands(total),
                if detail { "" } else { "; list folded" }
            ),
            false,
        ),
        Some(p) if n > 0 => (
            format!(
                "; {} of the program's entries ({} of {})",
                if p == 0 {
                    "under 1%".to_string()
                } else {
                    format!("{p}%")
                },
                thousands(n),
                thousands(total)
            ),
            false,
        ),
        _ => ("; no entry among them".to_string(), false),
    }
}

/// A population's entries by kind and by context, when it has any.
fn kind_lines(up: &Value, key: &str, indent: &str) -> Vec<String> {
    let mut out = Vec::new();
    let line = |m: &Value, plural_of: &dyn Fn(&str) -> String| -> Option<String> {
        let o = m.as_object()?;
        // Print order: the entry kinds' (a user or a job first), not the
        // JSON object's alphabetical one.
        let rank = |k: &str| {
            crate::program::impact_graph::reverse::EntryKind::ALL
                .iter()
                .position(|x| crate::engine::vocab::entry_kind(*x) == k)
                .unwrap_or(usize::MAX)
        };
        let mut rows: Vec<(&String, &Value)> = o.iter().collect();
        rows.sort_by_key(|(k, _)| rank(k));
        (!o.is_empty()).then(|| {
            rows.iter()
                .map(|(k, n)| {
                    format!(
                        "{} {}",
                        plural_of(k),
                        thousands(n.as_u64().unwrap_or(0) as usize)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
    };
    if let Some(l) = line(&up["entriesByKind"][key], &|k| {
        crate::engine::vocab::entry_kind_plural(k)
    }) {
        out.push(format!("{indent}  entries by kind: {l}"));
    }
    if let Some(l) = line(&up["entriesByContext"][key], &|k| k.to_string()) {
        out.push(format!(
            "{indent}  by context (contexts may overlap; not summed): {l}"
        ));
    }
    out
}

/// The ancestors of a population key in `byRelation.ancestors`.
fn population_count(up: &Value, key: &str) -> usize {
    let a = &up["byRelation"]["ancestors"];
    match key {
        "code" | "uiNavigation" => u(&a[key], "local") + u(&a[key], "viaDependency"),
        "overConfiguration" => {
            let c = &a["candidates"]["overConfiguration"];
            u(c, "code") + u(c, "uiNavigation")
        }
        k => u(&a["candidates"], k),
    }
}

/// The lines of a reach beyond the direct code callers (output validation
/// V2, §3.2, F12; round-2 r2 §2.1 Output, W2): one line per population, code
/// first with its nearest witness whatever its size, then UI navigation,
/// then the candidates over configuration, then the candidates whose
/// relation is not established (over a rule edge, over a hub). Each
/// established population names its relation and its tiers and its share of
/// the program's entries (`program-wide` from 90%, counts by kind folded);
/// when no single population reaches 90% but their union does, one line says
/// so. `external:` belongs to the code population. With `detail`, each
/// population's first entry of each kind. The hubs on the paths close it
/// (`hubs`: the answer's one hubs line). `direct_both`: the routine has
/// direct code callers and raising events both (the code population counts
/// both ways in once).
fn reach_lines(
    up: &Value,
    detail: bool,
    indent: &str,
    surface: crate::engine::follow_up::Surface,
    hubs: bool,
    direct_both: bool,
) -> Vec<String> {
    let detail_in = crate::engine::follow_up::Input::Detail.on(surface);
    let mut out = Vec::new();
    let a = &up["byRelation"]["ancestors"];
    let limited = up["limited"].as_str();
    let candidates = population_count(up, "overConfiguration")
        + population_count(up, "overRule")
        + population_count(up, "overHub");
    let firsts = |out: &mut Vec<String>, key: &str| {
        for w in up["firstByKind"][key].as_array().into_iter().flatten() {
            out.push(format!(
                "{indent}    first {}: {}",
                s(w, "kind"),
                witness_text(w)
            ));
        }
    };
    let nearest = |out: &mut Vec<String>, key: &str| {
        let w = &up["nearest"][key];
        if !w.is_null() {
            let label = if w["entry"].is_null() {
                "nearest"
            } else {
                "nearest entry"
            };
            out.push(format!("{indent}  {label}: {}", witness_text(w)));
        }
    };
    let mut wide_any = false;
    for (key, words) in [("code", "code"), ("uiNavigation", "UI navigation")] {
        let n = population_count(up, key);
        if n == 0 {
            if key == "code" {
                out.push(format!(
                    "{indent}reached by code: none — no established code-path witness in the \
                     analyzed graph{}",
                    match limited {
                        Some(l) => format!("; analysis partial (limit: {l})"),
                        None if candidates > 0 => {
                            " (the candidates below are not decided)".to_string()
                        }
                        None => " (every candidate decided)".to_string(),
                    }
                ));
            }
            continue;
        }
        let (share, folded) = share_clause(up, key, detail, detail_in);
        wide_any |= population_share(up, key).is_some_and(|p| p >= WIDE_SHARE_PERCENT);
        out.push(format!(
            "{indent}reached by {words}: {}{} — local {}, via-dependency {}{share}",
            plural(n, "ancestor", "ancestors"),
            if key == "code" && direct_both {
                ", through its direct callers and its publisher together"
            } else {
                ""
            },
            thousands(u(&a[key], "local")),
            thousands(u(&a[key], "viaDependency")),
        ));
        nearest(&mut out, key);
        if !folded {
            out.extend(kind_lines(up, key, indent));
        }
        if detail {
            firsts(&mut out, key);
        }
        if key == "code" {
            // `external:` prints for the code population only.
            let ext: Vec<String> = up["externalCode"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|e| format!("{} {}", s(e, "why"), thousands(u(e, "count"))))
                .collect();
            if !ext.is_empty() {
                out.push(format!(
                    "{indent}  external: apps outside the program may call routines of its code \
                     population (by reason, routines): {}",
                    ext.join(", ")
                ));
            }
        }
    }
    let conf = population_count(up, "overConfiguration");
    if conf > 0 {
        let c = &a["candidates"]["overConfiguration"];
        out.push(format!(
            "{indent}candidates over configuration (depends on configuration data not read): {} \
             (code {}, UI navigation {})",
            plural(conf, "ancestor", "ancestors"),
            thousands(u(c, "code")),
            thousands(u(c, "uiNavigation")),
        ));
        nearest(&mut out, "overConfiguration");
        if detail {
            out.extend(kind_lines(up, "overConfiguration", indent));
            firsts(&mut out, "overConfiguration");
        }
    }
    let (rule, hub) = (
        population_count(up, "overRule"),
        population_count(up, "overHub"),
    );
    if rule + hub > 0 {
        out.push(format!(
            "{indent}candidates (relation not established): over a rule edge {}, over a hub {}",
            thousands(rule),
            plural(hub, "ancestor", "ancestors"),
        ));
        if detail {
            for key in ["overRule", "overHub"] {
                nearest(&mut out, key);
            }
        }
    }
    // §R2.4 landing changes 1-2: the totals say what they include that is
    // not one entry per action or per run.
    let notes = &up["entryNotes"];
    let pages = u(&notes["pagesWithUnnamedActions"], "reached");
    let bg = u(&notes["backgroundTargetsWithoutOnRun"], "reached");
    if pages + bg > 0 {
        let mut parts = Vec::new();
        if pages > 0 {
            parts.push(format!(
                "{} counted once each (their actions are not named)",
                plural(pages, "page", "pages")
            ));
        }
        if bg > 0 {
            parts.push(format!(
                "{} with no OnRun found (counted apart)",
                plural(bg, "background target", "background targets")
            ));
        }
        out.push(format!(
            "{indent}entry totals include {}",
            parts.join(", and ")
        ));
    }
    // Round-2 r2 §2.1 Rules: no single population reaches the threshold,
    // but their union does.
    if !wide_any
        && let Some(p) = population_share(up, "all")
        && p >= WIDE_SHARE_PERCENT
    {
        let sh = &up["programShare"]["all"];
        out.push(format!(
            "{indent}together these populations reach {p}% of the program's entries ({} of {})",
            thousands(u(sh, "reached")),
            thousands(u(sh, "program"))
        ));
    }
    if !hubs {
        return out;
    }
    let hubs: Vec<String> = up["hubs"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|h| {
            let at = s(h, "at");
            crate::engine::vocab::hub(s(h, "kind"), None, (!at.is_empty()).then_some(at))
        })
        .collect();
    if !hubs.is_empty() {
        // §7.6: the hubs stay three, counted, with `detail` too (the JSON
        // lists every one).
        let cap = 3;
        out.push(format!(
            "{indent}  hubs on these paths: {}{}",
            hubs.iter()
                .take(cap)
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
            if hubs.len() > cap {
                format!(
                    ", … +{} ({} lists them)",
                    hubs.len() - cap,
                    crate::engine::follow_up::Input::FormatJson.on(surface)
                )
            } else {
                String::new()
            }
        ));
    }
    out
}

/// Per root: the best status over the test parts, and its counts.
fn root_tests(t: &TestsOutcome, id: &str) -> Value {
    let rank = |st: &str| match st {
        "reached" => 0,
        "no-known-path" => 1,
        "analysis-incomplete" => 2,
        "not-loaded" => 3,
        "not-in-projection" => 4,
        _ => 5,
    };
    let per: Vec<&Value> = t
        .parts
        .iter()
        .filter_map(|p| p.result.as_ref().ok())
        .filter_map(|v| {
            v["roots"]
                .as_array()
                .and_then(|rs| rs.iter().find(|r| s(r, "stableId") == id))
        })
        .collect();
    let status = per
        .iter()
        .min_by_key(|r| rank(s(r, "status")))
        .map(|r| s(r, "status").to_string())
        .unwrap_or_else(|| "not-analyzed".into());
    let sum = |k: &str| -> usize { per.iter().map(|r| u(&r["tests"], k)).sum() };
    let nearest = per
        .iter()
        .filter(|r| r.get("nearest").is_some())
        .min_by_key(|r| (s(r, "nearestTier") != "local", u(r, "nearestHops")))
        .map(|r| json!({"test": r["nearest"], "tier": r["nearestTier"], "hops": r["nearestHops"]}));
    json!({
        "status": status,
        "local": sum("local"),
        "throughDependency": sum("throughDependency"),
        "onlyOpen": sum("onlyOpen"),
        "nearest": nearest,
        "nearestTestedCaller": t.nearest.get(id).cloned(),
    })
}

fn tests_clause(rt: &Value) -> String {
    let st = s(rt, "status");
    let total = u(rt, "local") + u(rt, "throughDependency") + u(rt, "onlyOpen");
    match st {
        "reached" if total == 0 || rt["nearest"].is_null() => {
            // Over many changed routines one walk labels each test with its
            // NEAREST changed routine (test selection's own rule), so this
            // routine's tests are counted under another one.
            "tests: tests with a modeled path (each test is counted under its nearest changed routine; this one's are counted under others)".into()
        }
        "reached" => {
            let n = &rt["nearest"];
            format!(
                "tests: tests with a modeled path: {} ({} by a local path); nearest {} ({}, {})",
                thousands(total),
                thousands(u(rt, "local")),
                s(n, "test"),
                match s(n, "tier") {
                    "" => "local",
                    t => crate::engine::vocab::tier_key(t),
                },
                plural(u(n, "hops"), "hop", "hops")
            )
        }
        "not-in-projection" | "not-loaded" => {
            let st = crate::engine::vocab::test_status(st);
            let how = if st == "not loaded" {
                "the test universe has no body for it"
            } else {
                "no test universe has it"
            };
            match rt["nearestTestedCaller"].as_object() {
                Some(_) => {
                    let n = &rt["nearestTestedCaller"];
                    format!(
                        "tests: {st} ({how}, so no test path to it is known); nearest tested caller: {} ({} up), reached by {} ({})",
                        s(n, "display"),
                        plural(u(n, "hopsUp"), "hop", "hops"),
                        s(n, "nearestTest"),
                        plural(u(n, "nearestTestHops"), "hop", "hops")
                    )
                }
                None => format!(
                    "tests: {st} ({how}, so no test path to it is known); no nearer caller a test reaches was found"
                ),
            }
        }
        "no-known-path" => "tests: no modeled path (its cone is complete)".into(),
        "analysis-incomplete" => {
            "tests: analysis partial — no modeled path found, but its cone is incomplete (not proof that none does)"
                .into()
        }
        _ => "tests: not analyzed".into(),
    }
}

/// The exit rule (output validation F24): an answer with no changed routine
/// is a complete answer (exit 0) unless the input itself could not be
/// resolved: a `--routine` selector or a `--file` that matched nothing or
/// more than one thing, a diff file that names two workspace files, or a
/// diff that does not parse. Only then is there no answer (exit 2).
fn input_unresolved(diags: &[Value]) -> bool {
    diags.iter().any(|d| {
        matches!(
            s(d, "kind"),
            "selector-unmatched"
                | "selector-ambiguous"
                | "file-unmatched"
                | "file-ambiguous"
                | "diff-file-ambiguous"
                | "diff-parse-error"
        )
    })
}

/// The ranking key of a root (§8.4 step 5): reached from a user or a job
/// first, then by ancestors, then by writes and commits, then by name.
fn rank_key(
    r: &Value,
) -> (
    bool,
    std::cmp::Reverse<usize>,
    std::cmp::Reverse<usize>,
    u8,
    String,
) {
    let up = &r["up"];
    // A root that is an entry itself (round-2 r2 §2.1: no longer listed
    // among the entries it reaches) still ranks as reached from a user.
    let user = u(up, "userOrJobEntries") > 0 || up["rootUserOrJob"] == json!(true);
    let commits = match s(&r["effects"]["commits"], "answer") {
        "yes" => 0,
        "may" => 1,
        _ => 2,
    };
    (
        !user,
        std::cmp::Reverse(u(&up["ancestors"], "total")),
        std::cmp::Reverse(u(&r["effects"]["mayWriteTables"], "count")),
        commits,
        s(r, "display").to_string(),
    )
}

/// The sites one count change lists by default (all of them with
/// `detail`); the clause's own counts (`17 → 33 sites`) stay whole.
const SITES_SHOWN: usize = 2;

/// A count change's site list: the first [`SITES_SHOWN`] (every one with
/// `detail`), then how many it hides.
fn sites_list(sites: &[Value], detail: bool) -> String {
    let all: Vec<&str> = sites.iter().filter_map(Value::as_str).collect();
    let cap = if detail { usize::MAX } else { SITES_SHOWN };
    let mut s = all.iter().take(cap).copied().collect::<Vec<_>>().join(", ");
    if all.len() > cap {
        s.push_str(&format!(", … +{}", all.len() - cap));
    }
    s
}

/// The changes of one root's delta block, own first, as human clauses.
fn delta_clauses(d: &Value, detail: bool) -> Vec<String> {
    ["own", "cone"]
        .iter()
        .flat_map(|k| d[*k].as_array().cloned().unwrap_or_default())
        .map(|c| {
            let mut s = match s(&c, "change") {
                "added" => format!("+ {}", s(&c, "label")),
                "removed" => format!("- {}", s(&c, "label")),
                "no-longer-observed" => format!(
                    "- {}: no longer observed ({})",
                    s(&c, "label"),
                    s(&c, "reason")
                ),
                _ => {
                    let which = if c["sites"] == "ambiguous" {
                        " (which site: ambiguous)".to_string()
                    } else if let Some(a) = c["addedSites"].as_array() {
                        format!(" (added at {})", sites_list(a, detail))
                    } else if let Some(r) = c["removedSites"].as_array() {
                        format!(" (removed: base {})", sites_list(r, detail))
                    } else {
                        String::new()
                    };
                    format!(
                        "{}: {} → {} sites{which}",
                        s(&c, "label"),
                        c["base"],
                        c["new"]
                    )
                }
            };
            if c["dependencyReaching"] == json!(true) && !d["unreliable"].is_null() {
                s.push_str(" [unreliable: the dependency declaration changed]");
            }
            s
        })
        .collect()
}

/// Whether a root's delta block holds a change.
fn delta_changed(d: &Value) -> bool {
    ["own", "cone"]
        .iter()
        .any(|k| d[*k].as_array().is_some_and(|a| !a.is_empty()))
}

/// The base line's clause (`base analyzed under …`).
fn base_clause(b: &Value) -> String {
    let pk = b["environment"]["packages"].as_array().map_or(0, Vec::len);
    let same = if b["packagesRead"]["same"] == json!(true) {
        "the same files on both sides"
    } else {
        "the two sides read different package files"
    };
    let cache = match s(&b["cache"], "status") {
        "hit" => "cached base".to_string(),
        "off" => "built (cache off)".to_string(),
        st => format!("built (cache {st})"),
    };
    let deps = b["dependencyDeclarationChanged"]
        .as_array()
        .filter(|a| !a.is_empty())
        .map(|a| {
            format!(
                "; unreliable where a change reaches dependency code: the dependency declaration changed ({})",
                a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("; ")
            )
        })
        .unwrap_or_default();
    format!(
        "base {} analyzed under the current dependency environment (packages: {pk}, {same}; {}); {cache}{deps}",
        short(s(b, "commit")),
        s(&b["environment"], "packagesDigest")
            .chars()
            .take(21)
            .collect::<String>()
    )
}

/// Render the answer.
#[must_use]
pub fn render(
    cmp: &Comparison,
    main: &Value,
    up: &Result<Value, String>,
    tests: &Result<TestsOutcome, String>,
    delta: &Result<super::run::DeltaOutcome, String>,
    opts: RenderOpts,
) -> Rendered {
    let deltas = delta.as_ref().ok().map(|d| &d.delta["deltas"]);
    let acc = &main["accounting"];
    let ups: BTreeMap<&str, &Value> = up
        .as_ref()
        .ok()
        .and_then(|u| u["roots"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|r| r["stableId"].as_str().map(|id| (id, r)))
        .collect();
    // Every root with its up, effects and tests.
    let mut roots: Vec<Value> = main["roots"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|mut r| {
            let id = s(&r, "stableId").to_string();
            r["up"] = ups
                .get(id.as_str())
                .map(|v| (*v).clone())
                .unwrap_or(Value::Null);
            r["tests"] = match tests {
                Ok(t) => root_tests(t, &id),
                Err(_) => Value::Null,
            };
            r["delta"] = deltas
                .and_then(|d| d.get(id.as_str()))
                .cloned()
                .unwrap_or(Value::Null);
            match s(&r["delta"], "pairing") {
                "added" => r["status"] = json!("added"),
                "signature" => r["status"] = json!("signatureChanged"),
                _ => {}
            }
            r
        })
        .collect();
    roots.sort_by_key(rank_key);
    // With a base: the routines whose behavior changed first (stable).
    if deltas.is_some() {
        roots.sort_by_key(|r| !delta_changed(&r["delta"]));
    }
    let files =
        acc["files"].as_array().map_or(0, Vec::len) + acc["onlyIn"].as_array().map_or(0, Vec::len);
    let diags: Vec<Value> = main["diagnosticsDetail"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let hunks_outside: Vec<Value> = acc["files"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|f| {
            f["hunks"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|h| h["routines"].as_array().is_some_and(Vec::is_empty))
                .map(move |h| {
                    json!({"file": f["path"], "startLine": h["startLine"], "endLine": h["endLine"],
                           "buckets": h["buckets"]})
                })
        })
        .collect();
    let mismatches: Vec<Value> = acc["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| !f["mismatch"].is_null())
        .map(|f| json!({"file": f["path"], "firstLine": f["mismatch"]["firstLine"], "lines": f["mismatch"]["lines"]}))
        .collect();
    let schema_lines: usize = ["field", "key", "fieldgroup"]
        .iter()
        .map(|k| u(&acc["totals"]["objectDeclaration"]["byKind"], k))
        .sum();
    let exit = if roots.is_empty() && input_unresolved(&diags) {
        2
    } else {
        0
    };
    let mut limits: Vec<String> = Vec::new();
    if let Err(e) = up {
        limits.push(format!("callers and entries not computed: {e}"));
    }
    for r in &roots {
        if let Some(l) = r["up"]["limited"].as_str() {
            limits.push(format!("{}: the reverse walk stopped ({l})", named(r)));
        }
    }
    let mut assumptions = vec![
        match delta {
            Ok(_) => "before/after: the base is analyzed under the current dependency environment (its commit's files, today's packages); effects are compared by semantic keys, never by position".to_string(),
            Err(_) => "current effects only: no base was built, so there is no before/after and no routine is called added or deleted".to_string(),
        },
        "callers and entries are static may-paths over the sited graph (R0's reverse walk)".to_string(),
        "tests are static reachability, not coverage".to_string(),
    ];
    if delta.is_ok() {
        assumptions.push("a key only the base side has is \"no longer observed\", not removed, when the new side's coverage of that routine is incomplete".into());
    }
    if let Err(e) = delta
        && cmp.comparison_base.is_some()
        && e != "not asked"
    {
        limits.push(format!("before/after not computed: {e}"));
    }
    // Output validation §5.2: the header's facts (human: the four lines;
    // JSON: `result.scope`, F76).
    let header = impact_header(
        cmp,
        &roots,
        files,
        up,
        delta,
        exit,
        &assumptions,
        &limits,
        opts,
    );

    if opts.json {
        let doc = json!({
            "schema": SCHEMA, "version": VERSION, "lineBase": 1,
            "comparison": cmp.to_json(),
            "input": {
                "rootsResolved": roots.len(),
                "accounting": acc,
                "diagnostics": diags,
                "hunksOutsideRoutines": hunks_outside,
                "mismatches": mismatches,
                "untracked": cmp.untracked,
                "layers": {"committed": cmp.committed.len(), "staged": cmp.staged.len(),
                           "unstaged": cmp.unstaged.len(), "untracked": cmp.untracked.len()},
            },
            "roots": roots,
            "union": up.as_ref().ok().map(|u| u["union"].clone()),
            "schemaChanges": delta.as_ref().ok().map(|d| d.delta["schemaChanges"].clone()),
            "deleted": delta.as_ref().ok().map(|d| d.delta["deleted"].clone()),
            "behavior": delta.as_ref().ok().map(|d| json!({
                "paired": d.delta["paired"], "behaviorChanged": d.delta["behaviorChanged"],
                "added": roots.iter().filter(|r| r["status"] == "added").count(),
                "deleted": d.delta["deleted"].as_array().map_or(0, Vec::len),
            })),
            "schemaLines": schema_lines,
            "tests": match tests {
                Ok(t) => t.doc.clone(),
                Err(e) => json!({"notComputed": e}),
            },
            "revisions": null,
            "coverage": {"graph": up.as_ref().ok().map(|u| u["graph"].clone()),
                         "states": up.as_ref().ok().map(|u| json!({"perRoot": u["cost"]["perRootStates"], "union": u["cost"]["unionStates"]}))},
            "assumptions": assumptions,
            "limits": limits,
            // Output validation V4 (F76, additive): the header's facts; the
            // tests' selection when they were computed.
            "result": {
                "scope": header.scope_json(serde_json::Map::new()),
                "selection": match tests {
                    Ok(t) => t.doc.pointer("/result/selection").cloned().unwrap_or(Value::Null),
                    Err(_) => Value::Null,
                },
            },
            "base": match delta {
                Ok(d) => d.block.clone(),
                Err(e) if e == "not asked" => Value::Null,
                Err(e) => json!({"notComputed": e}),
            },
        });
        let text = serde_json::to_string_pretty(&doc).expect("serializes") + "\n";
        let outcome = impact_outcome(roots.len(), exit);
        return Rendered {
            summary: impact_summary(
                &header,
                outcome,
                summary_text(cmp, main, &roots, files, tests, delta),
                &roots,
            ),
            text,
            exit,
            outcome,
        };
    }

    let mut out: Vec<String> = vec![header.human().trim_end().to_string()];
    // §6.1: inputs in the surface's words.
    let base_in = match opts.surface {
        crate::engine::follow_up::Surface::Cli => "--base <ref>",
        crate::engine::follow_up::Surface::Mcp => "base: \"<ref>\"",
    };
    let more_in = format!(
        "{} or {}",
        opts.input(crate::engine::follow_up::Input::Detail),
        opts.input(crate::engine::follow_up::Input::FormatJson)
    );
    // Output validation V2 (§4.7, F27): a change mode's first line states
    // the computation performed, and whether the diff matched the source.
    match (cmp.source, delta) {
        ("diff", Err(_)) => out.push(format!(
            "computed: current effects of the {} this diff touches, in the working tree (the \
             diff's lines match it: {}); no before/after computed — {base_in} gives it",
            plural(roots.len(), "routine", "routines"),
            if mismatches.is_empty() {
                "yes".to_string()
            } else {
                format!(
                    "no, {} at uncertain positions; routines placed by name",
                    plural(mismatches.len(), "file", "files")
                )
            }
        )),
        ("base", Err(e)) if e != "not asked" => out.push(format!(
            "computed: base {} given; before/after not computed ({e})",
            cmp.requested_ref.as_deref().unwrap_or("")
        )),
        _ => {}
    }
    out.push(comparison_line(cmp, files));
    if let Ok(d) = delta {
        out.push(base_clause(&d.block));
    }
    // 1. Input accounting (always first).
    let mut input = match delta {
        Ok(d) => format!(
            "input: {} resolved: {} paired with the base, {} added, {} deleted (complete inventories)",
            plural(roots.len(), "changed routine", "changed routines"),
            d.delta["paired"],
            roots.iter().filter(|r| r["status"] == "added").count(),
            d.delta["deleted"].as_array().map_or(0, Vec::len)
        ),
        Err(_) => format!(
            "input: {} resolved",
            plural(roots.len(), "changed routine", "changed routines")
        ),
    };
    if !acc.is_null() {
        input.push_str(&format!(
            "; changed lines {} = {}",
            thousands(u(acc, "changedLines")),
            buckets_clause(&acc["totals"])
        ));
    }
    out.push(input);
    out.extend(object_header_line(acc));
    if cmp.source == "base" {
        out.push(format!(
            "  layers: committed {}, staged {}, unstaged {}, untracked .al {}{}",
            cmp.committed.len(),
            cmp.staged.len(),
            cmp.unstaged.len(),
            cmp.untracked.len(),
            if cmp.untracked.is_empty() {
                String::new()
            } else {
                let show = if opts.detail { usize::MAX } else { 5 };
                format!(
                    " ({}{})",
                    cmp.untracked
                        .iter()
                        .take(show)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", "),
                    if cmp.untracked.len() > show {
                        format!(", … +{}", cmp.untracked.len() - show)
                    } else {
                        String::new()
                    }
                )
            }
        ));
    }
    if !hunks_outside.is_empty() {
        let show = if opts.detail { usize::MAX } else { 5 };
        let first: Vec<String> = hunks_outside
            .iter()
            .take(show)
            .map(|h| {
                format!(
                    "{}:{}-{} {}",
                    s(h, "file"),
                    h["startLine"],
                    h["endLine"],
                    buckets_short(&h["buckets"])
                )
            })
            .collect();
        let rest = hunks_outside.len().saturating_sub(first.len());
        out.push(format!(
            "  hunks outside routines: {} ({}{})",
            hunks_outside.len(),
            first.join("; "),
            if rest > 0 {
                format!("; … +{rest}, every one with {more_in}")
            } else {
                String::new()
            }
        ));
    }
    let unmapped: Vec<String> = acc["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| !f["unmapped"].is_null())
        .map(|f| {
            format!(
                "{} ({}, {})",
                s(f, "path"),
                unmapped_human(s(f, "unmapped")),
                plural(u(f, "added") + u(f, "removed"), "line", "lines")
            )
        })
        .collect();
    if !unmapped.is_empty() {
        let show = if opts.detail { usize::MAX } else { 5 };
        out.push(format!(
            "  files not mapped: {} ({}{})",
            unmapped.len(),
            unmapped
                .iter()
                .take(show)
                .cloned()
                .collect::<Vec<_>>()
                .join("; "),
            if unmapped.len() > show {
                format!("; … +{}", unmapped.len() - show)
            } else {
                String::new()
            }
        ));
    }
    let removed: Vec<String> = acc["files"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|f| {
            f["removedRoutineNames"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|n| n.as_str().map(|n| format!("{n} ({})", s(f, "path"))))
        })
        .collect();
    if !removed.is_empty() {
        out.push(format!(
            "  removed lines held {} no routine of the new side has (removed or moved; a base decides): {}",
            plural(removed.len(), "routine header", "routine headers"),
            removed.iter().take(if opts.detail { usize::MAX } else { 5 }).cloned().collect::<Vec<_>>().join(", ")
                + &if removed.len() > 5 && !opts.detail { format!(", … +{}", removed.len() - 5) } else { String::new() }
        ));
    }
    if !mismatches.is_empty() {
        out.push(format!(
            "  the diff does not match the working tree: {} ({}); their routines' positions are uncertain",
            plural(mismatches.len(), "file", "files"),
            mismatches
                .iter()
                .take(5)
                .map(|m| format!("{}:{}", s(m, "file"), m["firstLine"]))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let other_diags: Vec<String> = diags
        .iter()
        .filter(|d| s(d, "kind") != "hunk-outside-routines")
        .map(|d| s(d, "text").to_string())
        .collect();
    if !other_diags.is_empty() {
        let mut by: BTreeMap<&str, usize> = BTreeMap::new();
        for d in diags
            .iter()
            .filter(|d| s(d, "kind") != "hunk-outside-routines")
        {
            *by.entry(s(d, "kind")).or_default() += 1;
        }
        out.push(format!(
            "  root diagnostics: {} ({})",
            other_diags.len(),
            by.iter()
                .map(|(k, n)| format!("{k} {n}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        let show = if opts.detail { usize::MAX } else { 5 };
        for d in other_diags.iter().take(show) {
            out.push(format!("    {d}"));
        }
        if other_diags.len() > show {
            out.push(format!("    … +{} ({more_in})", other_diags.len() - show));
        }
    }
    if roots.is_empty() {
        out.push("no changed routine resolved, so no caller, effect or test was computed".into());
        if let Ok(d) = delta {
            before_after_tail(&mut out, d, opts);
        } else if let Err(e) = delta
            && e != "not asked"
        {
            out.push(format!("before/after: not computed — {e}"));
        }
        let text = out.join("\n") + "\n";
        let outcome = impact_outcome(roots.len(), exit);
        return Rendered {
            summary: impact_summary(&header, outcome, text.clone(), &roots),
            text,
            exit,
            outcome,
        };
    }
    // 2. Per root: each shown routine's block, then the whole change (3),
    // the tests (4) and what was not seen (5). The routines shown are the
    // first `max_roots`, fewer when the answer would pass its size budget
    // (`RenderOpts::budget`); the fold says how many it hides and why.
    let block_header = |shown: usize| -> Vec<String> {
        let which = if shown == roots.len() {
            "all shown".to_string()
        } else {
            format!("first {shown}")
        };
        match delta {
            Ok(d) => vec![format!(
                "behavior changed in {} of the {} paired with the base; ranked: changed behavior first, then reached from a user or a job, then by ancestors; {which}:",
                d.delta["behaviorChanged"],
                plural(u(&d.delta, "paired"), "routine", "routines"),
            )],
            Err(e) => {
                let mut h = Vec::new();
                if e != "not asked" {
                    h.push(format!("before/after: not computed — {e}"));
                }
                // §7.6: "no base given" prints once, in the header.
                h.push(format!(
                    "current effects of the {}; ranked: reached from a user or a job first, then by ancestors; {which}:",
                    plural(roots.len(), "changed routine", "changed routines"),
                ));
                h
            }
        }
    };
    // Round-2 r2 §2.1: one hubs line per answer: a routine's block prints it
    // only when it is the only routine (else the whole change does).
    let one_root = roots.len() == 1 || up.is_err();
    let blocks: Vec<Vec<String>> = roots
        .iter()
        .take(opts.max_roots)
        .map(|r| root_block(r, opts, one_root))
        .collect();
    let mut tail: Vec<String> = Vec::new();
    // 3. The whole change (§7.6: not for a change of one routine, whose
    // block already says it).
    if let Ok(up) = up
        && roots.len() > 1
    {
        let un = &up["union"];
        tail.push("the whole change:".to_string());
        // Round-2 r2 §2.1: the hubs line prints once per answer, here
        // (the union holds every routine's).
        tail.extend(reach_lines(un, false, "  ", opts.surface, true, false));
    }
    if let Ok(d) = delta {
        before_after_tail(&mut tail, d, opts);
    } else if schema_lines > 0 {
        tail.push(format!(
            "schema: {} field, key or field group lines changed (current revision only; a base gives the schema findings)",
            thousands(schema_lines)
        ));
    }
    // 4. Tests.
    tail.push(match tests {
        Err(e) if e.starts_with("not asked") => format!(
            "tests: not asked ({})",
            match opts.surface {
                crate::engine::follow_up::Surface::Cli => "--no-tests",
                crate::engine::follow_up::Surface::Mcp => "tests: false",
            }
        ),
        Err(e) => format!("tests: not computed — {e}"),
        Ok(t) => tests_line(&t.doc),
    });
    // 5. What was not seen: the header's limits and assumptions (§5.2).
    let fold = |shown: usize, cut: bool| -> Option<String> {
        (roots.len() > shown).then(|| {
            format!(
                "  … +{} ({}{} to show more, or {} for every one)",
                plural(roots.len() - shown, "routine", "routines"),
                if cut {
                    format!(
                        "the answer is kept under {} KiB: ",
                        opts.budget.unwrap_or(0) / 1024
                    )
                } else {
                    String::new()
                },
                opts.input(crate::engine::follow_up::Input::MaxRoots),
                opts.input(crate::engine::follow_up::Input::FormatJson)
            )
        })
    };
    let bytes = |lines: &[String]| -> usize { lines.iter().map(|l| l.len() + 1).sum() };
    let mut shown = blocks.len();
    if let Some(budget) = opts.budget {
        let fixed = bytes(&out) + bytes(&tail);
        let size = |k: usize| {
            fixed
                + bytes(&block_header(k))
                + blocks[..k].iter().map(|b| bytes(b)).sum::<usize>()
                + fold(k, k < blocks.len()).map_or(0, |l| l.len() + 1)
        };
        // At least one routine is always shown.
        while shown > 1 && size(shown) > budget {
            shown -= 1;
        }
    }
    out.extend(block_header(shown));
    for b in &blocks[..shown] {
        out.extend(b.iter().cloned());
    }
    out.extend(fold(shown, shown < blocks.len()));
    out.extend(tail);
    let text = out.join("\n") + "\n";
    let outcome = impact_outcome(roots.len(), exit);
    Rendered {
        summary: impact_summary(
            &header,
            outcome,
            summary_text(cmp, main, &roots, files, tests, delta),
            &roots,
        ),
        text,
        exit,
        outcome,
    }
}

/// The impact answer's four header lines (output validation §5.2): what was
/// compared and the routines it resolved, the accounting outcome, the
/// analysis, assumptions and limits, and what shows more.
#[allow(clippy::too_many_arguments)]
fn impact_header(
    cmp: &Comparison,
    roots: &[Value],
    files: usize,
    up: &Result<Value, String>,
    delta: &Result<super::run::DeltaOutcome, String>,
    exit: u8,
    assumptions: &[String],
    limits: &[String],
    opts: RenderOpts,
) -> crate::engine::result_header::Header {
    use crate::engine::follow_up::Input;
    use crate::engine::result_header::{Analysis, Header};
    let what = match cmp.source {
        "base" => format!(
            "base {} (comparison base {}), {}",
            cmp.requested_ref.as_deref().unwrap_or(""),
            cmp.comparison_base
                .as_deref()
                .map(short)
                .unwrap_or_default(),
            plural(files, "file", "files")
        ),
        "diff" => format!("diff ({})", plural(files, "file", "files")),
        "files" => "files".to_string(),
        _ => "routines".to_string(),
    };
    let first = roots.first().map(named).unwrap_or_default();
    let inputs = format!(
        "{what} → {}",
        plural(roots.len(), "changed routine", "changed routines")
    );
    let mut h = Header::new("impact", inputs, opts.single_app);
    h.answer = match (roots.is_empty(), exit) {
        (true, 0) => {
            "no routine changed — every changed line is accounted outside routines (outcome: \
             no-routine-changed)"
                .to_string()
        }
        (true, _) => "no answer — the input did not map to the working tree's routines (outcome: \
             input-not-mapped)"
            .to_string(),
        (false, _) => {
            let behavior = match delta {
                Ok(d) => format!(
                    "; behavior changed in {} of the {} paired with the base",
                    d.delta["behaviorChanged"],
                    u(&d.delta, "paired")
                ),
                Err(_) => String::new(),
            };
            format!(
                "{} resolved{behavior}; ranked first: {first}",
                plural(roots.len(), "changed routine", "changed routines")
            )
        }
    };
    h.analysis = match up {
        Err(e) if !roots.is_empty() => {
            Analysis::Partial(vec![format!("callers and entries not computed: {e}")])
        }
        _ if roots.is_empty() && exit != 0 => {
            Analysis::NotAnalyzed("the input did not map".to_string())
        }
        _ => Analysis::Complete,
    };
    // Round-2 n09: the effects' completeness counts too (it said "analysis
    // complete" where explain on the same routine said "partial (… 8
    // interface calls open world)"). One routine names its reasons; several
    // are counted, each routine's effects line naming its own.
    let partial: Vec<Analysis> = roots
        .iter()
        .filter_map(|r| effects_analysis(&r["effects"]))
        .collect();
    match partial.as_slice() {
        [] => {}
        [one] if roots.len() == 1 => {
            h.analysis = std::mem::replace(&mut h.analysis, Analysis::Complete).and(
                Analysis::Partial(vec![format!("effects: {}", one.reasons().join("; "))]),
            );
        }
        many => {
            h.analysis = std::mem::replace(&mut h.analysis, Analysis::Complete).and(
                Analysis::Partial(vec![format!(
                    "effects partial for {} of {} (each routine's effects line says why)",
                    many.len(),
                    plural(roots.len(), "changed routine", "changed routines")
                )]),
            );
        }
    }
    h.assumptions = assumptions.to_vec();
    h.limits = limits
        .iter()
        .filter(|l| !l.starts_with("callers and entries not computed"))
        .cloned()
        .collect();
    // §R2.4 landing change 3: once per answer, what the callers do not
    // count.
    let local_source = up
        .as_ref()
        .is_ok_and(|u| u["localSourceDependencies"] == json!(true));
    h.scope = format!(
        "callers and entries over the whole program, dependency code in its own tier; test code \
         is not in these callers (tests: reports it); the roots.config entry overlay is not \
         applied{}; effects through every resolved call with literal arguments",
        if local_source {
            "; local includes dependencies loaded from source folders"
        } else {
            ""
        }
    );
    if !opts.detail && !roots.is_empty() {
        h.more.push(format!(
            "{} lists each routine's direct callers and its first entry of each kind",
            opts.input(Input::Detail)
        ));
    }
    h.more.push(format!(
        "{} for every routine with its callers, effects and tests",
        opts.input(Input::FormatJson)
    ));
    h
}

/// One routine's lines in the human answer: its name and anchor, its
/// before/after (with a base), its current effects, who reaches it, its
/// tests. Every list is capped (all of it with `detail`), each cap saying
/// how many it hides.
fn root_block(r: &Value, opts: RenderOpts, hubs: bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    {
        let anchor = &r["anchor"];
        let status = match s(r, "status") {
            "signatureChanged" => "signature changed",
            "addedInDiff" => {
                "new in this diff (its header is an added line, none of its lines removed)"
            }
            "signatureLinesChanged" => "signature lines changed in the diff",
            "positionUncertain" => "position uncertain (the diff does not match the working tree)",
            "added" => "added (new: its current effects are its whole behavior)",
            _ => "changed",
        };
        out.push(format!(
            "  {} ({}:{}) — {status}",
            named(r),
            s(anchor, "file"),
            anchor["line"]
        ));
        let d = &r["delta"];
        if !d.is_null() && s(d, "pairing") != "added" {
            if !d["notComputed"].is_null() {
                out.push(format!(
                    "      before/after: not computed — {}",
                    s(d, "notComputed")
                ));
            } else {
                let clauses = delta_clauses(d, opts.detail);
                let cap = if opts.detail { usize::MAX } else { 6 };
                out.push(format!(
                    "      before/after{}: {}{}",
                    if s(d, "pairing") == "signature" {
                        " (paired by signature)"
                    } else {
                        ""
                    },
                    if clauses.is_empty() {
                        "no behavior change".to_string()
                    } else {
                        clauses
                            .iter()
                            .take(cap)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join("; ")
                    },
                    if clauses.len() > cap {
                        format!(
                            "; … +{} ({} or {})",
                            clauses.len() - cap,
                            opts.input(crate::engine::follow_up::Input::Detail),
                            opts.input(crate::engine::follow_up::Input::FormatJson)
                        )
                    } else {
                        String::new()
                    }
                ));
            }
        } else if let Some(reason) = d["reason"].as_str() {
            out.push(format!("      not paired: {reason}"));
        }
        out.push(format!(
            "      {}{}",
            if d.is_null() { "" } else { "now: " },
            effects_clause(&r["effects"])
        ));
        // Round-2 r2 §3.9 (G15): the routine's direct callees, explain's
        // `calls:` line.
        if let Some(c) = r["callsLine"].as_str() {
            out.push(format!("      {c}"));
        }
        let up = &r["up"];
        if up.is_null() {
            out.push("      reached from: not computed".into());
        } else if s(up, "status") == "not-in-graph" {
            out.push("      reached from: not in the sited graph (no caller is known)".into());
        } else {
            // Output validation V2 (§3.2, F12): code callers first, then the
            // reach beyond them split by tier, kind and context, lists folded
            // when the reach is broad, the hubs named.
            // §7.6 (F46): with `detail`, the direct callers with their call
            // sites (at most [`DETAIL_ITEMS`], the rest counted).
            // Round-2 r2 §2.1 (W1 interim): the computed direct code
            // callers (`select::direct_callers`), never a claim from the
            // routine's kind; a subscriber's publishers are `raised by:`.
            let c = &up["callers"];
            let first: Vec<String> = c["first"]
                .as_array()
                .into_iter()
                .flatten()
                .take(if opts.detail { DETAIL_ITEMS } else { 1 })
                .map(|f| match f["line"].as_u64() {
                    Some(l) => format!("{} at {}:{l}", s(f, "display"), s(f, "file")),
                    None => s(f, "display").to_string(),
                })
                .collect();
            // §R2.4 landing change 6: a changed routine that is an entry
            // says so (its kind first among its own).
            if let Some(k) = up["rootEntryKinds"]
                .as_array()
                .and_then(|a| a.first())
                .and_then(Value::as_str)
            {
                out.push(format!(
                    "      the changed routine is a {} (an entry itself)",
                    crate::engine::vocab::entry_key(k).0
                ));
            }
            out.push(if u(c, "count") == 0 {
                "      direct code callers: none found (test code is not counted; see tests:)"
                    .to_string()
            } else {
                let rest = u(c, "count").saturating_sub(first.len());
                format!(
                    "      direct code callers: {} ({}{})",
                    thousands(u(c, "count")),
                    first.join("; "),
                    if rest > 0 {
                        format!("; … +{rest}")
                    } else {
                        String::new()
                    }
                )
            });
            let direct_both = u(c, "count") > 0 && u(&up["eventPublishers"]["direct"], "count") > 0;
            out.extend(reach_lines(
                up,
                opts.detail,
                "      ",
                opts.surface,
                hubs,
                direct_both,
            ));
            // Round-2 n09: "raised by" is the events that raise the routine
            // itself (its own subscriptions); the publishers anywhere among
            // its ancestors are a different count, said so.
            let p = &up["eventPublishers"];
            let take = if opts.detail { DETAIL_ITEMS } else { 1 };
            let list = |v: &Value, item: fn(&Value) -> String| {
                v["first"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .take(take)
                    .map(item)
                    .collect::<Vec<_>>()
                    .join("; ")
            };
            let direct = &p["direct"];
            if u(direct, "count") > 0 {
                out.push(format!(
                    "      raised by: {} ({})",
                    plural(u(direct, "count"), "event", "events"),
                    list(direct, node_short)
                ));
            }
            // §R2.4 landing change 4: the publishers anywhere among the
            // ancestors are a legacy list of every relation, not a
            // population: dropped from the human text (JSON
            // `eventPublishers` keeps it). The direct `raised by:` stays.
        }
        if !r["tests"].is_null() {
            out.push(format!("      {}", tests_clause(&r["tests"])));
        }
    }
    out
}

/// The deleted routines and the schema findings of a before/after answer.
fn before_after_tail(out: &mut Vec<String>, d: &super::run::DeltaOutcome, opts: RenderOpts) {
    let deleted = d.delta["deleted"].as_array().cloned().unwrap_or_default();
    if !deleted.is_empty() {
        let called: Vec<&Value> = deleted
            .iter()
            .filter(|x| u(&x["baseCallers"], "count") > 0)
            .collect();
        out.push(format!(
            "deleted: {} (in the base, no routine of the working tree has or pairs with them; complete inventories); {} still called in the base",
            plural(deleted.len(), "routine", "routines"),
            called.len()
        ));
        let show = if opts.detail { usize::MAX } else { 5 };
        for x in deleted.iter().take(show) {
            let c = &x["baseCallers"];
            let first = c["first"]
                .as_array()
                .and_then(|a| a.first())
                .map(|f| {
                    format!(
                        " (first: {} at {}:{})",
                        s(f, "display"),
                        s(f, "file"),
                        f["line"]
                    )
                })
                .unwrap_or_default();
            out.push(format!(
                "  {} (in the base at {}:{}): called in the base by {}{}{}{}",
                named(x),
                s(&x["anchor"], "file"),
                x["anchor"]["line"],
                plural(u(c, "count"), "routine", "routines"),
                first,
                if u(c, "pageActions") > 0 {
                    format!(
                        ", {}",
                        plural(u(c, "pageActions"), "page action", "page actions")
                    )
                } else {
                    String::new()
                },
                x["reason"]
                    .as_str()
                    .map(|r| format!("; not paired: {r}"))
                    .unwrap_or_default()
            ));
        }
        if deleted.len() > show {
            out.push(format!(
                "  … +{} ({} or {})",
                deleted.len() - show,
                opts.input(crate::engine::follow_up::Input::Detail),
                opts.input(crate::engine::follow_up::Input::FormatJson)
            ));
        }
    }
    let sc = &d.delta["schemaChanges"];
    let findings = sc["findings"].as_array().cloned().unwrap_or_default();
    let mut by: BTreeMap<String, usize> = BTreeMap::new();
    for f in &findings {
        *by.entry(s(f, "kind").to_string()).or_default() += 1;
    }
    let schema: Vec<&Value> = findings
        .iter()
        .filter(|f| f["category"] == "schema")
        .collect();
    out.push(format!(
        "schema and contract (the diff engine's abi and schema checks): {}{}",
        if by.is_empty() {
            "no finding".to_string()
        } else {
            by.iter()
                .map(|(k, n)| format!("{k} {n}"))
                .collect::<Vec<_>>()
                .join(", ")
        },
        if sc["fatal"] == json!(true) {
            " (the preflight stopped the diff)".to_string()
        } else {
            String::new()
        }
    ));
    for f in schema.iter().take(if opts.detail { usize::MAX } else { 5 }) {
        let id = s(&f["subject"], "normalizedStableId");
        // §6.1: the complete call for the field's consumers.
        let pointer = match id.split_once('#') {
            Some((table, field)) if !field.starts_with('K') => {
                let f = crate::engine::follow_up::FollowUp::new(
                    crate::engine::follow_up::Tool::QueryTouches,
                    crate::engine::follow_up::Cx::new(opts.surface, opts.single_app),
                    "the field's consumers",
                )
                .arg("table", table.rsplit(':').next().unwrap_or(table))
                .arg("field", field);
                format!("; consumers: {}", f.text(opts.surface))
            }
            _ => String::new(),
        };
        out.push(format!(
            "  {} {} ({id}){pointer}",
            s(f, "kind"),
            s(&f["subject"], "displayName")
        ));
    }
    // §6.1: a command with no MCP tool is named so (`alsem diff`).
    let not_computed = s(sc, "notComputed");
    let not_computed = match opts.surface {
        crate::engine::follow_up::Surface::Mcp => not_computed.replace(
            "(alsem diff <base> <new> gives them)",
            "(the CLI's diff command gives them; it is not available over MCP)",
        ),
        crate::engine::follow_up::Surface::Cli => not_computed.to_string(),
    };
    out.push(format!("  not computed here: {not_computed}"));
}

/// The tests summary line from the al-sem.tests/1 block.
fn tests_line(d: &Value) -> String {
    let apps: Vec<String> = d["scope"]["apps"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|a| s(a, "name").to_string())
        .collect();
    let by = &d["byTier"];
    let complete = d["selectionComplete"] == json!(true);
    let reasons: Vec<String> = d["requiredFallback"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|f| match s(f, "reason") {
            // Why a test app was not analyzed is the next action: say it.
            "test-app-not-analyzed" => format!("test app not analyzed: {}", s(f, "detail")),
            r => r.to_string(),
        })
        .collect();
    let mut codeunits: Vec<String> = Vec::new();
    for t in d["runFirst"].as_array().into_iter().flatten() {
        if let Some(n) = t["codeunitId"].as_i64() {
            let n = n.to_string();
            if !codeunits.contains(&n) {
                codeunits.push(n);
            }
        }
    }
    // Output validation V2 (§9.5, F63): what to run first, in the test
    // app's terms, then the selection's completeness and the fallback.
    // Round-2 c20: the codeunits cover the tests `runFirst` picked (the
    // block's tier, local by default), not every test with a modeled path;
    // the others are counted apart, by tier.
    let (picked_on, left_out) = match s(d, "tier") {
        "all" => ("on any path", Vec::new()),
        "dependency" => (
            "on a local or via-dependency path",
            vec![("via a hub", u(by, "onlyOpen"))],
        ),
        _ => (
            "on a local path",
            vec![
                ("via dependency code", u(by, "throughDependency")),
                ("via a hub", u(by, "onlyOpen")),
            ],
        ),
    };
    let picked = d["runFirstTotal"]
        .as_u64()
        .map_or(d["runFirst"].as_array().map_or(0, Vec::len), |n| n as usize);
    let rest: usize = left_out.iter().map(|(_, n)| n).sum();
    let rest_clause = if rest == 0 {
        String::new()
    } else {
        format!(
            "{} {} a modeled path only {} and {} not in this filter",
            thousands(rest),
            if rest == 1 { "test has" } else { "tests have" },
            left_out
                .iter()
                .filter(|(_, n)| *n > 0)
                .map(|(w, n)| format!("{w} ({})", thousands(*n)))
                .collect::<Vec<_>>()
                .join(" or "),
            if rest == 1 { "is" } else { "are" },
        )
    };
    let run = if codeunits.is_empty() {
        if rest == 0 {
            "run first: no test with a modeled path".to_string()
        } else {
            format!("run first: no test {picked_on}; {rest_clause}")
        }
    } else {
        format!(
            "run first ({}): codeunits {} — whole codeunits, so every test in them runs: a \
             superset of the {} picked {picked_on}{}",
            if apps.is_empty() {
                "the workspace".to_string()
            } else {
                format!("app \"{}\"", apps.join("\", \""))
            },
            codeunits.join("|"),
            plural(picked, "test", "tests"),
            if rest_clause.is_empty() {
                String::new()
            } else {
                format!("; {rest_clause}")
            }
        )
    };
    format!(
        "tests: {run}; {}; workspace {} [Test]; {} ({})",
        if complete {
            "selection complete".to_string()
        } else {
            format!(
                "then the whole suite (selection incomplete: {})",
                reasons.join(", ")
            )
        },
        u(&d["scope"], "workspaceTests"),
        plural(apps.len(), "test app", "test apps"),
        if apps.is_empty() {
            "none found".to_string()
        } else {
            apps.join(", ")
        },
    )
}

/// The inline summary when the answer goes to a file (MCP): the comparison,
/// the input line, the first five routines (one line each), the tests line.
fn summary_text(
    cmp: &Comparison,
    main: &Value,
    roots: &[Value],
    files: usize,
    tests: &Result<TestsOutcome, String>,
    delta: &Result<super::run::DeltaOutcome, String>,
) -> String {
    let acc = &main["accounting"];
    let mut out = vec![comparison_line(cmp, files)];
    if let Ok(d) = delta {
        out.push(base_clause(&d.block));
        out.push(format!(
            "before/after: behavior changed in {} of {} paired routines; {} added; {} deleted",
            d.delta["behaviorChanged"],
            d.delta["paired"],
            roots.iter().filter(|r| r["status"] == "added").count(),
            d.delta["deleted"].as_array().map_or(0, Vec::len)
        ));
        for r in roots.iter().filter(|r| delta_changed(&r["delta"])).take(5) {
            let c = delta_clauses(&r["delta"], false);
            out.push(format!(
                "  {}: {}{}",
                named(r),
                c.iter().take(3).cloned().collect::<Vec<_>>().join("; "),
                if c.len() > 3 {
                    format!("; … +{}", c.len() - 3)
                } else {
                    String::new()
                }
            ));
        }
    }
    let mut input = format!(
        "input: {} resolved",
        plural(roots.len(), "changed routine", "changed routines")
    );
    if !acc.is_null() {
        input.push_str(&format!(
            "; changed lines {} = {}",
            thousands(u(acc, "changedLines")),
            buckets_clause(&acc["totals"])
        ));
    }
    out.push(input);
    out.extend(object_header_line(acc));
    out.push(if delta.is_ok() {
        "current effects, first by rank:".into()
    } else {
        "current effects; no before/after (no base given). First by rank:".into()
    });
    for r in roots.iter().take(5) {
        let up = &r["up"];
        out.push(format!(
            "  {} — {}; ancestors: by code {}, by UI navigation {}, candidates {}",
            named(r),
            effects_clause(&r["effects"]),
            thousands(population_count(up, "code")),
            thousands(population_count(up, "uiNavigation")),
            thousands(
                population_count(up, "overConfiguration")
                    + population_count(up, "overRule")
                    + population_count(up, "overHub")
            ),
        ));
    }
    if roots.len() > 5 {
        out.push(format!("  … +{} in the file", roots.len() - 5));
    }
    out.push(match tests {
        Err(e) => format!("tests: not computed — {e}"),
        Ok(t) => tests_line(&t.doc),
    });
    out.join("\n") + "\n"
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
