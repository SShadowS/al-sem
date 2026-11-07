//! Round-3 c18: where the table of a `RecordRef` at one record operation
//! comes from, in words, for `query touches --table unknown`'s reason.
//!
//! A RecordRef gets its table from `Open`, from `GetTable` of a record, by an
//! assignment (`R := Other`, `R := Other.Duplicate()`), from the caller (a
//! parameter) or from wherever a global was last set. The reason names the
//! origins that can REACH the operation: a reaching-definitions walk over the
//! routine's statement tree (every statement node carries its source range).
//! At `CDOEMailHandler.Codeunit.al:450` (`RecRef.Get(SavedRecID)`) only the
//! `RecRef := RecRefFilter.Duplicate()` at :449 reaches, not the routine's
//! `RecRef.GetTable(VariantRec)` at :308, which an earlier answer named.
//!
//! The walk is the usual over-approximation: both arms of a branch, a loop's
//! back edge, `break`/`continue` to their loop's exit and head. A routine
//! with no statement tree takes every origin in the routine.

use crate::program::body::features::{PCFNNode, PCallee};
use crate::program::model::workspace::{Model, ModelRecordOperation, ModelRoutine};
use std::collections::BTreeSet;

/// One way the variable gets a table, at a source point.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    Open,
    GetTable,
    /// `R := X` (`duplicate` false) or `R := X.Duplicate()`.
    Copy {
        from: String,
        duplicate: bool,
    },
    /// `R := <anything else>`.
    Expression,
}

#[derive(Debug, Clone)]
struct Origin {
    at: (u32, u32),
    kind: Kind,
}

/// The value on entry ([`reaching`]'s set member before any origin).
const ENTRY: usize = usize::MAX;

/// The origins of `v` in `r`, in source order.
fn origins(r: &ModelRoutine, v: &str) -> Vec<Origin> {
    let mut out: Vec<Origin> = Vec::new();
    for c in &r.call_sites {
        if let PCallee::Member { receiver, method } = &c.callee
            && receiver.eq_ignore_ascii_case(v)
        {
            let kind = match method.to_ascii_lowercase().as_str() {
                "open" => Kind::Open,
                "gettable" => Kind::GetTable,
                _ => continue,
            };
            let a = &c.source_anchor;
            out.push(Origin {
                at: (a.start_line, a.start_column),
                kind,
            });
        }
    }
    for a in &r.var_assignments {
        if !a.lhs_name.eq_ignore_ascii_case(v) {
            continue;
        }
        let kind = match (&a.rhs_identifier, &a.rhs_member) {
            (Some(x), _) => Kind::Copy {
                from: x.clone(),
                duplicate: false,
            },
            (None, Some((x, m))) if m.eq_ignore_ascii_case("duplicate") => Kind::Copy {
                from: x.clone(),
                duplicate: true,
            },
            _ => Kind::Expression,
        };
        let s = &a.source_anchor;
        out.push(Origin {
            at: (s.start_line, s.start_column),
            kind,
        });
    }
    out.sort_by_key(|o| o.at);
    out
}

fn contains(range: Option<(u32, u32, u32, u32)>, p: (u32, u32)) -> bool {
    range.is_some_and(|(sl, sc, el, ec)| (sl, sc) <= p && p <= (el, ec))
}

/// The reaching-definitions walk for one site.
struct Walk<'o> {
    origins: &'o [Origin],
    site: (u32, u32),
    /// What reaches the site (every visit's set, joined).
    at_site: BTreeSet<usize>,
    seen: bool,
    breaks: Vec<BTreeSet<usize>>,
    continues: Vec<BTreeSet<usize>>,
}

impl Walk<'_> {
    fn record(&mut self, s: &BTreeSet<usize>) {
        self.seen = true;
        self.at_site.extend(s.iter().copied());
    }

    fn seq(&mut self, nodes: &[PCFNNode], mut s: BTreeSet<usize>) -> BTreeSet<usize> {
        for n in nodes {
            s = self.node(n, s);
        }
        s
    }

    /// Is the site in `n`'s own part (its condition), not in a child?
    fn in_own_part(&self, n: &PCFNNode) -> bool {
        contains(n.source_range, self.site)
            && !n
                .children
                .iter()
                .flatten()
                .chain(n.else_children.iter().flatten())
                .any(|c| contains(c.source_range, self.site))
    }

    fn lp(
        &mut self,
        body: &[PCFNNode],
        input: BTreeSet<usize>,
        at_least_once: bool,
    ) -> BTreeSet<usize> {
        self.breaks.push(BTreeSet::new());
        self.continues.push(BTreeSet::new());
        let mut head = input.clone();
        let mut out;
        loop {
            out = self.seq(body, head.clone());
            let mut next = input.clone();
            next.extend(out.iter().copied());
            next.extend(self.continues.last().expect("pushed").iter().copied());
            if next == head {
                break;
            }
            head = next;
        }
        let conts = self.continues.pop().expect("pushed");
        let brks = self.breaks.pop().expect("pushed");
        // `repeat`'s condition follows the body; the others' precede it.
        let mut exit = if at_least_once {
            let mut e = out;
            e.extend(conts);
            e
        } else {
            head
        };
        exit.extend(brks);
        exit
    }

    fn node(&mut self, n: &PCFNNode, input: BTreeSet<usize>) -> BTreeSet<usize> {
        let kids = n.children.as_deref().unwrap_or(&[]);
        match n.kind.as_str() {
            "block" | "case-branch" => self.seq(kids, input),
            "if" => {
                if self.in_own_part(n) {
                    self.record(&input);
                }
                let mut out = self.seq(kids, input.clone());
                match n.else_children.as_deref() {
                    Some(e) => out.extend(self.seq(e, input)),
                    None => out.extend(input),
                }
                out
            }
            "case" => {
                if self.in_own_part(n) {
                    self.record(&input);
                }
                let mut out = BTreeSet::new();
                for b in kids {
                    out.extend(self.node(b, input.clone()));
                }
                if !kids.iter().any(|b| b.is_case_else) {
                    out.extend(input);
                }
                out
            }
            "while" | "for" | "foreach" => {
                // The condition runs at the loop head (the input and every
                // back edge), which the loop's exit holds.
                let out = self.lp(kids, input, false);
                if self.in_own_part(n) {
                    self.record(&out);
                }
                out
            }
            "repeat" => {
                let out = self.lp(kids, input, true);
                if self.in_own_part(n) {
                    self.record(&out);
                }
                out
            }
            "exit" => {
                if contains(n.source_range, self.site) {
                    self.record(&input);
                }
                BTreeSet::new()
            }
            "break" => {
                if let Some(b) = self.breaks.last_mut() {
                    b.extend(input);
                }
                BTreeSet::new()
            }
            "continue" => {
                if let Some(c) = self.continues.last_mut() {
                    c.extend(input);
                }
                BTreeSet::new()
            }
            _ if !kids.is_empty() => self.seq(kids, input),
            _ => {
                // A statement: the site sees what reaches it; the last
                // origin inside it is what it leaves.
                if contains(n.source_range, self.site) {
                    self.record(&input);
                }
                match self
                    .origins
                    .iter()
                    .rposition(|o| contains(n.source_range, o.at))
                {
                    Some(i) => BTreeSet::from([i]),
                    None => input,
                }
            }
        }
    }
}

/// The origins (indices into `origins`, [`ENTRY`] for the value on entry)
/// that reach `site`.
fn reaching(r: &ModelRoutine, origins: &[Origin], site: (u32, u32)) -> BTreeSet<usize> {
    let every = || {
        let mut s: BTreeSet<usize> = (0..origins.len()).collect();
        s.insert(ENTRY);
        s
    };
    let Some(tree) = r.statement_tree.as_deref() else {
        return every();
    };
    let mut w = Walk {
        origins,
        site,
        at_site: BTreeSet::new(),
        seen: false,
        breaks: Vec::new(),
        continues: Vec::new(),
    };
    w.node(tree, BTreeSet::from([ENTRY]));
    if w.seen { w.at_site } else { every() }
}

/// What a variable of `r` named `name` is: `"parameter"`, `"local"` or
/// `"global"` (`None`: not declared, an implicit name).
fn scope_of<'r>(r: &'r ModelRoutine, name: &str) -> Option<&'r str> {
    if r.parameters
        .iter()
        .any(|p| p.name.eq_ignore_ascii_case(name))
    {
        return Some("parameter");
    }
    r.variables
        .iter()
        .find(|x| x.name.eq_ignore_ascii_case(name))
        .and_then(|x| x.scope.as_deref())
}

/// `, which X sets` / `, which X and Y set`: the routines of `r`'s object
/// other than `r` that give the global `g` a value.
fn global_setters(model: &Model, r: &ModelRoutine, g: &str) -> String {
    let mut names: Vec<&str> = model
        .workspace
        .routines
        .iter()
        .filter(|x| x.object_id == r.object_id && x.id != r.id)
        .filter(|x| scope_of(x, g) == Some("global") && !origins(x, g).is_empty())
        .map(|x| x.name.as_str())
        .collect();
    names.sort_unstable();
    names.dedup();
    const SHOWN: usize = 3;
    let more = names.len().saturating_sub(SHOWN);
    names.truncate(SHOWN);
    match names.as_slice() {
        [] => ", which no other routine of the object sets".to_string(),
        [one] if more == 0 => format!(", which {one} sets"),
        _ => format!(
            ", which {}{} set",
            names.join(", "),
            if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            }
        ),
    }
}

/// `x` (folded) as the source spells it: as a call's receiver or a record
/// operation's variable in `r` or its object's routines (the declared names
/// are folded), else folded.
fn spelling(model: &Model, r: &ModelRoutine, x: &str) -> String {
    let in_routine = |y: &ModelRoutine| -> Option<String> {
        y.call_sites
            .iter()
            .find_map(|c| match &c.callee {
                PCallee::Member { receiver, .. } if receiver.eq_ignore_ascii_case(x) => {
                    Some(receiver.clone())
                }
                _ => None,
            })
            .or_else(|| {
                y.record_operations
                    .iter()
                    .find(|o| o.record_variable_name.eq_ignore_ascii_case(x))
                    .map(|o| o.record_variable_name.clone())
            })
    };
    in_routine(r)
        .or_else(|| {
            model
                .workspace
                .routines
                .iter()
                .filter(|y| y.object_id == r.object_id)
                .find_map(in_routine)
        })
        .unwrap_or_else(|| x.to_string())
}

/// The global or not of `x`, in `r`: `the global X, which … sets` or `X`.
fn named(model: &Model, r: &ModelRoutine, x: &str, shown: &str) -> String {
    if scope_of(r, x) == Some("global") {
        format!("the global {shown}{}", global_setters(model, r, x))
    } else {
        shown.to_string()
    }
}

/// `RecordRef <name>: <how its table is set>` for an operation on a RecordRef
/// whose table is not resolved (callers check `receiver_is_record_ref`).
#[must_use]
pub fn recref_reason(model: &Model, r: &ModelRoutine, o: &ModelRecordOperation) -> String {
    let v = o.record_variable_name.as_str();
    let all = origins(r, v);
    let site = (o.source_anchor.start_line, o.source_anchor.start_column);
    let mut phrases: Vec<String> = Vec::new();
    for i in reaching(r, &all, site) {
        let p = if i == ENTRY {
            match scope_of(r, v) {
                Some("parameter") => "passed by the caller".to_string(),
                Some("global") => {
                    format!("a global{}", global_setters(model, r, v))
                }
                _ => continue,
            }
        } else {
            match &all[i].kind {
                Kind::Open => "opened from a variable".to_string(),
                Kind::GetTable => {
                    "set by GetTable from a record whose table is not resolved".to_string()
                }
                Kind::Copy { from, duplicate } => {
                    let src = named(model, r, from, &spelling(model, r, from));
                    if *duplicate {
                        format!("copied by Duplicate() from {src}")
                    } else {
                        format!("assigned from {src}")
                    }
                }
                Kind::Expression => {
                    "assigned from an expression the analysis does not follow".to_string()
                }
            }
        };
        if !phrases.contains(&p) {
            phrases.push(p);
        }
    }
    let reason = if phrases.is_empty() {
        "not opened in this routine".to_string()
    } else {
        phrases.join("; or ")
    };
    format!("RecordRef {v}: {reason}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::model::program_calls::assemble_and_resolve_inline_program_default;

    const APP: &str = "11111111-0000-0000-0000-0000000c0018";

    /// The reason of each `RecRef.<op>` in `routine`, by 1-based line.
    fn reasons(src: &str, routine: &str) -> Vec<(u32, String)> {
        let model = assemble_and_resolve_inline_program_default(
            &[("src/Code.al".to_string(), src.to_string())],
            APP,
        );
        let r = model
            .workspace
            .routines
            .iter()
            .find(|r| r.name == routine)
            .expect("routine");
        let mut out: Vec<(u32, String)> = r
            .record_operations
            .iter()
            .filter(|o| o.receiver_is_record_ref)
            .map(|o| (o.source_anchor.start_line + 1, recref_reason(&model, r, o)))
            .collect();
        out.sort();
        out
    }

    /// The c18 shape, hand-stated: a global set by a setter, copied by
    /// `Duplicate()` in one branch, `GetTable` in the other, and copied again
    /// inside the loop just before the `Get`.
    const HANDLER: &str = "codeunit 50100 Handler
{
    var
        RecRefFilter: RecordRef;

    procedure SetRecRefFilter(var RecRef: RecordRef)
    begin
        RecRefFilter := RecRef;
    end;

    procedure RunReportID(VariantRec: Variant; SavedRecID: RecordId)
    var
        RecRef: RecordRef;
    begin
        if RecRefFilter.Number > 0 then
            RecRef := RecRefFilter.Duplicate()
        else
            RecRef.GetTable(VariantRec);
        if RecRef.FindSet() then
            repeat
                RecRef.Reset();
                if RecRefFilter.Number > 0 then begin
                    RecRef := RecRefFilter.Duplicate();
                    RecRef.Get(SavedRecID);
                end;
            until RecRef.Next() = 0;
    end;
}
";

    #[test]
    fn the_get_after_a_duplicate_names_the_duplicate_not_gettable() {
        let got = reasons(HANDLER, "RunReportID");
        let at = |line: u32| {
            got.iter()
                .find(|(l, _)| *l == line)
                .unwrap_or_else(|| panic!("no RecRef op at line {line}: {got:?}"))
                .1
                .clone()
        };
        let dup = "copied by Duplicate() from the global RecRefFilter, which SetRecRefFilter sets";
        let get_table = "set by GetTable from a record whose table is not resolved";
        // :24 `RecRef.Get` — only the Duplicate at :23 reaches it.
        assert_eq!(at(24), format!("RecordRef RecRef: {dup}"));
        // :19 `FindSet` — either branch of the `if`, never the loop's copy.
        assert_eq!(at(19), format!("RecordRef RecRef: {dup}; or {get_table}"));
        // :21 `Reset` inside the loop — both branches and the loop's copy
        // over the back edge (one phrase for both Duplicates).
        assert_eq!(at(21), format!("RecordRef RecRef: {dup}; or {get_table}"));
    }

    #[test]
    fn a_parameter_a_local_and_an_open() {
        let src = "codeunit 50101 P
{
    procedure Param(var R: RecordRef)
    begin
        R.FindFirst();
    end;

    procedure Local()
    var
        R: RecordRef;
    begin
        R.FindFirst();
    end;

    procedure Opened(T: Integer)
    var
        R: RecordRef;
    begin
        R.Open(T);
        R.FindFirst();
    end;
}
";
        let reasons_of = |routine: &str| reasons(src, routine);
        assert_eq!(
            reasons_of("Param"),
            vec![(5, "RecordRef R: passed by the caller".to_string())]
        );
        assert_eq!(
            reasons_of("Local"),
            vec![(12, "RecordRef R: not opened in this routine".to_string())]
        );
        assert_eq!(
            reasons_of("Opened"),
            vec![(20, "RecordRef R: opened from a variable".to_string())]
        );
    }
}
