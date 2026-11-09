//! `alsem_status`'s freshness check (agent-tasks design §2): the disk
//! compared with the input descriptor each engine's worker published for
//! its revision (§1.2 M0-6), in the MCP process, on a checker thread.
//!
//! **Control path (§2.2).** The reader loop takes the published
//! descriptors from the workers' snapshots and hands them to a checker
//! thread, which answers the status call when every check is done; the
//! reader loop keeps reading meanwhile. The check never goes through an
//! engine's front end, so a busy child, a busy worker or a running build
//! cannot delay it, and it never touches the child's session: it is not a
//! `pin` (it builds nothing, clears no `dirty` path, counts no fresh hit).
//!
//! **The check.** [`Manifest::check_against`] the descriptor's manifest:
//! stat-cached (`hashing: cached`, a file is read only when its length or
//! modification time moved), or every file hashed with `verify`. Each check
//! has a deadline (5 s cached, 30 s verify); one that does not finish, or
//! that meets an input it cannot read, is `complete: false` with the reason
//! and never `fresh`.
//!
//! **Honesty (§2.4).** `fresh: true` only after a complete check in this
//! call, bound to `checkedRevision`. The check sees the disk: unsaved
//! editor buffers are not inputs, and a rewrite that keeps a file's length
//! and modification time is seen only with `verify`. While a call builds,
//! the check compares with the last published revision and says so.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Map, Value, json};

use crate::engine::resident::manifest::{Ident, InputChange, Manifest};
use crate::engine::resident::session::InputDescriptor;

/// The status JSON's version (§2.3).
pub const STATUS_VERSION: u64 = 2;

/// A cached check's deadline.
pub const CACHED_DEADLINE: Duration = Duration::from_secs(5);
/// A `verify` check's deadline.
pub const VERIFY_DEADLINE: Duration = Duration::from_secs(30);

/// The most changes listed per engine.
pub const MAX_CHANGES: usize = 50;

/// How one check runs.
#[derive(Clone, Copy, Debug)]
pub struct CheckOptions {
    /// Hash every input (`verify: true`), not only those whose stat moved.
    pub verify: bool,
    pub deadline: Duration,
    /// A test's delay before the observation (the `status-check` hook).
    pub delay: Duration,
}

impl CheckOptions {
    /// The defaults for `verify`, with the test hook's delay and deadline
    /// when one is set.
    #[must_use]
    pub fn new(verify: bool) -> CheckOptions {
        let o = CheckOptions {
            verify,
            deadline: if verify {
                VERIFY_DEADLINE
            } else {
                CACHED_DEADLINE
            },
            delay: Duration::ZERO,
        };
        // A shipped build has no test hook, so `o` is returned as it is
        // (a `let mut o` there would be an unused `mut`).
        #[cfg(any(feature = "engine-test-hooks", debug_assertions))]
        if let Some(crate::engine::host::test_hooks::Hook::StatusCheck {
            delay_ms,
            deadline_ms,
        }) = crate::engine::host::test_hooks::from_env()
        {
            return CheckOptions {
                delay: Duration::from_millis(delay_ms),
                deadline: Duration::from_millis(deadline_ms),
                ..o
            };
        }
        o
    }
}

fn unix_ms(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// The `freshness` block of a command answer's header (engine limits wave 0,
/// B8; design §5.1, §5.3): what was asked, what was served, and what the
/// check behind it cannot see. `served` names the check's strength, so an
/// answer that may be stale is never labelled plainly `fresh`:
/// `latest-built` when the answer came from an older revision (`stale`),
/// `verified` after a `verify` (every input hashed), else
/// `fresh-by-size-and-time` (a file whose length and modification time did
/// not move was trusted; a same-size, same-time rewrite is missed until a
/// `verify`, design §5.3's stated residual). `residual` says what the check
/// cannot see, as the status answer's `residual` does for the same strength.
#[must_use]
pub fn answer_freshness(requested: crate::mcp::tools::Freshness, stale: bool) -> Value {
    use crate::mcp::tools::Freshness;
    let verify = requested == Freshness::Verify;
    let (served, residual) = if stale {
        (
            "latest-built",
            "files changed since this revision was built (changedPaths lists them)",
        )
    } else if verify {
        ("verified", "unsaved editor buffers are not inputs")
    } else {
        (
            "fresh-by-size-and-time",
            "a rewrite that keeps a file's size and modification time is seen only with \
             freshness: verify; unsaved editor buffers are not inputs",
        )
    };
    json!({
        "requested": requested.as_str(),
        "served": served,
        "residual": residual,
    })
}

/// The refresh class the changed categories imply (§2.3's table), from the
/// session's own rules (`program::session`): a package, the root `app.json`
/// or a source dependency is `Package` (the dependency key or the app's
/// identity moves); a `.al` file added, removed or renamed is
/// `Declaration`; a `roots.config.json`, coverage config or nested
/// `app.json` change alone is `BodyOnly` (no file's projection moves); a
/// `.al` edit is `BodyOnly` or `Declaration`, known only after parsing it.
/// `None` when nothing changed.
#[must_use]
pub fn refresh_class(categories: &BTreeSet<&str>) -> Option<Value> {
    if categories.is_empty() {
        return None;
    }
    let package: Vec<&str> = categories
        .iter()
        .copied()
        .filter(|c| {
            matches!(
                *c,
                "package"
                    | "package-list"
                    | "unreadable-package-folder"
                    | "app-json"
                    | "source-dependency"
                    | "injected-source"
                    | "injected-packages"
                    | "meta"
                    | "other"
            )
        })
        .collect();
    let (class, certain, reason) = if !package.is_empty() {
        (
            "Package",
            true,
            format!(
                "{} changed: the dependencies or the app's identity are read again",
                package.join(", ")
            ),
        )
    } else if categories.contains("al-walk") {
        (
            "Declaration",
            true,
            "a .al file was added, removed or renamed".to_string(),
        )
    } else if categories.contains("al-file") {
        (
            "BodyOnly|Declaration",
            false,
            "only .al file contents changed; whether a declaration changed is known after parsing them"
                .to_string(),
        )
    } else {
        let names: Vec<&str> = categories.iter().copied().collect();
        (
            "BodyOnly",
            true,
            format!(
                "only {} changed: no file is parsed or resolved again",
                names.join(", ")
            ),
        )
    };
    Some(json!({"class": class, "certain": certain, "reason": reason}))
}

/// The class as a headline says it.
fn class_words(class: &str) -> &str {
    match class {
        "BodyOnly|Declaration" => "BodyOnly or Declaration (known after parsing them)",
        c => c,
    }
}

/// What changed, in a few words per category, for the headline.
fn summary(changes: &[InputChange]) -> String {
    let mut by: Vec<(&str, usize)> = Vec::new();
    for c in changes {
        match by.iter_mut().find(|(k, _)| *k == c.category) {
            Some((_, n)) => *n += 1,
            None => by.push((c.category, 1)),
        }
    }
    let plural = |n: usize, one: &str, many: &str| {
        if n == 1 {
            format!("1 {one}")
        } else {
            format!("{n} {many}")
        }
    };
    let words: Vec<String> = by
        .into_iter()
        .map(|(cat, n)| match cat {
            "al-file" => plural(n, ".al file", ".al files"),
            "al-walk" => plural(n, ".al file added or removed", ".al files added or removed"),
            "app-json" => "app.json".to_string(),
            "app-json-paths" => "the app.json files under the root".to_string(),
            "roots-config" => "roots.config.json".to_string(),
            "coverage-config" => "al-sem.coverage.yaml".to_string(),
            "source-dependency" => plural(n, "source-dependency input", "source-dependency inputs"),
            "injected-source" => plural(
                n,
                "production source input (injected into the test universe)",
                "production source inputs (injected into the test universe)",
            ),
            "injected-packages" => plural(
                n,
                "package from the production root",
                "packages from the production root",
            ),
            "package" => plural(n, "package", "packages"),
            "package-list" => plural(n, "package added or removed", "packages added or removed"),
            "unreadable-package-folder" => "the unreadable package folders".to_string(),
            "meta" => "the engine or grammar version".to_string(),
            other => format!("{n} {other}"),
        })
        .collect();
    words.join(", ")
}

/// One engine's check against its published `descriptor` (§2.3's `check`).
#[must_use]
pub fn check(descriptor: &InputDescriptor, opts: CheckOptions) -> Value {
    let started_at = SystemTime::now();
    let start = Instant::now();
    let checked = json!({
        "generation": descriptor.generation,
        "inputsStamp": descriptor.inputs_stamp,
    });
    let hashing = if opts.verify { "all" } else { "cached" };
    let incomplete = |reason: String| {
        json!({
            "checkedRevision": checked,
            "startedAtUnixMs": unix_ms(started_at),
            "finishedAtUnixMs": unix_ms(SystemTime::now()),
            "checkMs": start.elapsed().as_secs_f64() * 1000.0,
            "hashing": hashing,
            "complete": false,
            "incompleteReasons": [reason],
            "fresh": false,
        })
    };
    let published = match Manifest::from_descriptor_entries(&descriptor.manifest) {
        Ok(m) => m,
        Err(e) => return incomplete(format!("the published descriptor does not decode: {e}")),
    };
    let single_app = descriptor.universe == "singleApp";
    let workspace = descriptor.workspace.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let observer = std::thread::Builder::new()
        .name("alsem-status-check".into())
        .spawn({
            let published = published.clone();
            let workspace = workspace.clone();
            move || {
                std::thread::sleep(opts.delay);
                let _ = tx.send(Manifest::check_against(
                    &published,
                    &workspace,
                    single_app,
                    opts.verify,
                ));
            }
        });
    if let Err(e) = observer {
        return incomplete(format!("the check could not start: {e}"));
    }
    // A check past its deadline answers now; its thread finishes on its own.
    let observed = match rx.recv_timeout(opts.deadline) {
        Ok(m) => m,
        Err(_) => {
            return incomplete(format!(
                "the check did not finish within {} ms",
                opts.deadline.as_millis()
            ));
        }
    };
    let mut reasons: Vec<String> = observed
        .unreadable()
        .into_iter()
        .map(|(k, why)| format!("{k} could not be read: {why}"))
        .collect();
    reasons.truncate(10);
    let complete = reasons.is_empty();
    let all = published.changes(&observed, &workspace);
    let categories: BTreeSet<&str> = all.iter().map(|c| c.category).collect();
    let class = refresh_class(&categories);
    let truncated = all.len() > MAX_CHANGES;
    let headline_summary = summary(&all);
    let changes: Vec<&InputChange> = all.iter().take(MAX_CHANGES).collect();
    let counts = counts(&observed);
    json!({
        "checkedRevision": checked,
        "startedAtUnixMs": unix_ms(started_at),
        "finishedAtUnixMs": unix_ms(SystemTime::now()),
        "checkMs": start.elapsed().as_secs_f64() * 1000.0,
        "hashing": hashing,
        // §4.5 (additive, statusVersion stays 2): how hard the check was.
        "strength": if opts.verify { "hashed" } else { "size-and-time" },
        "filesHashed": observed.hashed().len(),
        "complete": complete,
        "incompleteReasons": reasons,
        "fresh": complete && all.is_empty(),
        "changeCount": all.len(),
        "categories": categories,
        "changes": changes,
        "changesTruncated": truncated,
        "refreshClass": class,
        // §9.6 (F65, additive, statusVersion stays 2): every category.
        "inputs": counts,
        "summary": headline_summary,
        "residual": if opts.verify {
            "unsaved editor buffers are not inputs"
        } else {
            "a rewrite that keeps size and modification time is seen only with verify: true; unsaved editor buffers are not inputs"
        },
    })
}

/// The `test-config` input (§2.3, milestone C item 3): the `.al-sem.json`
/// `testApps` a production engine's last `alsem_tests` answer was discovered
/// under (`recorded`), compared with the file now. It is no input of the
/// engine's revision (scope discovery runs on every tests answer), so it
/// never makes the engine stale and implies no refresh class: `fresh` and
/// `refreshClass` keep the engine's own inputs' verdict, and a change is
/// listed with category `test-config`, counted in `changeCount`, and said in
/// `testScope` and the headline ("the next alsem_tests rediscovers").
pub fn apply_test_config(check: &mut Value, workspace: &str, recorded: &Value) {
    let now = crate::engine::test_select::scope::read_test_config_identity(std::path::Path::new(
        workspace,
    ));
    let changed = now != *recorded;
    check["testScope"] = json!({
        "atLastTestsAnswer": recorded,
        "now": now,
        "changed": changed,
    });
    if !changed {
        return;
    }
    let spell = |v: &Value| match v {
        Value::Null => "absent".to_string(),
        v => v.to_string(),
    };
    let change = json!({
        "category": "test-config",
        "path": ".al-sem.json",
        "detail": format!(
            "testApps was {} at the last alsem_tests answer, now {}: test scope changed; the next alsem_tests rediscovers",
            spell(recorded),
            spell(&now)
        ),
    });
    if let Some(list) = check["changes"].as_array_mut() {
        list.push(change);
    }
    if let Some(cats) = check["categories"].as_array_mut() {
        cats.push(json!("test-config"));
    }
    if let Some(n) = check["changeCount"].as_u64() {
        check["changeCount"] = json!(n + 1);
    }
    if check["refreshClass"].is_null() {
        check["refreshClass"] = json!({
            "class": Value::Null,
            "certain": true,
            "reason": "not a refresh of this engine: test scope changed; the next alsem_tests rediscovers",
        });
    }
}

/// The input category of a manifest key that names one file, by its JSON
/// name, or `None` for a key that is no file (a listing, `meta:`, the
/// injected roots).
fn input_category(key: &str) -> Option<&'static str> {
    match key {
        "pkg:list" | "pkg:unreadable-folders" => None,
        "ws:app.json" => Some("appJson"),
        "ws:roots.config.json" => Some("rootsConfig"),
        "ws:al-sem.coverage.yaml" => Some("coverageConfig"),
        k if k.starts_with("al:") => Some("alFiles"),
        k if k.starts_with("pkg:") => Some("packages"),
        k if k.starts_with("src:") => Some("sourceDependencyFiles"),
        k if k.starts_with("srcapp:") => Some("sourceDependencyAppJson"),
        _ => None,
    }
}

/// The JSON names of [`input_category`] in print order, with their human
/// words.
const INPUT_CATEGORIES: &[(&str, &str)] = &[
    ("alFiles", ".al files"),
    ("packages", "packages"),
    ("sourceDependencyFiles", "source-dependency files"),
    ("appJson", "app.json"),
    ("rootsConfig", "roots.config.json"),
    ("coverageConfig", "al-sem.coverage.yaml"),
    ("sourceDependencyAppJson", "source-dependency app.json"),
];

/// The files an observation names, counted by category (output validation
/// §9.6, F65): every category of a file that is there (a file that is not
/// there is compared by its absence, never hashed, and not counted). Under
/// `verify` every counted file is hashed, so the counts add up to
/// `filesHashed`. The first three keys are always present (the 1.x shape);
/// the others when non-zero.
fn counts(m: &Manifest) -> Map<String, Value> {
    let mut c: BTreeMap<&str, usize> = BTreeMap::new();
    for (k, ident) in m.entries() {
        if matches!(ident, Ident::Absent) {
            continue;
        }
        if let Some(cat) = input_category(k) {
            *c.entry(cat).or_default() += 1;
        }
    }
    INPUT_CATEGORIES
        .iter()
        .enumerate()
        .filter_map(|(i, (cat, _))| {
            let n = c.get(cat).copied().unwrap_or(0);
            (i < 3 || n > 0).then(|| ((*cat).to_string(), Value::from(n)))
        })
        .collect()
}

/// `inputs` in words: each category with a file, and the total.
fn inputs_human(inputs: &Value) -> String {
    let mut total = 0;
    let parts: Vec<String> = INPUT_CATEGORIES
        .iter()
        .filter_map(|(cat, words)| {
            let n = inputs[*cat].as_u64().filter(|n| *n > 0)?;
            total += n;
            Some(format!("{words} {n}"))
        })
        .collect();
    if parts.is_empty() {
        "none".to_string()
    } else {
        format!("{} ({total} files)", parts.join(", "))
    }
}

/// What the headline of one engine needs beside its check.
pub struct EngineState<'a> {
    pub workspace: &'a str,
    /// `cross-app` or `single-app`.
    pub universe: &'a str,
    /// The check, or `None` with why it did not run.
    pub check: Result<&'a Value, &'a str>,
    /// `activity` and its age in ms (`pin`, `refresh`, ...), when busy.
    pub activity: Option<(&'a str, u64)>,
    /// The refresh state (`refresh.state`).
    pub refresh_state: &'a str,
    /// The last failure's message and the deaths, when the engine failed.
    pub failed: Option<(&'a str, u64)>,
}

/// One engine's headline (§2.3's table; the texts are a contract).
#[must_use]
pub fn headline(e: &EngineState<'_>) -> String {
    let who = format!("{} engine for {}", e.universe, e.workspace);
    let check = match e.check {
        Err(why) => {
            if let Some((last, deaths)) = e.failed {
                return format!(
                    "{} engine failed {deaths} time{} (last: {last}). The next command starts a new one.",
                    e.universe,
                    if deaths == 1 { "" } else { "s" }
                );
            }
            return format!("{who}: {why}, so nothing was checked.");
        }
        Ok(c) => c,
    };
    if check["complete"] != json!(true) {
        let reason = check["incompleteReasons"][0].as_str().unwrap_or("unknown");
        return format!(
            "{} engine: the check did not finish ({reason}); freshness unknown.",
            e.universe
        );
    }
    let generation = match check["checkedRevision"]["generation"].as_u64() {
        Some(n) => format!("generation {n}"),
        None => "the failed build".to_string(),
    };
    // Output validation V2 (§4.5, F22): the headline says how hard it
    // checked: every input hashed, or size and modification time only.
    let verdict = if check["fresh"] == json!(true) {
        if check["strength"] == json!("hashed") {
            format!(
                "FRESH: no input changed since {generation} (every input hashed just now: {} inputs).",
                check["filesHashed"]
            )
        } else {
            format!(
                "FRESH: no change seen since {generation} (size and modification time, just now; verify: true hashes every input)."
            )
        }
    } else {
        let class = check["refreshClass"]["class"].as_str().unwrap_or("Package");
        format!(
            "STALE. {} changed on disk since {generation}. Refresh class: {}. Next: any command with freshness fresh (the default) refreshes first; or call alsem_refresh.",
            check["summary"].as_str().unwrap_or("inputs"),
            class_words(class)
        )
    };
    let busy = match (e.refresh_state, e.activity) {
        ("running", _) | (_, Some(("refresh", _))) => Some(format!(
            "a refresh is running (started {} s ago)",
            e.activity.map_or(0, |a| a.1 / 1000)
        )),
        (_, Some(("pin", ms))) => Some(format!(
            "a call is checking freshness or building (started {} s ago)",
            ms / 1000
        )),
        _ => None,
    };
    match busy {
        Some(b) => {
            let gen_n = check["checkedRevision"]["generation"]
                .as_u64()
                .map_or("none".to_string(), |n| n.to_string());
            format!(
                "{} engine: {b}. Disk compared with the last published revision (generation {gen_n}): {verdict}",
                e.universe
            )
        }
        None => format!("{who}: {verdict}"),
    }
}

/// The headline of a workspace with no engine (§2.3).
#[must_use]
pub fn no_engine_headline(workspace: &str, universe: &str) -> String {
    let estimate = if universe == "cross-app" {
        " (about 25 s cross-app on CDO)"
    } else {
        ""
    };
    // Output validation §6.1 (F37): the call that builds it, not "any
    // command" (a second status call builds nothing).
    let single = if universe == "single-app" {
        ", \"singleApp\": true"
    } else {
        ""
    };
    format!(
        "no engine is running for {workspace} ({universe}), so nothing was checked. An analysis \
         call builds it{estimate}, for instance alsem_explain {{\"routines\": [\"<routine>\"]{single}}}; \
         a status call builds nothing."
    )
}

/// The descriptor a worker published, decoded.
pub fn decode_descriptor(v: &Value) -> Result<InputDescriptor, String> {
    serde_json::from_value(v.clone()).map_err(|e| e.to_string())
}

/// `alsem_status`'s body (§2.3): `server`, then every engine `rows` names
/// with its check (run here, each engine's on its own thread), its refresh
/// state and its operator `details`; a `headline` first. With no engine for
/// the workspace asked (or the default one), `noEngine` says so and
/// nothing is checked. Runs on the checker thread, never the reader loop.
#[must_use]
pub fn status_body(
    server: Value,
    rows: Vec<crate::mcp::pool::EngineRow>,
    asked: Option<(&str, &str)>,
    opts: CheckOptions,
) -> Value {
    let checks: Vec<Result<Value, String>> = std::thread::scope(|sc| {
        let handles: Vec<_> = rows
            .iter()
            .map(|r| {
                sc.spawn(move || match &r.descriptor {
                    None => Err(String::new()),
                    Some(d) => decode_descriptor(d)
                        .map(|d| {
                            let mut c = check(&d, opts);
                            if let Some(recorded) = &r.test_config {
                                apply_test_config(&mut c, &r.workspace, recorded);
                            }
                            c
                        })
                        .map_err(|e| format!("its published descriptor does not decode ({e})")),
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err("the check failed".to_string()))
            })
            .collect()
    });
    let mut headlines = Vec::new();
    let mut engines = Vec::new();
    for (row, checked) in rows.into_iter().zip(checks) {
        let mut details = row.details;
        let refresh = details
            .as_object_mut()
            .and_then(|m| m.remove("refresh"))
            .unwrap_or(Value::Null);
        let activity = details["freshness"]["activity"]
            .as_str()
            .filter(|a| *a != "idle")
            .map(|a| (a, details["freshness"]["activityMs"].as_u64().unwrap_or(0)));
        let running_child = !details["engine"].is_null();
        let reason = match &checked {
            Ok(_) => None,
            Err(e) if !e.is_empty() => Some(e.clone()),
            Err(_) if activity.is_some() => {
                Some("no revision is published yet (the first build is running)".to_string())
            }
            Err(_) => Some("no revision is published yet".to_string()),
        };
        let failed = (!running_child && checked.is_err())
            .then(|| {
                details["lastFailure"]["message"]
                    .as_str()
                    .map(|m| (m, details["deaths"].as_u64().unwrap_or(0)))
            })
            .flatten();
        let universe = row.universe.as_str();
        let mut line = headline(&EngineState {
            workspace: &row.workspace,
            universe,
            check: match (&checked, &reason) {
                (Ok(c), _) => Ok(c),
                (Err(_), Some(r)) => Err(r.as_str()),
                (Err(_), None) => Err("no revision is published yet"),
            },
            activity,
            refresh_state: refresh["state"].as_str().unwrap_or("idle"),
            failed,
        });
        if checked
            .as_ref()
            .is_ok_and(|c| c["testScope"]["changed"] == json!(true))
        {
            line.push_str(
                " Test scope changed: .al-sem.json testApps differ from the last alsem_tests answer; the next alsem_tests rediscovers it.",
            );
        }
        if checked.is_ok() && !running_child {
            line.push_str(
                " Its engine child is not running (it ended idle or by the children bound), so the next command builds again.",
            );
        }
        headlines.push(line.clone());
        engines.push(json!({
            "workspace": row.workspace,
            "universe": universe,
            "headline": line,
            "check": checked.ok(),
            "checkNotRun": reason,
            "refresh": refresh,
            "details": details,
        }));
    }
    let mut body = json!({
        "kind": "status",
        "statusVersion": STATUS_VERSION,
        "headline": Value::Null,
        "server": server,
        "engines": engines,
    });
    if headlines.is_empty() {
        let line = match asked {
            Some((ws, universe)) => no_engine_headline(ws, universe),
            None => "no engine is running, so nothing was checked. An analysis call (for \
                     instance alsem_explain {\"routines\": [\"<routine>\"]}) builds one; a status \
                     call builds nothing."
                .to_string(),
        };
        body["noEngine"] = json!({
            "workspace": asked.map(|a| a.0),
            "universe": asked.map(|a| a.1),
            "check": Value::Null,
            "reason": "no engine is running",
        });
        headlines.push(line);
    }
    body["headline"] = json!(headlines.join("\n"));
    body
}

/// The status answer's text block (output validation §5.4, F33): one copy,
/// about 500 bytes. Per engine its headline, then `check` (fresh, strength,
/// complete, the checked revision and when, the inputs by category, the
/// residual) and `refresh`; with no engine the headline and `not checked`.
/// The full document (operator data under `details`) is the
/// `structuredContent`.
#[must_use]
pub fn status_text(body: &Value) -> String {
    let mut out: Vec<String> = Vec::new();
    for e in body["engines"].as_array().into_iter().flatten() {
        out.push(e["headline"].as_str().unwrap_or("").to_string());
        let c = &e["check"];
        if c.is_object() {
            let changes = c["changeCount"].as_u64().unwrap_or(0);
            let fresh = if c["fresh"] == json!(true) {
                "fresh".to_string()
            } else if changes > 0 {
                format!(
                    "stale ({} changed: {})",
                    changes,
                    c["summary"].as_str().unwrap_or("")
                )
            } else {
                "not established".to_string()
            };
            let strength = match c["strength"].as_str() {
                Some("hashed") => "every input hashed",
                _ => "size and modification time trusted",
            };
            let complete = if c["complete"] == json!(true) {
                "freshness check complete".to_string()
            } else {
                format!(
                    "freshness check incomplete ({})",
                    c["incompleteReasons"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("; ")
                )
            };
            let when = c["finishedAtUnixMs"]
                .as_u64()
                .map(crate::engine::gate::format_json::iso8601_of_unix_ms)
                .unwrap_or_default();
            out.push(format!(
                "  check: {fresh}; {strength}; {complete}; generation {} checked at {when}; \
                 inputs: {}; residual: {}",
                c["checkedRevision"]["generation"],
                inputs_human(&c["inputs"]),
                c["residual"].as_str().unwrap_or("")
            ));
        } else {
            out.push(format!(
                "  not checked: {}",
                e["checkNotRun"]
                    .as_str()
                    .unwrap_or("no revision is published yet")
            ));
        }
        let r = &e["refresh"];
        let mut refresh = format!("  refresh: {}", r["state"].as_str().unwrap_or("idle"));
        if let Some(did) = r["did"].as_str() {
            refresh.push_str(&format!(" ({did})"));
        }
        if let Some(err) = r["error"].as_str() {
            refresh.push_str(&format!(" — {err}"));
        }
        out.push(refresh);
    }
    if out.is_empty() {
        // Round-2 G25: the headline already says that no engine is running
        // and nothing was checked; a `not checked:` line would repeat it.
        out.push(body["headline"].as_str().unwrap_or("").to_string());
    }
    out.join("\n") + "\n"
}

#[cfg(test)]
#[path = "freshness_tests.rs"]
mod tests;
