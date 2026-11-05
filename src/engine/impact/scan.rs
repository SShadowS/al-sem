//! **What each source line declares** (agent-tasks design §8.3): a line
//! scanner over AL text that labels every line with the kind of declaration
//! it belongs to, so `alsem impact` can account for a changed line that no
//! routine holds (a field, a key, a global variable, a property, an action,
//! layout, an enum value, the object header, or nothing: outside any object).
//!
//! The scanner reads structure only: braces (`{ }`, outside comments and
//! strings), the head text before each `{` (`fields`, `key(PK; "No.")`,
//! `area(Processing)`, `table 50100 "X"`), `var` sections and routine
//! headers. A routine's own lines are labelled [`LineKind::Routine`]; the
//! caller decides which routine from the model's spans (the same spans that
//! resolve the changed roots), so the scanner never names one. A deleted run
//! of lines is labelled by scanning its own text from the state the scanner
//! had at the gap ([`Scanner::clone`] of the state after the line above it):
//! deleted lines are classified by what they were, not by what is now there.
//!
//! What it does not do: it does not parse AL. A head it does not know
//! (`dataset`, `requestpage`, `views`, …) is [`DeclKind::Other`] with the
//! word kept; text it cannot place inside an object is `Other` too. Every
//! line gets exactly one label.

/// The kind of object-level declaration a line belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DeclKind {
    /// The object's own header (`table 50100 "X"`, its braces).
    ObjectHeader,
    Field,
    Key,
    FieldGroup,
    /// A global variable (an object-level `var` section).
    GlobalVar,
    /// An object-level property (`Caption = 'X';`).
    Property,
    Action,
    /// Page layout (and a page extension's layout changes).
    Layout,
    /// A routine attribute or trigger header the model holds no routine for.
    TriggerDecl,
    EnumValue,
    /// A comment or a blank line inside an object, outside any routine.
    CommentOrBlank,
    /// Anything else inside an object (`dataset`, `requestpage`, `views`, a
    /// preprocessor line, text the scanner cannot place).
    Other,
}

impl DeclKind {
    /// Every kind, in print order.
    pub const ALL: [DeclKind; 12] = [
        DeclKind::Field,
        DeclKind::Key,
        DeclKind::FieldGroup,
        DeclKind::GlobalVar,
        DeclKind::Property,
        DeclKind::Action,
        DeclKind::Layout,
        DeclKind::TriggerDecl,
        DeclKind::EnumValue,
        DeclKind::ObjectHeader,
        DeclKind::CommentOrBlank,
        DeclKind::Other,
    ];

    /// The JSON and human name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            DeclKind::ObjectHeader => "object-header",
            DeclKind::Field => "field",
            DeclKind::Key => "key",
            DeclKind::FieldGroup => "fieldgroup",
            DeclKind::GlobalVar => "global-var",
            DeclKind::Property => "property",
            DeclKind::Action => "action",
            DeclKind::Layout => "layout",
            DeclKind::TriggerDecl => "trigger-decl",
            DeclKind::EnumValue => "enum-value",
            DeclKind::CommentOrBlank => "comment-or-blank",
            DeclKind::Other => "other",
        }
    }

    /// The human plural (`fields 84`).
    #[must_use]
    pub fn human(self) -> &'static str {
        match self {
            DeclKind::ObjectHeader => "object headers",
            DeclKind::Field => "fields",
            DeclKind::Key => "keys",
            DeclKind::FieldGroup => "field groups",
            DeclKind::GlobalVar => "global vars",
            DeclKind::Property => "properties",
            DeclKind::Action => "actions",
            DeclKind::Layout => "layout",
            DeclKind::TriggerDecl => "trigger declarations",
            DeclKind::EnumValue => "enum values",
            DeclKind::CommentOrBlank => "comments or blank",
            DeclKind::Other => "other",
        }
    }

    /// Whether a change of this kind changes a table's schema (fields,
    /// keys, field groups): a base gives the schema findings.
    #[must_use]
    pub fn is_schema(self) -> bool {
        matches!(self, DeclKind::Field | DeclKind::Key | DeclKind::FieldGroup)
    }
}

/// One line's label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LineKind {
    /// Outside every object (before, between or after objects; `namespace`
    /// and `using` lines).
    Outside,
    /// An object-level declaration.
    Decl(DeclKind),
    /// A routine's text (its attributes, header, local variables or body).
    /// `name`: the routine's name when its header was scanned (lowercased),
    /// `None` for an attribute line above it.
    Routine { name: Option<String> },
}

/// What a `{` opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Zone {
    /// The object itself; `enum_like` when its values are `value(…)`.
    Object {
        enum_like: bool,
    },
    Decl(DeclKind),
}

/// A routine being scanned.
#[derive(Clone, Debug, PartialEq, Eq)]
struct RoutineMode {
    name: Option<String>,
    /// `begin`/`case` minus `end` words so far.
    depth: i32,
    /// A `begin` was seen: the routine ends when `depth` returns to 0.
    started: bool,
    /// The header line was seen (an attribute line starts the mode before it).
    header: bool,
}

/// The scanner's state between lines (cheap to clone: a deleted run is
/// scanned from a copy).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scanner {
    in_block_comment: bool,
    stack: Vec<Zone>,
    /// The code since the last `;` (outside parentheses), `{` or `}`.
    head: String,
    /// The first line of `head`, when it holds code.
    head_line: Option<usize>,
    paren: i32,
    var_mode: bool,
    routine: Option<RoutineMode>,
}

const OBJECT_WORDS: &[&str] = &[
    "codeunit",
    "table",
    "tableextension",
    "page",
    "pageextension",
    "pagecustomization",
    "report",
    "reportextension",
    "query",
    "xmlport",
    "enum",
    "enumextension",
    "interface",
    "controladdin",
    "permissionset",
    "permissionsetextension",
    "profile",
    "profileextension",
    "entitlement",
    "dotnet",
];

/// The line's code: comments removed, the inside of strings and quoted
/// identifiers blanked (their quotes kept), `in_block` carried across lines.
fn code_of(line: &str, in_block: &mut bool) -> String {
    let b = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < b.len() {
        if *in_block {
            if b[i] == b'*' && b.get(i + 1) == Some(&b'/') {
                *in_block = false;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => break,
            b'/' if b.get(i + 1) == Some(&b'*') => {
                *in_block = true;
                i += 2;
            }
            q @ (b'\'' | b'"') => {
                out.push(q as char);
                i += 1;
                while i < b.len() {
                    if b[i] == q {
                        // A doubled quote inside a string is one quote.
                        if q == b'\'' && b.get(i + 1) == Some(&b'\'') {
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    out.push('x');
                    i += 1;
                }
                if i < b.len() {
                    out.push(q as char);
                    i += 1;
                }
            }
            c => {
                // Keep ASCII; a multi-byte char is not structure.
                out.push(if c.is_ascii() { c as char } else { 'x' });
                i += 1;
            }
        }
    }
    out
}

/// The first word of `code`, lowercased (letters, digits, `_`).
fn first_word(code: &str) -> String {
    code.trim_start()
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect::<String>()
        .to_ascii_lowercase()
}

/// The routine name of a header line (`procedure X(`, `local procedure
/// "My X"(`, `trigger OnRun()`), lowercased, quotes removed; `None` when the
/// line is no routine header.
fn routine_header(raw: &str, code: &str) -> Option<Option<String>> {
    let lc = code.trim_start().to_ascii_lowercase();
    let mut rest = lc.as_str();
    for m in ["local ", "internal ", "protected "] {
        if let Some(r) = rest.strip_prefix(m) {
            rest = r.trim_start();
        }
    }
    let after = rest
        .strip_prefix("procedure")
        .or_else(|| rest.strip_prefix("trigger"))?;
    if !after.starts_with([' ', '\t', '"']) && !after.is_empty() {
        return None;
    }
    // The name from the raw text, at the same offset (quotes are kept there).
    let raw_lc = raw.trim_start().to_ascii_lowercase();
    let off = raw_lc.len() - rest.len() + (rest.len() - after.len());
    let name_part = raw_lc.get(off..).unwrap_or("").trim_start();
    let name = if let Some(q) = name_part.strip_prefix('"') {
        q.split('"').next().unwrap_or("").to_string()
    } else {
        name_part
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect()
    };
    Some((!name.is_empty()).then_some(name))
}

/// `begin`/`case` minus `end` words of `code`.
fn block_delta(code: &str) -> (i32, bool) {
    let lc = code.to_ascii_lowercase();
    let mut d = 0;
    let mut began = false;
    for w in lc.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
        match w {
            "begin" => {
                d += 1;
                began = true;
            }
            "case" => d += 1,
            "end" => d -= 1,
            _ => {}
        }
    }
    (d, began)
}

/// The zone a `{` opens, from its head and the zone it is in.
fn open_zone(head: &str, parent: Option<Zone>) -> Zone {
    let w = first_word(head);
    let Some(parent) = parent else {
        let enum_like = w == "enum" || w == "enumextension";
        return Zone::Object { enum_like };
    };
    match w.as_str() {
        "fields" => Zone::Decl(DeclKind::Field),
        "keys" => Zone::Decl(DeclKind::Key),
        "fieldgroups" => Zone::Decl(DeclKind::FieldGroup),
        "layout" => Zone::Decl(DeclKind::Layout),
        "actions" => Zone::Decl(DeclKind::Action),
        "value" if matches!(parent, Zone::Object { enum_like: true }) => {
            Zone::Decl(DeclKind::EnumValue)
        }
        _ => match parent {
            Zone::Object { .. } => Zone::Decl(DeclKind::Other),
            z => z,
        },
    }
}

fn zone_kind(z: Zone) -> DeclKind {
    match z {
        Zone::Object { .. } => DeclKind::ObjectHeader,
        Zone::Decl(k) => k,
    }
}

impl Scanner {
    /// A scanner at the start of a file.
    #[must_use]
    pub fn new() -> Scanner {
        Scanner::default()
    }

    /// Labels `lines` (one file, or a deleted run continuing this state).
    /// `snapshot_after(i)` is asked after each line `i` (0-based index into
    /// `lines`) and the state is kept when it says so: a deletion gap's state.
    pub fn scan<'a>(
        &mut self,
        lines: impl IntoIterator<Item = &'a str>,
        mut keep: impl FnMut(usize, &Scanner),
    ) -> Vec<LineKind> {
        let mut out: Vec<LineKind> = Vec::new();
        for (i, raw) in lines.into_iter().enumerate() {
            let label = self.line(raw, i, &mut out);
            out.push(label);
            keep(i, self);
        }
        out
    }

    /// One line's label; `out` holds the labels so far, so a `{` can relabel
    /// the lines of its head.
    fn line(&mut self, raw: &str, ix: usize, out: &mut [LineKind]) -> LineKind {
        let code = code_of(raw, &mut self.in_block_comment);
        let trimmed = code.trim();
        // Inside a routine: its text until its body's last `end`.
        if let Some(r) = &mut self.routine {
            let header = routine_header(raw, &code);
            if !r.header {
                // An attribute run: more attributes, then the header.
                if trimmed.is_empty() || trimmed.starts_with('[') {
                    return LineKind::Routine { name: None };
                }
                if let Some(h) = header {
                    r.header = true;
                    r.name = h;
                    let (d, began) = block_delta(&code);
                    r.depth += d;
                    r.started |= began;
                    let name = r.name.clone();
                    if r.started && r.depth <= 0 {
                        self.routine = None;
                    }
                    return LineKind::Routine { name };
                }
                // Attributes no header follows: read the line as it is.
                self.routine = None;
            } else if !r.started
                && (header.is_some()
                    || trimmed.starts_with('[')
                    || trimmed.starts_with('}')
                    || trimmed.contains('{'))
            {
                // The routine had no body (an interface procedure).
                self.routine = None;
            } else if !r.started && trimmed.is_empty() {
                // A blank or comment line after a header with no body yet.
                return LineKind::Decl(DeclKind::CommentOrBlank);
            } else {
                let (d, began) = block_delta(&code);
                r.depth += d;
                r.started |= began;
                let name = r.name.clone();
                if r.started && r.depth <= 0 {
                    self.routine = None;
                }
                return LineKind::Routine { name };
            }
        }
        self.line_outside_routine(raw, &code, ix, out)
    }

    fn line_outside_routine(
        &mut self,
        raw: &str,
        code: &str,
        ix: usize,
        out: &mut [LineKind],
    ) -> LineKind {
        let trimmed = code.trim();
        if trimmed.is_empty() {
            return if self.stack.is_empty() {
                LineKind::Outside
            } else {
                LineKind::Decl(DeclKind::CommentOrBlank)
            };
        }
        let top = self.stack.last().copied();
        // A routine header or its attribute (inside an object).
        if top.is_some() && self.head.trim().is_empty() {
            if let Some(name) = routine_header(raw, code) {
                self.var_mode = false;
                let (d, began) = block_delta(code);
                let r = RoutineMode {
                    name: name.clone(),
                    depth: d,
                    started: began,
                    header: true,
                };
                if !(r.started && r.depth <= 0) {
                    self.routine = Some(r);
                }
                return LineKind::Routine { name };
            }
            if trimmed.starts_with('[') && matches!(top, Some(Zone::Object { .. })) {
                self.var_mode = false;
                self.routine = Some(RoutineMode {
                    name: None,
                    depth: 0,
                    started: false,
                    header: false,
                });
                return LineKind::Routine { name: None };
            }
        }
        // The label from where the line starts.
        let w = first_word(code);
        let mut label = match top {
            None => {
                if OBJECT_WORDS.contains(&w.as_str()) || !self.head.trim().is_empty() {
                    LineKind::Decl(DeclKind::ObjectHeader)
                } else {
                    LineKind::Outside
                }
            }
            Some(Zone::Object { .. }) => {
                if w == "var" || (w == "protected" && code.to_ascii_lowercase().contains("var")) {
                    self.var_mode = true;
                    LineKind::Decl(DeclKind::GlobalVar)
                } else if trimmed.starts_with('#') {
                    LineKind::Decl(DeclKind::Other)
                } else if self.var_mode && !trimmed.contains('{') && !trimmed.starts_with('}') {
                    LineKind::Decl(DeclKind::GlobalVar)
                } else {
                    self.var_mode = false;
                    if trimmed.starts_with('}') {
                        LineKind::Decl(DeclKind::ObjectHeader)
                    } else {
                        // A property, or the head of a section (relabelled
                        // at its `{`).
                        LineKind::Decl(DeclKind::Property)
                    }
                }
            }
            Some(z) => LineKind::Decl(zone_kind(z)),
        };
        // Structure: braces, heads.
        for c in code.chars() {
            match c {
                '(' => {
                    self.paren += 1;
                    self.push_head(c, ix);
                }
                ')' => {
                    self.paren -= 1;
                    self.push_head(c, ix);
                }
                ';' if self.paren <= 0 => {
                    self.head.clear();
                    self.head_line = None;
                    self.paren = 0;
                }
                '{' => {
                    let zone = open_zone(&self.head, self.stack.last().copied());
                    let kind = LineKind::Decl(zone_kind(zone));
                    if let Some(first) = self.head_line {
                        for l in out.iter_mut().skip(first).take(ix.saturating_sub(first)) {
                            if !matches!(l, LineKind::Decl(DeclKind::CommentOrBlank)) {
                                *l = kind.clone();
                            }
                        }
                    }
                    label = kind;
                    self.stack.push(zone);
                    self.var_mode = false;
                    self.head.clear();
                    self.head_line = None;
                    self.paren = 0;
                }
                '}' => {
                    if let Some(z) = self.stack.pop() {
                        // A line that only closes takes the closed zone's kind.
                        if trimmed.starts_with('}') {
                            label = LineKind::Decl(zone_kind(z));
                        }
                    }
                    self.head.clear();
                    self.head_line = None;
                    self.paren = 0;
                }
                c => self.push_head(c, ix),
            }
        }
        label
    }

    fn push_head(&mut self, c: char, ix: usize) {
        if self.head.is_empty() && c.is_whitespace() {
            return;
        }
        if self.head_line.is_none() {
            self.head_line = Some(ix);
        }
        if self.head.len() < 256 {
            self.head.push(c);
        }
    }

    /// Whether the scanner is inside an object.
    #[must_use]
    pub fn in_object(&self) -> bool {
        !self.stack.is_empty()
    }
}

/// The first line of a routine's body (0-based): the first line at or after
/// `header` (the routine node's first line) whose code starts with `var` or
/// `begin`, before `end` (inclusive); `None` when the routine has none (a
/// declaration without a body, or a one-line routine). Lines before it are
/// the routine's signature.
#[must_use]
pub fn body_start(lines: &[&str], header: usize, end: usize) -> Option<usize> {
    let mut in_block = false;
    for (i, raw) in lines.iter().enumerate().take(end + 1).skip(header) {
        let code = code_of(raw, &mut in_block);
        let w = first_word(&code);
        if i > header && (w == "var" || w == "begin") {
            return Some(i);
        }
    }
    None
}

#[cfg(test)]
#[path = "scan_tests.rs"]
mod tests;
