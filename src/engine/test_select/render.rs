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

/// The scope line (§4.6) and whether there is any test scope.
fn scope_line(roots: &Value, parts: &[Part]) -> (String, bool) {
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
        let listed: Vec<String> = apps
            .iter()
            .map(|a| {
                let part = parts.iter().find(|p| p.app["root"] == a["root"]);
                let loaded = part
                    .and_then(|p| p.result.as_ref().ok())
                    .map(|v| {
                        format!(
                            "; {} loaded; production app loaded as local source, tier {}",
                            n_of(u(v, "testsLoaded"), "test", "tests"),
                            s(v, "productionTier")
                        )
                    })
                    .unwrap_or_default();
                let how = match s(a, "found") {
                    "auto" => "auto-discovered",
                    "configured" => "configured in .al-sem.json",
                    _ => "named in the call",
                };
                format!("{} ({}, {how}{loaded})", s(a, "name"), s(a, "root"))
            })
            .collect();
        line.push_str(&format!(
            "; {} — {}",
            n_of(apps.len(), "test app", "test apps"),
            listed.join(", ")
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
    let skipped = scope["skipped"].as_array().cloned().unwrap_or_default();
    if !skipped.is_empty() {
        // Grouped by reason, in first-seen order.
        let mut by: Vec<(String, Vec<String>)> = Vec::new();
        for k in &skipped {
            let d = s(k, "detail").to_string();
            let name = if s(k, "name").is_empty() {
                s(k, "root").to_string()
            } else {
                s(k, "name").to_string()
            };
            match by.iter_mut().find(|(r, _)| *r == d) {
                Some((_, v)) => v.push(name),
                None => by.push((d, vec![name])),
            }
        }
        line.push_str("; skipped: ");
        line.push_str(
            &by.iter()
                .map(|(d, v)| format!("{} ({d})", v.join(", ")))
                .collect::<Vec<_>>()
                .join("; "),
        );
    }
    (line, ws_tests > 0 || !apps.is_empty())
}

/// Render the answer (see the module doc).
#[must_use]
pub fn render(roots: &Value, parts: &[Part], opts: RenderOpts) -> Rendered {
    let (scope_text, any_scope) = scope_line(roots, parts);
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
        Some(m) => picked.iter().copied().take(m).collect(),
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

    // The next step when the standard test libraries are missing.
    let ms_missing: Vec<&str> = missing
        .iter()
        .filter(|m| s(m, "publisher").eq_ignore_ascii_case("microsoft"))
        .map(|m| s(m, "name"))
        .collect();
    let next = (!ms_missing.is_empty()).then(|| {
        format!(
            "next: the test app depends on Microsoft's {}, not loaded here. Put their symbols in the test app's .alpackages (in VS Code: open the test app and run \"AL: Download symbols\"; or copy Microsoft_<name>_<version>.app from a BC artifact's Applications folder), then ask again: calls into them then resolve{}",
            ms_missing.join(", "),
            if unresolved > 0 {
                " and the selection can tighten"
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
    // §9.5 (F63): what to run, first, in the test app's terms.
    let run_line = if picked.is_empty() {
        format!("run first: no test with a modeled {}", opts.tier.words())
    } else {
        format!(
            "run first ({}): codeunits {} — a superset: whole codeunits, for the {} on a {}",
            if test_apps.is_empty() {
                "the workspace app".to_string()
            } else {
                format!("app \"{}\"", test_apps.join("\", \""))
            },
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
        format!(
            "; then the whole suite (selection incomplete: {})",
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
    });

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
        return Rendered {
            text: serde_json::to_string_pretty(&doc).expect("serializes") + "\n",
            exit,
        };
    }

    // §5.2: the run line is the header's `answer:`.
    let mut out: Vec<String> = vec![header.human().trim_end().to_string()];
    out.push(scope_text.replace(
        "tier localApproximate",
        "tier local (projection: approximate)",
    ));
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
        };
    }
    if !ok_parts.is_empty() {
        let same = ok_parts.iter().all(|p| p["productionSourceStamp"] == stamp);
        out.push(if same {
            "production bytes: the same in the production and test universes".to_string()
        } else {
            "production bytes: DIFFER between the production and a test universe".to_string()
        });
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
            "tests on a {}: {} in {} (code paths first, then UI navigation; then by tier and hops):",
            opts.tier.words(),
            n_of(picked.len(), "test", "tests"),
            n_of(groups.len(), "codeunit", "codeunits")
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
            let nav = if relation(first) == crate::engine::vocab::RELATION_NAVIGATION {
                "; this witness requires UI navigation; invocation of the action is not established"
            } else {
                ""
            };
            out.push(format!(
                "  {k}: {}{dist}; first [{}] {} (first: {}; via {}){nav}",
                n_of(v.len(), "test", "tests"),
                crate::engine::vocab::tier_key(s(first, "tier")),
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
                " — effectively the suite's posting tests, not specific to this change"
            } else {
                tier_note.as_str()
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
                .map(|f| s(f, "detail").to_string())
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            },
        );
        let v: Value = serde_json::from_str(&r.text).unwrap();
        let first = &v["runFirst"][0];
        assert_eq!(first["stableId"], "t2", "{v:#}");
        assert_eq!(first["relation"], "code", "{v:#}");
        assert_eq!(v["runFirst"][1]["relation"], "UI navigation", "{v:#}");
    }
}
