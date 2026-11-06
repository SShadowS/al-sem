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
