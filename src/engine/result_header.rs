//! One result header on every tool (output validation spec 2026-11-06 §5.2,
//! F30, F31, F32, F76): the same four lines lead every human answer, and the
//! same facts are a structured `result.scope` in every JSON document.
//!
//! ```text
//! <tool> <resolved inputs, canonical names>   [universe]
//! answer: <the one-line result>
//! analysis complete · assumptions: <full labels> · limits: <none | kinds with counts> · scope: <one clause>
//! more: <inputs that show more, in the surface's words>
//! ```
//!
//! The third line is the only place analysis, assumptions, limits and scope
//! print. A JSON document never gains a text preamble; it carries
//! [`Header::scope_json`] instead, readable without the MCP envelope.
//!
//! The revision generation is not printed: a one-shot CLI run has none, and
//! the MCP answer must equal the CLI's stdout byte for byte. The envelope's
//! `revision` carries it.

use serde_json::{Map, Value, json};

/// The §8.1 completeness of an analysis, always with its subject.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Analysis {
    Complete,
    /// `analysis partial (<reasons>)`.
    Partial(Vec<String>),
    /// `not analyzed (<reason>)`.
    NotAnalyzed(String),
}

impl Analysis {
    /// `Complete` when `complete`, else `Partial(reasons)`.
    #[must_use]
    pub fn of(complete: bool, reasons: Vec<String>) -> Analysis {
        if complete {
            Analysis::Complete
        } else {
            Analysis::Partial(reasons)
        }
    }

    /// The human words.
    #[must_use]
    pub fn human(&self) -> String {
        match self {
            Analysis::Complete => "analysis complete".to_string(),
            Analysis::Partial(r) if r.is_empty() => "analysis partial".to_string(),
            Analysis::Partial(r) => format!("analysis partial ({})", r.join("; ")),
            Analysis::NotAnalyzed(why) => format!("not analyzed ({why})"),
        }
    }

    /// The JSON key (`complete` / `partial` / `notAnalyzed`).
    #[must_use]
    pub fn key(&self) -> &'static str {
        match self {
            Analysis::Complete => "complete",
            Analysis::Partial(_) => "partial",
            Analysis::NotAnalyzed(_) => "notAnalyzed",
        }
    }

    /// The reasons (none for a complete analysis).
    #[must_use]
    pub fn reasons(&self) -> Vec<String> {
        match self {
            Analysis::Complete => Vec::new(),
            Analysis::Partial(r) => r.clone(),
            Analysis::NotAnalyzed(why) => vec![why.clone()],
        }
    }

    /// The weaker of two (`NotAnalyzed` > `Partial` > `Complete`), reasons
    /// joined without repeats.
    #[must_use]
    pub fn and(self, other: Analysis) -> Analysis {
        match (self, other) {
            (Analysis::NotAnalyzed(a), _) | (_, Analysis::NotAnalyzed(a)) => {
                Analysis::NotAnalyzed(a)
            }
            (Analysis::Partial(mut a), Analysis::Partial(b)) => {
                for r in b {
                    if !a.contains(&r) {
                        a.push(r);
                    }
                }
                Analysis::Partial(a)
            }
            (Analysis::Partial(a), Analysis::Complete)
            | (Analysis::Complete, Analysis::Partial(a)) => Analysis::Partial(a),
            (Analysis::Complete, Analysis::Complete) => Analysis::Complete,
        }
    }

    /// The completeness the reported families state (`result.scope.families`,
    /// each `{…, blockers: {callNotResolved, interfaceDispatchOpenWorld}}`):
    /// `Partial` when any family has a call not resolved or an open-world
    /// interface dispatch, with one reason per distinct blocker set naming
    /// its families; else `Complete`. Round-2 astra M1: query effects said
    /// `analysis: complete` beside `beyondDependencyCalls.blockers
    /// {callNotResolved: 21, interfaceDispatchOpenWorld: 267}`.
    #[must_use]
    pub fn of_families(families: &Value) -> Analysis {
        let n = |b: &Value, k: &str| b.get(k).and_then(Value::as_u64).unwrap_or(0);
        let mut groups: Vec<((u64, u64), Vec<&str>)> = Vec::new();
        for (name, fam) in families.as_object().into_iter().flatten() {
            let b = &fam["blockers"];
            let key = (n(b, "callNotResolved"), n(b, "interfaceDispatchOpenWorld"));
            if key == (0, 0) {
                continue;
            }
            match groups.iter_mut().find(|(k, _)| *k == key) {
                Some((_, names)) => names.push(name),
                None => groups.push((key, vec![name])),
            }
        }
        if groups.is_empty() {
            return Analysis::Complete;
        }
        Analysis::Partial(
            groups
                .into_iter()
                .map(|((calls, open), names)| {
                    let mut what = Vec::new();
                    if calls > 0 {
                        what.push(format!(
                            "{calls} {} not resolved",
                            if calls == 1 { "call" } else { "calls" }
                        ));
                    }
                    if open > 0 {
                        what.push(format!(
                            "{open} interface {} open world",
                            if open == 1 { "dispatch" } else { "dispatches" }
                        ));
                    }
                    format!("{}: {}", names.join(", "), what.join(", "))
                })
                .collect(),
        )
    }
}

/// The default assumption of every whole-program answer (§5.3: the full
/// sentence, "loaded program" alone hides the exclusion).
pub const LOADED_PROGRAM: &str =
    "loaded program; unloaded implementations and subscribers are not enumerated";

/// The four header lines of one answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    /// `explain`, `prove`, `digest`, `query effects`, `query touches`,
    /// `tests`, `impact`.
    pub tool: &'static str,
    /// The resolved inputs in canonical names (or, with no answer, the input
    /// as given).
    pub inputs: String,
    /// The call was single-app (`[single-app]`), else `[cross-app]`.
    pub single_app: bool,
    /// The one-line result.
    pub answer: String,
    pub analysis: Analysis,
    /// The component the analysis words qualify, when the answer has
    /// several (r3 §5.2, c06: `analysis complete (the routine's own cone)`
    /// above a callers' context block); `None`: the whole answer.
    pub analysis_subject: Option<&'static str>,
    /// The full assumption labels.
    pub assumptions: Vec<String>,
    /// The caps that cut this answer, each with its count; empty: `none`.
    pub limits: Vec<String>,
    /// One clause: what the answer covers that limits **this** answer
    /// (dependency depth, a family not reported, the dependencies count).
    pub scope: String,
    /// The inputs that show more, in the surface's words.
    pub more: Vec<String>,
}

impl Header {
    /// A header with the defaults: complete analysis, the loaded-program
    /// assumption, no limit.
    #[must_use]
    pub fn new(tool: &'static str, inputs: String, single_app: bool) -> Header {
        Header {
            tool,
            inputs,
            single_app,
            answer: String::new(),
            analysis: Analysis::Complete,
            analysis_subject: None,
            assumptions: vec![LOADED_PROGRAM.to_string()],
            limits: Vec::new(),
            scope: String::new(),
            more: Vec::new(),
        }
    }

    /// `cross-app` or `single-app`.
    #[must_use]
    pub fn universe(&self) -> &'static str {
        if self.single_app {
            "single-app"
        } else {
            "cross-app"
        }
    }

    /// The four lines, each ending in `\n`.
    #[must_use]
    pub fn human(&self) -> String {
        let limits = if self.limits.is_empty() {
            "none".to_string()
        } else {
            self.limits.join("; ")
        };
        let analysis = match (self.analysis_subject, &self.analysis) {
            (None, a) => a.human(),
            (Some(s), Analysis::Complete) => format!("analysis complete ({s})"),
            (Some(s), Analysis::Partial(r)) if r.is_empty() => format!("analysis partial ({s})"),
            (Some(s), Analysis::Partial(r)) => format!("analysis partial ({s}: {})", r.join("; ")),
            (Some(_), a) => a.human(),
        };
        let mut third = format!(
            "{analysis} · assumptions: {} · limits: {limits}",
            if self.assumptions.is_empty() {
                "none".to_string()
            } else {
                self.assumptions.join("; ")
            }
        );
        if !self.scope.is_empty() {
            third.push_str(" · scope: ");
            third.push_str(&self.scope);
        }
        let more = if self.more.is_empty() {
            "nothing more to show".to_string()
        } else {
            self.more.join("; ")
        };
        format!(
            "{} {}   [{}]\nanswer: {}\n{third}\nmore: {more}\n",
            self.tool,
            self.inputs,
            self.universe(),
            self.answer
        )
    }

    /// A lookup miss's header (round-2 output validation G40): the inputs
    /// line and the one failure sentence on the `answer:` line; no
    /// analysis, assumptions, limits or scope (nothing was analyzed) and no
    /// `more:` (there is nothing more to show). The lines that help the next
    /// call follow it.
    #[must_use]
    pub fn miss_human(&self) -> String {
        format!(
            "{} {}   [{}]\nanswer: {}\n",
            self.tool,
            self.inputs,
            self.universe(),
            self.answer
        )
    }

    /// `result.scope` (F76): the same facts as structured fields. `extra`
    /// adds the tool's own (`families`, `selection`).
    #[must_use]
    pub fn scope_json(&self, extra: Map<String, Value>) -> Value {
        let mut m = Map::new();
        // A tool that reports `families` cannot be more complete than they
        // say (round-2 astra M1): their blockers count here, for every tool.
        let analysis = match extra.get("families") {
            Some(f) => self.analysis.clone().and(Analysis::of_families(f)),
            None => self.analysis.clone(),
        };
        m.insert("analysis".into(), json!(analysis.key()));
        m.insert("reasons".into(), json!(analysis.reasons()));
        m.insert("assumptions".into(), json!(self.assumptions));
        m.insert("limits".into(), json!(self.limits));
        m.insert("universe".into(), json!(self.universe()));
        m.insert("scope".into(), json!(self.scope));
        for (k, v) in extra {
            m.insert(k, v);
        }
        Value::Object(m)
    }
}

/// Output validation §9.8 (F78): every JSON location object (one with
/// `file` and `line`) says what it is — `role: definition` (a routine's or
/// object member's declaration), `callSite` (a call), `operation` (an
/// operation's own place). Decided by where the location sits: under
/// `anchor` it is an operation when its holder is an effect or operation
/// row (`op`, `opKinds`, `effectType`, `operationId`), a call site when it
/// is a blocker's (`kind` with `detail`), a definition when its holder is a
/// routine or object reference (`stableId`, `display`, `name`), else (a
/// rule, a check) an operation;
/// under `callsite` / `callSite` / `callsiteAnchor` / `site` / `enteredAt`
/// a call site; under `origin` / `evidence` an operation. Other locations
/// are left as they are.
pub fn tag_location_roles(v: &mut Value) {
    fn is_location(m: &Map<String, Value>) -> bool {
        m.contains_key("file") && m.contains_key("line")
    }
    fn walk(v: &mut Value) {
        match v {
            Value::Object(m) => {
                let operation_row = ["op", "opKinds", "effectType", "operationId"]
                    .iter()
                    .any(|k| m.contains_key(*k));
                // A blocker (`kind` + `detail`) is anchored at its call site;
                // a routine or object reference (`stableId`, `display`,
                // `name`) at its declaration; anything else (a rule, a
                // check) at the statement it describes.
                let blocker = m.contains_key("kind") && m.contains_key("detail");
                let reference = ["stableId", "display", "name", "qualifiedDisplay"]
                    .iter()
                    .any(|k| m.contains_key(*k));
                for (k, x) in m.iter_mut() {
                    let role = match k.as_str() {
                        "anchor" if operation_row => Some("operation"),
                        "anchor" if blocker => Some("callSite"),
                        "anchor" if reference => Some("definition"),
                        "anchor" => Some("operation"),
                        "callsite" | "callSite" | "callsiteAnchor" | "site" | "enteredAt" => {
                            Some("callSite")
                        }
                        "origin" | "evidence" => Some("operation"),
                        _ => None,
                    };
                    if let (Some(r), Value::Object(loc)) = (role, &mut *x)
                        && is_location(loc)
                    {
                        loc.insert("role".into(), Value::String(r.into()));
                    }
                    walk(x);
                }
            }
            Value::Array(a) => a.iter_mut().for_each(walk),
            _ => {}
        }
    }
    walk(v);
}

/// What a CLI answer's exit code means (output validation §6.3, F36): the
/// envelope's `outcome`. `answered` and `no-routine-changed` exit 0;
/// `input-not-mapped`, `not-found`, `ambiguous` and `usage-error` exit 2
/// (no answer). Read off the answer's own words: the header's `answer:`
/// line (`no answer — …`, impact's `(outcome: …)`), the JSON's resolution
/// words, or clap's error on stderr with no stdout.
#[must_use]
pub fn outcome(exit: u8, stdout: Option<&str>, stderr: &[String]) -> &'static str {
    let text = stdout.unwrap_or("");
    let answer = text
        .lines()
        .find(|l| l.starts_with("answer: "))
        .unwrap_or("");
    match exit {
        0 if answer.contains("(outcome: no-routine-changed)") => "no-routine-changed",
        0 => "answered",
        2 if answer.contains("(outcome: input-not-mapped)") => "input-not-mapped",
        2 if text.trim().is_empty() && stdout.is_some() => "usage-error",
        2 if stdout.is_none() && stderr.iter().any(|l| l.starts_with("error:")) => "usage-error",
        2 => {
            let lower = text.to_ascii_lowercase();
            if lower.contains("ambiguous") || answer.contains("names several routines") {
                "ambiguous"
            } else if lower.contains("input-not-mapped") {
                "input-not-mapped"
            } else {
                "not-found"
            }
        }
        _ => "usage-error",
    }
}

/// The §6.3 outcome of a no-answer from its structured resolution
/// diagnostics' `kind`s (round-2 n08, c26: the outcome came from the
/// rendered words, and a filed answer had none): `ambiguous` when any input
/// names several things (`selector-ambiguous`, `routine-ambiguous`,
/// `table-ambiguous`, …), else `not-found` when one names nothing
/// (`…-not-found`, `…-unmatched`, `unresolved`), else `None`. Each tool puts
/// it in its summary as `outcome`; the MCP envelope takes it from there.
#[must_use]
pub fn outcome_of_kinds<'a>(kinds: impl IntoIterator<Item = &'a str>) -> Option<&'static str> {
    let mut found = None;
    for k in kinds {
        if k.ends_with("ambiguous") {
            return Some("ambiguous");
        }
        if k.ends_with("not-found") || k.ends_with("unmatched") || k == "unresolved" {
            found = Some("not-found");
        }
    }
    found
}

/// What a CLI answer's exit code means (§6.3), from the tool's structured
/// `summary.outcome` when it states one (round-2 n08: an ambiguous selector
/// is `ambiguous` because its diagnostic says so, whether the answer was
/// inlined or filed), else from the answer's words ([`outcome`]).
#[must_use]
pub fn outcome_of_answer(
    exit: u8,
    summary: Option<&Value>,
    stdout: Option<&str>,
    stderr: &[String],
) -> &'static str {
    let stated = summary
        .and_then(|s| s.get("outcome"))
        .and_then(Value::as_str);
    match (exit, stated) {
        (2, Some("ambiguous")) => "ambiguous",
        (2, Some("not-found")) => "not-found",
        (2, Some("input-not-mapped")) => "input-not-mapped",
        (0, Some("no-routine-changed")) => "no-routine-changed",
        _ => outcome(exit, stdout, stderr),
    }
}

/// A document's `volatile` (round-2 output validation G44, beside
/// `deterministic`): the envelope fields whose value changes from one run to
/// the next on the same input. `["generatedAt"]` unless `--deterministic`
/// pins it; `[]` then.
#[must_use]
pub fn volatile(deterministic: bool) -> Value {
    if deterministic {
        json!([])
    } else {
        json!(["generatedAt"])
    }
}

/// `dependencies: N apps` (§5.3, F32): the one form of the dependencies line;
/// the full list only with `all`/`detail`, and in the envelope's `universe`.
#[must_use]
pub fn dependencies_clause(n: usize) -> String {
    format!("dependencies: {n} {}", if n == 1 { "app" } else { "apps" })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §9.8 (F78): a routine's anchor is a definition, an effect row's an
    /// operation, a call site a call site.
    #[test]
    fn locations_say_what_they_are() {
        let mut v = serde_json::json!({
            "routine": {"stableId": "x", "anchor": {"file": "a", "line": 1}},
            "writes": [{"op": "Modify", "anchor": {"file": "a", "line": 2}}],
            "hop": {"callsite": {"file": "a", "line": 3}},
            "notALocation": {"anchor": "text"},
        });
        tag_location_roles(&mut v);
        assert_eq!(v["routine"]["anchor"]["role"], "definition");
        assert_eq!(v["writes"][0]["anchor"]["role"], "operation");
        assert_eq!(v["hop"]["callsite"]["role"], "callSite");
        assert_eq!(v["notALocation"]["anchor"], "text");
    }

    /// §6.3: each exit-code meaning from the answer's own words.
    #[test]
    fn outcomes_follow_the_answer_words() {
        let h =
            |answer: &str| format!("tool x   [cross-app]\nanswer: {answer}\nanalysis complete\n");
        assert_eq!(outcome(0, Some(&h("yes")), &[]), "answered");
        assert_eq!(
            outcome(
                0,
                Some(&h("no routine changed — … (outcome: no-routine-changed)")),
                &[]
            ),
            "no-routine-changed"
        );
        assert_eq!(
            outcome(
                2,
                Some(&h("no answer — … (outcome: input-not-mapped)")),
                &[]
            ),
            "input-not-mapped"
        );
        assert_eq!(
            outcome(
                2,
                Some(&h("no answer — the selector names no routine")),
                &[]
            ),
            "not-found"
        );
        assert_eq!(
            outcome(
                2,
                Some(&h("no answer — the selector names several routines")),
                &[]
            ),
            "ambiguous"
        );
        assert_eq!(
            outcome(2, Some(""), &["error: unexpected argument".into()]),
            "usage-error"
        );
        assert_eq!(outcome(1, Some(""), &[]), "usage-error");
    }

    #[test]
    fn four_lines_with_the_completeness_subject() {
        let mut h = Header::new("prove", "A may-commit".into(), false);
        h.answer = "no (every candidate decided)".into();
        h.scope = dependencies_clause(11);
        h.more = vec!["detail: true lists every blocker".into()];
        let text = h.human();
        assert_eq!(text.lines().count(), 4, "{text}");
        assert!(text.starts_with("prove A may-commit   [cross-app]\nanswer: no"));
        assert!(text.contains(
            "\nanalysis complete · assumptions: loaded program; unloaded implementations and \
             subscribers are not enumerated · limits: none · scope: dependencies: 11 apps\n"
        ));
        let s = h.scope_json(Map::new());
        assert_eq!(s["analysis"], "complete");
        assert_eq!(s["universe"], "cross-app");
        assert_eq!(
            Analysis::Complete
                .and(Analysis::Partial(vec!["a".into()]))
                .human(),
            "analysis partial (a)"
        );
    }
}
