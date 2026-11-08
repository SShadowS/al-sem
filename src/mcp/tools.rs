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
    /// A test universe (agent-tasks §4.3): the engine's workspace is the test
    /// app; its descriptor is registered with the pool before the first call
    /// (`Pool::run_tests`). Never a call's own choice.
    Tests,
}

impl Universe {
    /// The envelope's spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Universe::CrossApp => "cross-app",
            Universe::SingleApp => "single-app",
            Universe::Tests => "tests",
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
    /// Status: hash every input in the freshness check, not only those
    /// whose size or modification time moved.
    pub verify: bool,
}

/// A call of `alsem_tests` (agent-tasks §4): `command` is the CLI command
/// it equals (`alsem tests …`, the byte contract); the pool runs it as a
/// composite request, part by part ([`crate::engine::test_select::run`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestsCall {
    pub command: CommandCall,
    pub request: crate::engine::test_select::run::TestsRequest,
    pub opts: crate::engine::test_select::render::RenderOpts,
}

/// A call of `alsem_impact` (agent-tasks §8): `command` is the CLI command
/// it equals (`alsem impact …`); the pool resolves the comparison tuple
/// (git, read only) and runs the parts as a composite request
/// ([`crate::engine::impact::run`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImpactCall {
    pub command: CommandCall,
    pub request: crate::engine::impact::run::ImpactRequest,
    pub opts: crate::engine::impact::render::RenderOpts,
}

/// A validated tool call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolCall {
    Command(CommandCall),
    Status(EngineCall),
    Refresh(EngineCall),
    Tests(TestsCall),
    Impact(ImpactCall),
}

/// The inline cap when a call names none (MCP output plan 1c, item T's E4):
/// Claude Code files a result over 50,000 characters instead of showing it,
/// and it measures `structuredContent`, which holds the stdout escaped plus
/// the envelope. 40 KiB leaves that room; the wire's own default (64 KiB)
/// still serves the LSP front end.
///
/// It was 48 KiB from output validation V3 until V5 (owner decision
/// 2026-11-06: the one routine display grew RunReportID's CDO explain card
/// to 45,374 bytes). V5's path segments (spec 2026-11-06 §7.5/§7.7: a shared
/// run of hops prints once, under a segment id) brought the card back under
/// 40 KiB, so the cap is 40 KiB again.
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
    "alsem_tests",
    "alsem_impact",
    "alsem_status",
    "alsem_refresh",
];

/// The `freshness` argument's description (agent-tasks design §2.3: what
/// `pin` does for each mode; `verify` rebuilds when stale, it does not
/// merely report it).
pub const FRESHNESS_DESCRIPTION: &str = "fresh (default): the answer reflects the files as they are now (a file whose size and modification time are unchanged is trusted). latest-built: the newest built revision may answer, even if files changed since. verify: like fresh, but hashes every input instead of trusting size and modification time; rebuilds first when anything changed.";

/// The properties every command tool takes (design r7 §8.2).
fn common_properties() -> Map<String, Value> {
    let mut m = Map::new();
    m.insert(
        "workspace".into(),
        json!({"type": "string", "description": "Path to the AL workspace root (the folder holding app.json). Defaults to the server's --workspace."}),
    );
    m.insert(
        "freshness".into(),
        json!({"type": "string", "enum": ["fresh", "latest-built", "verify"], "description": FRESHNESS_DESCRIPTION}),
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
const SELECTOR: &str = "a stable routine id, `Owner::Routine` or `Owner.Routine` (type word and AL quotes optional), or a bare routine name unique in the workspace; an overloaded name takes its parameter types in parentheses, `Owner::Routine(Integer, Text)` (or the parameter names), as an ambiguity answer prints each candidate; a field or action trigger takes its member, `Object.\"Field\"::OnValidate`, `Object.action(Name)::OnAction`. Every JSON routine row carries `selector`, the form that names it alone: pass that value back (a human display is pasteable for these forms only)";

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
    json!({"type": "boolean", "description": "Pin generatedAt and alsemVersion for byte-stable output. It concerns those two fields only, never the answer; the document's own `deterministic` field says whether they were pinned."})
}

/// Every tool, as `tools/list` returns it.
#[must_use]
pub fn tool_list() -> Vec<Value> {
    let tool = |name: &str, description: &str, input: Value| json!({"name": name, "description": description, "inputSchema": input});
    vec![
        tool(
            "alsem_explain",
            "Use before editing or reviewing AL routines: one evidence card per routine with its own rules (conditions and messages), inherited and caller rules, the physical tables it may write (owned by the app and by dependencies), commits, events, UI, HTTP, file, background tasks and errors, who calls it (routines and page actions), the tests that reach it, and the gaps the engine could not follow. Exactly one mode: `routines` (selectors), `diff` (a unified diff's text: one card per routine it changes; `diffRoot` is the folder its paths are relative to, default the git top level), or `table` with `field` (every workspace routine that writes the field). Default (`format: human`): the four header lines (the routine; `answer:`, its summary line; the analysis · assumptions · limits · scope line; `more:`), then the sections; writes to temporary records do not directly persist those records and are listed apart; counts are distinct tables unless a line says operations; long lists folded with their full count; one path per item. Ask for more: `detail: true` (every list complete, up to 3 paths per item), `maxPaths: 0..3`, `showSource: true` (the cited source line ±2 under each location), `writesTables: [T]` (does it write T? per card, prove's own answer), `callersContext: true` (a section with the three callers' context answers for adding a COMMIT, an error or UI: pending writes, intercepted errors, non-interactive entries; it builds the caller graph once, about a second on a large workspace), `format: json` (schema al-sem.explain/4, for parsing; 270-580 KB for a routine that calls Base Application, so a file with a summary of each card's header and counts). Size: a few lines for a small routine, 30-40 KB human for one that calls into Base Application. Lines are 1-based. Dependency code: the whole cone with literal arguments, grouped per entry point (tables per kind, names folded). Next: alsem_prove writes-table:<T> or may-commit for one witness; alsem_query_effects for every operation. Returns the exact `alsem explain` stdout.",
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
                        "callersContext",
                        json!({"type": "boolean", "description": "Add each card's callers' context section (pending writes, intercepted errors, non-interactive entries), for adding a COMMIT, an error or UI."}),
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
            "A yes/no question about one AL routine (`routine`, a selector), answered with its evidence. `question`, about the routine's existing effects (what it and its callees already do): may-commit, commits-on-success-path, writes-table:<table>, publishes-event:<event>, reaches-ui, throws-error. Or about its callers' context, before adding a COMMIT, an error or UI to it: callers-have-pending-writes (may a caller have a database write pending when it runs), callers-intercept-errors (may a caller turn its error into false: a TryFunction whose result is used, a checked Codeunit.Run, asserterror, collectible errors; a TryFunction does not roll back database changes), callers-may-run-noninteractive (may it run where no user answers UI: API pages, job queue and web service roots from roots.config.json, install and upgrade codeunits, background sessions); these walk every caller up to its entry points, and a public routine's callers in apps that are not loaded block a no unless `assumeNoDependents: true`. Answers: yes (a path to the effect exists; a may-answer), no (proven absent under the stated assumptions), unknown (no matching effect observed, but coverage is incomplete, so not a proof). Writes to temporary records do not directly persist those records, so writes-table does not count them. Default (`format: human`): about 10 lines with the answer, each evidence row with its location and one witness path, then the blockers of an unknown grouped by kind with their counts and the first 3 of each (workspace ones first), then coverage (calls not resolved and interface calls open world, with the top callees), assumptions and limits. Ask for more: `detail: true` (every blocker), `closedWorld: true` (interface calls dispatch only to the loaded implementations; it can turn unknown into no, and says so under assumptions), `format: json` (every evidence row, its group path and witness hops; 5-60 KB; its `deterministic` field concerns `generatedAt` and `alsemVersion` only, never the answer: false means a real timestamp). Why did an error or a write happen: filter to the sites that may explain it with `match: \"<text>\"` (throws-error: the error's static message text, a literal or a Label's or TextConst's value with %1 placeholders kept, or the Label's name), `site: \"<file>:<line>\"` (throws-error, writes-table, may-commit, reaches-ui), `op: \"<kind>\"` (throws-error: Error, TestField, FieldError, Get, …; writes-table: Insert, Modify, ModifyAll, Delete, DeleteAll, Rename) or `table: \"<table>\"` (throws-error: errors a record operation on that table raises); the answer is then about the matching sites, each with its message text, one route from the routine and up to 2 innermost enclosing conditions per site on the route (selected context, not all necessary conditions, with the count left out; a while condition is loop-entry, a repeat-until one continuation). `routes: N` (1-10, with a filter) lists up to N distinct routes per matching site, fewest hops first, each with its guards: a route is a simple path over (routine, known argument values) states, so recursion is followed only while it changes the known values; the answer says whether the enumeration completed (then every route is listed) or which cap fired (the routes asked, or 50,000 candidate edges per site), and the candidate edges it examined and rejected (by literal arguments, by returning to a state on the route), never a count of routes ruled out. Static candidates, not an observed trace. An error whose message is not static (a variable, an ErrorInfo, the platform's own) never matches and blocks a filtered no; an implicit error matches only by its kind word. Size: 1-5 KB human. Lines are 1-based. Dependency code: the whole call cone is followed with literal arguments. Next: on unknown, alsem_explain lists the workspace gaps behind it. Returns the exact `alsem prove` stdout.",
            schema(
                vec![
                    (
                        "routine",
                        json!({"type": "string", "description": format!("The routine: {SELECTOR}.")}),
                    ),
                    (
                        "question",
                        json!({"type": "string", "description": "Existing effects: may-commit | commits-on-success-path | writes-table:<name> | publishes-event:<name> | reaches-ui | throws-error. Callers' context: callers-have-pending-writes | callers-intercept-errors | callers-may-run-noninteractive"}),
                    ),
                    (
                        "assumeNoDependents",
                        json!({"type": "boolean", "description": "Callers' context questions: assume no app outside the program calls the workspace's public, protected or friend-visible procedures (it can turn unknown into no, and says so under assumptions). Not the same as closedWorld."}),
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
                    (
                        "match",
                        json!({"type": "string", "description": "throws-error only: keep the error sites whose static message text or Label/TextConst name contains this text (case-insensitive; %1 placeholders kept). An error whose message is not static never matches and blocks a no."}),
                    ),
                    (
                        "site",
                        json!({"type": "string", "description": "<file>:<line> (a source path or its tail, 1-based line): keep only the sites there. throws-error, writes-table, may-commit, reaches-ui."}),
                    ),
                    (
                        "op",
                        json!({"type": "string", "description": "throws-error or writes-table: keep only the sites of this operation (Error, TestField, FieldError, Get, …; Insert, Modify, ModifyAll, Delete, DeleteAll, Rename)."}),
                    ),
                    (
                        "table",
                        json!({"type": "string", "description": "throws-error only: keep only the errors a record operation on this table raises."}),
                    ),
                    (
                        "routes",
                        json!({"type": "integer", "minimum": 1, "maximum": 10, "description": "With match, site, op or table: up to this many distinct routes per matching site (default 1), fewest hops first, each with its guards; says whether the enumeration completed (every route listed) or which cap fired, and the candidate edges examined and rejected."}),
                    ),
                    ("deterministic", deterministic()),
                ],
                &["routine", "question"],
                true,
            ),
        ),
        tool(
            "alsem_digest",
            "The transitive effects (writes, commits, errors, events, UI, HTTP, files, background tasks) of a set of roots. The change modes: `files` (changed source files, relative to the workspace or the git top level) or `diff` (a unified diff's text; `diffRoot` is the folder its and the files' paths are relative to, default the git top level): the routines they change are the roots. `routines` (selectors) digests named routines: explain's effects with paths, without rules. Give at least one. Default (`format: json`): schema 5.0.0 as one compact line (no indentation), every effect row, paths as hop arrays (`[edge, line, to, file?]`, a shared tail as its `t:<n>` id) into a `payload.routines` table, one path per decision effect; up to about 600 KB for a routine that calls Base Application. It always goes to a file in the server's output folder (except a lookup failure, ambiguous or not found, whose whole answer fits the inline cap: that comes back inline); the answer carries its path, size, SHA-256 and a summary of at most 8 KiB (roots requested and resolved; per root: coverage, effects by type, tables it may write owned by the app and by dependencies, caller-dependent tables, commits, events, kind families, gaps by where, temporary-only tables, writes with no path kept, entry points; the first names of each list): read the summary first. Ask for more or less: `format: human` (30-35 KB for such a routine, grouped as explain groups it), `detail: true` (every path up to 3, scoped guarantee, diagnostic, unresolved call site and group witness), `maxPaths: 0..3`, `compact: false` (the indented JSON, about 60% larger), `includeReads: true` (DB_READ effects, one per table read site), `tests: true` (a testCoverage block: which roots a [Test] procedure reaches), `writesTables: [T]` (does it write T? per root, prove's own answer), `schema: 2` (the 2.x document: the old shape over the current analysis, for one release window; not with detail, maxPaths or compact). Lines are 1-based. Dependency code: the whole cone with literal arguments, grouped per entry point. Next: alsem_explain on a root for its rules and callers. Returns the exact `alsem digest` stdout.",
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
                        json!({"type": "boolean", "description": "Add a testCoverage block: which changed routines a [Test] procedure of the workspace app reaches. Sibling test apps are named, not loaded: alsem_tests selects from them."}),
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
                        json!({"type": "boolean", "description": "JSON without indentation: the default of the 5.0.0 file (the summary's `rowDetail` says so); `compact: false` writes it indented. Schemas 4, 3 and 2 stay indented unless `compact: true`."}),
                    ),
                    (
                        "schema",
                        json!({"type": "integer", "enum": [2, 3, 4, 5], "description": "The document's schema major (default 5: as 4, with `tableAnswers[].tableRef` and `asked` for the table, one `candidates` array for an ambiguous selector, and `dependencyCalls` / `beyondDependencyCalls` / `dependencyCall` / `throughDependencyCall` for the old entry-point keys). 4: each effect's intrinsic facts once in `payload.effects`, each root's references with its own liveness, temp state, paths and refutation evidence; for one release. 3: the 3.x document, for one release. 2: the 2.x document (old shape, current analysis), for one release window; not with detail, maxPaths or compact."}),
                    ),
                    ("deterministic", deterministic()),
                ],
                &[],
                true,
            ),
        ),
        tool(
            "alsem_query_effects",
            "Every record operation one routine (`routine`, a selector) performs transitively, each with its temp state and `via` provenance: reads, locks, validates, temporary records and caller-dependent writes included, each operation in exactly one bucket. Default (`format: human`): a summary line (writes that may persist, writes beyond dependency entry points, caller-dependent writes, locks, validate, reads, temporary records, on a RecordRef (table decided at run time), table not resolved), then writes per table with the first operation's location and first hop, then the writes beyond dependency entry points (alsem_explain's own section); about 20-25 KB for a routine that reaches Base Application. Ask for more: `all: true` (one line per operation, temporary records and reads included), `showSource: true` (the cited source line ±2 under each location), `format: json` (schema 5.0.0, as 4.0.0: every row with its `firstHop` and `origin.routine` as ids into `payload.routines`, `origin` holding the operation's own file, line and column, `via` only where it differs from `payload.defaults.via`, and a `summary`; a few hundred KB for such a routine, so a file with a summary of counts by operation and temp state; schema 5.0.0 names the dependency boundary `dependencyCalls` and adds `origin.fileDisplay`; `schema: 4` / `schema: 3` for the previous documents, one release). Lines are 1-based. Dependency code: per operation one routine deep (the operations of the first dependency routine entered); physical writes beyond each dependency entry point follow the whole cone with literal arguments, as alsem_explain does (`summary.beyondDependencyCalls` in JSON); operations the cone rules out under literal arguments are still listed here, each row marked as the cone judges its route (`literalRoute`: ruledOut and notInCone, no route runs; otherRoute, the index's first hop is not the cone's live route, which it names; coneRoute; absent, not judged), and the human table lines say so. Next: alsem_prove writes-table:<T> for one table's witness. Returns the exact `alsem query effects` stdout.",
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
                    (
                        "schema",
                        json!({"type": "integer", "enum": [3, 4, 5], "description": "JSON schema major (default 5: as 4, with `dependencyCalls` / `beyondDependencyCalls` / `dependencyCall` for the old entry-point keys and `origin.fileDisplay` naming a dependency file's app). 4: each routine once in `payload.routines`, rows refer to it by id; for one release. 3: the 3.x document, for one release."}),
                    ),
                    ("deterministic", deterministic()),
                ],
                &["routine"],
                true,
            ),
        ),
        tool(
            "alsem_query_touches",
            "Is a table touched by any record operation, transitively? `table`: a number, a name, an internal table id, or unknown (operations whose table is decided at run time). Without `from`: every workspace routine that touches it, by the per-operation index, and the table's own sites by family (record writes first with their temporary-record count, validations, locks, key-relevant uses, reads), each family with its count and a share of the samples; `opFamily: \"write\"` lists every record-write site. With `from` (a routine selector), `direction`: down (does it touch the table, and through which operations; when no indexed operation does, the answer is `unknown` with its reasons, such as operations on a RecordRef, calls not resolved or dependency code deeper than one routine, and `no` only when none applies), up (its transitive callers: how many touch it, do not, or are unknown; then, listed apart, the workspace callers that reach it only through dependency code, nearest first, each with its witness through every dependency routine and event, tier `configured` when only a configured-dispatch site such as Base Application's workflow responses connects it; potential callers, path feasibility not checked), or both (the default). Touches are counted on persistent records, on possibly persistent records and on temporary records (a temporary-record touch persists nothing but uses the table's schema and runs its triggers); when the routine touches the table itself its callers inherit the touch and are counted, not listed. With `field` (a field name or number of `table`): every code consumer of that one field instead, by use kind (read, write, validate, filter, key-use, load, calc, rule, metadata, copy-candidate for TransferFields, by-number for RecordRef.Field of a literal), each site once with its uses (`F += 1` is one site, read and write), temporary records counted with persistent ones, a record of an unresolved table as a candidate, the table's keys holding it, and what was searched: every workspace body and the dependency bodies loaded, never declaration-level consumers (CalcFormula, TableRelation, page and report columns); with no site the answer is \"no indexed consumer observed\", never no; with `from`, only the routine's cone (down) and its callers (up, by the reverse walk alsem_tests uses, each with its tier: local, through dependency code, only through an open site). Default (`format: human`): a few lines to about 10 KB, lists folded. Ask for more: `callers: true` (every caller once, by object, with its definition line and hop distance; the shared first touch once, a caller's own only when it differs; no witness paths), `all: true` (every witness, the callers' included, no fold; tens of megabytes on a hub routine), `showSource: true` (the cited source line ±2), `format: json` (every down witness, about 200 KB on a hub routine; over the cap a file with a summary of the down and up headlines; schema 5.0.0: routines in `payload.routines` and shared first touches in `payload.witnesses`, rows refer to them by id, the dependency boundary named `dependencyCall`; `schema: 4` / `schema: 3` for the previous documents, one release). Lines are 1-based. Dependency code: per operation one routine deep; with `from` asked down, the dependency entry points beyond which the whole cone may write the table are listed too (`down.beyond`). Next: alsem_query_effects for the routine's other tables. Returns the exact `alsem query touches` stdout.",
            schema(
                vec![
                    (
                        "table",
                        json!({"type": "string", "description": "A table number or name, an internal table id, or `unknown`."}),
                    ),
                    (
                        "field",
                        json!({"type": "string", "description": "A field of `table` (its name or number): the field's code consumers by use kind instead of the table's record operations (`--field`)."}),
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
                        json!({"type": "boolean", "description": "Name every transitive caller once, grouped by object, with its definition line and hop distance; the first touch they share prints once, and a caller's own first touch only when it differs from the shared one; no witness paths (`--callers`)."}),
                    ),
                    (
                        "opFamily",
                        json!({"type": "string", "enum": ["write"], "description": "Without `from` and `field` only: one operation family in full. `write` lists every record-write site (Insert, Modify, ModifyAll, Delete, DeleteAll, Rename), temporary records included and tagged with their temp state (`--op-family write`)."}),
                    ),
                    (
                        "showSource",
                        json!({"type": "boolean", "description": "Human output: the cited source line ±2 under each location (`--show-source`)."}),
                    ),
                    (
                        "format",
                        json!({"type": "string", "enum": ["human", "json"], "description": "Output format (default human)."}),
                    ),
                    (
                        "schema",
                        json!({"type": "integer", "enum": [3, 4, 5], "description": "JSON schema major (default 5: as 4, with `dependencyCalls` / `dependencyCall` for the old entry-point keys and `origin.fileDisplay`). 4: routines and shared witnesses in tables, rows refer to them by id; for one release. 3: the 3.x document, for one release."}),
                    ),
                    ("deterministic", deterministic()),
                ],
                &["table"],
                true,
            ),
        ),
        tool(
            "alsem_tests",
            "Which tests to run first for a change, and whether that is enough. Exactly one of `routines` (selectors), `files` (workspace-relative .al files: every routine in them) or `diff` (a unified diff's text; `diffRoot` is the folder its paths are relative to, default the git top level). The test scope comes first: the workspace's own [Test] procedures and the sibling test apps, from `testApps` (folders), else `testApps` in .al-sem.json (`[]` turns them off), else found under the git top level (at most 3 levels and 64 app.json files; a cap says discovery incomplete). Each test app is analyzed in a test universe of its own (the workspace app loaded into it as source; nothing is written into the test app), started on the first call; the production engine resolves the changed routines. Then each changed routine's status (reached, no-known-path, analysis-incomplete, not-loaded, not-in-projection), the tests to run first ranked by tier, hops and codeunit, grouped per codeunit with the first test's path, `selection complete: yes|no` and, when no, every reason the whole suite must run as well (ways to run no edge shows, calls from test code that resolve nowhere, a root not in a test universe), and a codeunit filter. Default (`format: human`): one line per test codeunit (the first 5), 2-6 KB. Ask for more: `tier: dependency` (also tests whose path passes dependency code) or `tier: all` (also paths over open sites), `maxTests: N` (at most N tests in runFirst), `format: json` (schema al-sem.tests/3: runFirst, each row with its selector and every changed routine it reaches (`reaches`: the changed routine's `key`, hops, relation `code` when any code path reaches it, `UI navigation` when every path opens a page, `candidate` when the page may be read-only at runtime), requiredFallback, selectionComplete, byTier, hidden by class, next; `schema: 2` for version 2, one release). Lines are 1-based. Dependency code: a test reaching the routine only through dependency code is counted in its own tier, never as local, and when those paths go through nodes that may execute most of the program the answer calls the set broad and says why. Static reachability, not coverage. Next: run the runFirst filter, then the whole suite when selection complete is no. Returns the exact `alsem tests` stdout.",
            schema(
                vec![
                    (
                        "routines",
                        string_array(&format!("Mode 1, the changed routines: {SELECTOR}.")),
                    ),
                    (
                        "files",
                        string_array(
                            "Mode 2, the changed .al files, workspace-relative: every routine in them.",
                        ),
                    ),
                    (
                        "diff",
                        json!({"type": "string", "description": "Mode 3, a unified diff's text: the routines its hunks touch."}),
                    ),
                    (
                        "diffRoot",
                        json!({"type": "string", "description": "With `diff`: the folder its paths are relative to (default: the git top level)."}),
                    ),
                    (
                        "testApps",
                        string_array(
                            "The sibling test apps' folders, instead of .al-sem.json's testApps and discovery.",
                        ),
                    ),
                    (
                        "tier",
                        json!({"type": "string", "enum": ["local", "dependency", "all"], "description": "Which tests runFirst lists: local (default), dependency (also through dependency code), all (also over open sites)."}),
                    ),
                    (
                        "maxTests",
                        json!({"type": "integer", "minimum": 0, "description": "List at most this many tests in runFirst; the counts stay whole."}),
                    ),
                    (
                        "format",
                        json!({"type": "string", "enum": ["human", "json"], "description": "Output format (default human)."}),
                    ),
                    (
                        "schema",
                        json!({"type": "integer", "enum": [1, 2, 3], "description": "JSON document version (default 3: `changed[]` entries keyed `c:1`, …; each runFirst row's `reaches` one object per reached changed routine with its hops, relation and candidate step; selectors on runFirst rows). 2: the version 2 document, for one release; 1: the version 1 document."}),
                    ),
                ],
                &[],
                true,
            ),
        ),
        tool(
            "alsem_impact",
            "Reviewing a diff or a PR: what the change touches, who it reaches, and with `base` how each changed routine's behavior differs from the base. Exactly one of `routines` (selectors), `files` (workspace-relative .al files: every routine in them), `diff` (a unified diff's text; `diffRoot` is the folder its paths are relative to, default the git top level) or `base` (a git ref: the working tree against the merge base of the ref and HEAD, committed, staged, unstaged and untracked .al files; git is only read); `base` with `diff` compares that diff against the merge base, whose files must match the diff's old lines. With `base` the base is built (that commit's files, analyzed under TODAY's packages, labelled so, package hashes recorded on both sides; first time about 40 s and 4 GB on a large app, then cached per base, at most 10 bases) in a one-shot child that takes a place of the engine children (it waits up to 60 s for an idle one, else the answer says why and keeps the rest), and each changed routine gets a before/after: writes by table, operation and class, commits, UI, errors, events, calls and the effects beyond calls, compared by meaning and never by line (a moved line is no change; a table swapped for another shows both); a loss is `no longer observed` (not removed) when the new side's analysis of that routine is incomplete; routines are paired by id, else by signature (a changed parameter list), else are added or deleted (deleted ones with their callers in the base); schema findings (fields, keys, enums) and contract findings as alsem diff finds them; a change to app.json dependencies marks dependency-reaching changes unreliable. `delta: false` skips the base build (current effects only). The input accounting comes first: the comparison tuple (requested ref, resolved commit, merge base, comparison base, HEAD and the revision's inputs stamp, the diff's hash), then every changed line in one bucket (routine body or signature, object declarations by kind: fields, keys, global vars, properties, actions, layout …, outside objects, removed routine text, unmapped with the reason), the hunks outside routines, files not mapped, diff lines that do not match the working tree. Then per changed routine, ranked reached-from-a-user-or-job first, then by ancestors: its current effects (writes, commits, UI, errors, events), who reaches it (direct code callers with their call site; the ancestors as one population of code calls, UI navigation, configuration and hubs, with no relation recorded, by where their paths go and the share of the program's declared entries they hold; entries by kind; event publishers; external callers in JSON only) and its tests (tests reaching it, the nearest; a routine no test universe has gets its nearest tested caller), then the tests line (selection complete and why not, a codeunit filter). Without `base`: current effects only, nothing called added or deleted. Default (`format: human`): the first 20 routines, a few lines each with the first item of each list; 2-5 KB for one routine, about 35 KB for a 94-file change with 699 routines; over the cap a file with a summary (the accounting, the first five routines, the tests line). Ask for more: `maxRoots: N` (routines shown), `detail: true` (an evidence budget per routine: its direct callers and event publishers with call sites, and the first entry of each kind, each on its own line; every hunk and diagnostic), `format: json` (schema al-sem.impact/1, every routine with `up`, `effects`, `tests` and its `delta` (null without a base), the base block, `deleted` and `schemaChanges`; megabytes for a large change, so a file). `tests: false` builds no test universe; `testApps` names the sibling test apps (as alsem_tests); `assumeNoDependents: true` reports no external callers. Lines are 1-based. Dependency code: callers and entries follow the whole program (dependency code included, counted in its own tier); effects follow every resolved call with literal arguments, as alsem_digest does. Next: alsem_explain on a top-ranked routine; alsem_tests for the full selection. Returns the exact `alsem impact` stdout.",
            schema(
                vec![
                    (
                        "routines",
                        string_array(&format!("Mode 1, the changed routines: {SELECTOR}.")),
                    ),
                    (
                        "files",
                        string_array(
                            "Mode 2, the changed .al files, workspace-relative: every routine in them.",
                        ),
                    ),
                    (
                        "diff",
                        json!({"type": "string", "description": "Mode 3, a unified diff's text: every changed line accounted."}),
                    ),
                    (
                        "diffRoot",
                        json!({"type": "string", "description": "With `diff`: the folder its paths are relative to (default: the git top level)."}),
                    ),
                    (
                        "base",
                        json!({"type": "string", "description": "Mode 4, a git ref: the working tree against the merge base of the ref and HEAD (committed, staged, unstaged, untracked .al), with a before/after per changed routine. With `diff`: that diff against the merge base."}),
                    ),
                    (
                        "delta",
                        json!({"type": "boolean", "description": "With `base`: build the base and print the before/after (default true); false gives current effects only."}),
                    ),
                    (
                        "testApps",
                        string_array(
                            "The sibling test apps' folders, instead of .al-sem.json's testApps and discovery.",
                        ),
                    ),
                    (
                        "tests",
                        json!({"type": "boolean", "description": "Compute the tests (default true; false builds no test universe)."}),
                    ),
                    (
                        "maxRoots",
                        json!({"type": "integer", "minimum": 0, "description": "Routines shown in the human output (default 20, fewer when the answer would pass 38 KiB; given, that budget is off); the JSON holds every one."}),
                    ),
                    (
                        "detail",
                        json!({"type": "boolean", "description": "Per routine its direct callers and event publishers with call sites and the first entry of each kind; every hunk outside routines and every diagnostic."}),
                    ),
                    (
                        "assumeNoDependents",
                        json!({"type": "boolean", "description": "No app outside the program depends on these apps: report no external callers."}),
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
            "alsem_status",
            "Is the engine current with the files on disk, and what is it doing? Read `headline` first: per engine, FRESH (no input changed since the revision that answers), STALE (what changed, the refresh class, what to do next), not checked (no engine yet, and why), or the check did not finish. It checks the disk now, against the inputs of the last published revision, without waiting for a busy engine and without building anything. Per engine: `check` (`checkedRevision` with `generation` and `inputsStamp`, the same `revision` every answer's envelope carries; `fresh`; `complete` and `incompleteReasons`; `changes`, each input with its `category` (al-file, al-walk, app-json, app-json-paths, roots-config, coverage-config, source-dependency, package, package-list, unreadable-package-folder) and `path`; `refreshClass`: Package, Declaration, BodyOnly, or BodyOnly|Declaration when only .al contents changed (known after parsing them)); `refresh` (the last alsem_refresh: idle, queued, running, completed or failed, with what it did); `details` (the last build, `activity`, the child's memory, pid and counters, its last failure and stderr tail). The default check trusts a file's size and modification time; `verify: true` hashes every input. Starts nothing; answers at once, even while a call runs.",
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
                    (
                        "verify",
                        json!({"type": "boolean", "description": "Hash every input instead of trusting size and modification time (slower; default false)."}),
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
            verify: a.bool("verify")?.unwrap_or(false),
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
            v.switch("callers-context", a.flag("callersContext")?);
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
            v.switch("no-dependents", a.flag("assumeNoDependents")?);
            v.switch("detail", a.flag("detail")?);
            // Agent-tasks E1: the filters; which question each goes with is
            // the CLI's own check (one error text).
            for (key, flag) in [
                ("match", "match"),
                ("site", "site"),
                ("op", "op"),
                ("table", "table"),
            ] {
                if let Some(x) = a.string(key)? {
                    v.opt(flag, x);
                }
            }
            // Agent-tasks E2: `--routes` only when set; its range and that it
            // needs a filter are the CLI's own checks.
            if let Some(n) = a.count("routes")? {
                v.opt("routes", &n.to_string());
            }
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
            // Round-2 G35: compact is the 5.0.0 file's default; `compact:
            // false` asks for the indented document.
            match a.bool("compact")? {
                Some(true) => v.switch("compact", true),
                Some(false) => v.opt("compact", "false"),
                None => {}
            }
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
            if let Some(n) = a.count("schema")? {
                v.opt("schema", &n.to_string());
            }
            v.switch("deterministic", a.flag("deterministic")?);
            v
        }
        "alsem_query_touches" => {
            let mut v = Argv::new(&["query", "touches"], &workspace);
            v.opt("table", a.required("table")?);
            if let Some(field) = a.string("field")? {
                v.opt("field", field);
            }
            if let Some(from) = a.string("from")? {
                v.opt("from", from);
            }
            v.opt(
                "direction",
                &a.enumerated("direction", &["down", "up", "both"], "both")?,
            );
            v.switch("all", a.flag("all")?);
            v.switch("callers", a.flag("callers")?);
            if let Some(family) = a.string("opFamily")? {
                v.opt("op-family", family);
            }
            v.switch("show-source", a.flag("showSource")?);
            v.opt(
                "format",
                &a.enumerated("format", &["human", "json"], "human")?,
            );
            if let Some(n) = a.count("schema")? {
                v.opt("schema", &n.to_string());
            }
            v.switch("deterministic", a.flag("deterministic")?);
            v
        }
        "alsem_tests" => {
            let mut v = Argv::new(&["tests"], &workspace);
            let routines = a.strings("routines")?;
            let files = a.strings("files")?;
            let diff = a.string("diff")?;
            let modes = [routines.is_some(), files.is_some(), diff.is_some()];
            if modes.iter().filter(|m| **m).count() != 1 {
                return Err(
                    "alsem_tests takes exactly one of 'routines', 'files' or 'diff'".into(),
                );
            }
            if a.string("diffRoot")?.is_some() && diff.is_none() {
                return Err("'diffRoot' goes with 'diff'".into());
            }
            let mut req = crate::engine::test_select::run::TestsRequest {
                workspace: workspace.clone(),
                single_app: universe == Universe::SingleApp,
                ..Default::default()
            };
            for r in routines.unwrap_or_default() {
                v.opt("routine", r);
                req.routines.push(r.to_string());
            }
            for f in files.unwrap_or_default() {
                v.opt("file", f);
                req.files.push(f.to_string());
            }
            if let Some(d) = diff {
                v.opt("diff", "-");
                req.diff = Some("-".into());
                diff_text = Some(d.to_string());
                if let Some(root) = a.string("diffRoot")? {
                    v.opt("diff-root", root);
                    req.diff_root = Some(root.to_string());
                }
            }
            for t in a.strings("testApps")?.unwrap_or_default() {
                let t = absolute(t);
                v.opt("test-app", &t);
                req.test_apps.push(t);
            }
            let tier = a.enumerated("tier", &["local", "dependency", "all"], "local")?;
            v.opt("tier", &tier);
            let max_tests = a.count("maxTests")?;
            if let Some(n) = max_tests {
                v.opt("max-tests", &n.to_string());
            }
            let format = a.enumerated("format", &["human", "json"], "human")?;
            v.opt("format", &format);
            v.switch("single-app", universe == Universe::SingleApp);
            // Output validation r3 T1: `schema: 2` keeps the version 2
            // document for one release (`schema: 1` the version 1 one).
            let schema = match a.count("schema")? {
                None => 3,
                Some(n @ (1..=3)) => {
                    v.opt("schema", &n.to_string());
                    n as u32
                }
                Some(n) => {
                    return Err(format!(
                        "alsem_tests: unknown schema {n}. Expected: 1, 2 or 3"
                    ));
                }
            };
            // §6.1: the MCP front end's answers are worded for it; the CLI's
            // argv carries the same `--surface=mcp`, so the bytes are equal.
            v.opt("surface", "mcp");
            let opts = crate::engine::test_select::render::RenderOpts {
                tier: crate::engine::test_select::render::TierPick::parse(&tier)?,
                max_tests: max_tests.map(|n| n as usize),
                json: format == "json",
                surface: crate::engine::follow_up::Surface::Mcp,
                single_app: universe == Universe::SingleApp,
                schema,
            };
            return Ok(ToolCall::Tests(TestsCall {
                command: CommandCall {
                    tool,
                    workspace,
                    universe,
                    freshness,
                    argv: v.done(),
                    diff_text,
                    out,
                    max_inline_bytes,
                    always_file,
                    client_budget_chars,
                },
                request: req,
                opts,
            }));
        }
        "alsem_impact" => {
            let mut v = Argv::new(&["impact"], &workspace);
            let routines = a.strings("routines")?;
            let files = a.strings("files")?;
            let diff = a.string("diff")?;
            let base = a.string("base")?;
            // `base` with `diff` is one mode: the diff against that base (B2).
            let modes = [
                routines.is_some(),
                files.is_some(),
                diff.is_some() || base.is_some(),
            ];
            if modes.iter().filter(|m| **m).count() != 1 {
                return Err(
                    "alsem_impact takes exactly one of 'routines', 'files', 'diff' or 'base' ('base' with 'diff' compares the diff against that base)"
                        .into(),
                );
            }
            if a.string("diffRoot")?.is_some() && diff.is_none() {
                return Err("'diffRoot' goes with 'diff'".into());
            }
            let delta = match a.map.get("delta") {
                None | Some(Value::Null) => base.is_some(),
                Some(Value::Bool(b)) if base.is_some() => *b,
                Some(Value::Bool(_)) => return Err("'delta' goes with 'base'".into()),
                Some(_) => return Err("'delta' must be a boolean".into()),
            };
            if !delta && base.is_some() && diff.is_some() {
                return Err(
                    "'base' with 'diff' is a before/after; 'delta: false' drops it (give 'diff' alone)"
                        .into(),
                );
            }
            let mut req = crate::engine::impact::run::ImpactRequest {
                workspace: workspace.clone(),
                single_app: universe == Universe::SingleApp,
                tests: match a.map.get("tests") {
                    None | Some(Value::Null) => true,
                    Some(Value::Bool(b)) => *b,
                    Some(_) => return Err("'tests' must be a boolean".into()),
                },
                assume_no_dependents: a.flag("assumeNoDependents")?,
                detail: a.flag("detail")?,
                ..Default::default()
            };
            for r in routines.unwrap_or_default() {
                v.opt("routine", r);
                req.routines.push(r.to_string());
            }
            for f in files.unwrap_or_default() {
                v.opt("file", f);
                req.files.push(f.to_string());
            }
            if let Some(d) = diff {
                v.opt("diff", "-");
                req.diff = true;
                diff_text = Some(d.to_string());
                if let Some(root) = a.string("diffRoot")? {
                    v.opt("diff-root", root);
                    req.diff_root = Some(root.to_string());
                }
            }
            if let Some(b) = base {
                v.opt("base", b);
                req.base = Some(b.to_string());
            }
            req.delta = delta;
            v.switch("no-delta", base.is_some() && !delta);
            for t in a.strings("testApps")?.unwrap_or_default() {
                let t = absolute(t);
                v.opt("test-app", &t);
                req.test_apps.push(t);
            }
            v.switch("no-tests", !req.tests);
            let max_roots = a.count("maxRoots")?;
            if let Some(n) = max_roots {
                v.opt("max-roots", &n.to_string());
            }
            v.switch("detail", req.detail);
            v.switch("no-dependents", req.assume_no_dependents);
            let format = a.enumerated("format", &["human", "json"], "human")?;
            v.opt("format", &format);
            v.switch("single-app", universe == Universe::SingleApp);
            // §6.1, as for every command tool.
            v.opt("surface", "mcp");
            let opts = crate::engine::impact::render::RenderOpts::new(
                format == "json",
                max_roots.map(|n| n as usize),
                req.detail,
            )
            .with_surface(crate::engine::follow_up::Surface::Mcp)
            .with_single_app(universe == Universe::SingleApp);
            return Ok(ToolCall::Impact(ImpactCall {
                command: CommandCall {
                    tool,
                    workspace,
                    universe,
                    freshness,
                    argv: v.done(),
                    diff_text,
                    out,
                    max_inline_bytes,
                    always_file,
                    client_budget_chars,
                },
                request: req,
                opts,
            }));
        }
        _ => unreachable!("every tool is listed"),
    };
    argv.switch("single-app", universe == Universe::SingleApp);
    // Output validation §6.1: the answer names MCP inputs and complete
    // calls, not CLI flags; the CLI takes the same hidden flag, so the MCP
    // answer is still the CLI's stdout for this argv, byte for byte.
    argv.opt("surface", "mcp");
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

    /// Agent-tasks §5 (I-1): no tool description, argument description or
    /// server instruction calls an answer "safe": prove answers about
    /// existing effects or callers' context, never a safety verdict.
    #[test]
    fn no_description_says_safe() {
        fn texts(v: &Value, out: &mut Vec<String>) {
            match v {
                Value::String(s) => out.push(s.clone()),
                Value::Array(a) => a.iter().for_each(|x| texts(x, out)),
                Value::Object(o) => o.values().for_each(|x| texts(x, out)),
                _ => {}
            }
        }
        let mut all = vec![crate::mcp::INSTRUCTIONS.to_string()];
        for t in tool_list() {
            texts(&t, &mut all);
        }
        let hits: Vec<&String> = all
            .iter()
            .filter(|t| t.to_ascii_lowercase().contains("safe"))
            .collect();
        assert!(hits.is_empty(), "{hits:#?}");
        let prove = tool_list()
            .into_iter()
            .find(|t| t["name"] == "alsem_prove")
            .unwrap();
        let d = prove["description"].as_str().unwrap();
        assert!(
            d.contains("existing effects") && d.contains("callers' context"),
            "{d}"
        );
    }

    /// `assumeNoDependents` is prove's `--no-dependents`.
    #[test]
    fn prove_assume_no_dependents_is_its_own_switch() {
        let c = command(
            "alsem_prove",
            json!({"routine": "R", "question": "callers-intercept-errors", "assumeNoDependents": true}),
        )
        .unwrap();
        assert!(
            c.argv.iter().any(|a| a == "--no-dependents"),
            "{:?}",
            c.argv
        );
        assert!(
            !c.argv.iter().any(|a| a == "--closed-world"),
            "{:?}",
            c.argv
        );
    }

    /// Agent-tasks E1: prove's filters become the CLI's `--match`, `--site`,
    /// `--op` and `--table`, value and all; which question each goes with is
    /// the CLI's own check.
    #[test]
    fn prove_filters_become_cli_flags() {
        let c = command(
            "alsem_prove",
            json!({"routine": "R", "question": "throws-error", "match": "-already issued", "site": "a.al:3", "op": "Error", "table": "T", "routes": 3}),
        )
        .unwrap();
        for want in [
            "--match=-already issued",
            "--site=a.al:3",
            "--op=Error",
            "--table=T",
            "--routes=3",
        ] {
            assert!(c.argv.iter().any(|a| a == want), "{want}: {:?}", c.argv);
        }
        let c = command(
            "alsem_prove",
            json!({"routine": "R", "question": "throws-error"}),
        )
        .unwrap();
        assert!(
            !c.argv
                .iter()
                .any(|a| ["--match", "--site", "--op", "--table", "--routes"]
                    .iter()
                    .any(|f| a.starts_with(f))),
            "{:?}",
            c.argv
        );
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
                "--surface=mcp",
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
                "--surface=mcp",
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
        // MCP output plan 1c: human by default for an agent; 40 KiB inline
        // (48 KiB from V3 until V5), and (T-W2-23) the client's character
        // budget.
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
                "--surface=mcp",
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
                "--surface=mcp",
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
                "--surface=mcp",
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
                "--surface=mcp",
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
