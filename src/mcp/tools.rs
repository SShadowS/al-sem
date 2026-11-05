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
    /// The most characters the client's view of an inline answer may take
    /// (`structuredContent`, serialized), when the call left the inline cap
    /// to the server (item T, T-W2-23); `None` when it set `maxInlineBytes`.
    pub client_budget_chars: Option<usize>,
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

/// The inline cap when a call names none (MCP output plan 1c, item T's E4):
/// Claude Code files a result over 50,000 characters instead of showing it,
/// and it measures `structuredContent`, which holds the stdout escaped plus
/// the envelope. 40 KiB leaves that room; the wire's own default (64 KiB)
/// still serves the LSP front end.
pub const MCP_DEFAULT_INLINE_BYTES: u64 = 40 * 1024;

/// Claude Code's limit (measured 2026-11-04): a result whose serialized
/// `structuredContent` is over 50,000 characters is filed, not shown. The
/// raw byte cap above cannot keep an answer under it (a stdout of quotes and
/// backslashes doubles when escaped), so the server also bounds the escaped
/// answer (MCP output plan item T, T-W2-23).
pub const MCP_CLIENT_BUDGET_CHARS: usize = 50_000;

/// What the envelope around an inline answer may add to its
/// `structuredContent`: an answer whose escaped text plus this exceeds
/// [`MCP_CLIENT_BUDGET_CHARS`] goes to a file with its summary.
pub const MCP_ENVELOPE_ROOM_CHARS: usize = 4_096;

/// The whole serialized `tools/call` result's budget (T-W2-23's second
/// budget): the stdout escaped twice (the text block and
/// `structuredContent`) under the client budget, plus the envelope twice.
pub const MCP_WIRE_BUDGET_CHARS: usize = 2 * MCP_CLIENT_BUDGET_CHARS + 2 * MCP_ENVELOPE_ROOM_CHARS;

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
        json!({"type": "integer", "minimum": 0, "description": "The largest output returned inline (default 40960, at most 4194304); a larger one is written to a file in the server's output folder. Left unset, an answer over the client's 50,000 characters once escaped also goes to a file; set, the cap is exact."}),
    );
    m
}

/// The one selector grammar every routine argument takes (MCP output plan
/// 3b; the server instructions state it once).
const SELECTOR: &str = "a stable routine id, `Owner::Routine` or `Owner.Routine` (type word and AL quotes optional), or a bare routine name unique in the workspace";

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
            "Use before editing or reviewing AL routines: one evidence card per routine with its own rules (conditions and messages), inherited and caller rules, the physical tables it may write (owned by the app and by dependencies), commits, events, UI, HTTP, file, background tasks and errors, who calls it (routines and page actions), the tests that reach it, and the gaps the engine could not follow. Exactly one mode: `routines` (selectors), `diff` (a unified diff's text: one card per routine it changes; `diffRoot` is the folder its paths are relative to, default the git top level), or `table` with `field` (every workspace routine that writes the field). Default (`format: human`): a summary line first, then coverage, assumptions, limits and scope, then the sections; temporary records counted apart, never as writes; long lists folded with their full count; one path per item. Ask for more: `detail: true` (every list complete, up to 3 paths per item), `maxPaths: 0..3`, `showSource: true` (the cited source line ±2 under each location), `writesTables: [T]` (does it write T? per card, prove's own answer), `format: json` (schema al-sem.explain/3, for parsing; 270-580 KB for a routine that calls Base Application, so a file with a summary of each card's header and counts). Size: a few lines for a small routine, 30-40 KB human for one that calls into Base Application. Lines are 1-based. Dependency code: the whole cone with literal arguments, grouped per entry point (tables per kind, names folded). Next: alsem_prove writes-table:<T> or may-commit for one witness; alsem_query_effects for every operation. Returns the exact `alsem explain` stdout.",
            schema(
                vec![
                    (
                        "routines",
                        string_array(&format!("Mode 1, one card each: {SELECTOR}.")),
                    ),
                    (
                        "writesTables",
                        string_array(
                            "Mode 1: answer 'does it write table T?' per card, as alsem_prove writes-table:T answers.",
                        ),
                    ),
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
                    (
                        "detail",
                        json!({"type": "boolean", "description": "Every list complete, no fold (every table, entry point, page action, dependency gap, event name); up to 3 paths per item."}),
                    ),
                    (
                        "maxPaths",
                        json!({"type": "integer", "minimum": 0, "description": "Paths per item (default 1; 3 with detail; at most 3)."}),
                    ),
                    (
                        "showSource",
                        json!({"type": "boolean", "description": "Human output: the cited source line ±2 under each location (`--show-source`)."}),
                    ),
                ],
                &[],
                true,
            ),
        ),
        tool(
            "alsem_prove",
            "A yes/no question about one AL routine (`routine`, a selector), answered with its evidence. `question`: may-commit, commits-on-success-path, writes-table:<table>, publishes-event:<event>, reaches-ui, throws-error. Answers: yes (a path to the effect exists; a may-answer), no (proven absent under the stated assumptions), unknown (no matching effect observed, but coverage is incomplete, so not a proof). A temporary-record write is not a write for writes-table. Default (`format: human`): about 10 lines with the answer, each evidence row with its location and one witness path, then the blockers of an unknown grouped by kind with their counts and the first 3 of each (workspace ones first), then coverage (calls not resolved and interface calls open world, with the top callees), assumptions and limits. Ask for more: `detail: true` (every blocker), `closedWorld: true` (interface calls dispatch only to the loaded implementations; it can turn unknown into no, and says so under assumptions), `format: json` (every evidence row, its group path and witness hops; 5-60 KB). Size: 1-5 KB human. Lines are 1-based. Dependency code: the whole call cone is followed with literal arguments. Next: on unknown, alsem_explain lists the workspace gaps behind it. Returns the exact `alsem prove` stdout.",
            schema(
                vec![
                    (
                        "routine",
                        json!({"type": "string", "description": format!("The routine: {SELECTOR}.")}),
                    ),
                    (
                        "question",
                        json!({"type": "string", "description": "may-commit | commits-on-success-path | writes-table:<name> | publishes-event:<name> | reaches-ui | throws-error"}),
                    ),
                    (
                        "format",
                        json!({"type": "string", "enum": ["human", "json"], "description": "Output format (default human)."}),
                    ),
                    (
                        "closedWorld",
                        json!({"type": "boolean", "description": "Assume the loaded world is all there is: an interface call dispatches only to the loaded implementations."}),
                    ),
                    (
                        "detail",
                        json!({"type": "boolean", "description": "Human output: every blocker, not the first few of each kind (the JSON always lists every one)."}),
                    ),
                    ("deterministic", deterministic()),
                ],
                &["routine", "question"],
                true,
            ),
        ),
        tool(
            "alsem_digest",
            "The transitive effects (writes, commits, errors, events, UI, HTTP, files, background tasks) of a set of roots. The change modes: `files` (changed source files, relative to the workspace or the git top level) or `diff` (a unified diff's text; `diffRoot` is the folder its and the files' paths are relative to, default the git top level): the routines they change are the roots. `routines` (selectors) digests named routines: explain's effects with paths, without rules. Give at least one. Default (`format: json`): schema 3.0.0, every effect row, paths as structured hops into a `payload.routines` table, one path per decision effect; up to about 1,000 KB for a routine that calls Base Application. It always goes to a file in the server's output folder; the answer carries its path, size, SHA-256 and a summary of at most 8 KiB (roots requested and resolved; per root: coverage, effects by type, tables it may write owned by the app and by dependencies, caller-dependent tables, commits, events, kind families, gaps by where, temporary-only tables, writes with no path kept, entry points; the first names of each list): read the summary first. Ask for more or less: `format: human` (30-35 KB for such a routine, grouped as explain groups it), `detail: true` (every path up to 3, scoped guarantee, diagnostic, unresolved call site and group witness), `maxPaths: 0..3`, `compact: true` (JSON without indentation, about 40% smaller), `includeReads: true` (DB_READ effects, one per table read site), `tests: true` (a testCoverage block: which roots a [Test] procedure reaches), `writesTables: [T]` (does it write T? per root, prove's own answer), `schema: 2` (the 2.x document: the old shape over the current analysis, for one release window; not with detail, maxPaths or compact). Lines are 1-based. Dependency code: the whole cone with literal arguments, grouped per entry point. Next: alsem_explain on a root for its rules and callers. Returns the exact `alsem digest` stdout.",
            schema(
                vec![
                    (
                        "routines",
                        string_array(&format!("The roots by name: {SELECTOR}.")),
                    ),
                    (
                        "writesTables",
                        string_array(
                            "Answer 'does it write table T?' per root (entries[].tableAnswers), as alsem_prove writes-table:T answers.",
                        ),
                    ),
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
                    (
                        "detail",
                        json!({"type": "boolean", "description": "Unfolded 3.x: every path (up to 3), scoped guarantee, diagnostic, unresolved call site and group witness."}),
                    ),
                    (
                        "maxPaths",
                        json!({"type": "integer", "minimum": 0, "description": "Paths per effect (default 1 on a decision effect; 3 with detail; at most 3)."}),
                    ),
                    (
                        "compact",
                        json!({"type": "boolean", "description": "JSON without indentation."}),
                    ),
                    (
                        "schema",
                        json!({"type": "integer", "enum": [2, 3], "description": "2: the 2.x document (old shape, current analysis), for one release window; not with detail, maxPaths or compact."}),
                    ),
                    ("deterministic", deterministic()),
                ],
                &[],
                true,
            ),
        ),
        tool(
            "alsem_query_effects",
            "Every record operation one routine (`routine`, a selector) performs transitively, each with its temp state and `via` provenance: reads, locks, validates, temporary records and caller-dependent writes included, each operation in exactly one bucket. Default (`format: human`): a summary line (writes that may persist, writes beyond dependency entry points, caller-dependent writes, locks, validate, reads, temporary records, on a RecordRef (table decided at run time), table not resolved), then writes per table with the first operation's location and first hop, then the writes beyond dependency entry points (alsem_explain's own section); about 20-25 KB for a routine that reaches Base Application. Ask for more: `all: true` (one line per operation, temporary records and reads included), `showSource: true` (the cited source line ±2 under each location), `format: json` (every row with its `firstHop`, and a `summary`; about 900 KB for such a routine, so a file with a summary of counts by operation and temp state). Lines are 1-based. Dependency code: per operation one routine deep (the operations of the first dependency routine entered); physical writes beyond each dependency entry point follow the whole cone with literal arguments, as alsem_explain does (`summary.beyondEntryPoints` in JSON); operations the cone rules out under literal arguments are still listed here, each row marked as the cone judges its route (`literalRoute`: ruledOut and notInCone, no route runs; otherRoute, the index's first hop is not the cone's live route, which it names; coneRoute; absent, not judged), and the human table lines say so. Next: alsem_prove writes-table:<T> for one table's witness. Returns the exact `alsem query effects` stdout.",
            schema(
                vec![
                    (
                        "routine",
                        json!({"type": "string", "description": format!("The routine, a workspace one: {SELECTOR} (or an internal id).")}),
                    ),
                    (
                        "all",
                        json!({"type": "boolean", "description": "Every operation in the human output, one line each (`--all`); JSON always holds every row."}),
                    ),
                    (
                        "showSource",
                        json!({"type": "boolean", "description": "Human output: the cited source line ±2 under each location (`--show-source`)."}),
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
            "Is a table touched by any record operation, transitively? `table`: a number, a name, an internal table id, or unknown (operations whose table is decided at run time). Without `from`: every workspace routine that touches it, by the per-operation index. With `from` (a routine selector), `direction`: down (does it touch the table, and through which operations; when no indexed operation does, the answer is `unknown` with its reasons, such as operations on a RecordRef, calls not resolved or dependency code deeper than one routine, and `no` only when none applies), up (its transitive callers: how many touch it, do not, or are unknown), or both (the default). Touches are counted on persistent records, on possibly persistent records and on temporary records (a temporary-record touch persists nothing but uses the table's schema and runs its triggers); when the routine touches the table itself its callers inherit the touch and are counted, not listed. Default (`format: human`): a few lines to about 10 KB, lists folded. Ask for more: `callers: true` (every caller by name, depth and location, without witnesses), `all: true` (every witness, the callers' included, no fold; tens of megabytes on a hub routine), `showSource: true` (the cited source line ±2), `format: json` (every down witness, about 200 KB on a hub routine; over the cap a file with a summary of the down and up headlines). Lines are 1-based. Dependency code: per operation one routine deep; with `from` asked down, the dependency entry points beyond which the whole cone may write the table are listed too (`down.beyond`). Next: alsem_query_effects for the routine's other tables. Returns the exact `alsem query touches` stdout.",
            schema(
                vec![
                    (
                        "table",
                        json!({"type": "string", "description": "A table number or name, an internal table id, or `unknown`."}),
                    ),
                    (
                        "from",
                        json!({"type": "string", "description": format!("Scope the answer to one workspace routine: {SELECTOR}.")}),
                    ),
                    (
                        "direction",
                        json!({"type": "string", "enum": ["down", "up", "both"], "description": "down | up | both (default both); up requires from."}),
                    ),
                    (
                        "all",
                        json!({"type": "boolean", "description": "Every witness, the callers' included, and no folded list (`--all`). Large on a hub routine."}),
                    ),
                    (
                        "callers",
                        json!({"type": "boolean", "description": "Name every transitive caller with its depth and location, without their witnesses (`--callers`)."}),
                    ),
                    (
                        "showSource",
                        json!({"type": "boolean", "description": "Human output: the cited source line ±2 under each location (`--show-source`)."}),
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
            "Is the engine's answer current, and what is it doing? Per workspace and universe, `freshness` gives the revision that answers (`generation`, which a rebuild raises, and `inputsStamp`, the hash of the inputs it was built from, the same `revision` every answer's envelope carries), the last build (`class`: cold, P package, S declaration, B body; `startedAtUnixMs`, `durationMs`), `building` (true while a call checks freshness or builds, or a refresh runs; null after an unwaited refresh until the next call), `activity` (idle, asking, pin, prepare, compute, refresh) and the engine child's memory (`rssBytes`, `peakRssBytes`, read live where the OS allows). Also the child (pid, version), its counters, its last failure and stderr tail, and the server's output folder. Starts nothing; answers at once, even while a call runs.",
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
            "Refresh a workspace's engine: bring its kept revision up to date with the files as they are now (what a command tool's default freshness, fresh, does on demand), so later answers asked latest-built are current and fast; and leave a failed state (after repeated engine deaths) so the next call starts a new engine. With `wait: true` (the default) it answers after the refresh with the revision; `wait: false` answers at once. Use it after editing files when the next questions may take the last built revision. Answers no question itself; its output has no source lines.",
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

    /// A non-negative integer, or absent.
    fn count(&self, key: &str) -> Result<Option<u64>, Invalid> {
        match self.map.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v
                .as_u64()
                .map(Some)
                .ok_or_else(|| format!("'{key}' must be a non-negative integer")),
        }
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
        None | Some(Value::Null) => Some(MCP_DEFAULT_INLINE_BYTES),
        Some(v) => Some(
            v.as_u64()
                .ok_or("'maxInlineBytes' must be a non-negative integer")?,
        ),
    };
    // A caller that sets its own cap (the LSP front end does) asks for that
    // cap exactly; the server's default also keeps the client's view small.
    let client_budget_chars = match a.map.get("maxInlineBytes") {
        None | Some(Value::Null) => Some(MCP_CLIENT_BUDGET_CHARS),
        Some(_) => None,
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
            // Design §5: `--max-paths` only when the caller sets it, so
            // `detail` alone gives the detail cap.
            v.switch("detail", a.flag("detail")?);
            if let Some(n) = a.count("maxPaths")? {
                v.opt("max-paths", &n.to_string());
            }
            v.switch("show-source", a.flag("showSource")?);
            for t in a.strings("writesTables")?.unwrap_or_default() {
                v.opt("writes-table", t);
            }
            v
        }
        "alsem_prove" => {
            let mut v = Argv::new(&["prove"], &workspace);
            v.positionals.push(a.required("routine")?.to_string());
            v.positionals.push(a.required("question")?.to_string());
            // MCP output plan 1c (owner decision 3): one compact copy by default;
            // the CLI's own default stays json.
            v.opt(
                "format",
                &a.enumerated("format", &["json", "human"], "human")?,
            );
            v.switch("closed-world", a.flag("closedWorld")?);
            v.switch("detail", a.flag("detail")?);
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
            // Design §5: `--max-paths` only when set (`detail` alone gives 3).
            v.switch("detail", a.flag("detail")?);
            v.switch("compact", a.flag("compact")?);
            if let Some(n) = a.count("maxPaths")? {
                v.opt("max-paths", &n.to_string());
            }
            if let Some(n) = a.count("schema")? {
                v.opt("schema", &n.to_string());
            }
            for t in a.strings("writesTables")?.unwrap_or_default() {
                v.opt("writes-table", t);
            }
            always_file = true;
            v
        }
        "alsem_query_effects" => {
            let mut v = Argv::new(&["query", "effects"], &workspace);
            v.opt("routine", a.required("routine")?);
            v.switch("all", a.flag("all")?);
            v.switch("show-source", a.flag("showSource")?);
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
            v.switch("all", a.flag("all")?);
            v.switch("callers", a.flag("callers")?);
            v.switch("show-source", a.flag("showSource")?);
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
        client_budget_chars,
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
        // MCP output plan 1c: human by default for an agent; 40 KiB inline,
        // and (T-W2-23) the client's character budget.
        assert_eq!(c.max_inline_bytes, Some(MCP_DEFAULT_INLINE_BYTES));
        assert_eq!(c.client_budget_chars, Some(MCP_CLIENT_BUDGET_CHARS));
        let own = command(
            "alsem_prove",
            json!({"routine": "P", "question": "may-commit", "maxInlineBytes": 100}),
        )
        .unwrap();
        assert_eq!(own.client_budget_chars, None, "a caller's own cap is exact");
        assert_eq!(
            c.argv,
            [
                "prove",
                "--format=human",
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
