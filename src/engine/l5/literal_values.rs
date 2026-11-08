//! r3 E1: Integer, Option and Enum values in the literal reach (n10).
//!
//! Continia DO's `DocSave` passes `OutputAction::SaveAsPDF` into `RunReport`'s
//! Integer parameter `OutputActionInt`, which `RunReportID` reads as `case
//! OutputActionInt of OutputAction::Print: … OutputAction::SaveAsPDF: …`, the
//! labels being members of a global `Option` variable. Only the `SaveAsPDF`
//! arm can run from `DocSave`, so the effects only the other arms reach are
//! not `DocSave`'s.
//!
//! **Value domain.** A bound parameter holds a finite set of concrete values
//! ([`VSet`]); a parameter with no entry in the bindings is UNKNOWN. Each
//! search state is one route's bindings, so an unknown argument on one route
//! never inherits another route's literal, and an effect is ruled out only when
//! no reached state lets it run (the reach's own rule).
//!
//! **What binds a parameter.** A parameter is USABLE ([`value_frame`]) when it
//! is by value, of type Integer, BigInteger, Option or Enum, never assigned
//! (compound assignment included), never a `for`/`foreach` variable, and every
//! call it is passed to is a known routine taking it by value, or a platform
//! method other than `Evaluate` and `Clear` (a call the resolver did not
//! resolve makes it unusable). A call binds it when its argument is an integer
//! literal, a label ([`resolve_label`]), or the caller's own bound parameter.
//!
//! **Comparison.** Values are kept in the domain of the parameter
//! ([`Domain`]): an Integer or Option parameter holds ordinals (an Option
//! member's position, blank members counted); an Enum parameter holds (enum
//! name, value name). An Enum label against an Integer or Option parameter, an
//! Integer against an Enum parameter, or a label of another enum, is a
//! conversion this module does not model: the label is unresolved.
//!
//! **Where it prunes.** `case P of`, `if P = L`, `if P <> L`, `if P in [..]`
//! (and an `if … then exit;` of those for what follows it): an arm is dead
//! only when every label in it resolved and none matches; one unresolved label
//! keeps its arm AND the `else` live; the `else` is dead only when every bound
//! value matches a resolved label of some arm. A `case` with no `else` guards
//! nothing after it.

use super::Bindings;
use crate::engine::l4::param_guard::is_exit_only;
use crate::program::body::features::{PCFNNode, ValueFact, ValueLabel};
use crate::program::model::workspace::{ModelRoutine, ModelTable};
use std::collections::HashMap;
use std::sync::Arc;

/// One concrete value, in the domain of the parameter it binds or tests.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) enum Val {
    /// An Integer's value, or an Option's ordinal (blank members counted).
    Int(i64),
    /// An Enum value: (enum name, value name), both folded. A value name
    /// identifies the value within its enum and every extension of it, so a
    /// value is compared by name; the IR carries no value numbers.
    Enum(Arc<str>, Arc<str>),
    /// r3 E2: a Boolean (the within-routine flow's values only; no E1 frame
    /// binds one).
    Bool(bool),
    /// Round-4 literal routes: the static type a `Variant` holds, as the
    /// type keyword folded (`record`, `recordref`, `codeunit`, …; see
    /// [`variant_test`]). The within-routine flow's values only.
    Type(Arc<str>),
}

/// The static types whose `Variant` type tests [`variant_test`] decides.
/// Others (Text and Code, whose tests overlap; numbers; an enum or option
/// value) stay unknown.
const DECIDED_TYPES: &[&str] = &[
    "record",
    "recordref",
    "recordid",
    "fieldref",
    "codeunit",
    "guid",
    "boolean",
    "date",
    "time",
    "datetime",
];

/// The static type a variable of declared type `type_text` gives a `Variant`
/// it is passed to ([`Val::Type`]): its type keyword, folded, when
/// [`variant_test`] decides tests of it; `None` otherwise.
pub(super) fn variant_type_of(type_text: &str) -> Option<Val> {
    variant_type_name(type_text).map(|k| Val::Type(Arc::from(k)))
}

/// [`variant_type_of`]'s keyword (also read by the rules across calls).
pub(crate) fn variant_type_name(type_text: &str) -> Option<&'static str> {
    let t = type_text.trim();
    let kw: String = t
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect::<String>()
        .to_ascii_lowercase();
    // A whole keyword: `Record "X"` and `Record X temporary`, not
    // `RecordRefWrapper`.
    let rest = &t[kw.len()..];
    if !(rest.is_empty() || rest.starts_with(|c: char| c.is_whitespace() || c == '"')) {
        return None;
    }
    DECIDED_TYPES.iter().copied().find(|d| *d == kw.as_str())
}

/// Whether the `Variant` type test `method` (folded, `isrecord`, …) holds
/// for a Variant holding static type `ty` ([`DECIDED_TYPES`]); `None` for a
/// method this module does not know, or a type it does not decide.
pub(crate) fn variant_test(method: &str, ty: &str) -> Option<bool> {
    if !DECIDED_TYPES.contains(&ty) {
        return None;
    }
    let tested = match method {
        "isrecord" => "record",
        "isrecordref" => "recordref",
        "isrecordid" => "recordid",
        "isfieldref" => "fieldref",
        "iscodeunit" => "codeunit",
        "isguid" => "guid",
        "isboolean" => "boolean",
        "isdate" => "date",
        "istime" => "time",
        "isdatetime" => "datetime",
        // A test of a type this module does not track (IsText, IsInteger,
        // IsJsonObject, …) is false for every type it does: each of those
        // holds a value of exactly its own type.
        m if VARIANT_TESTS.contains(&m) => return Some(false),
        _ => return None,
    };
    Some(ty == tested)
}

/// Every `Variant.Is…` method of AL (folded), the ones [`variant_test`]
/// matches by name included.
pub(crate) const VARIANT_TESTS: &[&str] = &[
    "isrecord",
    "isrecordref",
    "isrecordid",
    "isfieldref",
    "iscodeunit",
    "isguid",
    "isboolean",
    "isdate",
    "istime",
    "isdatetime",
    "isaction",
    "isautomation",
    "isbiginteger",
    "isbinary",
    "ischar",
    "isclienttype",
    "iscode",
    "isdateformula",
    "isdecimal",
    "isdefaultlayout",
    "isdictionary",
    "isdotnet",
    "isduration",
    "isexecutionmode",
    "isfile",
    "isfilterpagebuilder",
    "isinstream",
    "isinteger",
    "isjsonarray",
    "isjsonobject",
    "isjsontoken",
    "isjsonvalue",
    "islist",
    "isnotification",
    "isobjecttype",
    "isoption",
    "isoutstream",
    "isreportformat",
    "issecurityfiltering",
    "istableconnectiontype",
    "istestpermissions",
    "istext",
    "istextbuilder",
    "istextconstant",
    "istextencoding",
    "istransactiontype",
    "iswidechar",
    "isxmlattribute",
    "isxmlcdata",
    "isxmlcomment",
    "isxmldeclaration",
    "isxmldocument",
    "isxmldocumenttype",
    "isxmlelement",
    "isxmlnamespacemanager",
    "isxmlnametable",
    "isxmlnode",
    "isxmlnodelist",
    "isxmlprocessinginstruction",
    "isxmlreadoptions",
    "isxmltext",
    "isxmlwriteoptions",
];

/// An `Interface "I"` (or `Interface Ns."I"`) type's interface name, folded
/// (last segment).
pub(super) fn interface_name(type_text: &str) -> Option<Arc<str>> {
    let rest = after_keyword(type_text, "interface")?;
    let last = split_outside_quotes(rest, '.').pop()?;
    let name = fold(last);
    (!name.is_empty()).then(|| Arc::from(name))
}

/// A finite set of values, sorted and distinct.
pub(super) type VSet = Arc<[Val]>;

/// The domain a parameter's values are compared in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Domain {
    /// Integer, BigInteger or Option: ordinals.
    Int,
    /// Enum of this name (folded, last namespace segment).
    Enum(Arc<str>),
}

/// A value requirement in one routine's frame: (parameter index, whether the
/// value must be IN the set (else NOT in it), the set).
pub(super) type ValueReq = (u32, bool, VSet);

/// One routine's value frame: its usable value parameters and, per operation
/// or call-site id, the requirements for it to run.
#[derive(Debug, Default)]
pub(super) struct ValueFrame {
    pub usable: Vec<(u32, Domain)>,
    pub by_site: HashMap<String, Vec<ValueReq>>,
}

pub(super) fn fold(s: &str) -> String {
    let t = s.trim();
    let inner = t
        .strip_prefix('"')
        .and_then(|x| x.strip_suffix('"'))
        .map(|x| x.replace("\"\"", "\""))
        .unwrap_or_else(|| t.to_string());
    inner.trim().to_lowercase()
}

/// Split `s` at `sep` outside double quotes.
fn split_outside_quotes(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_quotes = false;
    for (i, c) in s.char_indices() {
        if c == '"' {
            in_quotes = !in_quotes;
        } else if c == sep && !in_quotes {
            out.push(&s[start..i]);
            start = i + c.len_utf8();
        }
    }
    out.push(&s[start..]);
    out
}

/// The text after the type keyword `kw` (case-insensitive), when `t` starts
/// with it as a whole word.
fn after_keyword<'t>(t: &'t str, kw: &str) -> Option<&'t str> {
    let t = t.trim();
    let head = t.get(..kw.len())?;
    if !head.eq_ignore_ascii_case(kw) {
        return None;
    }
    let rest = &t[kw.len()..];
    match rest.chars().next() {
        None => Some(""),
        Some(c) if c.is_whitespace() || c == '"' => Some(rest.trim()),
        _ => None,
    }
}

/// An `Option a,b,,c` type's members, folded, blank ones kept in place.
/// `None` for any other type.
pub(super) fn option_members(type_text: &str) -> Option<Vec<String>> {
    let rest = after_keyword(type_text, "option")?;
    if rest.is_empty() {
        return Some(Vec::new());
    }
    Some(
        split_outside_quotes(rest, ',')
            .into_iter()
            .map(fold)
            .collect(),
    )
}

/// An `Enum "X"` (or `Enum Ns."X"`) type's enum name, folded (last segment).
pub(super) fn enum_name(type_text: &str) -> Option<Arc<str>> {
    let rest = after_keyword(type_text, "enum")?;
    let last = split_outside_quotes(rest, '.').pop()?;
    let name = fold(last);
    (!name.is_empty()).then(|| Arc::from(name))
}

/// The domain of a parameter of type `type_text`; `None` when the type is
/// none of Integer, BigInteger, Option or Enum.
pub(super) fn domain_of(type_text: &str) -> Option<Domain> {
    let t = type_text.trim();
    if t.eq_ignore_ascii_case("integer") || t.eq_ignore_ascii_case("biginteger") {
        return Some(Domain::Int);
    }
    if option_members(t).is_some() {
        return Some(Domain::Int);
    }
    enum_name(t).map(Domain::Enum)
}

/// `v` in domain `d` after AL's conversion, when this module models it.
pub(super) fn in_domain(v: &Val, d: &Domain) -> Option<Val> {
    match (v, d) {
        (Val::Int(n), Domain::Int) => Some(Val::Int(*n)),
        (Val::Enum(e, m), Domain::Enum(want)) if e == want => {
            Some(Val::Enum(Arc::clone(e), Arc::clone(m)))
        }
        _ => None,
    }
}

/// A label `V::M` on a variable or field of type `type_text`: the Option
/// member's ordinal, or the Enum value.
fn typed_label(type_text: &str, member: &str) -> Option<Val> {
    if let Some(ms) = option_members(type_text) {
        return ms
            .iter()
            .position(|m| m == member)
            .map(|i| Val::Int(i64::try_from(i).expect("an option ordinal fits i64")));
    }
    enum_name(type_text).map(|e| Val::Enum(e, Arc::from(member)))
}

/// What a label means in routine `r`'s scope (`tables`: the model's tables
/// by id, a real table preferred over an extension stub):
/// - an integer literal is itself;
/// - `Enum::"X"::M` is enum `X`'s value `M`;
/// - `V::M`, `V` an Option- or Enum-typed parameter, local or global of `r`,
///   is `M`'s ordinal (blank members counted) or the enum value; with no
///   variable `V`, a field `V` of the implicit `Rec`, as below;
/// - `R.F::M`, `R` a record variable of known table whose field `F` is an
///   Enum (or an Option whose members the model knows), likewise.
///
/// `None` when the label is not one of these or does not resolve (an Option
/// member the type does not list, a field the table does not have).
pub(super) fn resolve_label(
    r: &ModelRoutine,
    tables: &HashMap<&str, &ModelTable>,
    l: &ValueLabel,
) -> Option<Val> {
    let (path, member) = match l {
        ValueLabel::Int(n) => return Some(Val::Int(*n)),
        ValueLabel::Qualified { path, member } => (path, member.as_str()),
    };
    if path.len() >= 2 && path[0] == "enum" {
        let e = path.last()?;
        return Some(Val::Enum(Arc::from(e.as_str()), Arc::from(member)));
    }
    let field_of = |table_id: Option<&str>, field: &str| -> Option<Val> {
        let t = tables.get(table_id?)?;
        let f = t.fields.iter().find(|f| fold(&f.name) == field)?;
        typed_label(&f.data_type, member)
    };
    match path.as_slice() {
        [v] => {
            if let Some(var) = r.variables.iter().find(|x| fold(&x.name) == *v) {
                return typed_label(&var.declared_type, member);
            }
            // Round-4 literal routes: `"Enum Name"::Value` (no `Enum::`) names
            // the enum's value when no variable and no field of the implicit
            // `Rec` has that name (a Rec whose table is not loaded could have
            // such a field: then nothing is concluded).
            let enum_value = || Some(Val::Enum(Arc::from(v.as_str()), Arc::from(member)));
            let Some(rec) = r.record_variables.iter().find(|x| x.is_implicit_rec) else {
                return enum_value();
            };
            let table = tables.get(rec.table_id.as_deref()?)?;
            if table.fields.iter().any(|f| fold(&f.name) == *v) {
                return field_of(rec.table_id.as_deref(), v);
            }
            enum_value()
        }
        [rv, f] => {
            let rec = r.record_variables.iter().find(|x| fold(&x.name) == *rv)?;
            field_of(rec.table_id.as_deref(), f)
        }
        _ => None,
    }
}

/// The labels in domain `d`, sorted and distinct; `None` when one does not
/// resolve or converts to no value of `d`.
fn label_set(
    labels: &[Option<ValueLabel>],
    d: &Domain,
    resolve: &dyn Fn(&ValueLabel) -> Option<Val>,
) -> Option<Vec<Val>> {
    let mut out = labels
        .iter()
        .map(|l| l.as_ref().and_then(resolve).and_then(|v| in_domain(&v, d)))
        .collect::<Option<Vec<Val>>>()?;
    out.sort();
    out.dedup();
    Some(out)
}

/// Whether no value of `held` meets a requirement that it be (or not be) in
/// `set` (both sorted).
pub(super) fn violates(held: &[Val], must_be_in: bool, set: &[Val]) -> bool {
    let in_set = |v: &Val| set.binary_search(v).is_ok();
    if must_be_in {
        !held.iter().any(in_set)
    } else {
        held.iter().all(in_set)
    }
}

pub(super) fn contradicts(reqs: &[ValueReq], b: &Bindings) -> bool {
    reqs.iter().any(|(i, must_be_in, set)| {
        b.values
            .iter()
            .any(|(j, held)| j == i && violates(held, *must_be_in, set))
    })
}

/// The loop variables of `for` / `foreach` statements in `node`'s subtree.
fn loop_vars(node: &PCFNNode, out: &mut Vec<String>) {
    if let Some(ValueFact::LoopVar(v)) = &node.value_fact {
        out.push(v.clone());
    }
    for c in node
        .condition_leaves
        .iter()
        .chain(&node.children)
        .chain(&node.else_children)
        .flatten()
    {
        loop_vars(c, out);
    }
}

/// `r`'s value frame (module doc). `passes_by_value(callsite_id, argument
/// index)` says whether the call passes that argument by value to a known
/// callee or to a platform method that does not write it; `resolve` reads a
/// label in `r`'s scope. `None` when `r` has no usable value parameter.
pub(super) fn value_frame(
    r: &ModelRoutine,
    passes_by_value: impl Fn(&str, u32) -> bool,
    resolve: &dyn Fn(&ValueLabel) -> Option<Val>,
) -> Option<ValueFrame> {
    let tree = r.statement_tree.as_ref()?;
    let mut loops = Vec::new();
    loop_vars(tree, &mut loops);
    let mut usable: HashMap<String, (u32, Domain)> = HashMap::new();
    for p in &r.parameters {
        if p.is_var {
            continue;
        }
        let Some(d) = domain_of(&p.type_text) else {
            continue;
        };
        let name = fold(&p.name);
        if r.var_assignments.iter().any(|a| fold(&a.lhs_name) == name) || loops.contains(&name) {
            continue;
        }
        let passed_unsafely = r.call_sites.iter().any(|cs| {
            cs.argument_bindings.iter().any(|b| {
                b.source_kind == "parameter"
                    && b.source_parameter_index == Some(p.index)
                    && !passes_by_value(&cs.id, b.parameter_index)
            })
        });
        if passed_unsafely {
            continue;
        }
        usable.insert(name, (p.index, d));
    }
    if usable.is_empty() {
        return None;
    }
    let mut by_site: HashMap<String, Vec<ValueReq>> = HashMap::new();
    let cx = ValueWalk {
        usable: &usable,
        resolve,
    };
    walk(tree, &[], &cx, &mut by_site);
    crate::engine::l4::param_guard::alias_error_ops(r, &mut by_site);
    let mut usable: Vec<(u32, Domain)> = usable.into_values().collect();
    usable.sort_by_key(|(i, _)| *i);
    Some(ValueFrame { usable, by_site })
}

struct ValueWalk<'a> {
    usable: &'a HashMap<String, (u32, Domain)>,
    resolve: &'a dyn Fn(&ValueLabel) -> Option<Val>,
}

/// An `if` node's requirements for its THEN branch and for its ELSE branch
/// (also what follows an `if … then exit;`): both empty unless its condition
/// is a [`ValueFact::Test`] of a usable parameter whose every label resolves.
fn if_reqs(node: &PCFNNode, cx: &ValueWalk) -> (Vec<ValueReq>, Vec<ValueReq>) {
    if node.kind != "if" {
        return (Vec::new(), Vec::new());
    }
    let Some(ValueFact::Test {
        name,
        equal,
        labels,
    }) = &node.value_fact
    else {
        return (Vec::new(), Vec::new());
    };
    let Some((p, d)) = cx.usable.get(name) else {
        return (Vec::new(), Vec::new());
    };
    match label_set(labels, d, cx.resolve) {
        Some(set) => {
            let set: VSet = set.into();
            (
                vec![(*p, *equal, Arc::clone(&set))],
                vec![(*p, !*equal, set)],
            )
        }
        None => (Vec::new(), Vec::new()),
    }
}

fn with(reqs: &[ValueReq], extra: &[ValueReq]) -> Vec<ValueReq> {
    let mut r = reqs.to_vec();
    r.extend(extra.iter().cloned());
    r
}

fn walk_list(
    nodes: &[PCFNNode],
    reqs: &[ValueReq],
    cx: &ValueWalk,
    by_site: &mut HashMap<String, Vec<ValueReq>>,
) {
    let mut reqs = reqs.to_vec();
    for n in nodes {
        walk(n, &reqs, cx, by_site);
        // `if <test> then exit;` (no else): what follows runs only when the
        // test fails.
        if n.kind == "if" && n.else_children.is_none() && is_exit_only(n.children.as_ref()) {
            reqs.extend(if_reqs(n, cx).1);
        }
    }
}

fn walk(
    node: &PCFNNode,
    reqs: &[ValueReq],
    cx: &ValueWalk,
    by_site: &mut HashMap<String, Vec<ValueReq>>,
) {
    if !reqs.is_empty() {
        for id in [&node.operation_id, &node.callsite_id]
            .into_iter()
            .flatten()
        {
            by_site.insert(id.clone(), reqs.to_vec());
        }
    }
    // Calls and operations in a condition run before a branch is chosen.
    for l in node.condition_leaves.iter().flatten() {
        walk(l, reqs, cx, by_site);
    }
    let on = match &node.value_fact {
        Some(ValueFact::CaseOn(name)) if node.kind == "case" => cx.usable.get(name),
        _ => None,
    };
    if let Some((p, d)) = on {
        let branches: Vec<&PCFNNode> = node.children.iter().flatten().collect();
        // Each non-else arm's label set; `None` when one label is unresolved.
        let arms: Vec<Option<Vec<Val>>> = branches
            .iter()
            .filter(|b| !b.is_case_else)
            .map(|b| match &b.value_fact {
                Some(ValueFact::Arm(labels)) => label_set(labels, d, cx.resolve),
                _ => None,
            })
            .collect();
        // The `else` is dead only when every bound value matches a resolved
        // label of some arm: with an unresolved label anywhere, never.
        let listed: Option<VSet> =
            arms.iter()
                .cloned()
                .collect::<Option<Vec<Vec<Val>>>>()
                .map(|sets| {
                    let mut all: Vec<Val> = sets.into_iter().flatten().collect();
                    all.sort();
                    all.dedup();
                    all.into()
                });
        let mut arm_sets = arms.into_iter();
        for b in branches {
            let extra: Option<ValueReq> = if b.is_case_else {
                listed.as_ref().map(|l| (*p, false, Arc::clone(l)))
            } else {
                arm_sets.next().flatten().map(|s| (*p, true, s.into()))
            };
            walk(b, &with(reqs, extra.as_slice()), cx, by_site);
        }
    } else {
        let (then, otherwise) = if_reqs(node, cx);
        if let Some(children) = &node.children {
            walk_list(children, &with(reqs, &then), cx, by_site);
        }
        if let Some(children) = &node.else_children {
            walk_list(children, &with(reqs, &otherwise), cx, by_site);
        }
        return;
    }
    for c in node.else_children.iter().flatten() {
        walk(c, reqs, cx, by_site);
    }
}
