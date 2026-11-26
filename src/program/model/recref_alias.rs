//! RecordRef alias classes of a routine (ENG-145): the one place every reader learns which
//! RecordRef (or Variant) variables may share one record.
//!
//! Measured on BC 28.4 (probe `docs/superpowers/notes/2026-11-20-eng145-recordref-aliasing.md`):
//! `A := B` makes both variables hold ONE record. `GetTable` through either rebinds both (the
//! other then holds the new table, and a temporary record's state with it). `Close` detaches
//! only the variable it is called on, and `Open` on an open variable errors. A copy shares the
//! one handle whether or not it is open (ENG-189, probe `engine-notes/eng189-probe2`, BC 28.4
//! 2026-11-26): `A := R` (or `Join(A, R)`, `A := Same(R)`) with R never opened, or closed before
//! the copy, then `A.Open(T)` opens R on T too. So only an `Open` right after the same variable's
//! own `Close` reaches no other holder. `B.Duplicate()` shares nothing.
//!
//! Readers treat every rebinding call through a class member as reaching every member, `Open`
//! and `Close` included: sound, and coarser than the platform (ENG-157 refines it). The kept
//! binds through a retainable holder ([`kept_bind_points`]) leave `Close` out and keep `Open`.
//!
//! A routine sees only its own assignments, so [`AliasClasses::for_routine`] also joins what it
//! cannot see (sol on ENG-145): every parameter and global holder may share a record with every
//! other already (`Victim(R, R)`, two globals assigned elsewhere), and an array of them keeps
//! no record of which element holds what (`Slot[1] := R`), so a routine with one joins them all.

use crate::program::body::features::{PAnchor, PCallSite, PCallee, PVarAssignment};
pub use crate::program::model::holder_key::{HolderKey, HolderScope};
use crate::program::model::workspace::ModelRoutine;
use al_syntax::IdentifierFoldExt;

// The one list of RecordRef binds lives with the body facts (ENG-175: the walk reads it too).
pub use crate::program::body::features::{RecRefBind, recref_bind};

/// Whether assignment `a` of `r` writes into an array of RecordRefs or Variants (`Slot[1] := R`).
/// The IR keeps no right side for an indexed target, so it may share any holder of `r` with
/// the slot, and a bind through the slot (`Slot[1].Get(Id)`) then binds it (ENG-160, sol).
/// The target is resolved from its source text by the routine's scope (ENG-176: `Slot[1]`,
/// `this.Slot[1]`, a global array a local hides).
#[must_use]
pub fn into_holder_array(r: &ModelRoutine, a: &PVarAssignment) -> bool {
    let scope = HolderScope::of(r);
    scope
        .text(&a.lhs_text)
        .is_some_and(|h| is_holder_array(scope, h.key))
}

/// `key` is declared an array of RecordRef or Variant.
fn is_holder_array(scope: HolderScope<'_>, key: HolderKey) -> bool {
    let t = scope.variable(key).declared_type.trim().fold_identifier();
    t.ends_with(" of recordref") || t.ends_with(" of variant")
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

/// `raw` trimmed, without parentheses around the whole of it: `S.Keep((RR))` passes `RR`
/// (Fable on ENG-159). A text whose first `(` closes before its end (`(A) + (B)`) keeps its
/// parentheses.
#[must_use]
pub fn strip_parens(raw: &str) -> &str {
    let mut s = raw.trim();
    while s.starts_with('(') && s.ends_with(')') {
        let mut depth = 0usize;
        let mut quoted = false;
        let closes_at_end = s.char_indices().all(|(i, ch)| {
            match ch {
                '"' => quoted = !quoted,
                '(' if !quoted => depth += 1,
                ')' if !quoted => depth = depth.saturating_sub(1),
                _ => {}
            }
            depth > 0 || i == s.len() - 1
        });
        if !closes_at_end {
            break;
        }
        s = s[1..s.len() - 1].trim();
    }
    s
}

/// Whether call `cs` in `r` may keep (or rebind) a RecordRef or Variant passed to it, so the
/// holder escapes (ENG-145). A platform method cannot: a method of a RecordRef, FieldRef or
/// record variable, or a platform keyword's method (`Report.SaveAs(Id, …, RecRef)`: the report's
/// triggers get their own record, B6). Anything else may (a procedure, an object's method, a
/// List or Dictionary storing a Variant, anything unresolved).
#[must_use]
pub fn may_keep_argument(
    r: &ModelRoutine,
    cs: &crate::program::body::features::PCallSite,
    shadowed: &ShadowedBuiltins,
) -> bool {
    use crate::program::body::features::PCallee;
    let PCallee::Member { receiver, method } = &cs.callee else {
        return true;
    };
    // ENG-176: the receiver by the routine's scope (`this.X` is the global).
    let scope = HolderScope::of(r);
    match scope.text(receiver).map(|h| scope.variable(h.key)) {
        Some(v) => {
            let ty = v.declared_type.trim().fold_identifier();
            let ty = ty.rsplit(" of ").next().unwrap_or(&ty).trim();
            // A table's own procedure may keep it (Fable on ENG-159).
            record_like_builtin(ty, method, shadowed).is_none_or(|builtin| !builtin)
        }
        // A platform keyword receiver (`Report.SaveAs(…)`): not a variable.
        None => !matches!(
            strip_parens(receiver)
                .trim_matches('"')
                .fold_identifier()
                .as_str(),
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

/// For a variable of folded type `ty`: whether `method` is a platform method of a RecordRef,
/// FieldRef or record (`member_catalog`), not a table's own procedure. A record's built-in name
/// that some table declares as a procedure ([`ShadowedBuiltins`]) may be that procedure. `None`
/// for another type.
fn record_like_builtin(ty: &str, method: &str, shadowed: &ShadowedBuiltins) -> Option<bool> {
    use crate::program::resolve::member_catalog::{MemberCatalogKind, member_builtin};
    let m = method.fold_identifier();
    let kind = match ty {
        "recordref" => MemberCatalogKind::RecordRef,
        "fieldref" => MemberCatalogKind::FieldRef,
        t if t.starts_with("record ") => {
            return Some(member_builtin(MemberCatalogKind::Record, &m) && !shadowed.0.contains(&m));
        }
        _ => return None,
    };
    Some(member_builtin(kind, &m))
}

/// The record built-in method names some table or table extension of the program declares as
/// its own procedure (alc binds `T.RecordId()` to the procedure, with warning AL0524; sol on
/// ENG-159). On a record, such a name may run that procedure: user code, which may keep an
/// argument. Built from every routine of the model, dependency routines from symbols included.
#[derive(Debug, Clone, Default)]
pub struct ShadowedBuiltins(std::collections::HashSet<String>);

impl ShadowedBuiltins {
    /// The names among `routines`' table and table extension procedures.
    #[must_use]
    pub fn of<'a>(routines: impl IntoIterator<Item = &'a ModelRoutine>) -> Self {
        use crate::program::resolve::member_catalog::{MemberCatalogKind, member_builtin};
        Self(
            routines
                .into_iter()
                .filter(|r| {
                    matches!(
                        r.object_type.fold_identifier().as_str(),
                        "table" | "tableextension"
                    )
                })
                .map(|r| r.name.fold_identifier())
                .filter(|n| member_builtin(MemberCatalogKind::Record, n))
                .collect(),
        )
    }

    /// No name is shadowed: for a caller that states no table declares one.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    fn contains(&self, name: &str) -> bool {
        self.0.contains(&name.fold_identifier())
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
///
/// Each comes with where it starts (where a fact it makes is anchored) and where it ends: its
/// user code runs then, after its arguments, so [`AliasClasses::rebinds_at`] takes the end.
/// A record operation whose name a table declares as a procedure ([`ShadowedBuiltins`]) may run
/// it, so it counts too.
#[must_use]
pub fn user_code_points<'a>(
    r: &'a ModelRoutine,
    shadowed: &ShadowedBuiltins,
) -> Vec<UserCodePoint<'a>> {
    let point = |id: &'a str, a: &crate::program::body::features::PAnchor| UserCodePoint {
        id,
        at: (a.start_line, a.start_column),
        runs: (a.end_line, a.end_column),
        except: None,
    };
    let calls = r
        .call_sites
        .iter()
        .filter(|cs| may_run_user_code(r, cs, shadowed))
        .map(|cs| point(cs.id.as_str(), &cs.source_anchor));
    let ops = r
        .record_operations
        .iter()
        .filter(|op| {
            FIRING_METHODS.contains(&op.op.fold_identifier().as_str()) || shadowed.contains(&op.op)
        })
        .map(|op| point(op.id.as_str(), &op.source_anchor));
    calls.chain(ops).collect()
}

/// The in-place binds ([`bind_sites`]: `GetTable`, `Get`, `Open`) made through a holder that is
/// RETAINABLE at that point ([`AliasClasses::rebinds_at`]): the record that holder holds may be
/// shared with any other retainable holder the classes cannot join (a local passed to a call that
/// returned it, `L := Same(R)`; a call that joined two of them, `Join(L, R)`; a `var` Variant out
/// parameter), so the bind may rebind them all, as a place that may run user code may (ENG-189,
/// Fable's review of ENG-161: `L := Same(R); L.GetTable(Doc); Guarded(R)` answered `no`; BC 28.4:
/// a returned RecordRef is its argument's handle, ENG-175). Each as a [`UserCodePoint`], the
/// readers treat it as one. Not `Close` (sol r1 on ENG-189; BC 28.4, module doc: `Close` detaches
/// only its own variable). `Open` is kept (Fable r3 on ENG-189, measured BC 28.4 2026-11-26,
/// `engine-notes/eng189-probe2`): a copy shares one handle whether or not it is open, so
/// `L := Same(R); L.Open(Doc)` with R never opened, or closed before the copy, opens R too. An
/// `Open` right after its own receiver's `Close` fills a detached handle and reaches no other
/// holder; it is still kept here (a sound over-reach, ENG-212 refines it).
#[must_use]
pub fn kept_bind_points<'a>(r: &'a ModelRoutine, classes: &AliasClasses) -> Vec<UserCodePoint<'a>> {
    use crate::program::body::features::RecRefBind;
    let scope = HolderScope::of(r);
    bind_sites(r)
        .into_iter()
        .filter(|s| s.how != RecRefBind::Close)
        .filter_map(|s| {
            let h = scope.text(s.receiver)?;
            let at = (s.anchor.start_line, s.anchor.start_column);
            let runs = (s.anchor.end_line, s.anchor.end_column);
            // Retainable when the bind RUNS, after its own arguments (`L.Get(JoinId(L, R))`:
            // Fable r1 on ENG-189). An array element may be any handle the array was given
            // (`Join(Slot[1], R)`): the classes hold no array slots, so an element of a holder
            // array always counts (not of a record array: `Recs[1].Get('X')`, Fable r2).
            let holder_array = h.indexed && is_holder_array(scope, h.key);
            (holder_array || classes.rebinds_at(h.key, runs)).then_some(UserCodePoint {
                id: s.id,
                at,
                runs,
                // The receiver itself is the bind's (`ShadowAliasFlow`: `R.GetTable(E)` with R
                // copied into a global later binds R to E, not to anything). Not its whole class:
                // the in-place rule may leave a member out (an alias made only in the other
                // branch, Fable r1), so the other members are rebound like any kept holder.
                except: (!h.indexed).then_some(h.key),
            })
        })
        .collect()
}

/// A place that may run user code ([`user_code_points`]).
#[derive(Debug, Clone, Copy)]
pub struct UserCodePoint<'a> {
    /// The call site's or record operation's id.
    pub id: &'a str,
    /// Where it starts.
    pub at: (u32, u32),
    /// Where it ends: when its user code runs.
    pub runs: (u32, u32),
    /// A kept bind's own receiver ([`kept_bind_points`]): the reader's in-place rule binds it to
    /// the bind's table, so the point rebinds every OTHER retainable holder, the rest of its
    /// alias class included. `None` for a place that may run user code, and for an array
    /// element receiver (its slot is not a holder the classes know).
    pub except: Option<HolderKey>,
}

impl UserCodePoint<'_> {
    /// The holders this point may rebind: those `classes` says are retainable when it runs,
    /// leaving out its own receiver ([`Self::except`]).
    pub fn rebinds<'c>(&self, classes: &'c AliasClasses) -> impl Iterator<Item = HolderKey> + 'c {
        let own = self.except;
        classes
            .rebound_at(self.runs)
            .filter(move |h| Some(*h) != own)
    }
}

/// Whether call `cs` in `r` may run user code (AL a caller does not see: a procedure, an event
/// subscriber, a table trigger), which may rebind a RecordRef retained elsewhere (ENG-145, sol
/// round 2). Only a method of a variable of a type whose methods run no user code says no: a
/// RecordRef, FieldRef or record method that fires no trigger or table event, or a method of a
/// value type (Text, List, Json…). A bare call, an object's method, an interface, an implicit
/// `Rec`, `Insert`/`Modify`/`Delete`/`Rename`/`ModifyAll`/`DeleteAll`/`Validate`/`Init` (triggers
/// and table events fire even with `RunTrigger` false) and anything unresolved say yes.
#[must_use]
pub fn may_run_user_code(
    r: &ModelRoutine,
    cs: &crate::program::body::features::PCallSite,
    shadowed: &ShadowedBuiltins,
) -> bool {
    use crate::program::body::features::PCallee;
    let PCallee::Member { receiver, method } = &cs.callee else {
        return true;
    };
    // ENG-176: the receiver by the routine's scope (`this.X` is the global).
    let scope = HolderScope::of(r);
    let Some(ty) = scope
        .text(receiver)
        .map(|h| scope.variable(h.key).declared_type.trim().fold_identifier())
    else {
        return true;
    };
    let ty = ty.rsplit(" of ").next().unwrap_or(&ty).trim().to_string();
    // A platform method fires user code only when it fires a trigger or a table event; a
    // table's own procedure (`D.Poke()`) is user code (Fable on ENG-159).
    if let Some(builtin) = record_like_builtin(&ty, method, shadowed) {
        return !builtin || FIRING_METHODS.contains(&method.fold_identifier().as_str());
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
        // Not `HttpClient`: in a test codeunit its `Send` runs an `[HttpClientHandler]`
        // procedure (Fable on ENG-159).
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

/// Whether a declared type is a RecordRef or Variant holder, an array of them included.
fn is_holder_type(t: &str) -> bool {
    let t = t.trim().fold_identifier();
    t == "recordref" || t == "variant" || t.ends_with(" of recordref") || t.ends_with(" of variant")
}

/// The alias classes over the holders `tracked` accepts, keyed exactly ([`HolderKey`], ENG-176),
/// from `A := B` assignments between two of them. A variable in no assignment is its own class.
#[derive(Debug, Default)]
pub struct AliasClasses {
    parent: HashMap<HolderKey, HolderKey>,
    /// Each alias assignment's target and position (line, column).
    pub assignments: Vec<(HolderKey, (u32, u32))>,
    /// [`Self::build`]'s holders (keys); empty from [`Self::of`].
    holders: HashSet<HolderKey>,
    /// Holders already shared with code the routine cannot see, from entry (parameters,
    /// globals, a call's result): [`Self::build`] joins them.
    external: HashSet<HolderKey>,
    /// Holders whose record may be retained elsewhere, from where ([`Self::rebinds_at`]): `None`
    /// from entry, `Some(pos)` from the first call that may keep it.
    retainable: HashMap<HolderKey, Option<(u32, u32)>>,
}

impl AliasClasses {
    /// The classes `field_guard` and `record_types` read: over the routine's RecordRef and
    /// Variant holders (arrays of them included), its own and the globals of its object it can
    /// name bare (one a local hides is left out; a holder copied into it is still kept), from its
    /// own `A := B`, with every parameter and global holder joined (a caller or another routine
    /// may have aliased them) and, when the routine has an array holder, every holder joined.
    #[must_use]
    pub fn for_routine(r: &ModelRoutine, shadowed: &ShadowedBuiltins) -> Self {
        let own = r.variables.own().iter().enumerate().map(|(i, v)| {
            (
                HolderKey::Own(i as u32),
                v,
                v.is_parameter || v.scope.as_deref() != Some("local"),
            )
        });
        // A global a parameter or local hides is left out: it joins no class (`build` still takes
        // a holder copied into it as kept), so it cannot end a guard on the variable hiding it.
        let scope = HolderScope::of(r);
        let globals = r
            .variables
            .globals()
            .iter()
            .enumerate()
            .map(|(i, v)| (HolderKey::Global(i as u32), v, true))
            .filter(|(k, v, _)| scope.logical(&v.name) == Some(*k));
        let decls: Vec<(HolderKey, bool, bool)> = own
            .chain(globals)
            .filter(|(_, v, _)| is_holder_type(&v.declared_type))
            .map(|(k, v, ext)| {
                (
                    k,
                    ext,
                    v.declared_type.trim().fold_identifier().contains(" of "),
                )
            })
            .collect();
        let holders: Vec<(HolderKey, bool)> = decls.iter().map(|&(k, ext, _)| (k, ext)).collect();
        let any_array = decls.iter().any(|&(_, _, array)| array);
        Self::build(r, &holders, any_array, shadowed)
    }

    /// The classes over `holders` (`(key, external)`): `r`'s own `A := B`, every external
    /// holder joined with every other (and every holder when `any_array`), and a holder assigned
    /// a call's or expression's result counted external (`A := GetG()` may hand back another's
    /// record, sol round 2). A holder is RETAINABLE when its class holds an external one or one
    /// `r` passes to a call (the callee may keep it): a call that may run user code may rebind
    /// it ([`may_run_user_code`]). Every side and argument is resolved by `r`'s scope
    /// ([`HolderScope`], ENG-176): `this.G` is the global `G`, hidden or not.
    #[must_use]
    pub fn build(
        r: &ModelRoutine,
        holders: &[(HolderKey, bool)],
        any_array: bool,
        shadowed: &ShadowedBuiltins,
    ) -> Self {
        let scope = HolderScope::of(r);
        let keys: HashSet<HolderKey> = holders.iter().map(|(k, _)| *k).collect();
        let mut c = Self::of(scope, &r.var_assignments, |k| keys.contains(&k));
        let mut external: HashSet<HolderKey> = holders
            .iter()
            .filter(|(_, ext)| any_array || *ext)
            .map(|(k, _)| *k)
            .collect();
        for a in &r.var_assignments {
            let Some(l) = whole_variable(scope, &a.lhs_text) else {
                continue;
            };
            let copy = a
                .rhs_text
                .as_deref()
                .and_then(|w| whole_variable(scope, w))
                .is_some_and(|w| keys.contains(&w))
                || a.rhs_member
                    .as_ref()
                    .is_some_and(|(_, m)| m.eq_fold_identifier("duplicate"));
            if keys.contains(&l) && !copy {
                external.insert(l);
            }
            // A holder copied into a global the classes do not hold (one a local hides: a
            // caller's tracked set leaves it out) is kept by the object (ENG-162's `this.G := R`).
            if matches!(l, HolderKey::Global(_))
                && !keys.contains(&l)
                && let Some(w) = a.rhs_text.as_deref().and_then(|w| whole_variable(scope, w))
                && keys.contains(&w)
            {
                external.insert(w);
            }
        }
        let mut ext: Vec<HolderKey> = external.iter().copied().collect();
        ext.sort();
        c.join(&ext);
        // Where each class may first be kept elsewhere: an external member from entry; else the
        // first call that may keep a member (from entry when that call is in a loop, as a later
        // iteration's calls run after it). Nobody holds it before, so no call can rebind it. A
        // call keeps its argument when it runs, after its own arguments: at its END, so
        // `O.Go(S.Keep(R))`'s `Go` runs after `Keep` kept R (sol on ENG-159). User code is
        // compared by its end too ([`user_code_points`]).
        let mut from: HashMap<HolderKey, Option<(u32, u32)>> = HashMap::new();
        let earlier = |a: Option<(u32, u32)>, b: Option<(u32, u32)>| match (a, b) {
            (None, _) | (_, None) => None,
            (Some(x), Some(y)) => Some(x.min(y)),
        };
        for k in &external {
            from.insert(c.root(*k), None);
        }
        for cs in r
            .call_sites
            .iter()
            .filter(|cs| may_keep_argument(r, cs, shadowed))
        {
            let at = if cs.loop_stack.is_empty() {
                Some((cs.source_anchor.end_line, cs.source_anchor.end_column))
            } else {
                None
            };
            // An argument naming a holder or an element of one (`Slot[1]`: the array's record).
            for a in cs.argument_texts.iter().filter_map(|a| scope.text(a)) {
                if keys.contains(&a.key) {
                    let root = c.root(a.key);
                    let cur = from.get(&root).copied();
                    from.insert(root, cur.map_or(at, |f| earlier(f, at)));
                }
            }
        }
        c.retainable = keys
            .iter()
            .filter_map(|k| from.get(&c.root(*k)).map(|f| (*k, *f)))
            .collect();
        c.external = external;
        c.holders = keys;
        c
    }

    /// Whether the record of holder `x` may be retained by code the routine cannot see by the
    /// time of the call at `pos`, so that call, if it may run user code, may rebind it.
    #[must_use]
    pub fn rebinds_at(&self, x: HolderKey, pos: (u32, u32)) -> bool {
        self.retainable
            .get(&x)
            .is_some_and(|f| f.is_none_or(|from| from < pos))
    }

    /// The holders a call at `pos` that may run user code may rebind.
    pub fn rebound_at(&self, pos: (u32, u32)) -> impl Iterator<Item = HolderKey> + '_ {
        self.retainable
            .iter()
            .filter(move |(_, f)| f.is_none_or(|from| from < pos))
            .map(|(k, _)| *k)
    }

    /// Whether holder `x` is shared with code the routine cannot see from entry.
    #[must_use]
    pub fn is_external(&self, x: HolderKey) -> bool {
        self.external.contains(&x)
    }

    /// Whether `x` is a holder ([`Self::for_routine`] only).
    #[must_use]
    pub fn is_holder(&self, x: HolderKey) -> bool {
        self.holders.contains(&x)
    }

    /// Joins every key of `keys` into one class.
    fn join(&mut self, keys: &[HolderKey]) {
        let Some((&first, rest)) = keys.split_first() else {
            return;
        };
        for &n in rest {
            let (ra, rb) = (self.root(first), self.root(n));
            self.parent.entry(first).or_insert(first);
            self.parent.entry(n).or_insert(n);
            if ra != rb {
                self.parent.insert(ra, rb);
            }
        }
    }

    /// The classes over the holders `tracked` accepts, from the whole-variable copies among
    /// `assignments` (each side resolved by `scope` from its source text, ENG-176).
    #[must_use]
    pub fn of(
        scope: HolderScope<'_>,
        assignments: &[PVarAssignment],
        tracked: impl Fn(HolderKey) -> bool,
    ) -> Self {
        let mut c = Self::default();
        for a in assignments {
            let Some(l) = whole_variable(scope, &a.lhs_text) else {
                continue;
            };
            let Some(w) = a.rhs_text.as_deref().and_then(|w| whole_variable(scope, w)) else {
                continue;
            };
            if !tracked(l) || !tracked(w) {
                continue;
            }
            let (rl, rw) = (c.root(l), c.root(w));
            c.parent.entry(l).or_insert(l);
            c.parent.entry(w).or_insert(w);
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

    /// The class representative of `x`.
    #[must_use]
    pub fn root(&self, x: HolderKey) -> HolderKey {
        let mut cur = x;
        while let Some(&p) = self.parent.get(&cur) {
            if p == cur {
                break;
            }
            cur = p;
        }
        cur
    }

    /// Every holder sharing a record with `x` (`x` included), in key order.
    #[must_use]
    pub fn members(&self, x: HolderKey) -> Vec<HolderKey> {
        let r = self.root(x);
        let mut out: Vec<HolderKey> = self
            .parent
            .keys()
            .copied()
            .filter(|&n| self.root(n) == r)
            .collect();
        if out.is_empty() {
            out.push(x);
        }
        out.sort();
        out
    }

    /// Whether `x` shares its record with another variable.
    #[must_use]
    pub fn has_alias(&self, x: HolderKey) -> bool {
        self.members(x).len() > 1
    }

    /// Every holder in an alias class of two or more.
    pub fn names(&self) -> impl Iterator<Item = HolderKey> + '_ {
        self.parent.keys().copied()
    }
}

/// ENG-176: the whole variable source text names (`None` for an element `X[1]`, a member, an
/// expression or a name `scope` does not declare).
fn whole_variable(scope: HolderScope<'_>, text: &str) -> Option<HolderKey> {
    scope.text(text).filter(|h| !h.indexed).map(|h| h.key)
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
            lhs_text: l.to_string(),
            rhs_text: r.map(str::to_string),
            ternary_leaf: false,
        }
    }

    #[test]
    fn classes_join_through_chains_and_ignore_untracked_and_expressions() {
        let names = ["ref", "a", "b", "c", "other", "d", "e", "notrecref"];
        let own: Vec<crate::program::model::workspace::ModelVariable> = names
            .iter()
            .map(|n| crate::program::model::workspace::ModelVariable {
                name: (*n).to_string(),
                declared_type: "RecordRef".to_string(),
                is_parameter: false,
                parameter_index: None,
                initializer: None,
                scope: Some("local".to_string()),
                fixed_local: None,
            })
            .collect();
        let scope = HolderScope::new(&own, &[]);
        let k = |n: &str| scope.logical(n).expect("declared");
        let rows = [
            assign("A", Some("ref"), 1),
            assign("b", Some("a"), 2),
            assign("C", Some("other"), 3),
            assign("D", None, 4),
            assign("e", Some("notrecref"), 5),
        ];
        let untracked = k("notrecref");
        let c = AliasClasses::of(scope, &rows, |x| x != untracked);
        assert_eq!(c.members(k("ref")), [k("ref"), k("a"), k("b")]);
        assert_eq!(c.members(k("c")), [k("c"), k("other")]);
        assert!(!c.has_alias(k("d")), "an expression copy is no alias");
        assert!(!c.has_alias(k("e")), "an untracked source is no alias");
        assert_eq!(c.assignments.len(), 3);
    }
}
