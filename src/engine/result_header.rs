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
        let mut third = format!(
            "{} · assumptions: {} · limits: {limits}",
            self.analysis.human(),
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

    /// `result.scope` (F76): the same facts as structured fields. `extra`
    /// adds the tool's own (`families`, `selection`).
    #[must_use]
    pub fn scope_json(&self, extra: Map<String, Value>) -> Value {
        let mut m = Map::new();
        m.insert("analysis".into(), json!(self.analysis.key()));
        m.insert("reasons".into(), json!(self.analysis.reasons()));
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

/// `dependencies: N apps` (§5.3, F32): the one form of the dependencies line;
/// the full list only with `all`/`detail`, and in the envelope's `universe`.
#[must_use]
pub fn dependencies_clause(n: usize) -> String {
    format!("dependencies: {n} {}", if n == 1 { "app" } else { "apps" })
}

#[cfg(test)]
mod tests {
    use super::*;

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
