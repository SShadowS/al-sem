//! A reader for the C# BC's AL compiler writes into a runtime package
//! (`bin/*.cs`): just enough of C# to recover each AL routine's statements, in
//! order and in their control flow, and the calls inside them.
//!
//! The transpiler's output is regular: one class per object, a method per AL
//! routine carrying `[NavName("…")]` and `[MethodId(n)]`, its locals in an
//! `ALMethodScope<(…)>` tuple, and every AL statement in the body's blocks. This
//! module does not type-check or evaluate anything: it splits a file into
//! classes, members and statements, and a statement's expression into the calls
//! it makes. What a call means is `translate`'s question.

/// One token. Strings and characters are kept whole (their text is not read).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Tok {
    /// An identifier or keyword, `\uXXXX` escapes decoded (`γscope` is
    /// `γscope`). A verbatim `@name` loses the `@`.
    Ident(String),
    /// A number literal, as written.
    Num(String),
    /// A string literal's decoded text (escapes resolved), or a character literal.
    Str(String),
    /// Punctuation: one char, or `=>`, `==`, `!=`, `&&`, `||`, `<=`, `>=`, `::`.
    Punct(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Token {
    pub(crate) tok: Tok,
    /// 1-based source line.
    pub(crate) line: u32,
}

impl Token {
    pub(crate) fn is_punct(&self, p: &str) -> bool {
        matches!(&self.tok, Tok::Punct(q) if *q == p)
    }

    pub(crate) fn ident(&self) -> Option<&str> {
        match &self.tok {
            Tok::Ident(s) => Some(s),
            _ => None,
        }
    }

    pub(crate) fn is_ident(&self, s: &str) -> bool {
        self.ident() == Some(s)
    }
}

const PUNCT2: &[&str] = &["=>", "==", "!=", "&&", "||", "<=", ">=", "::", "++", "--"];
const PUNCT1: &str = "{}()[]<>;,.:=+-*/%!&|^~?";

fn punct1(c: char) -> &'static str {
    let i = PUNCT1.find(c).expect("a known punctuation char");
    &PUNCT1[i..i + c.len_utf8()]
}

/// Split C# text into tokens. Comments and preprocessor lines are skipped.
/// An unknown character is skipped too: the reader is for one compiler's
/// output, and what it cannot read shows up as a statement it cannot classify.
pub(crate) fn lex(text: &str) -> Vec<Token> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut line = 1u32;
    let mut at_line_start = true;
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            line += 1;
            i += 1;
            at_line_start = true;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if at_line_start && c == '#' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        at_line_start = false;
        // Comments.
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                if chars[i] == '\n' {
                    line += 1;
                }
                i += 1;
            }
            i += 2;
            continue;
        }
        // Strings: regular, verbatim, interpolated (`$"…{x}…"`, braces kept as text).
        let verbatim = (c == '@' && chars.get(i + 1) == Some(&'"'))
            || (c == '$' && chars.get(i + 1) == Some(&'@') && chars.get(i + 2) == Some(&'"'))
            || (c == '@' && chars.get(i + 1) == Some(&'$') && chars.get(i + 2) == Some(&'"'));
        let regular = c == '"' || (c == '$' && chars.get(i + 1) == Some(&'"'));
        if verbatim || regular {
            let start_line = line;
            while chars[i] != '"' {
                i += 1;
            }
            i += 1;
            let mut s = String::new();
            while i < chars.len() {
                let d = chars[i];
                if verbatim {
                    if d == '"' {
                        if chars.get(i + 1) == Some(&'"') {
                            s.push('"');
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                } else if d == '\\' {
                    i += 1;
                    match chars.get(i) {
                        Some('n') => s.push('\n'),
                        Some('t') => s.push('\t'),
                        Some('r') => s.push('\r'),
                        Some('0') => s.push('\0'),
                        Some('u') => {
                            let hex: String =
                                chars[i + 1..(i + 5).min(chars.len())].iter().collect();
                            if let Some(ch) =
                                u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32)
                            {
                                s.push(ch);
                            }
                            i += 4;
                        }
                        Some(&e) => s.push(e),
                        None => {}
                    }
                    i += 1;
                    continue;
                } else if d == '"' {
                    i += 1;
                    break;
                }
                if d == '\n' {
                    line += 1;
                }
                s.push(d);
                i += 1;
            }
            out.push(Token {
                tok: Tok::Str(s),
                line: start_line,
            });
            continue;
        }
        if c == '\'' {
            let mut s = String::new();
            i += 1;
            while i < chars.len() && chars[i] != '\'' {
                if chars[i] == '\\' {
                    i += 1;
                }
                if let Some(&d) = chars.get(i) {
                    s.push(d);
                }
                i += 1;
            }
            i += 1;
            out.push(Token {
                tok: Tok::Str(s),
                line,
            });
            continue;
        }
        // Identifiers (with `\uXXXX` escapes, and a verbatim `@`).
        let ident_start = |ch: char| ch.is_alphabetic() || ch == '_';
        if ident_start(c)
            || c == '\\'
            || (c == '@'
                && chars
                    .get(i + 1)
                    .is_some_and(|&d| ident_start(d) || d == '\\'))
        {
            if c == '@' {
                i += 1;
            }
            let mut s = String::new();
            while i < chars.len() {
                let d = chars[i];
                if d == '\\' && chars.get(i + 1) == Some(&'u') {
                    let hex: String = chars[(i + 2).min(chars.len())..(i + 6).min(chars.len())]
                        .iter()
                        .collect();
                    match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        Some(ch) => {
                            s.push(ch);
                            i += 6;
                            continue;
                        }
                        None => break,
                    }
                }
                if d.is_alphanumeric() || d == '_' {
                    s.push(d);
                    i += 1;
                } else {
                    break;
                }
            }
            if s.is_empty() {
                i += 1;
                continue;
            }
            out.push(Token {
                tok: Tok::Ident(s),
                line,
            });
            continue;
        }
        if c.is_ascii_digit() {
            let mut s = String::new();
            while i < chars.len()
                && (chars[i].is_ascii_alphanumeric()
                    || chars[i] == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit))
            {
                s.push(chars[i]);
                i += 1;
            }
            // A type suffix (`0U`, `5L`, `1.5m`) is not part of the value.
            if !s.starts_with("0x") && !s.starts_with("0X") {
                s.truncate(s.trim_end_matches(|c: char| "uUlLmMdDfF".contains(c)).len());
            }
            out.push(Token {
                tok: Tok::Num(s),
                line,
            });
            continue;
        }
        let two: String = chars[i..(i + 2).min(chars.len())].iter().collect();
        if let Some(p) = PUNCT2.iter().find(|p| **p == two) {
            out.push(Token {
                tok: Tok::Punct(p),
                line,
            });
            i += 2;
            continue;
        }
        if PUNCT1.contains(c) {
            out.push(Token {
                tok: Tok::Punct(punct1(c)),
                line,
            });
        }
        i += 1;
    }
    out
}

/// The index just past the bracket that closes the one at `open`
/// (`(`/`[`/`{`), or the end of `toks`.
pub(crate) fn matching(toks: &[Token], open: usize) -> usize {
    let (o, c) = match &toks[open].tok {
        Tok::Punct("(") => ("(", ")"),
        Tok::Punct("[") => ("[", "]"),
        Tok::Punct("{") => ("{", "}"),
        _ => return open + 1,
    };
    let mut depth = 0i32;
    for (j, t) in toks.iter().enumerate().skip(open) {
        if t.is_punct(o) {
            depth += 1;
        } else if t.is_punct(c) {
            depth -= 1;
            if depth == 0 {
                return j + 1;
            }
        }
    }
    toks.len()
}

/// An attribute: `[Name(arg, …)]`, its arguments as raw token runs.
#[derive(Debug, Clone)]
pub(crate) struct Attribute {
    pub(crate) name: String,
    pub(crate) args: Vec<Vec<Token>>,
}

impl Attribute {
    /// The first argument as a string literal (`NavName("x")`).
    pub(crate) fn str_arg(&self) -> Option<&str> {
        match self.args.first()?.as_slice() {
            [
                Token {
                    tok: Tok::Str(s), ..
                },
            ] => Some(s),
            _ => None,
        }
    }

    /// The first argument as an integer (`MethodId(-42)`).
    pub(crate) fn int_arg(&self) -> Option<i64> {
        let a = self.args.first()?;
        match a.as_slice() {
            [
                Token {
                    tok: Tok::Num(n), ..
                },
            ] => n.parse().ok(),
            [
                m,
                Token {
                    tok: Tok::Num(n), ..
                },
            ] if m.is_punct("-") => n.parse::<i64>().ok().map(|v| -v),
            _ => None,
        }
    }

    /// Every string-literal argument (`LocalsNames("A", "B")`).
    pub(crate) fn str_args(&self) -> Vec<&str> {
        self.args
            .iter()
            .filter_map(|a| match a.as_slice() {
                [
                    Token {
                        tok: Tok::Str(s), ..
                    },
                ] => Some(s.as_str()),
                _ => None,
            })
            .collect()
    }

    /// The tokens of every argument, flattened (`MethodScopeFlags(A | B)`).
    pub(crate) fn mentions(&self, ident: &str) -> bool {
        self.args.iter().flatten().any(|t| t.is_ident(ident))
    }
}

/// Split `toks[from..to]` at top-level commas.
/// The index after the generic argument list `<…>` opening at `i`
/// (`NavDictionary<NavText, NavText>`, `ByRef<NavList<NavText>>`), or `None`
/// when the `<` is a comparison: a generic list follows a type name (PascalCase;
/// C# locals are camelCase) and holds only type tokens.
fn generic_end(toks: &[Token], i: usize) -> Option<usize> {
    if !toks[i].is_punct("<")
        || !i
            .checked_sub(1)
            .and_then(|p| toks[p].ident())
            .is_some_and(|s| s.starts_with(|c: char| c.is_ascii_uppercase()))
    {
        return None;
    }
    let mut depth = 0i32;
    for (j, t) in toks.iter().enumerate().skip(i) {
        if t.is_punct("<") {
            depth += 1;
        } else if t.is_punct(">") {
            depth -= 1;
            if depth == 0 {
                return Some(j + 1);
            }
        } else if !(t.ident().is_some()
            || t.is_punct(",")
            || t.is_punct(".")
            || t.is_punct("[")
            || t.is_punct("]")
            || t.is_punct("?"))
        {
            return None;
        }
    }
    None
}

pub(crate) fn split_commas(toks: &[Token]) -> Vec<Vec<Token>> {
    let mut out = Vec::new();
    let mut cur = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        if t.is_punct("(") || t.is_punct("[") || t.is_punct("{") {
            let end = matching(toks, i);
            cur.extend_from_slice(&toks[i..end]);
            i = end;
            continue;
        }
        // A generic argument list `<A, B>` in a type: skip it as one unit.
        if let Some(end) = generic_end(toks, i) {
            cur.extend_from_slice(&toks[i..end]);
            i = end;
            continue;
        }
        if t.is_punct(",") {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(t.clone());
        }
        i += 1;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Read the attribute lists starting at `i`: `[A(…)][B, C(…)]`. Returns them and
/// the index after the last list.
fn read_attributes(toks: &[Token], mut i: usize) -> (Vec<Attribute>, usize) {
    let mut out = Vec::new();
    while i < toks.len() && toks[i].is_punct("[") {
        let end = matching(toks, i);
        for part in split_commas(&toks[i + 1..end - 1]) {
            let Some(name) = part.first().and_then(Token::ident) else {
                continue;
            };
            // `NavObjectId(ObjectId = 5)`: keep the run after `(`.
            let args = match part.iter().position(|t| t.is_punct("(")) {
                Some(p) => {
                    let close = matching(&part, p);
                    split_commas(&part[p + 1..close - 1])
                }
                None => Vec::new(),
            };
            out.push(Attribute {
                name: name.to_string(),
                args,
            });
        }
        i = end;
    }
    (out, i)
}

/// A parameter: its attributes (`[NavObjectId(ObjectId = 18)]`,
/// `[NavByReferenceAttribute]`), its C# type text and its name.
#[derive(Debug, Clone)]
pub(crate) struct Param {
    pub(crate) attributes: Vec<Attribute>,
    pub(crate) ty: Vec<Token>,
    pub(crate) name: String,
}

impl Param {
    pub(crate) fn by_reference(&self) -> bool {
        self.attributes
            .iter()
            .any(|a| a.name == "NavByReferenceAttribute" || a.name == "NavByReference")
    }

    /// `[NavObjectId(ObjectId = n)]`: the object id of a record or codeunit
    /// parameter.
    pub(crate) fn object_id(&self) -> Option<i64> {
        let a = self.attributes.iter().find(|a| a.name == "NavObjectId")?;
        let run = a.args.first()?;
        let n = run.iter().rev().find_map(|t| match &t.tok {
            Tok::Num(n) => n.parse::<i64>().ok(),
            _ => None,
        })?;
        let negative = run.len() >= 2 && run[run.len() - 2].is_punct("-");
        Some(if negative { -n } else { n })
    }
}

/// A member of a class.
#[derive(Debug, Clone)]
pub(crate) enum Member {
    /// A field: `[NavName("Cust")] private NavRecordHandle cust;`, with its
    /// initializer tokens when written inline. (Its attributes are not kept:
    /// bodies name it by its C# name.)
    Field {
        ty: Vec<Token>,
        name: String,
        init: Vec<Token>,
    },
    /// A method or constructor with a body (an expression-bodied one keeps its
    /// expression as a one-statement body).
    Method(Method),
}

#[derive(Debug, Clone)]
pub(crate) struct Method {
    pub(crate) attributes: Vec<Attribute>,
    /// The C# return type tokens (empty for a constructor).
    pub(crate) ret: Vec<Token>,
    pub(crate) name: String,
    pub(crate) params: Vec<Param>,
    pub(crate) body: Vec<Stmt>,
}

impl Method {
    pub(crate) fn attribute(&self, name: &str) -> Option<&Attribute> {
        self.attributes.iter().find(|a| a.name == name)
    }

    /// `[NavName("…")]`: the AL name.
    pub(crate) fn nav_name(&self) -> Option<&str> {
        self.attribute("NavName")?.str_arg()
    }

    /// `[MethodId(n)]`.
    pub(crate) fn method_id(&self) -> Option<i32> {
        i32::try_from(self.attribute("MethodId")?.int_arg()?).ok()
    }

    /// `MethodScopeFlags.IsTrigger` on the method.
    pub(crate) fn is_trigger(&self) -> bool {
        self.attribute("MethodScopeFlags")
            .is_some_and(|a| a.mentions("IsTrigger"))
    }
}

/// A class: its name and members. Nested classes are read as their own
/// `Class` (with `outer` set) so a display class's methods are not lost.
#[derive(Debug, Clone)]
pub(crate) struct Class {
    pub(crate) name: String,
    pub(crate) members: Vec<Member>,
    pub(crate) outer: Option<String>,
}

impl Class {
    pub(crate) fn methods(&self) -> impl Iterator<Item = &Method> {
        self.members.iter().filter_map(|m| match m {
            Member::Method(m) => Some(m),
            Member::Field { .. } => None,
        })
    }
}

/// A statement. Expressions stay token runs: `translate` reads the calls in
/// them ([`calls`]).
#[derive(Debug, Clone)]
pub(crate) enum Stmt {
    Expr(Vec<Token>),
    /// A local declaration `T x = e;` (its initializer is `init`).
    Local {
        ty: Vec<Token>,
        init: Vec<Token>,
    },
    If {
        cond: Vec<Token>,
        then: Vec<Stmt>,
        els: Option<Vec<Stmt>>,
    },
    /// `while`, `do … while`, `for`, `foreach`: `head` holds the condition (or
    /// the `for`/`foreach` header), evaluated at least once for `do`.
    Loop {
        head: Vec<Token>,
        body: Vec<Stmt>,
        post_test: bool,
    },
    Switch {
        scrutinee: Vec<Token>,
        arms: Vec<Vec<Stmt>>,
    },
    Try {
        body: Vec<Stmt>,
        catches: Vec<Vec<Stmt>>,
        finally: Vec<Stmt>,
    },
    Block(Vec<Stmt>),
    Return(Vec<Token>),
    Throw(Vec<Token>),
    Break,
    Continue,
    Goto,
    Label,
}

/// The first line a statement's tokens sit on.
pub(crate) fn first_line(toks: &[Token]) -> u32 {
    toks.first().map_or(0, |t| t.line)
}

struct Parser<'a> {
    toks: &'a [Token],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.toks.get(self.i)
    }

    /// Tokens up to (not including) the next top-level `;`, which is consumed.
    fn until_semicolon(&mut self) -> Vec<Token> {
        let start = self.i;
        while self.i < self.toks.len() {
            let t = &self.toks[self.i];
            if t.is_punct(";") {
                let run = self.toks[start..self.i].to_vec();
                self.i += 1;
                return run;
            }
            if t.is_punct("(") || t.is_punct("[") || t.is_punct("{") {
                self.i = matching(self.toks, self.i);
                continue;
            }
            if t.is_punct("}") {
                // A missing `;` before a block end: stop here.
                return self.toks[start..self.i].to_vec();
            }
            self.i += 1;
        }
        self.toks[start..].to_vec()
    }

    /// The parenthesized run at the cursor, without its parentheses.
    fn parens(&mut self) -> Vec<Token> {
        if !self.peek().is_some_and(|t| t.is_punct("(")) {
            return Vec::new();
        }
        let end = matching(self.toks, self.i);
        let run = self.toks[self.i + 1..end - 1].to_vec();
        self.i = end;
        run
    }

    /// A statement body: a block, or one statement.
    fn body(&mut self) -> Vec<Stmt> {
        if self.peek().is_some_and(|t| t.is_punct("{")) {
            self.block()
        } else {
            self.statement().into_iter().collect()
        }
    }

    /// `{ … }` at the cursor.
    fn block(&mut self) -> Vec<Stmt> {
        let end = matching(self.toks, self.i);
        let mut inner = Parser {
            toks: &self.toks[self.i + 1..end.saturating_sub(1)],
            i: 0,
        };
        self.i = end;
        inner.statements()
    }

    fn statements(&mut self) -> Vec<Stmt> {
        let mut out = Vec::new();
        while self.i < self.toks.len() {
            let before = self.i;
            if let Some(s) = self.statement() {
                out.push(s);
            }
            if self.i == before {
                self.i += 1;
            }
        }
        out
    }

    fn statement(&mut self) -> Option<Stmt> {
        let t = self.peek()?.clone();
        if t.is_punct(";") {
            self.i += 1;
            return None;
        }
        if t.is_punct("{") {
            return Some(Stmt::Block(self.block()));
        }
        let kw = t.ident().unwrap_or("");
        // A label `name:` (not `::`).
        if !kw.is_empty()
            && self.toks.get(self.i + 1).is_some_and(|n| n.is_punct(":"))
            && !matches!(kw, "case" | "default")
        {
            self.i += 2;
            return Some(Stmt::Label);
        }
        match kw {
            "if" => {
                self.i += 1;
                let cond = self.parens();
                let then = self.body();
                let els = if self.peek().is_some_and(|t| t.is_ident("else")) {
                    self.i += 1;
                    Some(self.body())
                } else {
                    None
                };
                Some(Stmt::If { cond, then, els })
            }
            "while" | "for" | "foreach" => {
                self.i += 1;
                let head = self.parens();
                let body = self.body();
                Some(Stmt::Loop {
                    head,
                    body,
                    post_test: false,
                })
            }
            "do" => {
                self.i += 1;
                let body = self.body();
                let mut head = Vec::new();
                if self.peek().is_some_and(|t| t.is_ident("while")) {
                    self.i += 1;
                    head = self.parens();
                    if self.peek().is_some_and(|t| t.is_punct(";")) {
                        self.i += 1;
                    }
                }
                Some(Stmt::Loop {
                    head,
                    body,
                    post_test: true,
                })
            }
            "switch" => {
                self.i += 1;
                let scrutinee = self.parens();
                let mut arms = Vec::new();
                if self.peek().is_some_and(|t| t.is_punct("{")) {
                    let end = matching(self.toks, self.i);
                    let inner = &self.toks[self.i + 1..end.saturating_sub(1)];
                    self.i = end;
                    arms = switch_arms(inner);
                }
                Some(Stmt::Switch { scrutinee, arms })
            }
            "try" => {
                self.i += 1;
                let body = self.body();
                let mut catches = Vec::new();
                let mut finally = Vec::new();
                loop {
                    if self.peek().is_some_and(|t| t.is_ident("catch")) {
                        self.i += 1;
                        let _ = self.parens();
                        // `when (…)` filter.
                        if self.peek().is_some_and(|t| t.is_ident("when")) {
                            self.i += 1;
                            let _ = self.parens();
                        }
                        catches.push(self.body());
                    } else if self.peek().is_some_and(|t| t.is_ident("finally")) {
                        self.i += 1;
                        finally = self.body();
                    } else {
                        break;
                    }
                }
                Some(Stmt::Try {
                    body,
                    catches,
                    finally,
                })
            }
            "return" => {
                self.i += 1;
                Some(Stmt::Return(self.until_semicolon()))
            }
            "throw" => {
                self.i += 1;
                Some(Stmt::Throw(self.until_semicolon()))
            }
            "break" => {
                self.until_semicolon();
                Some(Stmt::Break)
            }
            "continue" => {
                self.until_semicolon();
                Some(Stmt::Continue)
            }
            "goto" => {
                self.until_semicolon();
                Some(Stmt::Goto)
            }
            _ => {
                let run = self.until_semicolon();
                Some(local_or_expr(run))
            }
        }
    }
}

/// `case …: stmts` / `default: stmts` runs of a switch body.
fn switch_arms(toks: &[Token]) -> Vec<Vec<Stmt>> {
    // Find each arm's statement run: after `case X:` / `default:` labels, up to
    // the next label at depth 0.
    let mut starts = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        if t.is_punct("(") || t.is_punct("[") || t.is_punct("{") {
            i = matching(toks, i);
            continue;
        }
        if t.is_ident("case") || t.is_ident("default") {
            // Skip to the `:` ending the label (not a `::`).
            let mut j = i + 1;
            while j < toks.len() && !toks[j].is_punct(":") {
                if toks[j].is_punct("(") || toks[j].is_punct("[") {
                    j = matching(toks, j);
                    continue;
                }
                j += 1;
            }
            starts.push((i, j + 1));
            i = j + 1;
            continue;
        }
        i += 1;
    }
    let mut arms = Vec::new();
    for (k, &(_, body_start)) in starts.iter().enumerate() {
        let body_end = starts.get(k + 1).map_or(toks.len(), |&(label, _)| label);
        if body_start >= body_end {
            continue;
        }
        let mut p = Parser {
            toks: &toks[body_start..body_end],
            i: 0,
        };
        arms.push(p.statements());
    }
    arms
}

/// A statement run is a local declaration when it reads `Type name = …` or
/// `Type name` with a type before the name.
fn local_or_expr(run: Vec<Token>) -> Stmt {
    // `T x = e`: an identifier immediately before the first top-level `=`, with
    // something (the type) before it, and no `.` between them.
    let eq = run.iter().position(|t| t.is_punct("="));
    if let Some(eq) = eq
        && eq >= 2
        && run[eq - 1].ident().is_some()
        && !run[eq - 2].is_punct(".")
        && type_like(&run[..eq - 1])
    {
        return Stmt::Local {
            ty: run[..eq - 1].to_vec(),
            init: run[eq + 1..].to_vec(),
        };
    }
    Stmt::Expr(run)
}

/// Tokens that can be a C# type: identifiers, `.`, `<`, `>`, `,`, `(`, `)`,
/// `[`, `]`, `?`.
fn type_like(toks: &[Token]) -> bool {
    !toks.is_empty()
        && toks.iter().all(|t| {
            t.ident().is_some()
                || [".", "<", ">", ",", "(", ")", "[", "]", "?"]
                    .iter()
                    .any(|p| t.is_punct(p))
        })
        && toks[0]
            .ident()
            .is_some_and(|w| w != "await" && w != "return")
}

const MODIFIERS: &[&str] = &[
    "public",
    "private",
    "protected",
    "internal",
    "static",
    "async",
    "override",
    "sealed",
    "virtual",
    "readonly",
    "new",
    "abstract",
    "extern",
    "unsafe",
    "partial",
    "const",
    "volatile",
];

/// Read every class in `toks`.
pub(crate) fn classes(toks: &[Token]) -> Vec<Class> {
    let mut out = Vec::new();
    collect_classes(toks, None, &mut out);
    out
}

fn collect_classes(toks: &[Token], outer: Option<&str>, out: &mut Vec<Class>) {
    let mut i = 0;
    while i < toks.len() {
        if toks[i].is_ident("class")
            && let Some(name) = toks.get(i + 1).and_then(Token::ident)
        {
            let name = name.to_string();
            let mut j = i + 2;
            while j < toks.len() && !toks[j].is_punct("{") {
                j += 1;
            }
            if j >= toks.len() {
                return;
            }
            let end = matching(toks, j);
            let body = &toks[j + 1..end.saturating_sub(1)];
            let members = class_members(body, &name, out);
            out.push(Class {
                name,
                members,
                outer: outer.map(str::to_string),
            });
            i = end;
            continue;
        }
        i += 1;
    }
}

fn class_members(toks: &[Token], class_name: &str, nested: &mut Vec<Class>) -> Vec<Member> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let (attributes, after) = read_attributes(toks, i);
        i = after;
        // Modifiers.
        while toks
            .get(i)
            .and_then(Token::ident)
            .is_some_and(|w| MODIFIERS.contains(&w))
        {
            i += 1;
        }
        let Some(t) = toks.get(i) else { break };
        if t.is_ident("class") || t.is_ident("struct") {
            // A nested class: read it as its own class.
            let mut j = i;
            while j < toks.len() && !toks[j].is_punct("{") {
                j += 1;
            }
            let end = if j < toks.len() {
                matching(toks, j)
            } else {
                toks.len()
            };
            collect_classes(&toks[i..end], Some(class_name), nested);
            i = end;
            continue;
        }
        // Up to the member's name: the identifier before `(`, `=`, `;`, `{` or `=>`.
        let start = i;
        let mut j = i;
        let mut stop = None;
        while j < toks.len() {
            let u = &toks[j];
            if u.is_punct("<") {
                // A generic type argument list.
                let mut depth = 0;
                while j < toks.len() {
                    if toks[j].is_punct("<") {
                        depth += 1;
                    } else if toks[j].is_punct(">") {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    } else if toks[j].is_punct("(") {
                        j = matching(toks, j) - 1;
                    }
                    j += 1;
                }
                j += 1;
                continue;
            }
            if u.is_punct("(") && j > start {
                // A tuple type `(int a, bool b) Name(…)`: a `(` right at the
                // start, or right after another type token, is part of the type.
                if j == start {
                    j = matching(toks, j);
                    continue;
                }
                stop = Some(j);
                break;
            }
            if u.is_punct("(") {
                j = matching(toks, j);
                continue;
            }
            if u.is_punct("=") || u.is_punct(";") || u.is_punct("{") || u.is_punct("=>") {
                stop = Some(j);
                break;
            }
            j += 1;
        }
        let Some(stop) = stop else { break };
        let Some(name) = (stop > start).then(|| toks[stop - 1].ident()).flatten() else {
            i = stop + 1;
            continue;
        };
        let name = name.to_string();
        let ty = toks[start..stop - 1].to_vec();
        let at = &toks[stop];
        if at.is_punct("(") {
            let close = matching(toks, stop);
            let params = split_commas(&toks[stop + 1..close - 1])
                .into_iter()
                .filter_map(|run| {
                    let (attributes, k) = read_attributes(&run, 0);
                    let rest = &run[k..];
                    let rest: Vec<Token> = rest
                        .iter()
                        .filter(|t| {
                            !t.is_ident("ref")
                                && !t.is_ident("out")
                                && !t.is_ident("in")
                                && !t.is_ident("params")
                        })
                        .cloned()
                        .collect();
                    let name = rest.last()?.ident()?.to_string();
                    Some(Param {
                        attributes,
                        ty: rest[..rest.len() - 1].to_vec(),
                        name,
                    })
                })
                .collect();
            let mut k = close;
            // `: base(…)` / `: this(…)` on a constructor.
            if toks.get(k).is_some_and(|t| t.is_punct(":")) {
                while k < toks.len() && !toks[k].is_punct("{") && !toks[k].is_punct("=>") {
                    if toks[k].is_punct("(") {
                        k = matching(toks, k);
                        continue;
                    }
                    k += 1;
                }
            }
            // `where T : …` constraints.
            while k < toks.len()
                && !toks[k].is_punct("{")
                && !toks[k].is_punct("=>")
                && !toks[k].is_punct(";")
            {
                k += 1;
            }
            let body = if toks.get(k).is_some_and(|t| t.is_punct("{")) {
                let end = matching(toks, k);
                let mut p = Parser {
                    toks: &toks[k + 1..end.saturating_sub(1)],
                    i: 0,
                };
                let b = p.statements();
                i = end;
                b
            } else if toks.get(k).is_some_and(|t| t.is_punct("=>")) {
                let mut p = Parser { toks, i: k + 1 };
                let run = p.until_semicolon();
                i = p.i;
                vec![Stmt::Expr(run)]
            } else {
                i = k + 1;
                Vec::new()
            };
            out.push(Member::Method(Method {
                attributes,
                ret: ty,
                name,
                params,
                body,
            }));
            continue;
        }
        if at.is_punct("{") {
            // A property with accessors: skip it.
            i = matching(toks, stop);
            continue;
        }
        // A field, or an expression-bodied property.
        let mut p = Parser { toks, i: stop };
        let init = if at.is_punct("=") || at.is_punct("=>") {
            p.i += 1;
            p.until_semicolon()
        } else {
            p.until_semicolon();
            Vec::new()
        };
        i = p.i;
        if at.is_punct("=>") {
            continue;
        }
        out.push(Member::Field { ty, name, init });
    }
    out
}

/// A call in an expression: the dotted chain before the `(` (`γscope`,
/// `Target`, `cust`, `Target`, `ALInsertAsync`), its arguments, and the line.
#[derive(Debug, Clone)]
pub(crate) struct Call {
    pub(crate) chain: Vec<String>,
    pub(crate) args: Vec<Vec<Token>>,
    pub(crate) line: u32,
    /// `new X(…)`: an object creation, not a call.
    pub(crate) is_new: bool,
}

impl Call {
    pub(crate) fn method(&self) -> &str {
        self.chain.last().map_or("", String::as_str)
    }
}

/// Every call in `toks`, in textual order of their `(`, nested calls (in
/// arguments) after the call that contains them. A cast `(T)x` and a
/// parenthesized expression are not calls.
pub(crate) fn calls(toks: &[Token]) -> Vec<Call> {
    let mut out = Vec::new();
    collect_calls(toks, &mut out);
    out
}

fn collect_calls(toks: &[Token], out: &mut Vec<Call>) {
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        if t.is_punct("(") {
            let close = matching(toks, i);
            // The chain: identifiers joined by `.` ending right before `(`, a
            // generic `<…>` between them allowed.
            let mut chain = Vec::new();
            let mut k = i;
            // Skip a generic argument list `Name<T>(`.
            if k > 0 && toks[k - 1].is_punct(">") {
                let mut depth = 0;
                let mut m = k - 1;
                loop {
                    if toks[m].is_punct(">") {
                        depth += 1;
                    } else if toks[m].is_punct("<") {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    if m == 0 {
                        break;
                    }
                    m -= 1;
                }
                k = m;
            }
            while k > 0 {
                let Some(id) = toks[k - 1].ident() else { break };
                chain.push(id.to_string());
                if k >= 2 && toks[k - 2].is_punct(".") {
                    k -= 2;
                    // `x.Target.y` where `x` closes a call `f().y`: stop the chain.
                    continue;
                }
                k -= 1;
                break;
            }
            chain.reverse();
            let is_keyword = chain.len() == 1
                && matches!(
                    chain[0].as_str(),
                    "if" | "while"
                        | "for"
                        | "foreach"
                        | "switch"
                        | "catch"
                        | "when"
                        | "typeof"
                        | "nameof"
                        | "sizeof"
                        | "default"
                        | "checked"
                        | "unchecked"
                        | "return"
                );
            let args_toks = &toks[i + 1..close.saturating_sub(1)];
            if !chain.is_empty() && !is_keyword {
                let is_new = k > 0 && toks[k - 1].is_ident("new");
                out.push(Call {
                    chain,
                    args: split_commas(args_toks),
                    line: t.line,
                    is_new,
                });
            }
            collect_calls(args_toks, out);
            i = close;
            continue;
        }
        if t.is_punct("[") || t.is_punct("{") {
            let close = matching(toks, i);
            collect_calls(&toks[i + 1..close.saturating_sub(1)], out);
            i = close;
            continue;
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-written in the transpiler's shape (no publisher's code).
    const SAMPLE: &str = r#"
namespace Probe
{
    [NavCodeunitOptions(0, 0, CodeunitSubType.Normal, false)]
    public sealed class Codeunit50100 : NavCodeunit
    {
        [NavName("Cust")]
        private NavRecordHandle cust;

        [NavName("DoWork")]
        [MethodId(-12345)]
        [LocalsNames("Other", "Ok")]
        [MethodScopeFlags(MethodScopeFlags.IsStackFrame)]
        public async ValueTask<bool> DoWork([NavObjectId(ObjectId = 18)][NavByReferenceAttribute] INavRecordHandle rec, NavText name)
        {
            ALMethodScope<(INavRecordHandle rec, NavCodeunitHandle other, bool ok, bool γretVal)> γscope = default;
            try
            {
                γscope = new ALMethodScope<(INavRecordHandle rec, NavCodeunitHandle other, bool ok, bool γretVal)>(this, "DoWork", MethodScopeFlags.None, -12345);
                γscope.Target.other = new NavCodeunitHandle(γscope, 50101);
                {
                    if (γscope.CStmtHit(0) & ((await γscope.Target.rec.Target.ALFindSetAsync(DataError.TrapError))))
                        do
                        {
                            γscope.StmtHit(1);
                            await γscope.Target.rec.Target.ALModifyAsync(DataError.ThrowError);
                        }
                        while (!((await γscope.Target.rec.Target.ALNextAsync()) == 0));
                    γscope.StmtHit(2);
                    ALDatabase.ALCommit();
                    switch (γscope.Target.ok)
                    {
                        case true:
                            NavDialog.ALMessage(this.Session, System.Guid.Parse("x"), "a \"q\" b");
                            goto @tmp0;
                        default:
                            await γscope.Target.other.Target.InvokeAsync(42, new object[] { γscope.Target.rec });
                            break;
                    }
                    @tmp0:
                    γscope.Target.γretVal = true;
                    return γscope.Target.γretVal;
                }
            }
            catch (Exception αe)when (ALMethodScope.ALException(γscope, αe, out NavALException αale))
            {
                throw αale;
            }
            finally
            {
                ALMethodScope.ALFinally(γscope);
            }
        }

        public static ushort InherentPermissions => 16;
    }
}
"#;

    fn find_calls(stmts: &[Stmt], out: &mut Vec<String>) {
        for s in stmts {
            match s {
                Stmt::Expr(t) | Stmt::Return(t) | Stmt::Throw(t) => {
                    out.extend(calls(t).into_iter().map(|c| c.chain.join(".")))
                }
                Stmt::Local { init, .. } => {
                    out.extend(calls(init).into_iter().map(|c| c.chain.join(".")))
                }
                Stmt::If { cond, then, els } => {
                    out.extend(calls(cond).into_iter().map(|c| c.chain.join(".")));
                    find_calls(then, out);
                    if let Some(e) = els {
                        find_calls(e, out);
                    }
                }
                Stmt::Loop { head, body, .. } => {
                    find_calls(body, out);
                    out.extend(calls(head).into_iter().map(|c| c.chain.join(".")));
                }
                Stmt::Switch { scrutinee, arms } => {
                    out.extend(calls(scrutinee).into_iter().map(|c| c.chain.join(".")));
                    for a in arms {
                        find_calls(a, out);
                    }
                }
                Stmt::Try {
                    body,
                    catches,
                    finally,
                } => {
                    find_calls(body, out);
                    for c in catches {
                        find_calls(c, out);
                    }
                    find_calls(finally, out);
                }
                Stmt::Block(b) => find_calls(b, out),
                _ => {}
            }
        }
    }

    #[test]
    fn reads_classes_members_and_attributes() {
        let classes = classes(&lex(SAMPLE));
        let [c] = classes.as_slice() else {
            panic!("one class: {classes:?}")
        };
        assert_eq!(c.name, "Codeunit50100");
        let field = c.members.iter().find_map(|m| match m {
            Member::Field { name, .. } => Some(name.as_str()),
            Member::Method(_) => None,
        });
        assert_eq!(field, Some("cust"));
        let m = c.methods().find(|m| m.name == "DoWork").expect("DoWork");
        assert_eq!(m.nav_name(), Some("DoWork"));
        assert_eq!(m.method_id(), Some(-12345));
        assert!(!m.is_trigger());
        assert_eq!(
            m.attribute("LocalsNames").unwrap().str_args(),
            ["Other", "Ok"]
        );
        assert_eq!(m.params.len(), 2);
        assert_eq!(m.params[0].object_id(), Some(18));
        assert!(m.params[0].by_reference());
        assert_eq!(m.params[1].name, "name");
    }

    #[test]
    fn reads_statements_and_calls_in_order() {
        let classes = classes(&lex(SAMPLE));
        let m = classes[0].methods().find(|m| m.name == "DoWork").unwrap();
        let mut got = Vec::new();
        find_calls(&m.body, &mut got);
        let interesting: Vec<&str> = got
            .iter()
            .map(String::as_str)
            .filter(|c| c.contains(".AL") || c.contains("Invoke") || c.starts_with("NavDialog"))
            .filter(|c| !c.starts_with("ALMethodScope"))
            .collect();
        assert_eq!(
            interesting,
            [
                "γscope.Target.rec.Target.ALFindSetAsync",
                "γscope.Target.rec.Target.ALModifyAsync",
                "γscope.Target.rec.Target.ALNextAsync",
                "ALDatabase.ALCommit",
                "NavDialog.ALMessage",
                "γscope.Target.other.Target.InvokeAsync",
            ]
        );
    }

    /// Gated on `TRANSPILED_CS_DIR` (a directory of a real package's `bin/*.cs`,
    /// never committed): every file reads into classes, and every method that
    /// carries a `[MethodId]` has a `[NavName]`. Prints the counts.
    #[test]
    fn reads_a_real_package_when_given_one() {
        let Some(dir) = std::env::var_os("TRANSPILED_CS_DIR") else {
            return;
        };
        let (mut files, mut classes_n, mut methods, mut with_id) = (0, 0, 0, 0);
        for e in std::fs::read_dir(dir).expect("dir") {
            let p = e.expect("entry").path();
            if p.extension().is_none_or(|x| x != "cs") {
                continue;
            }
            files += 1;
            let text = std::fs::read_to_string(&p).expect("read");
            let cs = classes(&lex(&text));
            assert!(!cs.is_empty(), "{}: no class", p.display());
            classes_n += cs.len();
            for m in cs.iter().flat_map(Class::methods) {
                methods += 1;
                if m.method_id().is_some() {
                    with_id += 1;
                    assert!(
                        m.nav_name().is_some(),
                        "{}: {} has an id, no NavName",
                        p.display(),
                        m.name
                    );
                }
            }
        }
        eprintln!("files {files} classes {classes_n} methods {methods} with MethodId {with_id}");
    }

    #[test]
    fn decodes_strings_and_identifier_escapes() {
        let toks = lex(r#"xγy = @"a""b" + "c\"d";"#);
        assert_eq!(toks[0].ident(), Some("xγy"));
        assert_eq!(toks[2].tok, Tok::Str("a\"b".into()));
        assert_eq!(toks[4].tok, Tok::Str("c\"d".into()));
    }
}
