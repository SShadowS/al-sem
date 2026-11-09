//! Output validation V4 (spec 2026-11-06-output-validation-r1 §6.1, §12.1):
//! the follow-ups an answer names are complete MCP calls. Every JSON
//! `followUps[]` entry, and every `alsem_<tool> {…}` call printed in a human
//! answer at `--surface=mcp`, validates against that tool's input schema in
//! `src/mcp/tools.rs` (the server's own `parse_call`: known keys, types and
//! modes), so an agent can pass it back unchanged.
//!
//! Discrimination (2026-11-06): see the V4 commit message.

use serde_json::Value;

use crate::output_fixture::{ISSUE_DOC, ISSUE_DOC_Q, mcp_surface_cases, run_alsem, workspace};

fn defaults() -> al_sem::mcp::tools::Defaults {
    al_sem::mcp::tools::Defaults {
        workspace: Some(workspace().to_string_lossy().into_owned()),
        universe: al_sem::mcp::tools::Universe::CrossApp,
        freshness: al_sem::mcp::tools::Freshness::Fresh,
    }
}

/// Every `followUps` array anywhere in `v`, flattened.
fn follow_ups(v: &Value, out: &mut Vec<Value>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                if k == "followUps" {
                    out.extend(x.as_array().into_iter().flatten().cloned());
                } else {
                    follow_ups(x, out);
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| follow_ups(x, out)),
        _ => {}
    }
}

/// Every `alsem_<tool> {…}` call in `text`: (tool, arguments).
fn printed_calls(text: &str) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("alsem_") {
        let after = &rest[i..];
        let name_len = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(after.len());
        let name = &after[..name_len];
        let tail = &after[name_len..];
        if let Some(obj) = tail.strip_prefix(" {") {
            // The balanced object (strings may hold braces).
            let (mut depth, mut in_str, mut esc, mut end) = (1usize, false, false, None);
            for (j, c) in obj.char_indices() {
                match (in_str, esc, c) {
                    (true, true, _) => esc = false,
                    (true, false, '\\') => esc = true,
                    (true, false, '"') => in_str = false,
                    (false, _, '"') => in_str = true,
                    (false, _, '{') => depth += 1,
                    (false, _, '}') => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(j);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if let Some(e) = end {
                let json = format!("{{{}}}", &obj[..e]);
                let args: Value = serde_json::from_str(&json)
                    .unwrap_or_else(|err| panic!("a printed call is not JSON ({err}): {json}"));
                out.push((name.to_string(), args));
            }
        }
        rest = &after[name_len..];
    }
    out
}

fn validate(label: &str, tool: &str, args: &Value) -> Option<String> {
    match al_sem::mcp::tools::parse_call(tool, Some(args), &defaults()) {
        Ok(_) => None,
        Err(why) => Some(format!("{label}: {tool} {args}: {why:?}")),
    }
}

/// §12.1: every `followUps[].arguments` validates against its tool's input
/// schema, and so does every call a human answer prints at `--surface=mcp`.
#[test]
fn every_follow_up_validates_against_its_tool_schema() {
    let ws = workspace().to_string_lossy().into_owned();
    let json_runs: Vec<Vec<&str>> = vec![
        vec!["explain", &ws, "--routine", ISSUE_DOC, "--format", "json"],
        vec![
            "explain",
            &ws,
            "--routine",
            ISSUE_DOC,
            "--callers-context",
            "--format",
            "json",
        ],
        vec![
            "prove",
            &ws,
            ISSUE_DOC,
            "callers-intercept-errors",
            "--format",
            "json",
        ],
        vec!["prove", &ws, ISSUE_DOC, "may-commit", "--format", "json"],
    ];
    let mut failures = Vec::new();
    let mut seen = Vec::new();
    for a in &json_runs {
        let (out, err, code) = run_alsem(a);
        assert_eq!(code, 0, "{a:?}: {err}");
        let v: Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("{a:?}: {e}"));
        let mut list = Vec::new();
        follow_ups(&v, &mut list);
        for f in list {
            let tool = f["tool"].as_str().unwrap_or("").to_string();
            failures.extend(validate(&a.join(" "), &tool, &f["arguments"]));
            seen.push(f);
        }
    }
    // Preconditions: the runs carry ordinary and conditional follow-ups.
    assert!(seen.len() >= 3, "precondition: {} follow-ups", seen.len());
    assert!(
        seen.iter().any(|f| f["kind"] == "conditional"),
        "precondition: a conditional follow-up: {seen:?}"
    );
    let mut printed = 0usize;
    for c in mcp_surface_cases() {
        let mut args: Vec<&str> = c.argv.iter().map(String::as_str).collect();
        args.extend(["--format", "human"]);
        let (out, err, code) = run_alsem(&args);
        assert_eq!(code, c.exit_code, "{}: {err}", c.slug);
        for (tool, a) in printed_calls(&out) {
            printed += 1;
            failures.extend(validate(c.slug, &tool, &a));
        }
    }
    let (out, _, _) = run_alsem(&[
        "query",
        "touches",
        &ws,
        "--table",
        "Base Register",
        "--from",
        ISSUE_DOC_Q,
        "--direction",
        "down",
        "--surface=mcp",
        "--format",
        "human",
    ]);
    for (tool, a) in printed_calls(&out) {
        printed += 1;
        failures.extend(validate("touches down", &tool, &a));
    }
    assert!(printed >= 4, "precondition: {printed} printed calls");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// `s` split into shell words: single quotes group (`'\''` is a quote, as
/// `follow_up::shell_word` writes it), spaces separate.
fn shell_words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut quoted, mut any) = (false, false);
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match (quoted, c) {
            (true, '\'') => quoted = false,
            (false, '\'') => {
                quoted = true;
                any = true;
            }
            (false, '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                    any = true;
                }
            }
            (false, ' ') => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            (_, c) => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}

/// Every `alsem <command> …` follow-up a CLI human answer prints: the
/// command line up to its ` — why` tail, as argv after `alsem`, with
/// `workspace` placed after the command words (a printed follow-up leaves it
/// out). Templates (`<table>`, `N`) are skipped: they are not runnable as
/// printed by design.
fn printed_cli_follow_ups(text: &str, workspace: &str) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(i) = line.find("alsem ") else {
            continue;
        };
        let cmd = &line[i + "alsem ".len()..];
        let cmd = cmd.split(" — ").next().unwrap_or(cmd);
        if cmd.contains('<') || cmd.split(' ').any(|w| w == "N") {
            continue;
        }
        let words = shell_words(cmd);
        let n = match words.first().map(String::as_str) {
            Some("query") => 2,
            Some("prove" | "explain" | "digest" | "tests" | "impact") => 1,
            _ => continue,
        };
        let mut argv: Vec<String> = words[..n].to_vec();
        argv.push(workspace.to_string());
        argv.extend(words[n..].iter().cloned());
        out.push(argv);
    }
    out
}

/// Engine limits B8: every follow-up a CLI answer prints runs as printed, in
/// the mode the answer was given in. A single-app answer's follow-ups once
/// carried `--single-app` onto `prove … callers-intercept-errors` and
/// `explain --callers-context`, both of which the CLI refuses with it.
#[test]
fn every_cli_follow_up_runs_as_printed() {
    let ws = workspace().to_string_lossy().into_owned();
    let answers: Vec<Vec<&str>> = vec![
        vec!["prove", &ws, ISSUE_DOC, "throws-error", "--format", "human"],
        vec![
            "prove",
            &ws,
            ISSUE_DOC,
            "throws-error",
            "--format",
            "human",
            "--single-app",
        ],
        vec!["explain", &ws, "--routine", ISSUE_DOC],
        vec!["explain", &ws, "--routine", ISSUE_DOC, "--single-app"],
        vec![
            "query",
            "effects",
            &ws,
            "--routine",
            ISSUE_DOC,
            "--single-app",
        ],
    ];
    let mut ran = Vec::new();
    let mut failures = Vec::new();
    for a in &answers {
        let (out, err, code) = run_alsem(a);
        assert_eq!(code, 0, "{a:?}: {err}");
        for argv in printed_cli_follow_ups(&out, &ws) {
            let args: Vec<&str> = argv.iter().map(String::as_str).collect();
            let (_, err, code) = run_alsem(&args);
            if code != 0 {
                failures.push(format!("{a:?} printed {argv:?}: exit {code}: {err}"));
            }
            ran.push(argv.join(" "));
        }
    }
    // Preconditions: the single-app answers print the two cross-app-only
    // follow-ups (the regression's shapes), and a single-app follow-up that
    // keeps its scope.
    for want in ["callers-intercept-errors", "--callers-context"] {
        assert!(
            ran.iter().any(|r| r.contains(want)),
            "precondition: a printed follow-up with {want}: {ran:#?}"
        );
    }
    assert!(
        ran.iter().any(|r| r.contains("--single-app")),
        "precondition: a single-app follow-up: {ran:#?}"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
