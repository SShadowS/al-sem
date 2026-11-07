//! **The tests answer** (agent-tasks design §4.6): the roots part and every
//! select part, rendered as human text or as `al-sem.tests/1` JSON. The CLI
//! and the MCP tool call this one function on the same parts, so their
//! stdout is the same bytes.
//!
//! Honesty rules (§4.9), each a line of the human text:
//! - the scope comes first, with counts, how each app was found and what was
//!   skipped;
//! - "no test found" never means "no test needed";
//! - a root that is not loaded or not in the graph is never "not reached";
//! - `selection complete: no` always comes with the fallback's reasons, and
//!   is printed before the `run first` filter;
//! - static reachability is not coverage, said once at the end.

use std::collections::BTreeMap;

use serde_json::{Value, json};

/// Which tiers `runFirst` takes (`--tier`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TierPick {
    /// Local paths only (the default).
    Local,
    /// Local and through-dependency paths.
    Dependency,
    /// Every reached test, open paths included.
    All,
}

impl TierPick {
    /// `--tier`'s values.
    pub fn parse(s: &str) -> Result<TierPick, String> {
        match s {
            "local" => Ok(TierPick::Local),
            "dependency" => Ok(TierPick::Dependency),
            "all" => Ok(TierPick::All),
            other => Err(format!(
                "al-sem tests: invalid --tier '{other}'. Expected: local | dependency | all"
            )),
        }
    }

    fn takes(self, tier: &str) -> bool {
        match self {
            TierPick::Local => tier == "local",
            TierPick::Dependency => tier == "local" || tier == "throughDependency",
            TierPick::All => true,
        }
    }

    fn words(self) -> &'static str {
        match self {
            TierPick::Local => "local path",
            TierPick::Dependency => "local or via-dependency path",
            TierPick::All => "any path, via-hub included",
        }
    }
}

/// The relation of a test's path (§8.1): `UI navigation` when any hop is a
/// page the path opens (a bare page object between routines: the page's
/// lifecycle edge), else `code`.
fn relation(t: &Value) -> &'static str {
    let nav = t["path"]
        .as_array()
        .into_iter()
        .flatten()
        .skip(1)
        .filter_map(Value::as_str)
        .any(|h| (h.starts_with("Page ") || h.starts_with("PageExtension ")) && !h.contains("::"));
    if nav {
        crate::engine::vocab::RELATION_NAVIGATION
    } else {
        crate::engine::vocab::RELATION_CODE
    }
}

/// The changed routine's own object part and trigger name, when the answer
/// has exactly one (`Table "T"."F"`, `OnValidate`).
fn one_target(changed: &[Value]) -> Option<(&str, &str)> {
    match changed {
        [one] => s(one, "display").rsplit_once("::"),
        _ => None,
    }
}

/// What the navigation-only tests reach, for the headline (round-2 r2
/// §2.2): `this trigger`, `this routine`, or `the change`.
fn navigation_target(changed: &[Value]) -> &'static str {
    match one_target(changed) {
        Some((_, name)) if name.starts_with("On") => "this trigger",
        Some(_) => "this routine",
        None => "the change",
    }
}

/// The user step a navigation-only path still needs, once, in the headline
/// (round-2 r2 §2.2), from the target's routine kind: a table field's
/// trigger is reached through a page on the field's table (never "a page
/// that shows the field": page columns are not indexed); a page field's or
/// action's through its page.
fn navigation_caveat(changed: &[Value]) -> String {
    let Some((object, name)) = one_target(changed) else {
        return "each path opens a page; whether a user takes the step each one needs is not \
                established"
            .to_string();
    };
    let field_step = match name.to_ascii_lowercase().as_str() {
        "onvalidate" => Some("edits"),
        "onlookup" => Some("looks up"),
        "onassistedit" => Some("assist-edits"),
        _ => None,
    };
    let table = object.starts_with("Table ") || object.starts_with("TableExtension ");
    let page = object.starts_with("Page ") || object.starts_with("PageExtension ");
    match (table, page, field_step) {
        (true, _, Some("edits")) => "each opens a page on the field's table; whether the page \
                                     shows or edits the field is not established"
            .to_string(),
        (true, _, Some(step)) => format!(
            "each opens a page on the field's table; whether the user {step} the field there is \
             not established"
        ),
        (_, true, _) if object.contains(".action(") => {
            "each opens the action's page; whether the user invokes the action is not established"
                .to_string()
        }
        (_, true, Some(step)) => {
            format!("each opens the page; whether the user {step} the field is not established")
        }
        _ => "each path opens a page; whether a user takes the step it needs is not established"
            .to_string(),
    }
}

/// The `al-sem.tests/2` document of a version 1 one (output validation
/// V5, §7.4, F44; §8.3 for the words):
/// - `hidden` is `{"<class>": {"reason", "channel", "selectionRelevant",
///   "nodes": [{node, nodeId}]}}`: the class carries its most common reason,
///   channel and relevance once, and a node whose own differs keeps it (the
///   grouping loses nothing: [`hidden_v1`] gives the list back);
/// - `changed[].nearestTestedCaller` is left out when null;
/// - the legacy values leave: `changed[].status` (its §8.1 word is
///   `testStatus`), `runFirst[].tier` (its word is `pathTier`); `byTier`'s
///   keys and `changed[].nearest.tier` are the §8.1 tier words; a part's
///   `productionTier: "localApproximate"` is `"local"` with
///   `productionApproximate: true` (§8.1 projection).
#[must_use]
pub fn v2_of(mut doc: Value) -> Value {
    doc["version"] = json!(2);
    if let Some(Value::Array(h)) = doc.get("hidden").cloned() {
        doc["hidden"] = hidden_v2(&h);
    }
    if let Some(Value::Array(changed)) = doc.get_mut("changed") {
        for c in changed {
            let Value::Object(m) = c else { continue };
            m.remove("status");
            if m.get("nearestTestedCaller").is_some_and(Value::is_null) {
                m.remove("nearestTestedCaller");
            }
            if let Some(Value::String(t)) = m.get_mut("nearest").and_then(|n| n.get_mut("tier")) {
                *t = crate::engine::vocab::tier_key(t).to_string();
            }
        }
    }
    if let Some(Value::Array(rows)) = doc.get_mut("runFirst") {
        for r in rows {
            if let Value::Object(m) = r {
                m.remove("tier");
            }
        }
    }
    if let Some(Value::Object(by)) = doc.get("byTier").cloned() {
        doc["byTier"] = Value::Object(
            by.into_iter()
                .map(|(k, v)| (crate::engine::vocab::tier_key(&k).to_string(), v))
                .collect(),
        );
    }
    if let Some(Value::Array(apps)) = doc.pointer_mut("/scope/apps") {
        for a in apps {
            if a["productionTier"] == "localApproximate" {
                a["productionTier"] = json!("local");
                a["productionApproximate"] = json!(true);
            }
        }
    }
    doc
}

/// The fields a hidden node shares with its class when they agree.
const HIDDEN_SHARED: &[&str] = &["reason", "channel", "selectionRelevant"];

/// Version 1's `hidden` list grouped by class (§7.4): each shared field's
/// most common value (ties: the first met) on the class, the rest on the
/// node.
fn hidden_v2(rows: &[Value]) -> Value {
    let mut classes: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for r in rows {
        classes
            .entry(r["class"].as_str().unwrap_or("").to_string())
            .or_default()
            .push(r);
    }
    let mut out = serde_json::Map::new();
    for (class, nodes) in classes {
        let mut group = serde_json::Map::new();
        for f in HIDDEN_SHARED {
            let mut count: Vec<(Value, usize)> = Vec::new();
            for n in &nodes {
                let v = n.get(*f).cloned().unwrap_or(Value::Null);
                match count.iter_mut().find(|(x, _)| *x == v) {
                    Some((_, c)) => *c += 1,
                    None => count.push((v, 1)),
                }
            }
            // The first of the largest counts.
            let best = count
                .iter()
                .fold(None::<&(Value, usize)>, |b, x| match b {
                    Some(b) if b.1 >= x.1 => Some(b),
                    _ => Some(x),
                })
                .map(|(v, _)| v.clone())
                .unwrap_or(Value::Null);
            group.insert((*f).to_string(), best);
        }
        let listed: Vec<Value> = nodes
            .iter()
            .map(|n| {
                let mut m = serde_json::Map::new();
                for (k, v) in n.as_object().into_iter().flatten() {
                    let shared = HIDDEN_SHARED.contains(&k.as_str()) && group.get(k) == Some(v);
                    if k != "class" && !shared {
                        m.insert(k.clone(), v.clone());
                    }
                }
                // A node that lacks a field its class carries says so.
                for f in HIDDEN_SHARED {
                    if n.get(*f).is_none() && !group[*f].is_null() {
                        m.insert((*f).to_string(), Value::Null);
                    }
                }
                Value::Object(m)
            })
            .collect();
        group.insert("nodes".into(), Value::Array(listed));
        out.insert(class, Value::Object(group));
    }
    Value::Object(out)
}

/// [`hidden_v2`]'s inverse: the version 1 list, class by class in key
/// order, each node with its class's fields unless it has its own.
#[must_use]
pub fn hidden_v1(grouped: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    for (class, g) in grouped.as_object().into_iter().flatten() {
        for n in g["nodes"].as_array().into_iter().flatten() {
            let mut m = n.as_object().cloned().unwrap_or_default();
            m.insert("class".into(), class.clone().into());
            for f in HIDDEN_SHARED {
                match m.get(*f) {
                    Some(Value::Null) => {
                        m.remove(*f);
                    }
                    Some(_) => {}
                    None if !g[*f].is_null() => {
                        m.insert((*f).to_string(), g[*f].clone());
                    }
                    None => {}
                }
            }
            out.push(Value::Object(m));
        }
    }
    out
}

/// How to render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderOpts {
    pub tier: TierPick,
    pub max_tests: Option<usize>,
    pub json: bool,
    /// `--surface` (output validation §6.1).
    pub surface: crate::engine::follow_up::Surface,
    /// `--single-app` (the header's universe).
    pub single_app: bool,
    /// The JSON document's version (`--schema`): 2 (output validation V5,
    /// §7.4) or 1, kept for one release and for impact, which embeds the
    /// tests block in `al-sem.impact/1` until impact's next major.
    pub schema: u32,
}

/// One test universe's part: the app it is for and its select part's JSON,
/// or why it was not computed.
pub struct Part {
    /// The app as the scope lists it.
    pub app: Value,
    pub result: Result<Value, String>,
}

/// The rendered answer: stdout and the exit code.
pub struct Rendered {
    pub text: String,
    pub exit: u8,
    /// The envelope summary of a file answer (output validation §5.2): the
    /// run target, the selected tests (at most `maxTests`), the totals, the
    /// truncation and `selectionComplete`, with every selected test and
    /// changed routine as an index row.
    pub summary: Value,
}

fn n_of(n: usize, one: &str, many: &str) -> String {
    let w = if n == 1 { one } else { many };
    format!("{} {w}", thousands(n))
}

fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn u(v: &Value, k: &str) -> usize {
    v.get(k).and_then(Value::as_u64).unwrap_or(0) as usize
}

/// The returned tests under a cap of `cap` (round 2 G11): `picked` is the
/// selection in its global rank (code relation before navigation, then
/// tier, hops, codeunit, display). In `changed` order, each changed
/// routine takes its nearest test (the first in that rank that reaches it);
/// a test already taken for an earlier routine is skipped, so it counts
/// once; the remaining slots fill by the global rank. The result keeps the
/// global rank's order.
fn spread_cap<'v>(picked: &[&'v Value], changed: &[Value], cap: usize) -> Vec<&'v Value> {
    let mut taken: Vec<usize> = Vec::new();
    let reaches = |t: &Value, id: &str| match &t["reaches"] {
        Value::String(r) => r == id,
        Value::Array(rs) => rs.iter().any(|r| r.as_str() == Some(id)),
        _ => false,
    };
    let same = |a: &Value, b: &Value| a["stableId"] == b["stableId"];
    for c in changed {
        if taken.len() >= cap {
            break;
        }
        let id = s(c, "stableId");
        if let Some(i) = picked
            .iter()
            .position(|t| reaches(t, id) && !taken.iter().any(|&j| same(picked[j], t)))
        {
            taken.push(i);
        }
    }
    for (i, t) in picked.iter().enumerate() {
        if taken.len() >= cap {
            break;
        }
        if !taken.iter().any(|&j| same(picked[j], t)) {
            taken.push(i);
        }
    }
    taken.sort_unstable();
    taken.into_iter().map(|i| picked[i]).collect()
}

/// The scope line (§4.6) and whether there is any test scope. Output
/// validation §9.5 (F64): one clause per part — the test apps by name with
/// the tests loaded, the skipped apps counted (the JSON's `scope.skipped`
/// says why); roots, discovery ways and reasons are in the JSON.
fn scope_line(
    roots: &Value,
    parts: &[Part],
    surface: crate::engine::follow_up::Surface,
) -> (String, bool) {
    let scope = &roots["scope"];
    let ws_tests = u(roots, "workspaceTests");
    let apps = scope["apps"].as_array().cloned().unwrap_or_default();
    let mut line = format!(
        "test scope: workspace {}",
        n_of(ws_tests, "[Test] procedure", "[Test] procedures")
    );
    if apps.is_empty() {
        line.push_str(&format!("; {}", s(scope, "summary")));
    } else {
        let mut tests = 0;
        let listed: Vec<String> = apps
            .iter()
            .map(|a| {
                let part = parts
                    .iter()
                    .find(|p| p.app["root"] == a["root"])
                    .and_then(|p| p.result.as_ref().ok());
                tests += part.map_or(0, |v| u(v, "testsLoaded"));
                // §8.1 projection: an approximate projection never reads
                // as plain `local`.
                let approximate =
                    part.is_some_and(|v| s(v, "productionTier") == "localApproximate");
                format!(
                    "{}{}",
                    s(a, "name"),
                    if approximate {
                        ": production source projection approximate"
                    } else {
                        ""
                    }
                )
            })
            .collect();
        line.push_str(&format!(
            "; {} ({}), {}",
            n_of(apps.len(), "test app", "test apps"),
            listed.join(", "),
            n_of(tests, "test", "tests")
        ));
        if scope["complete"] == json!(false) {
            line.push_str(&format!(
                "; discovery incomplete ({})",
                scope["incomplete"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
    }
    let skipped = scope["skipped"].as_array().map_or(0, Vec::len);
    if skipped > 0 {
        line.push_str(&format!(
            "; {} skipped: {} lists why",
            n_of(skipped, "app", "apps"),
            crate::engine::follow_up::Input::FormatJson.on(surface)
        ));
    }
    (line, ws_tests > 0 || !apps.is_empty())
}

/// Render the answer (see the module doc).
#[must_use]
pub fn render(roots: &Value, parts: &[Part], opts: RenderOpts) -> Rendered {
    let (scope_text, any_scope) = scope_line(roots, parts, opts.surface);
    let changed: Vec<Value> = roots["roots"].as_array().cloned().unwrap_or_default();
    let diagnostics: Vec<String> = roots["diagnostics"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| d.as_str().map(str::to_string))
        .collect();
    let stamp = roots["productionSourceStamp"].clone();

    // Per root: the best status over the parts, and its nearest test.
    let rank = |st: &str| match st {
        "reached" => 0,
        "no-known-path" => 1,
        "analysis-incomplete" => 2,
        "not-loaded" => 3,
        "not-in-projection" => 4,
        _ => 5,
    };
    let mut fallback: Vec<Value> = Vec::new();
    let mut add = |reason: &str, count: usize, detail: String| {
        fallback.push(json!({"reason": reason, "count": count, "detail": detail}));
    };
    let ok_parts: Vec<&Value> = parts
        .iter()
        .filter_map(|p| p.result.as_ref().ok())
        .collect();
    let mut changed_out = Vec::new();
    for c in &changed {
        let id = s(c, "stableId");
        let per: Vec<&Value> = ok_parts
            .iter()
            .filter_map(|p| {
                p["roots"]
                    .as_array()
                    .and_then(|rs| rs.iter().find(|r| s(r, "stableId") == id))
            })
            .collect();
        let best = per
            .iter()
            .min_by_key(|r| rank(s(r, "status")))
            .map(|r| s(r, "status").to_string())
            .unwrap_or_else(|| "not-analyzed".to_string());
        let mut out = json!({"stableId": id, "display": c["display"], "status": best,
                             // §8.1, §8.3 (additive): the test-status words.
                             "testStatus": crate::engine::vocab::test_status(&best),
                             "nearestTestedCaller": Value::Null});
        if let Some(r) = per.iter().find(|r| r.get("nearest").is_some()) {
            out["nearest"] =
                json!({"test": r["nearest"], "tier": r["nearestTier"], "hops": r["nearestHops"]});
        }
        changed_out.push(out);
    }
    for st in ["not-in-projection", "not-loaded", "analysis-incomplete"] {
        let n = changed_out.iter().filter(|c| s(c, "status") == st).count();
        if n > 0 {
            add(
                &format!("root-{st}"),
                n,
                match st {
                    "not-in-projection" => "a changed routine is not in any test universe's graph, so no test path to it is known".to_string(),
                    "not-loaded" => "a changed routine has no body in the test universe (its app was not loaded from source)".to_string(),
                    _ => "a changed routine's cone is incomplete (a cap, or a way to run no edge shows) and no test reaches it".to_string(),
                },
            );
        }
    }
    // Every reached test over the parts, ranked; then the run-first subset.
    let mut tests: Vec<Value> = ok_parts
        .iter()
        .flat_map(|p| p["tests"].as_array().cloned().unwrap_or_default())
        .collect();
    let tier_rank = |t: &str| match t {
        "local" => 0,
        "throughDependency" => 1,
        _ => 2,
    };
    // Output validation V2 (§3.3, F13): code paths before UI-navigation ones,
    // then tier, hops, codeunit, name.
    let rel_rank = |t: &Value| usize::from(relation(t) != crate::engine::vocab::RELATION_CODE);
    tests.sort_by(|a, b| {
        (
            rel_rank(a),
            tier_rank(s(a, "tier")),
            u(a, "hops"),
            a["codeunitId"].as_i64().unwrap_or(i64::MAX),
            s(a, "display"),
        )
            .cmp(&(
                rel_rank(b),
                tier_rank(s(b, "tier")),
                u(b, "hops"),
                b["codeunitId"].as_i64().unwrap_or(i64::MAX),
                s(b, "display"),
            ))
    });
    let count = |t: &str| tests.iter().filter(|x| s(x, "tier") == t).count();
    let by_tier = json!({"local": count("local"), "throughDependency": count("throughDependency"),
                         "onlyOpen": count("onlyOpen")});
    let picked: Vec<&Value> = tests
        .iter()
        .filter(|t| opts.tier.takes(s(t, "tier")))
        .collect();
    let shown: Vec<&Value> = match opts.max_tests {
        Some(m) => spread_cap(&picked, &changed_out, m),
        None => picked.clone(),
    };
    // Fallback reasons from the parts.
    let universal = ok_parts.iter().any(|p| p["universalHub"] == json!(true));
    if universal {
        add(
            "universal-hub",
            1,
            "a cone reaches the universal hub (a call no static target resolves): any test may reach the change".into(),
        );
    }
    let mut hidden: Vec<Value> = Vec::new();
    for p in &ok_parts {
        hidden.extend(p["hidden"].as_array().cloned().unwrap_or_default());
    }
    let relevant: Vec<&Value> = hidden
        .iter()
        .filter(|h| h["selectionRelevant"] == json!(true))
        .collect();
    if !relevant.is_empty() {
        let mut by: BTreeMap<String, usize> = BTreeMap::new();
        for h in &relevant {
            *by.entry(s(h, "class").to_string()).or_default() += 1;
        }
        add(
            "hidden-callers",
            relevant.len(),
            format!(
                "the cones hold {} with a way to run no edge shows ({}); one that cannot be shown covered counts",
                n_of(relevant.len(), "node", "nodes"),
                by.iter()
                    .map(|(k, n)| format!("{k} {n}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
    }
    let unresolved: usize = ok_parts.iter().map(|p| u(p, "unresolvedTestCalls")).sum();
    let mut missing: Vec<Value> = Vec::new();
    for p in &ok_parts {
        for m in p["missingDependencies"].as_array().into_iter().flatten() {
            if !missing.contains(m) {
                missing.push(m.clone());
            }
        }
    }
    if unresolved > 0 {
        let names: Vec<&str> = missing.iter().map(|m| s(m, "name")).collect();
        add(
            "unresolved-test-calls",
            unresolved,
            if names.is_empty() {
                format!(
                    "test code makes {} that resolve nowhere; a callee may call back into this code",
                    n_of(unresolved, "call", "calls")
                )
            } else {
                format!(
                    "test code makes {} that resolve nowhere (the test app's dependencies {} are not loaded); a library may call back into this code",
                    n_of(unresolved, "call", "calls"),
                    names.join(", ")
                )
            },
        );
    }
    if roots["scope"]["complete"] == json!(false) {
        add(
            "discovery-incomplete",
            1,
            "test app discovery did not finish: a test app may exist that was not found".into(),
        );
    }
    for p in parts {
        if let Err(e) = &p.result {
            add(
                "test-app-not-analyzed",
                1,
                format!("{}: {e}", s(&p.app, "name")),
            );
        }
    }
    let limited = ok_parts
        .iter()
        .any(|p| p["limits"].as_array().is_some_and(|l| !l.is_empty()));
    if limited {
        add(
            "walk-limited",
            1,
            "a walk stopped at its state budget".into(),
        );
    }
    if !any_scope {
        add("no-test-scope", 0, "no test code was considered".into());
    }
    let complete = fallback.is_empty();
    let exit = if changed.is_empty() { 2 } else { 0 };

    // The next step when the test app's dependencies are missing: printed
    // once (output validation §9.5, F64; agent-tasks §13.3 Q3's action,
    // shortened). The selection-incomplete line points here.
    let missing_names: Vec<&str> = missing.iter().map(|m| s(m, "name")).collect();
    let lacking: Vec<&str> = parts
        .iter()
        .filter(|p| {
            p.result.as_ref().is_ok_and(|v| {
                v["missingDependencies"]
                    .as_array()
                    .is_some_and(|a| !a.is_empty())
            })
        })
        .map(|p| s(&p.app, "root"))
        .collect();
    let into = match lacking.as_slice() {
        [root] => format!(
            "{}/.alpackages",
            std::path::Path::new(root)
                .file_name()
                .map_or_else(|| (*root).to_string(), |n| n.to_string_lossy().into_owned())
        ),
        _ => "each test app's .alpackages".to_string(),
    };
    let next = (!missing_names.is_empty()).then(|| {
        format!(
            "missing test-library symbols: {} ({}) — add them to {into} (\"AL: Download symbols\" in the test app), then ask again{}",
            n_of(missing_names.len(), "app", "apps"),
            missing_names.join(", "),
            if unresolved > 0 {
                ": calls into them then resolve and the selection can tighten"
            } else {
                ""
            }
        )
    });

    // Run first, grouped per codeunit (first-seen order of the ranked list).
    let mut groups: Vec<(String, Vec<&Value>)> = Vec::new();
    for t in &picked {
        let key = format!(
            "Codeunit {} {}",
            t["codeunitId"]
                .as_i64()
                .map_or_else(String::new, |n| n.to_string()),
            t["codeunit"].as_str().unwrap_or("")
        );
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => v.push(t),
            None => groups.push((key, vec![t])),
        }
    }
    // A group's tests by tier (§3.3): `local 0, via-dependency 18`.
    let tiers_of = |v: &[&Value]| -> Vec<(&'static str, usize)> {
        ["local", "via-dependency", "via-hub"]
            .into_iter()
            .map(|w| {
                (
                    w,
                    v.iter()
                        .filter(|t| crate::engine::vocab::tier_key(s(t, "tier")) == w)
                        .count(),
                )
            })
            .collect()
    };
    let codeunit_ids: Vec<String> = groups
        .iter()
        .filter_map(|(_, v)| v[0]["codeunitId"].as_i64().map(|n| n.to_string()))
        .collect();
    let test_apps: Vec<String> = parts
        .iter()
        .filter(|p| p.result.is_ok())
        .map(|p| s(&p.app, "name").to_string())
        .collect();
    // Round-2 r2 §2.2 (G2): the selected tests by relation. A test whose
    // every witness needs UI navigation is not run first as if code called
    // the change: the headline says so whenever any selected test is
    // navigation-only.
    let by_relation = |rel: &str| -> Vec<&Value> {
        picked
            .iter()
            .copied()
            .filter(|t| relation(t) == rel)
            .collect()
    };
    let code_tests = by_relation(crate::engine::vocab::RELATION_CODE);
    let nav_tests = by_relation(crate::engine::vocab::RELATION_NAVIGATION);
    // Round 2 G11: under a cap the run line describes the returned tests
    // (with the full selection's size beside them); the totals,
    // `selectionComplete` and the fallback stay about the full selection.
    let capped = shown.len() < picked.len();
    let shown_of = |rel: &str| -> Vec<&Value> {
        shown
            .iter()
            .copied()
            .filter(|t| relation(t) == rel)
            .collect()
    };
    let (line_code, line_nav) = if capped {
        (
            shown_of(crate::engine::vocab::RELATION_CODE),
            shown_of(crate::engine::vocab::RELATION_NAVIGATION),
        )
    } else {
        (code_tests.clone(), nav_tests.clone())
    };
    let in_all = if capped {
        format!(" ({} selected in all)", thousands(picked.len()))
    } else {
        String::new()
    };
    let codeunits_of = |v: &[&Value]| -> String {
        let mut ids: Vec<String> = Vec::new();
        for t in v {
            if let Some(n) = t["codeunitId"].as_i64().map(|n| n.to_string())
                && !ids.contains(&n)
            {
                ids.push(n);
            }
        }
        ids.join("|")
    };
    let app_words = if test_apps.is_empty() {
        "the workspace app".to_string()
    } else {
        format!("app \"{}\"", test_apps.join("\", \""))
    };
    // §9.5 (F63): what to run, first, in the test app's terms.
    let run_line = if picked.is_empty() {
        format!("run first: no test with a modeled {}", opts.tier.words())
    } else if !line_nav.is_empty() && line_code.is_empty() {
        format!(
            "no modeled code-call witness in the selected scope; {} {} by UI navigation only \
             ({}){in_all} — candidates: codeunits {} (a superset: whole codeunits)",
            n_of(
                line_nav.len(),
                if capped {
                    "returned test reaches"
                } else {
                    "test reaches"
                },
                if capped {
                    "returned tests reach"
                } else {
                    "tests reach"
                }
            ),
            navigation_target(&changed_out),
            navigation_caveat(&changed_out),
            if capped {
                codeunits_of(&shown)
            } else {
                codeunit_ids.join("|")
            },
        )
    } else if !line_nav.is_empty() {
        format!(
            "run first ({app_words}): {} by code calls (codeunits {}; a superset: whole \
             codeunits); {} more by UI navigation only (codeunits {}; {}){in_all}",
            n_of(
                line_code.len(),
                if capped { "returned test" } else { "test" },
                if capped { "returned tests" } else { "tests" }
            ),
            codeunits_of(&line_code),
            thousands(line_nav.len()),
            codeunits_of(&line_nav),
            navigation_caveat(&changed_out),
        )
    } else if capped {
        format!(
            "run first ({app_words}): codeunits {} — a superset: whole codeunits, for the {}{in_all} on a {}",
            codeunits_of(&shown),
            n_of(shown.len(), "returned test", "returned tests"),
            opts.tier.words()
        )
    } else {
        format!(
            "run first ({app_words}): codeunits {} — a superset: whole codeunits, for the {} on a {}",
            codeunit_ids.join("|"),
            n_of(
                picked.len(),
                "selected test procedure",
                "selected test procedures"
            ),
            opts.tier.words()
        )
    } + &if complete {
        "; selection complete".to_string()
    } else {
        // Round 2 G18: the whole suite names its run target.
        let suite_tests: usize = u(roots, "workspaceTests")
            + ok_parts.iter().map(|p| u(p, "testsLoaded")).sum::<usize>();
        let target = if suite_tests == 0 {
            String::new()
        } else {
            format!(": {app_words}, {}", n_of(suite_tests, "test", "tests"))
        };
        format!(
            "; then the whole suite{target} (selection incomplete: {})",
            fallback
                .iter()
                .map(|f| s(f, "reason").to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };

    // Output validation §5.2: the header's facts (human: the four lines;
    // JSON: `result.scope` and `result.selection`, F76).
    let header = {
        use crate::engine::follow_up::Input;
        use crate::engine::result_header::{Analysis, Header};
        let names: Vec<&str> = changed_out.iter().map(|c| s(c, "display")).collect();
        let inputs = match names.len() {
            0 => "no routine resolved".to_string(),
            1 => names[0].to_string(),
            n => format!(
                "{n} changed routines: {}{}",
                names[..3.min(n)].join(", "),
                if n > 3 {
                    format!(", … +{}", n - 3)
                } else {
                    String::new()
                }
            ),
        };
        let mut h = Header::new("tests", inputs, opts.single_app);
        h.answer = if changed.is_empty() {
            "no answer — no changed routine resolved".to_string()
        } else {
            run_line.clone()
        };
        let reasons: Vec<String> = fallback
            .iter()
            .filter_map(|f| {
                let n = u(f, "count");
                Some(match s(f, "reason") {
                    "walk-limited" => "a walk stopped at its state budget".to_string(),
                    "root-analysis-incomplete" => {
                        format!("{} with an incomplete cone", n_of(n, "routine", "routines"))
                    }
                    "root-not-loaded" => format!("{} not loaded", n_of(n, "routine", "routines")),
                    "root-not-in-projection" => {
                        format!("{} not in projection", n_of(n, "routine", "routines"))
                    }
                    "discovery-incomplete" => "test app discovery incomplete".to_string(),
                    "test-app-not-analyzed" => "a test app not analyzed".to_string(),
                    _ => return None,
                })
            })
            .collect();
        h.analysis = if changed.is_empty() {
            Analysis::NotAnalyzed("no changed routine resolved".to_string())
        } else {
            Analysis::of(reasons.is_empty(), reasons)
        };
        h.assumptions = vec![
            "static reachability, not coverage: a reached test may not exercise the changed lines"
                .to_string(),
        ];
        if limited {
            h.limits = vec!["a walk stopped at its state budget".to_string()];
        }
        h.scope = format!("tests on a {}", opts.tier.words());
        let sf = opts.surface;
        if opts.tier == TierPick::Local && !changed.is_empty() {
            h.more.push(format!(
                "{} adds tests whose path passes dependency code",
                Input::TierDependency.on(sf)
            ));
        }
        h.more.push(format!(
            "{} for every selected test and its path",
            Input::FormatJson.on(sf)
        ));
        h
    };
    // `result.selection` (§5.2): complete, the cap asked, returned, total,
    // fallback.
    let selection = json!({
        "complete": complete,
        "capAsked": opts.max_tests,
        "returned": shown.len(),
        "total": picked.len(),
        "fallback": fallback.iter().map(|f| f["reason"].clone()).collect::<Vec<_>>(),
        // Round-2 r2 §2.2 (G2, additive): the selection by relation.
        "byRelation": serde_json::Map::from_iter([
            (crate::engine::vocab::RELATION_CODE.to_string(), json!(code_tests.len())),
            (crate::engine::vocab::RELATION_NAVIGATION.to_string(), json!(nav_tests.len())),
        ]),
    });
    let test_row = |t: &Value| {
        json!({"row": "test", "test": t["display"], "stableId": t["stableId"],
               "codeunitId": t["codeunitId"], "codeunit": t["codeunit"],
               "procedure": t["procedure"],
               "tier": crate::engine::vocab::tier_key(s(t, "tier")), "hops": t["hops"],
               "relation": relation(t)})
    };
    let mut index_rows: Vec<Value> = changed_out
        .iter()
        .map(|c| {
            json!({"row": "routine", "routine": c["display"], "stableId": c["stableId"],
                   "testStatus": c["testStatus"], "nearest": c.get("nearest").cloned()})
        })
        .collect();
    index_rows.extend(shown.iter().map(|t| test_row(t)));
    let summary = crate::engine::result_index::with_index(
        json!({
            "kind": "tests",
            "answer": header.answer,
            "analysis": header.analysis.human(),
            "runTarget": {"testApps": test_apps, "codeunits": codeunit_ids},
            "selectionComplete": complete,
            "fallback": selection["fallback"],
            "total": picked.len(),
            "returned": shown.len(),
            "capAsked": opts.max_tests,
            "truncated": shown.len() < picked.len(),
            "changed": changed_out.len(),
            "selectedTests": shown.iter().map(|t| test_row(t)).collect::<Vec<_>>(),
        }),
        "selectedTests",
        "test",
        index_rows,
    );

    if opts.json {
        let run_first: Vec<Value> = shown
            .iter()
            .map(|t| {
                json!({"stableId": t["stableId"], "codeunitId": t["codeunitId"], "codeunit": t["codeunit"],
                       "procedure": t["procedure"], "file": t["file"], "tier": t["tier"],
                       "class": t["class"], "hops": t["hops"], "reaches": [t["reaches"]],
                       "path": t["path"],
                       // §3.3, §8.3 (additive): the §8.1 words beside `tier`.
                       "pathTier": crate::engine::vocab::tier_key(s(t, "tier")),
                       "relation": relation(t)})
            })
            .collect();
        let doc = json!({
            "schema": "al-sem.tests", "version": 1, "lineBase": 1,
            "scope": {
                "workspaceTests": roots["workspaceTests"],
                "discovery": roots["scope"]["origin"],
                "discoveryComplete": roots["scope"]["complete"],
                "searched": roots["scope"]["searched"],
                "apps": roots["scope"]["apps"].as_array().cloned().unwrap_or_default().iter().map(|a| {
                    let part = parts.iter().find(|p| p.app["root"] == a["root"]).and_then(|p| p.result.as_ref().ok());
                    json!({"name": a["name"], "root": a["root"], "appId": a["appId"], "files": a["files"],
                           "testsLoaded": part.map(|p| p["testsLoaded"].clone()),
                           "found": a["found"], "productionTier": part.map(|p| p["productionTier"].clone()),
                           "universe": part.map(|p| p["universe"].clone())})
                }).collect::<Vec<_>>(),
                "skipped": roots["scope"]["skipped"],
            },
            "productionSourceStamp": stamp,
            "changed": changed_out,
            "diagnostics": diagnostics,
            "runFirst": run_first,
            "runFirstTotal": picked.len(),
            "tier": match opts.tier { TierPick::Local => "local", TierPick::Dependency => "dependency", TierPick::All => "all" },
            "byTier": by_tier,
            // §3.3 (additive): each codeunit group's tests by tier.
            "groups": groups.iter().map(|(_, v)| json!({
                "codeunitId": v[0]["codeunitId"], "codeunit": v[0]["codeunit"],
                "tests": v.len(),
                "tiers": tiers_of(v).into_iter()
                    .map(|(k, n)| (k.to_string(), json!(n)))
                    .collect::<serde_json::Map<String, Value>>(),
            })).collect::<Vec<_>>(),
            "runFirstLine": run_line,
            "requiredFallback": fallback,
            "selectionComplete": complete,
            "next": next,
            "hidden": hidden,
            "assumptions": ["static reachability, not coverage: a reached test may not exercise the changed lines"],
            "limits": [],
            // Output validation V4 (F76, additive): the header's facts.
            "result": {
                "scope": header.scope_json(serde_json::Map::new()),
                "selection": selection,
            },
        });
        let doc = if opts.schema >= 2 { v2_of(doc) } else { doc };
        return Rendered {
            text: serde_json::to_string_pretty(&doc).expect("serializes") + "\n",
            exit,
            summary,
        };
    }

    // §5.2: the run line is the header's `answer:`.
    let mut out: Vec<String> = vec![header.human().trim_end().to_string()];
    out.push(scope_text);
    if !any_scope {
        out.push(format!(
            "test scope: none — no [Test] procedure in the workspace app, and {}. No test was considered. This does not mean no tests are needed.",
            s(&roots["scope"], "summary")
        ));
    }
    for d in &diagnostics {
        out.push(d.clone());
    }
    if changed.is_empty() {
        out.push("changed: no routine resolved, so no test was selected".into());
        return Rendered {
            text: out.join("\n") + "\n",
            exit,
            summary,
        };
    }
    // Output validation §9.5 (F82): said only when it differs (the JSON's
    // `productionSourceStamp` per part says it either way).
    if ok_parts.iter().any(|p| p["productionSourceStamp"] != stamp) {
        out.push(
            "test universe: NOT the same production source as the main engine (a test universe was built from other bytes; ask again)"
                .to_string(),
        );
    }
    let shown_changed: Vec<String> = changed_out
        .iter()
        .take(8)
        .map(|c| {
            format!(
                "{} — {}",
                s(c, "display"),
                crate::engine::vocab::test_status(s(c, "status"))
            )
        })
        .collect();
    out.push(format!(
        "changed: {} ({}{})",
        n_of(changed_out.len(), "routine", "routines"),
        shown_changed.join("; "),
        if changed_out.len() > 8 {
            format!("; … +{}", changed_out.len() - 8)
        } else {
            String::new()
        }
    ));
    if picked.is_empty() {
        // §3.3: the nearest test is labelled with its tier and as not a
        // focused test.
        let nearest = changed_out
            .iter()
            .find_map(|c| c.get("nearest").cloned())
            .map(|n| {
                format!(
                    "; nearest [{}]: {} ({} hops) (not a focused test)",
                    match s(&n, "tier") {
                        "" => "local",
                        t => crate::engine::vocab::tier_key(t),
                    },
                    s(&n, "test"),
                    n["hops"]
                )
            })
            .unwrap_or_default();
        out.push(format!("tests on a {}: none{nearest}", opts.tier.words()));
    } else {
        out.push(format!(
            "tests on a {}: {} in {} — by relation: code {}, UI navigation {} (code paths first, \
             then by tier and hops):",
            opts.tier.words(),
            n_of(picked.len(), "test", "tests"),
            n_of(groups.len(), "codeunit", "codeunits"),
            thousands(code_tests.len()),
            thousands(nav_tests.len())
        ));
        for (k, v) in groups.iter().take(5) {
            let first = v[0];
            let path: Vec<String> = first["path"]
                .as_array()
                .into_iter()
                .flatten()
                .skip(1)
                .filter_map(|p| p.as_str().map(str::to_string))
                .collect();
            // §3.3 (F13): the group's tier distribution when its tests
            // differ in tier, its representative path's tier and relation.
            let tiers: Vec<(&str, usize)> = tiers_of(v);
            let mixed = tiers.iter().filter(|(_, n)| *n > 0).count() > 1;
            let dist = if mixed {
                format!(
                    ": {}",
                    tiers
                        .iter()
                        .map(|(w, n)| format!("{w} {n}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            } else {
                String::new()
            };
            // Round-2 r2 §2.2 (§5): tests mix relations, so each item
            // prints both axes; the navigation caveat is the headline's.
            out.push(format!(
                "  {k}: {}{dist}; first [{}, {}] {} (first: {}; via {})",
                n_of(v.len(), "test", "tests"),
                crate::engine::vocab::tier_key(s(first, "tier")),
                relation(first),
                n_of(u(first, "hops"), "hop", "hops"),
                first["procedure"].as_str().unwrap_or(s(first, "display")),
                path.join(" → ")
            ));
        }
        if groups.len() > 5 {
            let rest: usize = groups.iter().skip(5).map(|g| g.1.len()).sum();
            out.push(format!(
                "  … +{} ({})",
                n_of(groups.len() - 5, "codeunit", "codeunits"),
                n_of(rest, "test", "tests")
            ));
        }
    }
    let thr = count("throughDependency");
    let tier_note = format!(
        " ({} lists them)",
        crate::engine::follow_up::Input::TierDependency.on(opts.surface)
    );
    if thr > 0 && opts.tier == TierPick::Local {
        let same = ok_parts.iter().any(|p| {
            p["roots"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|r| r["throughDependency"]["sameness"] == json!(true))
        });
        out.push(format!(
            "[via-dependency] {}{}",
            n_of(thr, "test", "tests"),
            if same {
                // Round 2 G18: the input that gives their codeunit filter.
                format!(
                    " — effectively the suite's posting tests, not specific to this change; {} \
                     gives the {} tests' codeunit filter",
                    crate::engine::follow_up::Input::TierDependency.on(opts.surface),
                    thousands(thr)
                )
            } else {
                tier_note.clone()
            }
        ));
    }
    if complete {
        out.push("selection complete".into());
    } else {
        out.push(format!(
            "selection incomplete — run the whole suite as well (requiredFallback): {}",
            fallback
                .iter()
                .map(|f| {
                    // F64: the missing symbols are named once, below.
                    if s(f, "reason") == "unresolved-test-calls" && next.is_some() {
                        format!(
                            "test code makes {} that resolve nowhere (missing test-library symbols, below); a library may call back into this code",
                            n_of(u(f, "count"), "call", "calls")
                        )
                    } else {
                        s(f, "detail").to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    if let Some(n) = &next {
        out.push(n.clone());
    }
    // "Static reachability, not coverage" is the header's assumption (§5.2).
    Rendered {
        text: out.join("\n") + "\n",
        exit,
        summary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Output validation V5 (§7.4, F44): `hidden` grouped by class states
    /// each class's reason once, and a node whose reason, channel or
    /// relevance differs keeps its own, so the grouping loses nothing: the
    /// version 1 list comes back (as a set: the groups are in class order).
    /// Hand-stated rows: one class whose nodes share a reason but not a
    /// channel, and a node of it with its own reason. Discrimination
    /// (2026-11-06): leaving every shared field off the node whatever its
    /// value (no `group.get(k) == Some(v)` check) fails at the node's own
    /// reason; restored, it passes.
    #[test]
    fn hidden_by_class_states_the_reason_once_and_loses_nothing() {
        let row = |node: &str, class: &str, ch: &str, reason: &str, rel: bool| {
            json!({"node": node, "nodeId": node, "channel": ch, "class": class,
                   "selectionRelevant": rel, "reason": reason})
        };
        let v1 = vec![
            row("A", "page-opened", "page", "a page a user opens", true),
            row("B", "page-opened", "page", "a page a user opens", true),
            row(
                "C",
                "page-opened",
                "run",
                "opened by a run of a variable",
                true,
            ),
            row("D", "event", "event", "an event no subscriber shows", false),
        ];
        let g = hidden_v2(&v1);
        assert_eq!(g["page-opened"]["reason"], "a page a user opens", "{g:#}");
        assert_eq!(
            g["page-opened"]["nodes"][0],
            json!({"node": "A", "nodeId": "A"})
        );
        assert_eq!(
            g["page-opened"]["nodes"][2]["reason"],
            "opened by a run of a variable"
        );
        assert_eq!(g["page-opened"]["nodes"][2]["channel"], "run");
        let key = |v: &Value| v.to_string();
        let mut back: Vec<String> = hidden_v1(&g).iter().map(key).collect();
        let mut want: Vec<String> = v1.iter().map(key).collect();
        back.sort();
        want.sort();
        assert_eq!(back, want);
    }

    /// Output validation V2 (§3.3, F13): a code path ranks ahead of a path
    /// that needs UI navigation, whatever their tiers: a `local` test whose
    /// path opens a page comes after a `via-dependency` code path. Hand-stated
    /// parts (two reached tests of one root, the navigation one with the
    /// better tier and fewer hops). Discrimination (2026-11-06): see the
    /// commit message.
    #[test]
    fn code_paths_rank_before_navigation_paths() {
        let roots = json!({
            "roots": [{"stableId": "r1", "display": "Codeunit X::R"}],
            "scope": {"apps": [], "summary": "none", "complete": true},
            "workspaceTests": 1,
        });
        let nav = json!({"stableId": "t1", "codeunitId": 50001, "codeunit": "Nav Tests",
            "procedure": "OpensPage", "tier": "local", "hops": 2, "reaches": "r1",
            "display": "Codeunit Nav Tests::OpensPage",
            "path": ["Codeunit Nav Tests::OpensPage", "Page Card", "Page Card.action(Go)::OnAction", "Codeunit X::R"]});
        let code = json!({"stableId": "t2", "codeunitId": 50002, "codeunit": "Code Tests",
            "procedure": "CallsIt", "tier": "throughDependency", "hops": 5, "reaches": "r1",
            "display": "Codeunit Code Tests::CallsIt",
            "path": ["Codeunit Code Tests::CallsIt", "Codeunit Dep (9)::Run", "Codeunit X::R"]});
        let part = Part {
            app: json!({"name": "T", "root": "t"}),
            result: Ok(json!({"roots": [{"stableId": "r1", "status": "reached"}],
                              "tests": [nav, code]})),
        };
        let r = render(
            &roots,
            &[part],
            RenderOpts {
                tier: TierPick::All,
                max_tests: None,
                json: true,
                surface: Default::default(),
                single_app: false,
                schema: 2,
            },
        );
        let v: Value = serde_json::from_str(&r.text).unwrap();
        let first = &v["runFirst"][0];
        assert_eq!(first["stableId"], "t2", "{v:#}");
        assert_eq!(first["relation"], "code", "{v:#}");
        assert_eq!(v["runFirst"][1]["relation"], "UI navigation", "{v:#}");
    }

    /// Round-2 r2 §2.2 (G2): a field trigger reached only through a page on
    /// its table does not get a `run first` headline as if code called it:
    /// the headline says there is no code-call witness in the selected
    /// scope, how many tests reach it by UI navigation only and the user
    /// step that still needs; each item prints both axes. With a code path
    /// too, the code tests come first and the navigation ones are counted
    /// apart. Hand-stated parts (one local navigation path through a bare
    /// page object; then a code path added).
    ///
    /// DISCRIMINATION (2026-11-07): building the headline as before
    /// (`run first (…): codeunits …` whatever the relations, the two
    /// relation arms removed) fails the first assert.
    #[test]
    fn a_navigation_only_selection_says_so_in_the_headline() {
        let roots = json!({
            "roots": [{"stableId": "r1", "display": "Table \"Ledger\".\"No.\"::OnValidate"}],
            "scope": {"apps": [], "summary": "none", "complete": true},
            "workspaceTests": 1,
        });
        let nav = json!({"stableId": "t1", "codeunitId": 50001, "codeunit": "Nav Tests",
            "procedure": "EditsNo", "tier": "local", "hops": 3, "reaches": "r1",
            "display": "Codeunit Nav Tests::EditsNo",
            "path": ["Codeunit Nav Tests::EditsNo", "Page \"Ledger Card\"", "Table \"Ledger\"", "Table \"Ledger\".\"No.\"::OnValidate"]});
        let code = json!({"stableId": "t2", "codeunitId": 50002, "codeunit": "Code Tests",
            "procedure": "ValidatesNo", "tier": "local", "hops": 1, "reaches": "r1",
            "display": "Codeunit Code Tests::ValidatesNo",
            "path": ["Codeunit Code Tests::ValidatesNo", "Table \"Ledger\".\"No.\"::OnValidate"]});
        let opts = |json: bool| RenderOpts {
            tier: TierPick::Local,
            max_tests: None,
            json,
            surface: Default::default(),
            single_app: false,
            schema: 2,
        };
        let part = |tests: Vec<Value>| Part {
            app: json!({"name": "T", "root": "t"}),
            result: Ok(json!({"roots": [{"stableId": "r1", "status": "reached"}],
                              "tests": tests})),
        };
        let human = render(&roots, &[part(vec![nav.clone()])], opts(false)).text;
        assert!(
            human.contains(
                "answer: no modeled code-call witness in the selected scope; 1 test reaches this \
                 trigger by UI navigation only (each opens a page on the field's table; whether \
                 the page shows or edits the field is not established) — candidates: codeunits \
                 50001 (a superset: whole codeunits)"
            ),
            "{human}"
        );
        assert!(
            human.contains("by relation: code 0, UI navigation 1"),
            "{human}"
        );
        assert!(
            human.contains("first [local, UI navigation] 3 hops"),
            "{human}"
        );
        let v: Value =
            serde_json::from_str(&render(&roots, &[part(vec![nav.clone()])], opts(true)).text)
                .unwrap();
        assert_eq!(
            v["result"]["selection"]["byRelation"],
            json!({"code": 0, "UI navigation": 1}),
            "{v:#}"
        );
        let mixed = render(&roots, &[part(vec![nav, code])], opts(false)).text;
        assert!(
            mixed.contains(
                "answer: run first (app \"T\"): 1 test by code calls (codeunits 50002; a \
                 superset: whole codeunits); 1 more by UI navigation only (codeunits 50001;"
            ),
            "{mixed}"
        );
    }

    /// Round 2 G11: under a cap the returned tests spread over the changed
    /// routines. Hand-stated: routine r1 has three near tests, r2 one far
    /// one; a cap of 2 returns one test per routine, not r1's two nearest.
    /// A test nearest to both routines (t0, reaching r1 and r2) is taken
    /// once, for r1, and r2 still gets its own. The run line describes the
    /// returned tests, the totals the full selection.
    #[test]
    fn a_capped_selection_spreads_over_the_changed_routines() {
        let roots = json!({
            "roots": [{"stableId": "r1", "display": "Codeunit X::A"},
                      {"stableId": "r2", "display": "Codeunit X::B"}],
            "scope": {"apps": [], "summary": "none", "complete": true},
            "workspaceTests": 4,
        });
        let test = |id: &str, cu: i64, hops: u64, reaches: Value| {
            json!({"stableId": id, "codeunitId": cu, "codeunit": format!("T{cu}"),
                "procedure": id, "tier": "local", "hops": hops, "reaches": reaches,
                "display": format!("Codeunit T{cu}::{id}"),
                "path": [format!("Codeunit T{cu}::{id}"), "Codeunit X::A"]})
        };
        let opts = RenderOpts {
            tier: TierPick::Local,
            max_tests: Some(2),
            json: true,
            surface: Default::default(),
            single_app: false,
            schema: 2,
        };
        let part = |tests: Vec<Value>| Part {
            app: json!({"name": "T", "root": "t"}),
            result: Ok(json!({"roots": [{"stableId": "r1", "status": "reached"},
                                        {"stableId": "r2", "status": "reached"}],
                              "tests": tests})),
        };
        let returned = |tests: Vec<Value>| -> (Vec<String>, Value) {
            let v: Value =
                serde_json::from_str(&render(&roots, &[part(tests)], opts).text).unwrap();
            let ids = v["runFirst"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["stableId"].as_str().unwrap().to_string())
                .collect();
            (ids, v)
        };
        let base = vec![
            test("t1", 50001, 1, json!("r1")),
            test("t2", 50002, 2, json!("r1")),
            test("t3", 50003, 3, json!("r1")),
            test("t4", 50004, 4, json!("r2")),
        ];
        let (ids, v) = returned(base.clone());
        assert_eq!(ids, ["t1", "t4"], "{v:#}");
        assert_eq!(v["runFirstTotal"], 4, "{v:#}");
        let line = v["runFirstLine"].as_str().unwrap();
        assert!(
            line.contains("codeunits 50001|50004")
                && line.contains("for the 2 returned tests (4 selected in all)"),
            "{line}"
        );
        let mut shared = base;
        shared.insert(0, test("t0", 50000, 0, json!(["r1", "r2"])));
        let (ids, v) = returned(shared);
        assert_eq!(ids, ["t0", "t4"], "t0 counts once: {v:#}");
    }
}
