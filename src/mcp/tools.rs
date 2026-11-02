//! The MCP tools of `alsem mcp` (context-server design r7 §8.2): their input
//! schemas, the validation of a call's arguments, and the CLI command line
//! each call becomes.
//!
//! A tool never implements a command. It builds the argv the CLI would take
//! (`explain <ws> --routine X --format human`, ...) and the engine child
//! parses it with the binary's own clap, so flags, defaults and errors are
//! the CLI's. Options are written `--flag=value` and positionals follow a
//! `--`, so a value that starts with `-` (a routine named `-x`) is never
//! read as a flag.
//!
//! The schemas carry no top-level `oneOf`/`anyOf`: the Anthropic tool API
//! refuses them in an input schema. A tool's exclusive modes (explain's
//! `routines` | `diff` | `table`+`field`) are stated in its description and
//! enforced here, as `-32602` (invalid params), with a message naming the
//! rule.

use serde_json::{Map, Value, json};

/// The engine universe a call asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Universe {
    /// The default: the workspace plus the dependencies it requires.
    CrossApp,
    /// `--single-app`: the workspace alone.
    SingleApp,
}

impl Universe {
    /// The envelope's spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Universe::CrossApp => "cross-app",
            Universe::SingleApp => "single-app",
        }
    }
}

/// The freshness a call asks for (design r7 §5.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freshness {
    Fresh,
    LatestBuilt,
    Verify,
}

impl Freshness {
    /// The wire spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Freshness::Fresh => "fresh",
            Freshness::LatestBuilt => "latest-built",
            Freshness::Verify => "verify",
        }
    }

    /// The spelling parsed.
    #[must_use]
    pub fn parse(s: &str) -> Option<Freshness> {
        match s {
            "fresh" => Some(Freshness::Fresh),
            "latest-built" => Some(Freshness::LatestBuilt),
            "verify" => Some(Freshness::Verify),
            _ => None,
        }
    }
}

/// The server-wide defaults a call falls back to.
#[derive(Clone, Debug)]
pub struct Defaults {
    /// `--workspace`: the workspace of a call that names none.
    pub workspace: Option<String>,
    /// `--single-app`.
    pub universe: Universe,
    /// `--freshness`.
    pub freshness: Freshness,
}

/// What a call of a command tool asks the engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandCall {
    /// The tool's name.
    pub tool: &'static str,
    /// The workspace, as the call (or the default) spelled it, made absolute.
    pub workspace: String,
    pub universe: Universe,
    pub freshness: Freshness,
    /// The CLI command line, without the program name.
    pub argv: Vec<String>,
    /// The diff text a `--diff -` reads (sent in the protocol, never read
    /// by the child from a file).
    pub diff_text: Option<String>,
    /// `out`: where the server writes the stdout (inside its output folder).
    pub out: Option<String>,
    /// `maxInlineBytes`, as given (the child clamps it).
    pub max_inline_bytes: Option<u64>,
    /// Digest: the stdout always goes to a file, with a summary inline.
    pub always_file: bool,
}

/// A call of `alsem_status` / `alsem_refresh`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineCall {
    /// `None`: every engine the server runs (status only).
    pub workspace: Option<String>,
    pub universe: Universe,
    /// Refresh: wait for the refresh to finish before answering.
    pub wait: bool,
}

/// A validated tool call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolCall {
    Command(CommandCall),
    Status(EngineCall),
    Refresh(EngineCall),
}

/// The tool names, in `tools/list` order.
pub const TOOL_NAMES: &[&str] = &[
    "alsem_explain",
    "alsem_prove",
    "alsem_digest",
    "alsem_query_effects",
    "alsem_query_touches",
    "alsem_status",
    "alsem_refresh",
];

/// The properties every command tool takes (design r7 §8.2).
fn common_properties() -> Map<String, Value> {
    let mut m = Map::new();
    m.insert(
        "workspace".into(),
        json!({"type": "string", "description": "Path to the AL workspace root (the folder holding app.json). Defaults to the server's --workspace."}),
    );
    m.insert(
        "freshness".into(),
        json!({"type": "string", "enum": ["fresh", "latest-built", "verify"], "description": "fresh (default): the answer reflects the files as they are now. latest-built: the newest built revision may answer. verify: answer from the built revision and report whether it is stale."}),
    );
    m.insert(
        "singleApp".into(),
        json!({"type": "boolean", "description": "Analyse the workspace alone. The default loads the dependencies it requires (cross-app)."}),
    );
    m.insert(
        "out".into(),
        json!({"type": "string", "description": "Write the output to this file instead of inline: a path relative to the server's output folder (or an absolute path inside it). Paths outside it are refused."}),
    );
    m.insert(
        "maxInlineBytes".into(),
        json!({"type": "integer", "minimum": 0, "description": "The largest output returned inline (default 65536, at most 4194304); a larger one is written to a file in the server's output folder."}),
    );
    m
}

fn string_array(description: &str) -> Value {
    json!({"type": "array", "items": {"type": "string"}, "minItems": 1, "description": description})
}

fn schema(extra: Vec<(&str, Value)>, required: &[&str], common: bool) -> Value {
    let mut props = if common {
        common_properties()
    } else {
        Map::new()
    };
    for (k, v) in extra {
        props.insert(k.to_string(), v);
    }
    let mut s = json!({
        "type": "object",
        "properties": Value::Object(props),
        "additionalProperties": false,
    });
    if !required.is_empty() {
        s["required"] = json!(required);
    }
    s
}

fn deterministic() -> Value {
    json!({"type": "boolean", "description": "Pin timestamps and the version for byte-stable output."})
}

/// Every tool, as `tools/list` returns it.
#[must_use]
pub fn tool_list() -> Vec<Value> {
    let tool = |name: &str, description: &str, input: Value| json!({"name": name, "description": description, "inputSchema": input});
    vec![
        tool(
            "alsem_explain",
            "Explain AL routines: the evidence card of each (its own rules with their conditions and messages, inherited and caller rules, what it writes, commits and raises, the page actions and tests that reach it, and what the engine could not see). Exactly one mode: `routines` (one or more selectors: display name, `<owner>::<routine>` or stable id), `diff` (a unified diff's text; one card per routine it changes; `diffRoot` is the folder its paths are relative to, default the git top level), or `table` with `field` (every workspace routine that writes the field). Default format is the human card; `format: json` gives schema al-sem.explain/1. Returns the exact `alsem explain` stdout.",
            schema(
                vec![
                    ("routines", string_array("Routine selectors (mode 1).")),
                    (
                        "diff",
                        json!({"type": "string", "description": "A unified diff's text (mode 2)."}),
                    ),
                    (
                        "diffRoot",
                        json!({"type": "string", "description": "With diff: the folder the diff's paths are relative to."}),
                    ),
                    (
                        "table",
                        json!({"type": "string", "description": "With field (mode 3): a table name, number or id."}),
                    ),
                    (
                        "field",
                        json!({"type": "string", "description": "With table (mode 3): a field name or number."}),
                    ),
                    (
                        "format",
                        json!({"type": "string", "enum": ["human", "json"], "description": "Output format (default human)."}),
                    ),
                ],
                &[],
                true,
            ),
        ),
        tool(
            "alsem_prove",
            "Prove one absence-safety question about one AL routine: yes / no / unknown, with the reason. Questions: may-commit, commits-on-success-path, writes-table:<table>, publishes-event:<event>, reaches-ui, throws-error. Returns the exact `alsem prove` stdout (default format json).",
            schema(
                vec![
                    (
                        "routine",
                        json!({"type": "string", "description": "Routine selector: display name or stable id."}),
                    ),
                    (
                        "question",
                        json!({"type": "string", "description": "may-commit | commits-on-success-path | writes-table:<name> | publishes-event:<name> | reaches-ui | throws-error"}),
                    ),
                    (
                        "format",
                        json!({"type": "string", "enum": ["json", "human"], "description": "Output format (default json)."}),
                    ),
                    (
                        "closedWorld",
                        json!({"type": "boolean", "description": "Assume the loaded world is all there is: an interface call dispatches only to the loaded implementations."}),
                    ),
                    ("deterministic", deterministic()),
                ],
                &["routine", "question"],
                true,
            ),
        ),
        tool(
            "alsem_digest",
            "Digest changed AL code: the transitive effects (writes, commits, errors, events, UI) of the changed roots. Give at least one of `routines` (stable ids or display names), `files` (changed source files, relative to the workspace or the git top level) or `diff` (a unified diff's text). The output always goes to a file in the server's output folder; the answer carries its path, size, SHA-256 and a summary (roots requested and resolved, effect counts, per-root coverage).",
            schema(
                vec![
                    ("routines", string_array("Changed routines.")),
                    ("files", string_array("Changed source files.")),
                    (
                        "diff",
                        json!({"type": "string", "description": "A unified diff's text."}),
                    ),
                    (
                        "diffRoot",
                        json!({"type": "string", "description": "The folder the diff's and files' paths are relative to (default: the git top level)."}),
                    ),
                    (
                        "format",
                        json!({"type": "string", "enum": ["json", "human"], "description": "Output format (default json)."}),
                    ),
                    (
                        "tests",
                        json!({"type": "boolean", "description": "Add a testCoverage block: which changed routines a [Test] procedure reaches."}),
                    ),
                    (
                        "includeReads",
                        json!({"type": "boolean", "description": "Add DB_READ effects, one per table read site."}),
                    ),
                    ("deterministic", deterministic()),
                ],
                &[],
                true,
            ),
        ),
        tool(
            "alsem_query_effects",
            "A routine's complete transitive database-effect list, with `via` provenance. Returns the exact `alsem query effects` stdout (default format human); an output over the inline cap goes to a file.",
            schema(
                vec![
                    (
                        "routine",
                        json!({"type": "string", "description": "Routine selector: internal id, stable id, `Object.Routine`, or a bare routine name."}),
                    ),
                    (
                        "format",
                        json!({"type": "string", "enum": ["human", "json"], "description": "Output format (default human)."}),
                    ),
                    ("deterministic", deterministic()),
                ],
                &["routine"],
                true,
            ),
        ),
        tool(
            "alsem_query_touches",
            "Is a table touched by any database action, transitively, up or down the call stack? Without `from`, every routine that touches it. Returns the exact `alsem query touches` stdout (default format human); an output over the inline cap goes to a file.",
            schema(
                vec![
                    (
                        "table",
                        json!({"type": "string", "description": "A table number or name, an internal table id, or `unknown`."}),
                    ),
                    (
                        "from",
                        json!({"type": "string", "description": "Scope the answer to one routine."}),
                    ),
                    (
                        "direction",
                        json!({"type": "string", "enum": ["down", "up", "both"], "description": "down | up | both (default both); up requires from."}),
                    ),
                    (
                        "format",
                        json!({"type": "string", "enum": ["human", "json"], "description": "Output format (default human)."}),
                    ),
                    ("deterministic", deterministic()),
                ],
                &["table"],
                true,
            ),
        ),
        tool(
            "alsem_status",
            "The state of the server's engines: per workspace and universe, the engine child (pid, version, instance id), its counters, its last failure and its stderr tail; and the server's output folder. Starts nothing.",
            schema(
                vec![
                    (
                        "workspace",
                        json!({"type": "string", "description": "Only this workspace's engines."}),
                    ),
                    (
                        "singleApp",
                        json!({"type": "boolean", "description": "With workspace: only the single-app engine (default: the cross-app one)."}),
                    ),
                ],
                &[],
                false,
            ),
        ),
        tool(
            "alsem_refresh",
            "Refresh a workspace's engine: leave a failed state (after repeated engine deaths) so the next call starts a new engine. Today every call reads the workspace as it is now, so there is no built revision to update.",
            schema(
                vec![
                    (
                        "workspace",
                        json!({"type": "string", "description": "The workspace (default: the server's --workspace)."}),
                    ),
                    (
                        "singleApp",
                        json!({"type": "boolean", "description": "The single-app engine (default: the cross-app one)."}),
                    ),
                    (
                        "wait",
                        json!({"type": "boolean", "description": "Wait until the refresh is done (default true)."}),
                    ),
                ],
                &[],
                false,
            ),
        ),
    ]
}

/// Why a call's arguments are refused (a `-32602` message).
pub type Invalid = String;

/// A call's arguments, read field by field against the tool's schema.
struct Args<'a> {
    map: &'a Map<String, Value>,
}

impl<'a> Args<'a> {
    fn string(&self, key: &str) -> Result<Option<&'a str>, Invalid> {
        match self.map.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s)),
            Some(_) => Err(format!("'{key}' must be a string")),
        }
    }

    fn required(&self, key: &str) -> Result<&'a str, Invalid> {
        match self.string(key)? {
            Some(s) if !s.is_empty() => Ok(s),
            _ => Err(format!("'{key}' is required")),
        }
    }

    fn bool(&self, key: &str) -> Result<Option<bool>, Invalid> {
        match self.map.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(_) => Err(format!("'{key}' must be a boolean")),
        }
    }

    fn flag(&self, key: &str) -> Result<bool, Invalid> {
        Ok(self.bool(key)?.unwrap_or(false))
    }

    /// A selector array: absent, or one or more non-empty strings.
    fn strings(&self, key: &str) -> Result<Option<Vec<&'a str>>, Invalid> {
        match self.map.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Array(items)) => {
                if items.is_empty() {
                    return Err(format!("'{key}' must name at least one entry"));
                }
                items
                    .iter()
                    .map(|v| match v {
                        Value::String(s) if !s.is_empty() => Ok(s.as_str()),
                        _ => Err(format!("'{key}' entries must be non-empty strings")),
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(Some)
            }
            Some(_) => Err(format!("'{key}' must be an array of strings")),
        }
    }

    fn enumerated(
        &self,
        key: &str,
        allowed: &[&str],
        default: &'static str,
    ) -> Result<String, Invalid> {
        match self.string(key)? {
            None => Ok(default.to_string()),
            Some(s) if allowed.contains(&s) => Ok(s.to_string()),
            Some(s) => Err(format!("'{key}' must be one of {allowed:?}, not '{s}'")),
        }
    }
}

/// The properties of `tool`'s schema.
fn allowed_keys(tool: &str) -> Vec<String> {
    tool_list()
        .into_iter()
        .find(|t| t["name"] == tool)
        .and_then(|t| {
            t["inputSchema"]["properties"]
                .as_object()
                .map(|p| p.keys().cloned().collect())
        })
        .unwrap_or_default()
}

/// The argv under construction: options first (`--flag=value`), then `--`
/// and the positionals.
struct Argv {
    head: Vec<String>,
    options: Vec<String>,
    positionals: Vec<String>,
}

impl Argv {
    fn new(command: &[&str], workspace: &str) -> Argv {
        Argv {
            head: command.iter().map(|s| (*s).to_string()).collect(),
            options: Vec::new(),
            positionals: vec![workspace.to_string()],
        }
    }

    fn opt(&mut self, flag: &str, value: &str) {
        self.options.push(format!("--{flag}={value}"));
    }

    fn switch(&mut self, flag: &str, on: bool) {
        if on {
            self.options.push(format!("--{flag}"));
        }
    }

    fn done(self) -> Vec<String> {
        let mut v = self.head;
        v.extend(self.options);
        v.push("--".into());
        v.extend(self.positionals);
        v
    }
}

/// Validate a `tools/call` of `name` with `arguments` against the server's
/// defaults. `Err(None)`: no such tool; `Err(Some(why))`: invalid arguments.
pub fn parse_call(
    name: &str,
    arguments: Option<&Value>,
    defaults: &Defaults,
) -> Result<ToolCall, Option<Invalid>> {
    let Some(tool) = TOOL_NAMES.iter().copied().find(|t| *t == name) else {
        return Err(None);
    };
    let empty = Map::new();
    let map = match arguments {
        None | Some(Value::Null) => &empty,
        Some(Value::Object(m)) => m,
        Some(_) => return Err(Some("'arguments' must be an object".into())),
    };
    let allowed = allowed_keys(tool);
    if let Some(k) = map.keys().find(|k| !allowed.contains(k)) {
        return Err(Some(format!("{tool} takes no argument '{k}'")));
    }
    parse_known(tool, &Args { map }, defaults).map_err(Some)
}

/// `path` made absolute against the server's current directory (not
/// resolved: a symlinked workspace keeps its spelling, as on the CLI).
fn absolute(path: &str) -> String {
    std::path::absolute(path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string())
}

fn universe_of(args: &Args<'_>, defaults: &Defaults) -> Result<Universe, Invalid> {
    Ok(match args.bool("singleApp")? {
        Some(true) => Universe::SingleApp,
        Some(false) => Universe::CrossApp,
        None => defaults.universe,
    })
}

fn parse_known(tool: &'static str, a: &Args<'_>, defaults: &Defaults) -> Result<ToolCall, Invalid> {
    let universe = universe_of(a, defaults)?;
    if tool == "alsem_status" || tool == "alsem_refresh" {
        let workspace = a
            .string("workspace")?
            .map(str::to_string)
            .or_else(|| (tool == "alsem_refresh").then(|| defaults.workspace.clone())?);
        if tool == "alsem_refresh" && workspace.is_none() {
            return Err("'workspace' is required (the server has no --workspace)".into());
        }
        let call = EngineCall {
            workspace: workspace.as_deref().map(absolute),
            universe,
            wait: a.bool("wait")?.unwrap_or(true),
        };
        return Ok(if tool == "alsem_status" {
            ToolCall::Status(call)
        } else {
            ToolCall::Refresh(call)
        });
    }
    let workspace = match a.string("workspace")? {
        Some(w) if !w.is_empty() => w.to_string(),
        _ => defaults
            .workspace
            .clone()
            .ok_or("'workspace' is required (the server has no --workspace)")?,
    };
    let workspace = absolute(&workspace);
    let freshness = match a.string("freshness")? {
        None => defaults.freshness,
        Some(s) => Freshness::parse(s).ok_or_else(|| {
            format!("'freshness' must be fresh, latest-built or verify, not '{s}'")
        })?,
    };
    let max_inline_bytes = match a.map.get("maxInlineBytes") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_u64()
                .ok_or("'maxInlineBytes' must be a non-negative integer")?,
        ),
    };
    let out = a.string("out")?.map(str::to_string);
    if out.as_deref() == Some("") {
        return Err("'out' must name a file".into());
    }
    let mut diff_text = None;
    let mut always_file = false;
    let mut argv = match tool {
        "alsem_explain" => {
            let mut v = Argv::new(&["explain"], &workspace);
            let routines = a.strings("routines")?;
            let diff = a.string("diff")?;
            let (table, field) = (a.string("table")?, a.string("field")?);
            let modes = [
                routines.is_some(),
                diff.is_some(),
                table.is_some() || field.is_some(),
            ];
            if modes.iter().filter(|m| **m).count() != 1 {
                return Err(
                    "alsem_explain takes exactly one mode: 'routines', 'diff', or 'table' with 'field'"
                        .into(),
                );
            }
            if a.string("diffRoot")?.is_some() && diff.is_none() {
                return Err("'diffRoot' goes with 'diff'".into());
            }
            if let Some(rs) = routines {
                for r in rs {
                    v.opt("routine", r);
                }
            }
            if let Some(d) = diff {
                v.opt("diff", "-");
                diff_text = Some(d.to_string());
                if let Some(root) = a.string("diffRoot")? {
                    v.opt("diff-root", root);
                }
            }
            if table.is_some() || field.is_some() {
                match (table, field) {
                    (Some(t), Some(f)) => {
                        v.opt("table", t);
                        v.opt("field", f);
                    }
                    _ => return Err("'table' and 'field' go together".into()),
                }
            }
            v.opt(
                "format",
                &a.enumerated("format", &["human", "json"], "human")?,
            );
            v
        }
        "alsem_prove" => {
            let mut v = Argv::new(&["prove"], &workspace);
            v.positionals.push(a.required("routine")?.to_string());
            v.positionals.push(a.required("question")?.to_string());
            v.opt(
                "format",
                &a.enumerated("format", &["json", "human"], "json")?,
            );
            v.switch("closed-world", a.flag("closedWorld")?);
            v.switch("deterministic", a.flag("deterministic")?);
            v
        }
        "alsem_digest" => {
            let mut v = Argv::new(&["digest"], &workspace);
            let routines = a.strings("routines")?;
            let files = a.strings("files")?;
            let diff = a.string("diff")?;
            if routines.is_none() && files.is_none() && diff.is_none() {
                return Err(
                    "alsem_digest needs at least one of 'routines', 'files' or 'diff'".into(),
                );
            }
            if a.string("diffRoot")?.is_some() && diff.is_none() && files.is_none() {
                return Err("'diffRoot' goes with 'diff' or 'files'".into());
            }
            for r in routines.unwrap_or_default() {
                v.opt("routine", r);
            }
            for f in files.unwrap_or_default() {
                v.opt("file", f);
            }
            if let Some(d) = diff {
                v.opt("diff", "-");
                diff_text = Some(d.to_string());
            }
            if let Some(root) = a.string("diffRoot")? {
                v.opt("diff-root", root);
            }
            v.opt(
                "format",
                &a.enumerated("format", &["json", "human"], "json")?,
            );
            v.switch("tests", a.flag("tests")?);
            v.switch("include-reads", a.flag("includeReads")?);
            v.switch("deterministic", a.flag("deterministic")?);
            always_file = true;
            v
        }
        "alsem_query_effects" => {
            let mut v = Argv::new(&["query", "effects"], &workspace);
            v.opt("routine", a.required("routine")?);
            v.opt(
                "format",
                &a.enumerated("format", &["human", "json"], "human")?,
            );
            v.switch("deterministic", a.flag("deterministic")?);
            v
        }
        "alsem_query_touches" => {
            let mut v = Argv::new(&["query", "touches"], &workspace);
            v.opt("table", a.required("table")?);
            if let Some(from) = a.string("from")? {
                v.opt("from", from);
            }
            v.opt(
                "direction",
                &a.enumerated("direction", &["down", "up", "both"], "both")?,
            );
            v.opt(
                "format",
                &a.enumerated("format", &["human", "json"], "human")?,
            );
            v.switch("deterministic", a.flag("deterministic")?);
            v
        }
        _ => unreachable!("every tool is listed"),
    };
    argv.switch("single-app", universe == Universe::SingleApp);
    Ok(ToolCall::Command(CommandCall {
        tool,
        workspace,
        universe,
        freshness,
        argv: argv.done(),
        diff_text,
        out,
        max_inline_bytes,
        always_file,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> Defaults {
        Defaults {
            workspace: Some("/ws".into()),
            universe: Universe::CrossApp,
            freshness: Freshness::Fresh,
        }
    }

    fn command(name: &str, args: Value) -> Result<CommandCall, Option<Invalid>> {
        match parse_call(name, Some(&args), &defaults())? {
            ToolCall::Command(c) => Ok(c),
            other => panic!("{other:?}"),
        }
    }

    /// Each command tool builds the argv the CLI takes: options as
    /// `--flag=value`, then `--` and the positionals; the defaults are the
    /// amendment's (explain human) or the CLI's.
    #[test]
    fn calls_become_cli_argv() {
        let c = command("alsem_explain", json!({"routines": ["A", "-B"]})).unwrap();
        assert_eq!(
            c.argv,
            [
                "explain",
                "--routine=A",
                "--routine=-B",
                "--format=human",
                "--",
                "/ws"
            ]
        );
        let c = command(
            "alsem_explain",
            json!({"diff": "D", "diffRoot": "/r", "format": "json", "singleApp": true}),
        )
        .unwrap();
        assert_eq!(
            c.argv,
            [
                "explain",
                "--diff=-",
                "--diff-root=/r",
                "--format=json",
                "--single-app",
                "--",
                "/ws"
            ]
        );
        assert_eq!(c.diff_text.as_deref(), Some("D"));
        assert_eq!(c.universe, Universe::SingleApp);
        let c = command(
            "alsem_prove",
            json!({"routine": "P", "question": "may-commit", "deterministic": true}),
        )
        .unwrap();
        assert_eq!(
            c.argv,
            [
                "prove",
                "--format=json",
                "--deterministic",
                "--",
                "/ws",
                "P",
                "may-commit"
            ]
        );
        let c = command(
            "alsem_digest",
            json!({"routines": ["P"], "files": ["a.al"]}),
        )
        .unwrap();
        assert!(c.always_file);
        assert_eq!(
            c.argv,
            [
                "digest",
                "--routine=P",
                "--file=a.al",
                "--format=json",
                "--",
                "/ws"
            ]
        );
        let c = command(
            "alsem_query_touches",
            json!({"table": "50400", "from": "P", "direction": "up"}),
        )
        .unwrap();
        assert_eq!(
            c.argv,
            [
                "query",
                "touches",
                "--table=50400",
                "--from=P",
                "--direction=up",
                "--format=human",
                "--",
                "/ws"
            ]
        );
        let c = command("alsem_query_effects", json!({"routine": "P"})).unwrap();
        assert_eq!(
            c.argv,
            [
                "query",
                "effects",
                "--routine=P",
                "--format=human",
                "--",
                "/ws"
            ]
        );
    }

    /// Exclusive modes, empty selector arrays, unknown arguments and wrong
    /// types are refused with a message; an unknown tool is `Err(None)`.
    #[test]
    fn invalid_calls_are_refused() {
        let bad = |name: &str, args: Value| match command(name, args) {
            Err(Some(why)) => why,
            other => panic!("{name}: {other:?}"),
        };
        assert!(bad("alsem_explain", json!({})).contains("exactly one mode"));
        assert!(
            bad("alsem_explain", json!({"routines": ["A"], "diff": "x"}))
                .contains("exactly one mode")
        );
        assert!(bad("alsem_explain", json!({"routines": []})).contains("at least one"));
        assert!(bad("alsem_explain", json!({"table": "T"})).contains("go together"));
        assert!(
            bad(
                "alsem_explain",
                json!({"routines": ["A"], "diffRoot": "/r"})
            )
            .contains("diffRoot")
        );
        assert!(bad("alsem_explain", json!({"routines": ["A"], "bogus": 1})).contains("bogus"));
        assert!(
            bad("alsem_explain", json!({"routines": ["A"], "format": "xml"})).contains("format")
        );
        assert!(bad("alsem_digest", json!({})).contains("at least one"));
        assert!(bad("alsem_prove", json!({"routine": "P"})).contains("question"));
        assert!(bad("alsem_prove", json!({"routine": 3, "question": "q"})).contains("string"));
        assert!(
            bad(
                "alsem_prove",
                json!({"routine": "P", "question": "q", "maxInlineBytes": -1})
            )
            .contains("maxInlineBytes")
        );
        assert_eq!(parse_call("alsem_nope", None, &defaults()), Err(None));
        let no_default = Defaults {
            workspace: None,
            ..defaults()
        };
        assert!(matches!(
            parse_call("alsem_prove", Some(&json!({"routine": "P", "question": "q"})), &no_default),
            Err(Some(why)) if why.contains("workspace")
        ));
    }

    /// Every listed tool's schema is an object schema with no top-level
    /// combinator, and refuses unknown properties.
    #[test]
    fn schemas_are_plain_objects() {
        let tools = tool_list();
        let names: Vec<_> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, TOOL_NAMES);
        for t in &tools {
            let s = &t["inputSchema"];
            assert_eq!(s["type"], "object", "{t}");
            assert_eq!(s["additionalProperties"], false, "{t}");
            for k in ["oneOf", "anyOf", "allOf"] {
                assert!(s.get(k).is_none(), "{t}");
            }
        }
    }
}
