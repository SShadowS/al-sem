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

/// Whether assignment `a` of `r` writes into an array of RecordRefs or Variants (`Slot[1] := R`).
/// The IR keeps no right side for an indexed target, so it may share any holder of `r` with
/// the slot, and a bind through the slot (`Slot[1].Get(Id)`) then binds it (ENG-160, sol).
#[must_use]
pub fn into_holder_array(r: &ModelRoutine, a: &PVarAssignment) -> bool {
    let l = name_key(&a.lhs_name);
    r.variables.iter().any(|v| {
        name_key(&v.name) == l && {
            let t = v.declared_type.trim().fold_identifier();
            t.ends_with(" of recordref") || t.ends_with(" of variant")
        }
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

/// Whether call `cs` in `r` may keep (or rebind) a RecordRef or Variant passed to it, so the
/// holder escapes (ENG-145). A platform method cannot: a method of a RecordRef, FieldRef or
/// record variable, or a platform keyword's method (`Report.SaveAs(Id, …, RecRef)`: the report's
/// triggers get their own record, B6). Anything else may (a procedure, an object's method, a
/// List or Dictionary storing a Variant, anything unresolved).
#[must_use]
pub fn may_keep_argument(r: &ModelRoutine, cs: &crate::program::body::features::PCallSite) -> bool {
    use crate::program::body::features::PCallee;
    let PCallee::Member { receiver, .. } = &cs.callee else {
        return true;
    };
    let key = holder_key(receiver);
    match r.variables.iter().find(|v| name_key(&v.name) == key) {
        Some(v) => {
            let ty = v.declared_type.trim().fold_identifier();
            let ty = ty.rsplit(" of ").next().unwrap_or(&ty).trim();
            !(ty == "recordref" || ty == "fieldref" || ty.starts_with("record "))
        }
        None => !matches!(
            key.as_str(),
            "report"
                | "query"
                | "xmlport"
                | "page"
                | "codeunit"
                | "database"
                | "session"
                | "system"
        ),
    }
}

/// The record methods that may run user code: they fire a table trigger or a table event (the
/// events fire even with `RunTrigger` false).
const FIRING_METHODS: &[&str] = &[
    "insert",
    "modify",
    "delete",
    "rename",
    "modifyall",
    "deleteall",
    "validate",
    "init",
];

/// Every place in `r` that may run user code, by id with its position: the call sites
/// [`may_run_user_code`] says yes for, and the record operations that may fire a trigger or a
/// table event ([`FIRING_METHODS`]). In the IR a record's `Insert`, `Modify`, … is a record
/// OPERATION, never a call site, so a reader scanning call sites alone missed them (Fable on
/// ENG-159 Part B; `R.Insert(true)` whose `OnInsert` runs a store's `Rebind`).
#[must_use]
pub fn user_code_points(r: &ModelRoutine) -> Vec<(&str, (u32, u32))> {
    let calls = r
        .call_sites
        .iter()
        .filter(|cs| may_run_user_code(r, cs))
        .map(|cs| {
            (
                cs.id.as_str(),
                (cs.source_anchor.start_line, cs.source_anchor.start_column),
            )
        });
    let ops = r
        .record_operations
        .iter()
        .filter(|op| FIRING_METHODS.contains(&op.op.fold_identifier().as_str()))
        .map(|op| {
            (
                op.id.as_str(),
                (op.source_anchor.start_line, op.source_anchor.start_column),
            )
        });
    calls.chain(ops).collect()
}

/// Whether call `cs` in `r` may run user code (AL a caller does not see: a procedure, an event
/// subscriber, a table trigger), which may rebind a RecordRef retained elsewhere (ENG-145, sol
/// round 2). Only a method of a variable of a type whose methods run no user code says no: a
/// RecordRef, FieldRef or record method that fires no trigger or table event, or a method of a
/// value type (Text, List, Json…). A bare call, an object's method, an interface, an implicit
/// `Rec`, `Insert`/`Modify`/`Delete`/`Rename`/`ModifyAll`/`DeleteAll`/`Validate`/`Init` (triggers
/// and table events fire even with `RunTrigger` false) and anything unresolved say yes.
#[must_use]
pub fn may_run_user_code(r: &ModelRoutine, cs: &crate::program::body::features::PCallSite) -> bool {
    use crate::program::body::features::PCallee;
    let PCallee::Member { receiver, method } = &cs.callee else {
        return true;
    };
    let key = holder_key(receiver);
    let Some(ty) = r
        .variables
        .iter()
        .find(|v| name_key(&v.name) == key)
        .map(|v| v.declared_type.trim().fold_identifier())
    else {
        return true;
    };
    let ty = ty.rsplit(" of ").next().unwrap_or(&ty).trim().to_string();
    let record_like = ty == "recordref" || ty == "fieldref" || ty.starts_with("record ");
    if record_like {
        return FIRING_METHODS.contains(&method.fold_identifier().as_str());
    }
    const VALUE_TYPES: &[&str] = &[
        "text",
        "code",
        "integer",
        "biginteger",
        "decimal",
        "boolean",
        "date",
        "time",
        "datetime",
        "duration",
        "guid",
        "char",
        "byte",
        "option",
        "label",
        "list",
        "dictionary",
        "textbuilder",
        "jsonobject",
        "jsonarray",
        "jsontoken",
        "jsonvalue",
        "bigtext",
        "instream",
        "outstream",
        "secrettext",
        "keyref",
        "httpclient",
        "httpcontent",
        "httpheaders",
        "httprequestmessage",
        "httpresponsemessage",
        "xmldocument",
        "xmlelement",
        "xmlnode",
        "xmlattribute",
        "xmlnodelist",
        "xmlnamespacemanager",
        "xmltext",
        "xmlcdata",
        "xmlcomment",
        "xmldeclaration",
        "xmlprocessinginstruction",
        "xmlreadoptions",
        "xmlwriteoptions",
        "xmlattributecollection",
        "xmldocumenttype",
        "xmlnametable",
    ];
    let base = ty.split(['[', ' ', '(']).next().unwrap_or(&ty);
    !VALUE_TYPES.contains(&base)
}

/// The alias classes over the variables `tracked` accepts (folded names), from `A := B`
/// assignments between two of them. A variable in no assignment is its own class.
#[derive(Debug, Default)]
pub struct AliasClasses {
    parent: HashMap<String, String>,
    /// Each alias assignment's target and position (line, column).
    pub assignments: Vec<(String, (u32, u32))>,
    /// [`Self::build`]'s holders (keys); empty from [`Self::of`].
    holders: HashSet<String>,
    /// Holders already shared with code the routine cannot see, from entry (parameters,
    /// globals, a call's result): [`Self::build`] joins them.
    external: HashSet<String>,
    /// Holders whose record may be retained elsewhere, from where ([`Self::rebinds_at`]): `None`
    /// from entry, `Some(pos)` from the first call that may keep it.
    retainable: HashMap<String, Option<(u32, u32)>>,
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
        let holders: Vec<(String, bool)> = r
            .variables
            .iter()
            .filter(|v| holder_type(&v.declared_type))
            .map(|v| {
                (
                    name_key(&v.name),
                    v.is_parameter || v.scope.as_deref() != Some("local"),
                )
            })
            .collect();
        let any_array = r
            .variables
            .iter()
            .filter(|v| holder_type(&v.declared_type))
            .any(|v| v.declared_type.trim().fold_identifier().contains(" of "));
        Self::build(r, &holders, any_array)
    }

    /// The classes over `holders` (`(key, external)`): `r`'s own `A := B`, every external
    /// holder joined with every other (and every holder when `any_array`), and a holder assigned
    /// a call's or expression's result counted external (`A := GetG()` may hand back another's
    /// record, sol round 2). A holder is RETAINABLE when its class holds an external one or one
    /// `r` passes to a call (the callee may keep it): a call that may run user code may rebind
    /// it ([`may_run_user_code`]).
    #[must_use]
    pub fn build(r: &ModelRoutine, holders: &[(String, bool)], any_array: bool) -> Self {
        let keys: HashSet<String> = holders.iter().map(|(k, _)| k.clone()).collect();
        let mut c = Self::of(&r.var_assignments, |n| keys.contains(n));
        let mut external: HashSet<String> = holders
            .iter()
            .filter(|(_, ext)| any_array || *ext)
            .map(|(k, _)| k.clone())
            .collect();
        for a in &r.var_assignments {
            let l = name_key(&a.lhs_name);
            let copy = a.rhs_identifier.is_some()
                || a.rhs_member
                    .as_ref()
                    .is_some_and(|(_, m)| m.eq_fold_identifier("duplicate"));
            if keys.contains(&l) && !copy {
                external.insert(l);
            }
        }
        let mut ext: Vec<String> = external.iter().cloned().collect();
        ext.sort();
        c.join(&ext);
        // Where each class may first be kept elsewhere: an external member from entry; else the
        // first call that may keep a member (from entry when that call is in a loop, as a later
        // iteration's calls run after it). Nobody holds it before, so no call can rebind it.
        let mut from: HashMap<String, Option<(u32, u32)>> = HashMap::new();
        let earlier = |a: Option<(u32, u32)>, b: Option<(u32, u32)>| match (a, b) {
            (None, _) | (_, None) => None,
            (Some(x), Some(y)) => Some(x.min(y)),
        };
        for k in &external {
            from.insert(c.root(k), None);
        }
        for cs in r.call_sites.iter().filter(|cs| may_keep_argument(r, cs)) {
            let at = if cs.loop_stack.is_empty() {
                Some((cs.source_anchor.start_line, cs.source_anchor.start_column))
            } else {
                None
            };
            for a in cs.argument_texts.iter().map(|a| holder_key(a)) {
                if keys.contains(&a) {
                    let root = c.root(&a);
                    let cur = from.get(&root).copied();
                    from.insert(root, cur.map_or(at, |f| earlier(f, at)));
                }
            }
        }
        c.retainable = keys
            .iter()
            .filter_map(|k| from.get(&c.root(k)).map(|f| (k.clone(), *f)))
            .collect();
        c.external = external;
        c.holders = keys;
        c
    }

    /// Whether the record of holder `x` may be retained by code the routine cannot see by the
    /// time of the call at `pos`, so that call, if it may run user code, may rebind it.
    #[must_use]
    pub fn rebinds_at(&self, x: &str, pos: (u32, u32)) -> bool {
        self.retainable
            .get(x)
            .is_some_and(|f| f.is_none_or(|from| from < pos))
    }

    /// The holders a call at `pos` that may run user code may rebind.
    pub fn rebound_at(&self, pos: (u32, u32)) -> impl Iterator<Item = &String> {
        self.retainable
            .iter()
            .filter(move |(_, f)| f.is_none_or(|from| from < pos))
            .map(|(k, _)| k)
    }

    /// Whether holder `x` is shared with code the routine cannot see from entry.
    #[must_use]
    pub fn is_external(&self, x: &str) -> bool {
        self.external.contains(x)
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
