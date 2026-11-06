//! Follow-ups in the words of the surface that asked (output validation spec
//! 2026-11-06 §6.1, F34, F37): the one place that knows how a CLI flag is
//! spelled as an MCP input, so no renderer spells a flag.
//!
//! The MCP front end appends a hidden `--surface=mcp` to every command line it
//! builds. With it, a follow-up is a complete MCP call (`alsem_prove
//! {"routine": "…", "question": "writes-table:CDO File"}`) and an in-place
//! input change is the input (`all: true`, `tier: "dependency"`); without it
//! (the CLI's default) the same follow-up is a command line or a flag. The
//! byte contract holds: the MCP answer equals the CLI's stdout for the same
//! argv, `--surface=mcp` included.
//!
//! A follow-up keeps the assumptions of the call it came from (astra). One
//! that changes an assumption (`closedWorld`, `assumeNoDependents`) is never
//! an ordinary follow-up: it is the `re-analysis under an assumption:` line,
//! [`Kind::Conditional`], carried in JSON with `kind: "conditional"`.

use serde_json::{Map, Value, json};

/// Which front end the answer is for (`--surface`, hidden).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Surface {
    /// The CLI: follow-ups are command lines and flags.
    #[default]
    Cli,
    /// The MCP front end: follow-ups are tool calls and inputs.
    Mcp,
}

impl Surface {
    /// `cli` or `mcp`; anything else is `None`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Surface> {
        match s {
            "cli" => Some(Surface::Cli),
            "mcp" => Some(Surface::Mcp),
            _ => None,
        }
    }

    /// The flag's own spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Surface::Cli => "cli",
            Surface::Mcp => "mcp",
        }
    }
}

/// The argv element the MCP front end appends.
pub const MCP_FLAG: &str = "--surface=mcp";

/// An input that changes the answer of the same call in place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    /// Every row, no fold (query: `--all`).
    All,
    /// Every list complete (explain, digest, prove, impact: `--detail`).
    Detail,
    /// Every caller by name (touches: `--callers`).
    Callers,
    /// Tests whose path passes dependency code too (tests: `--tier dependency`).
    TierDependency,
    /// The JSON document (`--format json`).
    FormatJson,
    /// More routines shown (impact: `--max-roots N`).
    MaxRoots,
    /// The operations whose table is not resolved (touches: `--table unknown`).
    TableUnknown,
    /// Each cited source line ±2 (`--show-source`).
    ShowSource,
    /// The callers' context section (explain: `--callers-context`).
    CallersContext,
    /// Interface calls dispatch only to loaded implementations
    /// (`--closed-world`): an assumption change.
    ClosedWorld,
    /// No app outside the program calls these routines (`--no-dependents`):
    /// an assumption change.
    AssumeNoDependents,
}

impl Input {
    /// The CLI flag.
    #[must_use]
    pub fn cli(self) -> &'static str {
        match self {
            Input::All => "--all",
            Input::Detail => "--detail",
            Input::Callers => "--callers",
            Input::TierDependency => "--tier dependency",
            Input::FormatJson => "--format json",
            Input::MaxRoots => "--max-roots N",
            Input::TableUnknown => "--table unknown",
            Input::ShowSource => "--show-source",
            Input::CallersContext => "--callers-context",
            Input::ClosedWorld => "--closed-world",
            Input::AssumeNoDependents => "--no-dependents",
        }
    }

    /// The MCP input with its value.
    #[must_use]
    pub fn mcp(self) -> &'static str {
        match self {
            Input::All => "all: true",
            Input::Detail => "detail: true",
            Input::Callers => "callers: true",
            Input::TierDependency => "tier: \"dependency\"",
            Input::FormatJson => "format: \"json\"",
            Input::MaxRoots => "maxRoots: N",
            Input::TableUnknown => "table: \"unknown\"",
            Input::ShowSource => "showSource: true",
            Input::CallersContext => "callersContext: true",
            Input::ClosedWorld => "closedWorld: true",
            Input::AssumeNoDependents => "assumeNoDependents: true",
        }
    }

    /// The input in `surface`'s words.
    #[must_use]
    pub fn on(self, surface: Surface) -> &'static str {
        match surface {
            Surface::Cli => self.cli(),
            Surface::Mcp => self.mcp(),
        }
    }
}

/// `input` in `surface`'s words (the short form renderers call).
#[must_use]
pub fn input(surface: Surface, input: Input) -> &'static str {
    input.on(surface)
}

/// A tool a follow-up names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Explain,
    Prove,
    Digest,
    QueryEffects,
    QueryTouches,
    Tests,
    Impact,
}

impl Tool {
    /// The MCP tool name.
    #[must_use]
    pub fn mcp(self) -> &'static str {
        match self {
            Tool::Explain => "alsem_explain",
            Tool::Prove => "alsem_prove",
            Tool::Digest => "alsem_digest",
            Tool::QueryEffects => "alsem_query_effects",
            Tool::QueryTouches => "alsem_query_touches",
            Tool::Tests => "alsem_tests",
            Tool::Impact => "alsem_impact",
        }
    }

    /// The CLI command.
    #[must_use]
    pub fn cli(self) -> &'static str {
        match self {
            Tool::Explain => "alsem explain",
            Tool::Prove => "alsem prove",
            Tool::Digest => "alsem digest",
            Tool::QueryEffects => "alsem query effects",
            Tool::QueryTouches => "alsem query touches",
            Tool::Tests => "alsem tests",
            Tool::Impact => "alsem impact",
        }
    }

    /// The tool in `surface`'s words (`alsem_prove` | `alsem prove`).
    #[must_use]
    pub fn on(self, surface: Surface) -> &'static str {
        match surface {
            Surface::Cli => self.cli(),
            Surface::Mcp => self.mcp(),
        }
    }
}

/// An ordinary follow-up keeps the call's assumptions; a conditional one
/// changes one (`re-analysis under an assumption:`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Ordinary,
    Conditional,
}

/// The scope inputs every follow-up carries from the call it came from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cx {
    pub surface: Surface,
    /// The call was single-app: its follow-ups are too.
    pub single_app: bool,
}

impl Cx {
    #[must_use]
    pub fn new(surface: Surface, single_app: bool) -> Cx {
        Cx {
            surface,
            single_app,
        }
    }

    /// `input` in this surface's words.
    #[must_use]
    pub fn input(self, i: Input) -> &'static str {
        i.on(self.surface)
    }
}

/// One complete follow-up call: a tool and its arguments, by MCP input name.
#[derive(Clone, Debug, PartialEq)]
pub struct FollowUp {
    pub tool: Tool,
    pub arguments: Map<String, Value>,
    /// What it answers, in a few words.
    pub why: String,
    pub kind: Kind,
}

impl FollowUp {
    /// A follow-up of `tool` with no argument yet; `cx`'s scope inputs filled.
    #[must_use]
    pub fn new(tool: Tool, cx: Cx, why: &str) -> FollowUp {
        let mut arguments = Map::new();
        if cx.single_app {
            arguments.insert("singleApp".into(), Value::Bool(true));
        }
        FollowUp {
            tool,
            arguments,
            why: why.to_string(),
            kind: Kind::Ordinary,
        }
    }

    /// `alsem_prove {"routine": routine, "question": question}`.
    #[must_use]
    pub fn prove(cx: Cx, routine: &str, question: &str, why: &str) -> FollowUp {
        FollowUp::new(Tool::Prove, cx, why)
            .arg("routine", routine)
            .arg("question", question)
    }

    /// An argument (a string, a bool, a number or an array).
    #[must_use]
    pub fn arg(mut self, key: &str, value: impl Into<Value>) -> FollowUp {
        self.arguments.insert(key.to_string(), value.into());
        self
    }

    /// The follow-up as an assumption change.
    #[must_use]
    pub fn conditional(mut self) -> FollowUp {
        self.kind = Kind::Conditional;
        self
    }

    /// The call in `surface`'s words: `alsem_prove {"routine": "…", …}`, or the
    /// CLI command line (`alsem prove '…' may-commit --closed-world`).
    #[must_use]
    pub fn text(&self, surface: Surface) -> String {
        match surface {
            Surface::Mcp => {
                // Keys in a stable, readable order: the identity first.
                let mut parts: Vec<String> = Vec::new();
                let order = [
                    "routine",
                    "routines",
                    "question",
                    "table",
                    "field",
                    "from",
                    "direction",
                    "files",
                    "diff",
                    "base",
                ];
                let mut seen: Vec<&str> = Vec::new();
                for k in order {
                    if let Some(v) = self.arguments.get(k) {
                        parts.push(format!("{}: {v}", json!(k)));
                        seen.push(k);
                    }
                }
                for (k, v) in &self.arguments {
                    if !seen.contains(&k.as_str()) {
                        parts.push(format!("{}: {v}", json!(k)));
                    }
                }
                format!("{} {{{}}}", self.tool.mcp(), parts.join(", "))
            }
            Surface::Cli => {
                let mut out = vec![self.tool.cli().to_string()];
                let mut flags: Vec<String> = Vec::new();
                for (k, v) in &self.arguments {
                    match (self.tool, k.as_str()) {
                        (Tool::Prove, "routine" | "question") => {}
                        _ => flags.extend(cli_flags(k, v)),
                    }
                }
                if self.tool == Tool::Prove {
                    for k in ["routine", "question"] {
                        if let Some(Value::String(s)) = self.arguments.get(k) {
                            out.push(shell_word(s));
                        }
                    }
                }
                out.extend(flags);
                out.join(" ")
            }
        }
    }

    /// The JSON form: `{tool, arguments, why, kind}` (always the MCP call, on
    /// either surface: a JSON reader is a program).
    #[must_use]
    pub fn json(&self) -> Value {
        json!({
            "tool": self.tool.mcp(),
            "arguments": Value::Object(self.arguments.clone()),
            "why": self.why,
            "kind": match self.kind {
                Kind::Ordinary => "ordinary",
                Kind::Conditional => "conditional",
            },
        })
    }
}

/// `followUps` as JSON.
#[must_use]
pub fn json_list(list: &[FollowUp]) -> Value {
    Value::Array(list.iter().map(FollowUp::json).collect())
}

/// An MCP input key's CLI flag (the one mapping, flag → input, read
/// backwards): `writesTables` → `--writes-table`, `assumeNoDependents` →
/// `--no-dependents`, `routines` → `--routine` (one per item).
fn cli_flag_name(key: &str) -> String {
    match key {
        "routines" => "routine".into(),
        "files" => "file".into(),
        "writesTables" => "writes-table".into(),
        "testApps" => "test-app".into(),
        "assumeNoDependents" => "no-dependents".into(),
        k => {
            let mut s = String::new();
            for c in k.chars() {
                if c.is_ascii_uppercase() {
                    s.push('-');
                    s.push(c.to_ascii_lowercase());
                } else {
                    s.push(c);
                }
            }
            s
        }
    }
}

fn cli_flags(key: &str, value: &Value) -> Vec<String> {
    let flag = cli_flag_name(key);
    match value {
        Value::Bool(true) => vec![format!("--{flag}")],
        Value::Bool(false) | Value::Null => Vec::new(),
        Value::String(s) => vec![format!("--{flag} {}", shell_word(s))],
        Value::Array(items) => items.iter().flat_map(|v| cli_flags(key, v)).collect(),
        other => vec![format!("--{flag} {other}")],
    }
}

/// `s` as one shell word: single-quoted when it holds a space, a quote or a
/// shell metacharacter.
fn shell_word(s: &str) -> String {
    let plain = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.:/@%+,=".contains(c));
    if plain {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// Whether `text` holds a CLI flag (`--x`) or a CLI command (`alsem prove`):
/// the §6.1 invariant over renderer-owned follow-up text at `--surface=mcp`
/// (`more:` and `next:` lines and fold notes; not quoted source, diffs or
/// `---` headings, which legitimately hold `--`).
#[must_use]
pub fn cli_words_in(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    for (i, w) in text.match_indices("--") {
        let prev = i.checked_sub(1).map(|j| bytes[j]);
        let next = bytes.get(i + w.len()).copied();
        let starts = prev.is_none_or(|p| p == b' ' || p == b'(');
        if starts && next.is_some_and(|c| c.is_ascii_lowercase()) {
            let end = text[i..]
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
                .map_or(text.len(), |e| i + e);
            out.push(text[i..end].to_string());
        }
    }
    for cmd in [
        "alsem prove",
        "alsem explain",
        "alsem digest",
        "alsem query",
        "alsem tests",
        "alsem impact",
        "alsem diff",
        "alsem status",
    ] {
        if text.contains(cmd) {
            out.push(cmd.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two spellings of one follow-up, and its JSON.
    #[test]
    fn a_follow_up_prints_in_its_surface() {
        let cx = Cx::new(Surface::Mcp, true);
        let f = FollowUp::prove(
            cx,
            "Codeunit \"X\"::Y",
            "writes-table:CDO File",
            "one table's witness",
        );
        assert_eq!(
            f.text(Surface::Mcp),
            r#"alsem_prove {"routine": "Codeunit \"X\"::Y", "question": "writes-table:CDO File", "singleApp": true}"#
        );
        assert_eq!(
            f.text(Surface::Cli),
            "alsem prove 'Codeunit \"X\"::Y' 'writes-table:CDO File' --single-app"
        );
        assert_eq!(f.json()["tool"], "alsem_prove");
        assert_eq!(f.json()["kind"], "ordinary");
        assert_eq!(f.json()["arguments"]["singleApp"], true);
        let c = FollowUp::prove(Cx::default(), "A", "may-commit", "x")
            .arg("closedWorld", true)
            .conditional();
        assert_eq!(c.json()["kind"], "conditional");
        assert_eq!(
            c.text(Surface::Cli),
            "alsem prove A may-commit --closed-world"
        );
    }

    #[test]
    fn inputs_have_both_spellings() {
        assert_eq!(input(Surface::Cli, Input::All), "--all");
        assert_eq!(input(Surface::Mcp, Input::All), "all: true");
        assert_eq!(
            input(Surface::Mcp, Input::TierDependency),
            "tier: \"dependency\""
        );
    }

    #[test]
    fn cli_words_are_found_but_not_headings_or_dashes() {
        assert_eq!(cli_words_in("x (--detail lists them)"), ["--detail"]);
        assert_eq!(cli_words_in("next: alsem prove A"), ["alsem prove"]);
        assert!(cli_words_in("--- Codeunit X ---").is_empty());
        assert!(cli_words_in("a -- b; x--y").is_empty());
    }
}
