//! Output invariants of the MCP output plan's wave 3 (plan
//! `docs/superpowers/plans/2026-11-04-mcp-output-fixes.md`, items 3b and T): the tool
//! descriptions say only what the tools take, one selector grammar holds for every
//! tool, and item T's remaining rows (spec §8.3). They run on the cross-app fixture
//! `tests/fixtures/mcp-output/` in every `cargo test`; their CDO twins live in
//! `output_cdo`.

use std::collections::BTreeSet;

use crate::output_fixture::{ISSUE_DOC, run_alsem, workspace};

/// The properties every command tool takes, described once in the server
/// instructions, and `deterministic` (byte-stable output for tests).
const COMMON: &[&str] = &[
    "workspace",
    "freshness",
    "singleApp",
    "out",
    "maxInlineBytes",
    "deterministic",
];

/// The text between each pair of backticks.
fn backticked(s: &str) -> Vec<&str> {
    s.split('`').skip(1).step_by(2).collect()
}

/// `name` when `tok` is a camelCase identifier, or one followed by `: value`
/// (`detail: true`, `maxPaths: 0..3`): how a description names an input.
/// The second item says whether a value followed.
fn named(tok: &str) -> Option<(&str, bool)> {
    let (name, valued) = match tok.split_once(": ") {
        Some((n, _)) => (n, true),
        None => (tok, false),
    };
    let ident = name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && name.chars().all(|c| c.is_ascii_alphanumeric());
    ident.then_some((name, valued))
}

/// MCP output plan 3b: every tool description, and the server instructions,
/// name only inputs the tool's schema has (`name: value` in backticks is an
/// input), and every input of a tool's schema is named in its description
/// (the common ones in the instructions, once). Each command tool's
/// description says what the default prints, how to ask for more, the line
/// base, a size, its dependency depth and a next step; the instructions say
/// which tool answers which question, the selector grammar and what the
/// answer words mean.
///
/// Discrimination (2026-11-05): renaming `maxPaths: 0..3` to `maxPath:
/// 0..3` in explain's description fails at "names `maxPath`"; restored,
/// passes.
#[test]
fn descriptions_name_exactly_the_inputs_the_schemas_take() {
    let mut failures = Vec::new();
    let mut every_input = BTreeSet::new();
    for t in al_sem::mcp::tools::tool_list() {
        let name = t["name"].as_str().unwrap();
        let d = t["description"].as_str().unwrap();
        let props: BTreeSet<&str> = t["inputSchema"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        every_input.extend(props.iter().map(|p| p.to_string()));
        let mentioned: BTreeSet<&str> = backticked(d)
            .into_iter()
            .filter_map(named)
            .map(|(n, valued)| {
                if valued && !props.contains(n) {
                    failures.push(format!("{name}: names `{n}`, not in its schema"));
                }
                n
            })
            .collect();
        for p in &props {
            if !COMMON.contains(p) && !mentioned.contains(p) {
                failures.push(format!("{name}: input `{p}` is not named"));
            }
        }
        if matches!(name, "alsem_status" | "alsem_refresh") {
            continue;
        }
        for want in [
            "Default (`format: ",
            "Ask for more",
            "1-based",
            "Dependency code:",
            "Next:",
            " KB",
        ] {
            if !d.contains(want) {
                failures.push(format!("{name}: says nothing of {want:?}"));
            }
        }
    }
    let i = al_sem::mcp::INSTRUCTIONS;
    for (n, valued) in backticked(i).into_iter().filter_map(named) {
        if valued && !every_input.contains(n) {
            failures.push(format!("INSTRUCTIONS: names `{n}`, which no tool takes"));
        }
    }
    let mentioned: BTreeSet<&str> = backticked(i)
        .into_iter()
        .filter_map(named)
        .map(|(n, _)| n)
        .collect();
    for c in COMMON.iter().filter(|c| **c != "deterministic") {
        if !mentioned.contains(c) {
            failures.push(format!("INSTRUCTIONS: the common input `{c}` is not named"));
        }
    }
    for want in al_sem::mcp::tools::TOOL_NAMES.iter().copied().chain([
        "1-based",
        "`Owner::Routine`",
        "`Owner.Routine`",
        "`unknown`",
        "`coverage`",
        "`assumptions`",
        "Temporary records are never counted as writes",
        "summary",
    ]) {
        if !i.contains(want) {
            failures.push(format!("INSTRUCTIONS: says nothing of {want:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// MCP output plan 3b: one routine selector grammar for every tool. Each
/// spelling the instructions name (`Owner::Routine`, `Owner.Routine`, either
/// with the owner's type word or AL quotes, a unique bare name) selects the
/// same routine in explain, prove, digest, query effects and query touches
/// (`--from`): the answer is byte-identical to the one for the
/// `Codeunit Owner::Routine` spelling.
///
/// Discrimination (2026-11-05): dropping form 6 (`Object.Routine`) from
/// `selector_index::cascade` fails at "explain: `Out Issue Document.IssueDoc`";
/// dropping the `split_qualified` tier from query's
/// `resolve_routine_selector` fails at "query effects: `Out Issue
/// Document::IssueDoc`"; each restored, passes.
#[test]
fn one_selector_grammar_for_every_tool() {
    let ws = workspace().to_string_lossy().into_owned();
    let spellings = [
        "Codeunit Out Issue Document::IssueDoc",
        ISSUE_DOC,
        "Out Issue Document.IssueDoc",
        "Codeunit Out Issue Document.IssueDoc",
        "Codeunit \"Out Issue Document\"::IssueDoc",
        "\"Out Issue Document\".IssueDoc",
        "IssueDoc",
    ];
    type Argv = fn(&str, &str) -> Vec<String>;
    let tools: [(&str, Argv); 5] = [
        ("explain", |ws, s| {
            vec!["explain".into(), ws.into(), "--routine".into(), s.into()]
        }),
        ("prove", |ws, s| {
            vec![
                "prove".into(),
                ws.into(),
                s.into(),
                "may-commit".into(),
                "--format".into(),
                "human".into(),
            ]
        }),
        ("digest", |ws, s| {
            vec![
                "digest".into(),
                ws.into(),
                "--routine".into(),
                s.into(),
                "--format".into(),
                "human".into(),
            ]
        }),
        ("query effects", |ws, s| {
            vec![
                "query".into(),
                "effects".into(),
                ws.into(),
                "--routine".into(),
                s.into(),
            ]
        }),
        ("query touches", |ws, s| {
            vec![
                "query".into(),
                "touches".into(),
                ws.into(),
                "--table".into(),
                "Out Log".into(),
                "--from".into(),
                s.into(),
            ]
        }),
    ];
    let mut failures = Vec::new();
    for (tool, argv) in tools {
        let mut expected = None;
        for s in spellings {
            let a = argv(&ws, s);
            let a: Vec<&str> = a.iter().map(String::as_str).collect();
            let (out, err, code) = run_alsem(&a);
            if code != 0 {
                failures.push(format!("{tool}: `{s}` exits {code}: {err}"));
                continue;
            }
            match &expected {
                None => expected = Some(out),
                Some(e) if *e != out => {
                    failures.push(format!("{tool}: `{s}` answers otherwise:\n{out}"));
                }
                Some(_) => {}
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
