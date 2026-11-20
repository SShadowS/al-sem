//! RecordRef alias classes of a routine (ENG-145): the one place every reader learns which
//! RecordRef (or Variant) variables may share one record.
//!
//! Measured on BC 28.4 (probe `docs/superpowers/notes/2026-11-20-eng145-recordref-aliasing.md`):
//! `A := B` makes both variables hold ONE record. `GetTable` through either rebinds both (the
//! other then holds the new table, and a temporary record's state with it). `Close` detaches
//! only the variable it is called on, and `Open` needs a closed (so detached) variable, so
//! neither reaches the other. `B.Duplicate()` shares nothing.
//!
//! Readers treat every rebinding call through a class member as reaching every member, `Open`
//! and `Close` included: sound, and coarser than the platform (ENG-157 refines it).
//!
//! A routine sees only its own assignments, so [`AliasClasses::for_routine`] also joins what it
//! cannot see (sol on ENG-145): every parameter and global holder may share a record with every
//! other already (`Victim(R, R)`, two globals assigned elsewhere), and an array of them keeps
//! no record of which element holds what (`Slot[1] := R`), so a routine with one joins them all.

use crate::program::body::features::{PAnchor, PCallSite, PCallee, PVarAssignment};
use crate::program::model::workspace::ModelRoutine;
use al_syntax::IdentifierFoldExt;

/// How a RecordRef method binds the RecordRef it runs on: the one list (ENG-160), measured on
/// BC 28.4 (`docs/superpowers/notes/2026-11-20-eng160-recordref-table-moves.md` and ENG-145's
/// note). Through every alias of it, as `GetTable` (one handle). `Copy` moves neither the table
/// (another table is a runtime error) nor the temporary state; `GetBySystemId` looks only in its
/// own table; neither is here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecRefBind {
    /// `Open(No[, Temp])`: the table its first argument names.
    Open,
    /// `GetTable(Rec)`: its argument's table and temporary state.
    GetTable,
    /// `Get(RecordId)`: the RecordId's table when it finds the record (a different table is
    /// physical, the same table keeps its state), else nothing: any table, state unknown.
    Get,
    /// `Close`: no table.
    Close,
}

/// [`RecRefBind`] of a method name (any case).
#[must_use]
pub fn recref_bind(method: &str) -> Option<RecRefBind> {
    Some(match method.trim().fold_identifier().as_str() {
        "open" => RecRefBind::Open,
        "gettable" => RecRefBind::GetTable,
        "get" => RecRefBind::Get,
        "close" => RecRefBind::Close,
        _ => return None,
    })
}

/// One place a routine binds a variable in place through a [`RecRefBind`] method.
#[derive(Debug, Clone, Copy)]
pub struct BindSite<'r> {
    /// The receiver as written (`R`, `this.R`, `Slot[1]`).
    pub receiver: &'r str,
    pub how: RecRefBind,
    pub args: &'r [String],
    pub anchor: &'r PAnchor,
    /// The call site's or the record operation's id.
    pub id: &'r str,
    /// The call site, when it is one (`Open`, `GetTable`, `Close`).
    pub call: Option<&'r PCallSite>,
}

/// Every [`BindSite`] of `r`, in no particular order: its member calls, and its `Get` record
/// operations. In the IR a method of a record or RecordRef that `record_op_type` lists (`Get`,
/// `Copy`, `Insert`…) is a record OPERATION, never a call site, so a reader scanning call sites
/// alone never sees `R.Get(Id)` (ENG-160). A `Get` on a record variable is listed too: a reader
/// matches the receiver against its own RecordRef holders.
#[must_use]
pub fn bind_sites(r: &ModelRoutine) -> Vec<BindSite<'_>> {
    let calls = r.call_sites.iter().filter_map(|cs| match &cs.callee {
        PCallee::Member { receiver, method } => Some(BindSite {
            receiver,
            how: recref_bind(method)?,
            args: &cs.argument_texts,
            anchor: &cs.source_anchor,
            id: &cs.id,
            call: Some(cs),
        }),
        _ => None,
    });
    let ops = r.record_operations.iter().filter_map(|op| {
        (recref_bind(&op.op)? == RecRefBind::Get).then_some(BindSite {
            receiver: &op.record_variable_name,
            how: RecRefBind::Get,
            args: op.field_arguments.as_deref().unwrap_or_default(),
            anchor: &op.source_anchor,
            id: &op.id,
            call: None,
        })
    });
    calls.chain(ops).collect()
}
use std::collections::{HashMap, HashSet};

/// A variable reference's key, as every reader of the classes names it: an array element
/// `Recs[1]` is its array, `this.X` the global `X`, quotes stripped (`""` one `"`), folded.
/// `this.` and an index are syntax only OUTSIDE quotes: a variable named `"R[1]"` or
/// `"this.G"` keys as its whole name (sol round 2 on ENG-145).
#[must_use]
pub fn holder_key(raw: &str) -> String {
    let raw = raw.trim();
    let raw = match raw.get(..5) {
        Some(p) if p.eq_fold_identifier("this.") => &raw[5..],
        _ => raw,
    };
    // The first `[` outside quotes starts an index.
    let mut quoted = false;
    let mut end = raw.len();
    for (i, ch) in raw.char_indices() {
        match ch {
            '"' => quoted = !quoted,
            '[' if !quoted => {
                end = i;
                break;
            }
            _ => {}
        }
    }
    crate::program::body::node_util::strip_quotes(raw[..end].trim())
        .replace("\"\"", "\"")
        .fold_identifier()
}

/// A DECLARED variable name's key: a declaration's name is already logical (the lowerer strips
/// its quotes), so nothing in it is syntax; `"R[1]"` declared keys as `r[1]`, as its quoted uses
/// do through [`holder_key`]. The IR's assignment sides are logical names too.
#[must_use]
pub fn name_key(name: &str) -> String {
    crate::program::body::node_util::strip_quotes(name.trim())
        .replace("\"\"", "\"")
        .fold_identifier()
}

/// The alias classes over the variables `tracked` accepts (folded names), from `A := B`
/// assignments between two of them. A variable in no assignment is its own class.
#[derive(Debug, Default)]
pub struct AliasClasses {
    parent: HashMap<String, String>,
    /// Each alias assignment's target and position (line, column).
    pub assignments: Vec<(String, (u32, u32))>,
    /// [`Self::for_routine`]'s holders (keys); empty from [`Self::of`].
    holders: HashSet<String>,
}

impl AliasClasses {
    /// The classes `field_guard` and `record_types` read: over the routine's RecordRef and
    /// Variant holders (arrays of them included), from its own `A := B`, with every parameter
    /// and global holder joined (a caller or another routine may have aliased them) and, when
    /// the routine has an array holder, every holder joined.
    #[must_use]
    pub fn for_routine(r: &ModelRoutine) -> Self {
        let holder_type = |t: &str| {
            let t = t.trim().fold_identifier();
            t == "recordref"
                || t == "variant"
                || t.ends_with(" of recordref")
                || t.ends_with(" of variant")
        };
        let holders: Vec<&crate::program::model::workspace::ModelVariable> = r
            .variables
            .iter()
            .filter(|v| holder_type(&v.declared_type))
            .collect();
        let keys: HashSet<String> = holders.iter().map(|v| name_key(&v.name)).collect();
        let mut c = Self::of(&r.var_assignments, |n| keys.contains(n));
        let any_array = holders
            .iter()
            .any(|v| v.declared_type.trim().fold_identifier().contains(" of "));
        let joined: Vec<String> = holders
            .iter()
            .filter(|v| any_array || v.is_parameter || v.scope.as_deref() != Some("local"))
            .map(|v| name_key(&v.name))
            .collect();
        c.join(&joined);
        c.holders = keys;
        c
    }

    /// Whether the key `x` names a holder ([`Self::for_routine`] only).
    #[must_use]
    pub fn is_holder(&self, x: &str) -> bool {
        self.holders.contains(x)
    }

    /// Joins every name of `names` into one class.
    fn join(&mut self, names: &[String]) {
        let Some((first, rest)) = names.split_first() else {
            return;
        };
        for n in rest {
            let (ra, rb) = (self.root(first), self.root(n));
            self.parent
                .entry(first.clone())
                .or_insert_with(|| first.clone());
            self.parent.entry(n.clone()).or_insert_with(|| n.clone());
            if ra != rb {
                self.parent.insert(ra, rb);
            }
        }
    }
    #[must_use]
    pub fn of(assignments: &[PVarAssignment], tracked: impl Fn(&str) -> bool) -> Self {
        let mut c = Self::default();
        // The IR gives both sides as logical names (`name_key`, not source text).
        for a in assignments {
            let l = name_key(&a.lhs_name);
            let Some(w) = a.rhs_identifier.as_deref() else {
                continue;
            };
            let w = name_key(w);
            if !tracked(&l) || !tracked(&w) {
                continue;
            }
            let (rl, rw) = (c.root(&l), c.root(&w));
            c.parent.entry(l.clone()).or_insert_with(|| l.clone());
            c.parent.entry(w.clone()).or_insert_with(|| w.clone());
            if rl != rw {
                c.parent.insert(rl, rw);
            }
            c.assignments.push((
                l,
                (a.source_anchor.start_line, a.source_anchor.start_column),
            ));
        }
        c
    }

    /// The class representative of the folded name `x`.
    #[must_use]
    pub fn root(&self, x: &str) -> String {
        let mut cur = x.to_string();
        while let Some(p) = self.parent.get(&cur) {
            if *p == cur {
                break;
            }
            cur = p.clone();
        }
        cur
    }

    /// Every variable sharing a record with `x` (`x` included), folded.
    #[must_use]
    pub fn members(&self, x: &str) -> Vec<String> {
        let r = self.root(x);
        let mut out: Vec<String> = self
            .parent
            .keys()
            .filter(|n| self.root(n) == r)
            .cloned()
            .collect();
        if out.is_empty() {
            out.push(x.to_string());
        }
        out.sort();
        out
    }

    /// Whether `x` shares its record with another variable.
    #[must_use]
    pub fn has_alias(&self, x: &str) -> bool {
        self.members(x).len() > 1
    }

    /// Every folded name in an alias class of two or more.
    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.parent.keys()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::body::features::PAnchor;

    fn assign(l: &str, r: Option<&str>, line: u32) -> PVarAssignment {
        PVarAssignment {
            lhs_name: l.to_string(),
            rhs_literal_value: None,
            source_anchor: PAnchor {
                source_unit_id: "ws:test.al".into(),
                start_line: line,
                start_column: 0,
                end_line: line,
                end_column: 0,
                syntax_kind: "assignment_statement".into(),
            },
            rhs_identifier: r.map(str::to_string),
            rhs_member: None,
            lhs_is_member: false,
        }
    }

    #[test]
    fn classes_join_through_chains_and_ignore_untracked_and_expressions() {
        let rows = [
            assign("A", Some("ref"), 1),
            assign("b", Some("a"), 2),
            assign("C", Some("other"), 3),
            assign("D", None, 4),
            assign("e", Some("notrecref"), 5),
        ];
        let tracked = |n: &str| n != "notrecref";
        let c = AliasClasses::of(&rows, tracked);
        assert_eq!(c.members("ref"), ["a", "b", "ref"]);
        assert_eq!(c.members("c"), ["c", "other"]);
        assert!(!c.has_alias("d"), "an expression copy is no alias");
        assert!(!c.has_alias("e"), "an untracked source is no alias");
        assert_eq!(c.assignments.len(), 3);
    }
}
