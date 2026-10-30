//! Rules across calls (intent-evidence spec
//! `docs/superpowers/specs/2026-10-29-intent-evidence-design.md` §4.3): the
//! rules a routine inherits from the routines it calls, the rules its callers
//! apply before calling it, and the rules of the table triggers its writes run
//! or skip. `alsem explain` reads them; they are computed from the model alone
//! (the routines' own rules, `ModelRoutine::rules`, and the resolved calls).
//!
//! **What "runs before" means here.** Two facts decide whether a callee's rule
//! protects a write of its caller:
//!
//! - the call is `unconditional-on-success` (`conditionality.rs`, the call
//!   site's control context is `top-level`) and not under `asserterror`: it
//!   runs whenever the routine gets past it;
//! - by statement order, the call ends before the routine's first write
//!   starts (both are source anchors of the same routine).
//!
//! The order needs no new fact. A `top-level` site is not inside any branch or
//! loop, so in AL's structured statements every statement after it in the
//! source runs only after it has run, and every statement before it runs
//! before it. Comparing the call's END with the write's START also keeps a
//! write nested in the call's own arguments (`Check(Rec.Insert())`) from
//! counting as "after". This is why `RULES` does not need `ORDERING_FACTS`
//! (spec §4.1b as built): those facts are the IO / commit / UI hazard pairs
//! d47/d49/d51 read, not a statement order.
//!
//! A routine's **writes** are: an assignment to a record field (`R.F := …`,
//! compound forms included), a mutating record operation (`Insert`, `Modify`,
//! `ModifyAll`, `Delete`, `DeleteAll`, `Rename`, `Validate`) — both unless the
//! record is known temporary — and a call that may reach a write: a callee that
//! writes, through any number of resolved calls, or a callee with no body in
//! the model (it may write; the engine cannot see). Its **first write** is the
//! one that starts first in the source.
//!
//! **Inheritance**, for routine `R` and a call step `C` to `T`:
//!
//! - `C` ends before `R`'s first write: every rule `T` runs whenever it runs
//!   (`all(T)`: its own rules, and `all` of each callee it calls
//!   `top-level`) holds before the write;
//! - `C` is itself `R`'s first write (`T` writes): only the rules `T` runs
//!   before ITS first write (`pre(T)`) hold;
//! - otherwise nothing.
//!
//! A call step is a resolved `direct` / `method` call with exactly one target
//! routine. A call to a `[TryFunction]` whose result is used catches the error,
//! so it gives no step. A `Codeunit.Run`, a page or report run, an interface
//! dispatch with more than one implementer and an unresolved call give none
//! either.
//!
//! **Table triggers** (spec §5): a `Validate`, a `Rename`, or an `Insert` /
//! `Modify` / `Delete` with a literal `true` RunTrigger runs the table's
//! trigger (the resolver's `implicit-trigger` edge with `fires: always`). Its
//! rules run as part of that write, so they are inherited with the write
//! named (`TriggerAt`), whatever the write's position. A plain assignment, a
//! `Modify(false)` / `Modify()`, or a non-literal RunTrigger does not (surely)
//! run them: [`CrossRules::skipped_triggers`] lists those triggers' rules.
//!
//! **Parameters across the edge** (`param_guard::across_edge`'s rule, applied
//! to rule atoms): a callee's `ParamBool { i, v }` atom is rewritten with the
//! caller's argument `i`. A contradicting literal drops the rule (it cannot
//! fire through this call), a matching literal drops the atom, a forwarded
//! caller parameter re-anchors it to the caller's `ParamBool`. Any other
//! argument becomes an `Other` atom with the argument's text. Substitution is
//! valid only when the callee's parameter still holds the argument at the
//! check: `param_guard`'s usability (a by-value `Boolean`, never assigned,
//! never passed to a `var` parameter). For a parameter that is not usable the
//! atom becomes an `Other` atom with its text as written in the callee.
//!
//! **Caller rules**: for each known caller (a resolved call edge, or a record
//! operation's trigger edge, into the routine), the rules the caller runs
//! before every one of its calls: its own rules anchored before the call, and
//! `all` of each `top-level` call step that ends before it. A rule is
//! *applied* by a caller only when it always stops (`MayNotStop` rules are
//! listed, never applied).
//!
//! **Bounds.** Cycles are cut: a routine already on the walk's stack adds
//! nothing again (its rules are already being collected). Results are
//! memoised per routine when no cycle was cut beneath them. The whole run has
//! a budget of [`EXPANSION_BUDGET`] routine expansions, and one chain of calls
//! is followed at most [`MAX_DEPTH`] deep (the walk recurses once per call, so
//! this keeps it inside the thread's stack). Past either, the walk stops and
//! [`CrossRules::truncated`] says so (the card states it as a gap). No other
//! cap.
//!
//! **Stated limits.** A call after an early `if C then exit;` is `conditional`,
//! not `top-level`, so its rules are not inherited even though the write after
//! it is also past the exit. A call inside a branch is not inherited even when
//! every write is in the same branch. Writes made by event subscribers are not
//! counted as writes of the publisher's caller. A callee's `FieldCompare` and
//! similar atoms keep the callee's record-variable names (the `via` path says
//! whose). A caller's own rule counts as applied when it is anchored before
//! the call, even when it sits in a branch that does not lead to the call.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use al_syntax::IdentifierFoldExt;

use crate::engine::l4::combined_graph::TriggerFires;
use crate::engine::l4::param_guard::frame_guards_over;
use crate::engine::l5::conditionality::{UNCONDITIONAL, context_to_conditionality};
use crate::program::body::features::{
    FieldAccessKind, PAnchor, PCallSite, known_temp_suppresses, temp_state_suppresses,
};
use crate::program::body::rules::{ConditionAtom, PRule, Stops};
use crate::program::model::calls::CallEdge;
use crate::program::model::taxonomy::{DispatchKind, Resolution};
use crate::program::model::workspace::{
    Model, ModelRecordOperation, ModelRoutine, table_by_id_preferring_real,
};

/// The most routine expansions one run makes (see the module doc, "Bounds").
pub const EXPANSION_BUDGET: usize = 200_000;

/// The longest chain of calls one walk follows (see the module doc,
/// "Bounds"): the walk recurses once per call, so this keeps it inside the
/// thread's stack.
pub const MAX_DEPTH: usize = 512;

/// The record operations that write.
const WRITE_OPS: &[&str] = &[
    "insert",
    "modify",
    "modifyall",
    "delete",
    "deleteall",
    "rename",
    "validate",
];

/// A rule seen from another routine.
#[derive(Debug, Clone)]
pub struct Inh {
    /// The rule, its atoms rewritten into the frame of the routine it is
    /// listed for.
    pub rule: PRule,
    /// The row that holds the check.
    pub owner: usize,
    /// The rows from the first callee to `owner` (owner last); empty for a
    /// rule of the routine itself.
    pub via: Vec<usize>,
    /// For a table trigger's rule: the write that runs the trigger.
    pub trigger: Option<TriggerAt>,
}

/// The write that runs a table trigger.
#[derive(Debug, Clone)]
pub struct TriggerAt {
    /// The record operation as written (`Validate`, `Modify`, ...).
    pub op: String,
    pub anchor: PAnchor,
}

/// A rule a known caller has before its call.
#[derive(Debug, Clone)]
pub struct CallerRule {
    /// The rule, in the frame of the first caller that has it.
    pub inh: Inh,
    /// The callers (rows) that run it before every call they make.
    pub present_in: Vec<usize>,
    /// Those of them for which it always stops.
    pub applied_by: Vec<usize>,
    /// Every other known caller.
    pub skipped_by: Vec<usize>,
}

/// Table-trigger rules a write does not run.
#[derive(Debug, Clone)]
pub struct SkippedTriggers {
    /// `assign`, or the record operation as written.
    pub op: String,
    pub table: String,
    /// For an assignment, the field.
    pub field: Option<String>,
    /// Why the triggers do not (surely) run.
    pub reason: &'static str,
    pub triggers: Vec<usize>,
    pub rules: Vec<Inh>,
    pub anchor: PAnchor,
}

/// One site of a row that runs another routine for sure when it runs.
#[derive(Debug, Clone)]
struct Step<'m> {
    to: usize,
    anchor: &'m PAnchor,
    /// The site runs whenever the routine gets past it.
    top_level: bool,
    /// The call, for a call step (`None` for a trigger step).
    call: Option<&'m PCallSite>,
    /// The record operation, for a trigger step.
    op: Option<&'m ModelRecordOperation>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    All,
    Pre,
}

/// Finished `all` / `pre` results, per (row, kind).
type Memo = HashMap<(usize, Kind), Rc<Vec<Inh>>>;

/// Cross-call rules over one model. Build once per run, query per card.
pub struct CrossRules<'m> {
    model: &'m Model,
    first_row: HashMap<&'m str, usize>,
    /// Per row: its definite steps, in site order.
    steps: HashMap<usize, Vec<Step<'m>>>,
    /// Per row: every site that has an edge (call site id or op id) -> the
    /// target rows.
    site_targets: HashMap<usize, HashMap<&'m str, Vec<usize>>>,
    /// Per row: whether it may write (itself or through its callees).
    may_write: Vec<bool>,
    /// Per row: the callers' edges into it: (caller row, site anchor).
    callers: HashMap<usize, Vec<(usize, &'m PAnchor)>>,
    /// Object id -> its rows.
    rows_of_object: HashMap<&'m str, Vec<usize>>,
    usable: RefCell<HashMap<usize, Vec<u32>>>,
    memo: RefCell<Memo>,
    stack: RefCell<Vec<(usize, Kind)>>,
    expansions: Cell<usize>,
    truncated: Cell<bool>,
}

fn pos_start(a: &PAnchor) -> (u32, u32) {
    (a.start_line, a.start_column)
}

fn pos_end(a: &PAnchor) -> (u32, u32) {
    (a.end_line, a.end_column)
}

/// `a` ends before `b` starts (same routine, so the same file).
fn ends_before(a: &PAnchor, b: &PAnchor) -> bool {
    pos_end(a) <= pos_start(b)
}

fn same_site(a: &PAnchor, b: &PAnchor) -> bool {
    pos_start(a) == pos_start(b) && pos_end(a) == pos_end(b)
}

/// A rule's key: its id and its (rewritten) condition.
fn rule_key(i: &Inh) -> (String, Vec<String>) {
    (i.rule.id.clone(), i.rule.when_texts.clone())
}

/// Keep, per rule key, the entry with the shortest `via` (then the smallest).
fn dedup(v: &mut Vec<Inh>) {
    let mut best: BTreeMap<(String, Vec<String>), usize> = BTreeMap::new();
    for (i, x) in v.iter().enumerate() {
        match best.entry(rule_key(x)) {
            std::collections::btree_map::Entry::Vacant(e) => {
                e.insert(i);
            }
            std::collections::btree_map::Entry::Occupied(mut e) => {
                let cur = &v[*e.get()];
                if (x.via.len(), &x.via) < (cur.via.len(), &cur.via) {
                    e.insert(i);
                }
            }
        }
    }
    let keep: HashSet<usize> = best.into_values().collect();
    let mut i = 0;
    v.retain(|_| {
        let k = keep.contains(&i);
        i += 1;
        k
    });
}

/// `not X` / `not (X)` for an argument text.
fn not_text(t: &str) -> String {
    let simple = t
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '"' || c == '.');
    if simple {
        format!("not {t}")
    } else {
        format!("not ({t})")
    }
}

impl<'m> CrossRules<'m> {
    #[must_use]
    pub fn new(model: &'m Model) -> CrossRules<'m> {
        let routines = &model.workspace.routines;
        let mut first_row: HashMap<&str, usize> = HashMap::new();
        let mut rows_of_object: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, r) in routines.iter().enumerate() {
            first_row.entry(r.id.as_str()).or_insert(i);
            rows_of_object
                .entry(r.object_id.as_str())
                .or_default()
                .push(i);
        }
        // Edges grouped by (row, site).
        let mut groups: BTreeMap<(usize, &str), Vec<&CallEdge>> = BTreeMap::new();
        for e in &model.calls.edges {
            groups
                .entry((e.from_ix.get(), e.callsite_id.as_str()))
                .or_default()
                .push(e);
        }
        let mut steps: HashMap<usize, Vec<Step<'_>>> = HashMap::new();
        let mut site_targets: HashMap<usize, HashMap<&str, Vec<usize>>> = HashMap::new();
        let mut callers: HashMap<usize, Vec<(usize, &PAnchor)>> = HashMap::new();
        for ((from, site), edges) in &groups {
            let from = *from;
            let r = &routines[from];
            let tos: Vec<usize> = {
                let mut t: Vec<usize> = edges
                    .iter()
                    .filter_map(|e| e.to.as_deref().and_then(|to| first_row.get(to)).copied())
                    .collect();
                t.sort_unstable();
                t.dedup();
                t
            };
            site_targets
                .entry(from)
                .or_default()
                .insert(*site, tos.clone());
            let trigger = edges
                .iter()
                .any(|e| e.dispatch_kind == DispatchKind::ImplicitTrigger);
            if trigger {
                let Some(op) = r.record_operations.iter().find(|o| o.id == *site) else {
                    continue;
                };
                let os = r.operation_sites.iter().find(|o| o.id == *site);
                let context = os.and_then(|o| o.control_context.as_deref());
                let reachable = context != Some("unreachable");
                let under = os.is_some_and(|o| o.under_asserterror == Some(true));
                for &to in &tos {
                    callers
                        .entry(to)
                        .or_default()
                        .push((from, &op.source_anchor));
                }
                if !reachable || under {
                    continue;
                }
                let top_level = context_to_conditionality(context) == UNCONDITIONAL;
                for e in edges {
                    if e.fires != Some(TriggerFires::Always) {
                        continue;
                    }
                    let Some(&to) = e.to.as_deref().and_then(|t| first_row.get(t)) else {
                        continue;
                    };
                    steps.entry(from).or_default().push(Step {
                        to,
                        anchor: &op.source_anchor,
                        top_level,
                        call: None,
                        op: Some(op),
                    });
                }
                continue;
            }
            let Some(cs) = r.call_sites.iter().find(|c| c.id == *site) else {
                continue;
            };
            for &to in &tos {
                callers
                    .entry(to)
                    .or_default()
                    .push((from, &cs.source_anchor));
            }
            let definite = edges.iter().all(|e| {
                matches!(e.dispatch_kind, DispatchKind::Direct | DispatchKind::Method)
                    && e.resolution == Resolution::Resolved
                    && e.to.is_some()
            });
            if !definite || tos.len() != 1 {
                continue;
            }
            let to = tos[0];
            let callee = &routines[to];
            let try_function = callee
                .attributes_parsed
                .iter()
                .any(|a| a.name.eq_ignore_ascii_case("TryFunction"));
            if try_function && cs.result_consumed == Some(true) {
                // A used TryFunction result catches the error.
                continue;
            }
            let top_level = context_to_conditionality(cs.control_context.as_deref())
                == UNCONDITIONAL
                && cs.under_asserterror != Some(true);
            steps.entry(from).or_default().push(Step {
                to,
                anchor: &cs.source_anchor,
                top_level,
                call: Some(cs),
                op: None,
            });
        }
        for v in steps.values_mut() {
            v.sort_by_key(|s| (pos_start(s.anchor), s.to));
        }
        // May-write: own writes, or no body (cannot see), then up the callers.
        let mut may_write: Vec<bool> = routines
            .iter()
            .map(|r| !r.body_available || !own_writes(r).is_empty())
            .collect();
        let mut queue: Vec<usize> = (0..routines.len()).filter(|&i| may_write[i]).collect();
        while let Some(n) = queue.pop() {
            for &(c, _) in callers.get(&n).map(Vec::as_slice).unwrap_or(&[]) {
                if !may_write[c] {
                    may_write[c] = true;
                    queue.push(c);
                }
            }
        }
        CrossRules {
            model,
            first_row,
            steps,
            site_targets,
            may_write,
            callers,
            rows_of_object,
            usable: RefCell::new(HashMap::new()),
            memo: RefCell::new(HashMap::new()),
            stack: RefCell::new(Vec::new()),
            expansions: Cell::new(0),
            truncated: Cell::new(false),
        }
    }

    fn row(&self, i: usize) -> &'m ModelRoutine {
        &self.model.workspace.routines[i]
    }

    /// Whether the walk stopped at [`EXPANSION_BUDGET`].
    #[must_use]
    pub fn truncated(&self) -> bool {
        self.truncated.get()
    }

    /// The anchor of `row`'s first write, `None` when it writes nothing.
    #[must_use]
    pub fn first_write(&self, row: usize) -> Option<&'m PAnchor> {
        let r = self.row(row);
        let mut best: Option<&PAnchor> = own_writes(r).into_iter().min_by_key(|a| pos_start(a));
        let sites = self.site_targets.get(&row);
        for cs in &r.call_sites {
            let writes = sites
                .and_then(|s| s.get(cs.id.as_str()))
                .is_some_and(|tos| tos.iter().any(|&t| self.may_write[t]));
            if writes && best.is_none_or(|b| pos_start(&cs.source_anchor) < pos_start(b)) {
                best = Some(&cs.source_anchor);
            }
        }
        best
    }

    /// The usable guard parameters of `row` (`param_guard`'s rule).
    fn usable(&self, row: usize) -> Vec<u32> {
        if let Some(u) = self.usable.borrow().get(&row) {
            return u.clone();
        }
        let r = self.row(row);
        let routines = &self.model.workspace.routines;
        let targets = self.site_targets.get(&row);
        let by_id: HashMap<&str, &ModelRoutine> = targets
            .into_iter()
            .flat_map(|m| m.values())
            .flatten()
            .map(|&t| (routines[t].id.as_str(), &routines[t]))
            .collect();
        let callees_at = |cs: &str| -> Vec<&str> {
            targets
                .and_then(|m| m.get(cs))
                .map(|tos| tos.iter().map(|&t| routines[t].id.as_str()).collect())
                .unwrap_or_default()
        };
        let u = frame_guards_over(r, callees_at, &by_id)
            .map(|f| f.usable)
            .unwrap_or_default();
        self.usable.borrow_mut().insert(row, u.clone());
        u
    }

    /// `rule`, a rule in `callee`'s frame, seen from `caller` across `step`.
    /// `None`: an argument contradicts it, so it cannot fire through this call.
    fn across(&self, rule: &PRule, caller: usize, step: &Step<'_>) -> Option<PRule> {
        let mut when = Vec::with_capacity(rule.when.len());
        let mut texts = Vec::with_capacity(rule.when.len());
        let callee_usable = self.usable(step.to);
        for (atom, text) in rule.when.iter().zip(&rule.when_texts) {
            let ConditionAtom::ParamBool { index, value } = atom else {
                when.push(atom.clone());
                texts.push(text.clone());
                continue;
            };
            let (index, value) = (*index, *value);
            let as_written = || ConditionAtom::Other {
                text: text.clone(),
                anchor: rule.anchor.clone(),
            };
            let Some(cs) = step.call.filter(|_| callee_usable.contains(&index)) else {
                // Assigned (or passed to a `var`) in the callee, or a trigger:
                // the argument does not decide it.
                when.push(as_written());
                texts.push(text.clone());
                continue;
            };
            let arg = cs.argument_texts.get(index as usize).map(|t| t.trim());
            match arg.map(str::to_ascii_lowercase).as_deref() {
                Some("true") | Some("false") => {
                    if (arg.map(str::to_ascii_lowercase).as_deref() == Some("true")) != value {
                        return None;
                    }
                    // A matching literal: the atom holds.
                }
                Some(_) => {
                    let forwarded = cs
                        .argument_bindings
                        .iter()
                        .find(|b| b.parameter_index == index && b.source_kind == "parameter")
                        .and_then(|b| b.source_parameter_index)
                        .filter(|j| self.usable(caller).contains(j));
                    let arg = arg.unwrap_or_default();
                    match forwarded {
                        Some(j) => {
                            let name = self
                                .row(caller)
                                .parameters
                                .iter()
                                .find(|p| p.index == j)
                                .map_or_else(|| arg.to_string(), |p| p.name.clone());
                            when.push(ConditionAtom::ParamBool { index: j, value });
                            texts.push(if value { name } else { not_text(&name) });
                        }
                        None => {
                            let t = if value {
                                arg.to_string()
                            } else {
                                not_text(arg)
                            };
                            when.push(ConditionAtom::Other {
                                text: t.clone(),
                                anchor: cs.source_anchor.clone(),
                            });
                            texts.push(t);
                        }
                    }
                }
                None => {
                    when.push(as_written());
                    texts.push(text.clone());
                }
            }
        }
        Some(PRule {
            when_text: texts.join(" and "),
            when_texts: texts,
            when,
            ..rule.clone()
        })
    }

    /// `T`'s results seen from `caller` across `step`: rewritten, `via`
    /// prefixed with `T`.
    fn lift(&self, caller: usize, step: &Step<'_>, sub: &[Inh], out: &mut Vec<Inh>) {
        for i in sub {
            if let Some(rule) = self.across(&i.rule, caller, step) {
                let mut via = Vec::with_capacity(i.via.len() + 1);
                via.push(step.to);
                via.extend_from_slice(&i.via);
                out.push(Inh {
                    rule,
                    owner: i.owner,
                    via,
                    trigger: None,
                });
            }
        }
    }

    fn own(&self, row: usize) -> Vec<Inh> {
        self.row(row)
            .rules()
            .iter()
            .map(|r| Inh {
                rule: r.clone(),
                owner: row,
                via: Vec::new(),
                trigger: None,
            })
            .collect()
    }

    /// `all` / `pre` of `row`, memoised; `low` gets the lowest stack position
    /// a cut reached.
    fn compute(&self, row: usize, kind: Kind, low: &mut usize) -> Rc<Vec<Inh>> {
        if let Some(r) = self.memo.borrow().get(&(row, kind)) {
            return r.clone();
        }
        if let Some(p) = self.stack.borrow().iter().position(|&x| x == (row, kind)) {
            *low = (*low).min(p);
            return Rc::new(Vec::new());
        }
        if self.stack.borrow().len() >= MAX_DEPTH {
            // Too deep a chain of calls: stop, say so, and keep every
            // routine on the stack from memoising an incomplete result.
            self.truncated.set(true);
            *low = 0;
            return Rc::new(Vec::new());
        }
        if self.expansions.get() >= EXPANSION_BUDGET {
            self.truncated.set(true);
            return Rc::new(Vec::new());
        }
        self.expansions.set(self.expansions.get() + 1);
        let me = self.stack.borrow().len();
        self.stack.borrow_mut().push((row, kind));
        let mut my_low = usize::MAX;
        let fw = match kind {
            Kind::All => None,
            Kind::Pre => self.first_write(row),
        };
        let mut out: Vec<Inh> = self
            .own(row)
            .into_iter()
            .filter(|i| fw.is_none_or(|w| ends_before(&i.rule.anchor, w)))
            .collect();
        let steps = self.steps.get(&row).cloned().unwrap_or_default();
        for s in steps.iter().filter(|s| s.top_level) {
            let sub_kind = match fw {
                None => Some(Kind::All),
                Some(w) if s.op.is_some() => {
                    (pos_start(s.anchor) <= pos_start(w)).then_some(Kind::All)
                }
                Some(w) if ends_before(s.anchor, w) => Some(Kind::All),
                Some(w) if same_site(s.anchor, w) => Some(Kind::Pre),
                Some(_) => None,
            };
            let Some(sub_kind) = sub_kind else { continue };
            let sub = self.compute(s.to, sub_kind, &mut my_low);
            self.lift(row, s, &sub, &mut out);
        }
        dedup(&mut out);
        self.stack.borrow_mut().pop();
        let out = Rc::new(out);
        if my_low >= me {
            self.memo.borrow_mut().insert((row, kind), out.clone());
        } else {
            *low = (*low).min(my_low);
        }
        out
    }

    fn all(&self, row: usize) -> Rc<Vec<Inh>> {
        let mut low = usize::MAX;
        self.compute(row, Kind::All, &mut low)
    }

    /// Every rule `row` runs whenever it runs: its own and its `top-level`
    /// callees' (spec §5's subscriber rules use this).
    #[must_use]
    pub fn rules_run_by(&self, row: usize) -> Vec<Inh> {
        self.all(row).as_ref().clone()
    }

    /// The rules `rows` (one routine's rows) inherit: their callees' rules that
    /// run before the first write, and the table-trigger rules each write runs
    /// (with the write named). Sorted by the check's anchor.
    #[must_use]
    pub fn inherited(&self, rows: &[usize]) -> Vec<Inh> {
        let mut out = Vec::new();
        for &row in rows {
            // The routine itself is on the stack: a cycle back to it adds
            // nothing (its rules are its own).
            self.stack.borrow_mut().push((row, Kind::All));
            self.stack.borrow_mut().push((row, Kind::Pre));
            let fw = self.first_write(row);
            let steps = self.steps.get(&row).cloned().unwrap_or_default();
            for s in &steps {
                let mut low = usize::MAX;
                if let Some(op) = s.op {
                    // A trigger runs as part of its write, wherever it is.
                    let sub = self.compute(s.to, Kind::All, &mut low);
                    let start = out.len();
                    self.lift(row, s, &sub, &mut out);
                    for i in &mut out[start..] {
                        i.trigger = Some(TriggerAt {
                            op: op.op.clone(),
                            anchor: op.source_anchor.clone(),
                        });
                    }
                    continue;
                }
                if !s.top_level {
                    continue;
                }
                let kind = match fw {
                    None => Kind::All,
                    Some(w) if ends_before(s.anchor, w) => Kind::All,
                    Some(w) if same_site(s.anchor, w) => Kind::Pre,
                    Some(_) => continue,
                };
                let sub = self.compute(s.to, kind, &mut low);
                self.lift(row, s, &sub, &mut out);
            }
            self.stack.borrow_mut().pop();
            self.stack.borrow_mut().pop();
        }
        // A trigger rule keeps its write; dedup within each write and among
        // the plain ones.
        let (mut plain, triggered): (Vec<Inh>, Vec<Inh>) =
            out.into_iter().partition(|i| i.trigger.is_none());
        dedup(&mut plain);
        let mut by_write: BTreeMap<(u32, u32), Vec<Inh>> = BTreeMap::new();
        for i in triggered {
            let a = &i.trigger.as_ref().expect("a trigger rule").anchor;
            by_write.entry(pos_start(a)).or_default().push(i);
        }
        for mut v in by_write.into_values() {
            dedup(&mut v);
            plain.extend(v);
        }
        sort_inh(&mut plain);
        plain
    }

    /// The rules `caller` runs before `site` (its own, anchored before it,
    /// and `all` of each `top-level` step that ends before it).
    fn before_site(&self, caller: usize, site: &PAnchor) -> Vec<Inh> {
        let mut out: Vec<Inh> = self
            .own(caller)
            .into_iter()
            .filter(|i| ends_before(&i.rule.anchor, site))
            .collect();
        let steps = self.steps.get(&caller).cloned().unwrap_or_default();
        for s in steps
            .iter()
            .filter(|s| s.top_level && ends_before(s.anchor, site))
        {
            let mut low = usize::MAX;
            let sub = self.compute(s.to, Kind::All, &mut low);
            self.lift(caller, s, &sub, &mut out);
        }
        dedup(&mut out);
        out
    }

    /// The known callers of `rows` (other rows; distinct routines by stable
    /// id, the first row of each), in row order.
    #[must_use]
    pub fn known_callers(&self, rows: &[usize]) -> Vec<usize> {
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut out: BTreeSet<usize> = BTreeSet::new();
        for r in rows {
            for &(c, _) in self.callers.get(r).map(Vec::as_slice).unwrap_or(&[]) {
                if rows.contains(&c) {
                    continue;
                }
                if seen.insert(self.row(c).stable_routine_id.as_str()) {
                    out.insert(c);
                }
            }
        }
        out.into_iter().collect()
    }

    /// The caller rules of `rows` (spec §4.3).
    #[must_use]
    pub fn caller_rules(&self, rows: &[usize]) -> Vec<CallerRule> {
        let callers = self.known_callers(rows);
        // caller -> its sites into the routine.
        let mut sites: BTreeMap<usize, Vec<&PAnchor>> = BTreeMap::new();
        for r in rows {
            for &(c, a) in self.callers.get(r).map(Vec::as_slice).unwrap_or(&[]) {
                if rows.contains(&c) {
                    continue;
                }
                let id = self.row(c).stable_routine_id.as_str();
                let Some(&first) = callers
                    .iter()
                    .find(|&&x| self.row(x).stable_routine_id == id)
                else {
                    continue;
                };
                sites.entry(first).or_default().push(a);
            }
        }
        let mut found: BTreeMap<(String, Vec<String>), (Inh, Vec<usize>)> = BTreeMap::new();
        for (&c, at) in &sites {
            // Applied before EVERY call the caller makes.
            let mut common: Option<BTreeMap<(String, Vec<String>), Inh>> = None;
            for a in at {
                let here: BTreeMap<_, _> = self
                    .before_site(c, a)
                    .into_iter()
                    .map(|i| (rule_key(&i), i))
                    .collect();
                common = Some(match common {
                    None => here,
                    Some(prev) => prev
                        .into_iter()
                        .filter(|(k, _)| here.contains_key(k))
                        .collect(),
                });
            }
            for (k, i) in common.unwrap_or_default() {
                found.entry(k).or_insert_with(|| (i, Vec::new())).1.push(c);
            }
        }
        let mut out: Vec<CallerRule> = found
            .into_values()
            .map(|(inh, present_in)| {
                let applied_by: Vec<usize> = if inh.rule.stops == Stops::Always {
                    present_in.clone()
                } else {
                    Vec::new()
                };
                let skipped_by = callers
                    .iter()
                    .copied()
                    .filter(|c| !applied_by.contains(c))
                    .collect();
                CallerRule {
                    inh,
                    present_in,
                    applied_by,
                    skipped_by,
                }
            })
            .collect();
        out.sort_by(|a, b| {
            inh_key(&a.inh)
                .cmp(&inh_key(&b.inh))
                .then(a.present_in.cmp(&b.present_in))
        });
        out
    }

    /// The table-trigger rows a write would run: the table's own trigger and
    /// its table extensions' (`OnValidate` / `OnBeforeValidate` /
    /// `OnAfterValidate` of `field`, else `On<Op>` / `OnBefore<Op>` /
    /// `OnAfter<Op>`).
    #[must_use]
    pub fn table_triggers(&self, table_id: &str, field: Option<&str>, op: &str) -> Vec<usize> {
        let tables = table_by_id_preferring_real(&self.model.workspace.tables);
        let Some(table) = tables.get(table_id) else {
            return Vec::new();
        };
        let table_name = table.name.fold_identifier();
        let base = match field {
            Some(_) => "validate".to_string(),
            None => op.to_ascii_lowercase(),
        };
        let names = [
            format!("on{base}"),
            format!("onbefore{base}"),
            format!("onafter{base}"),
        ];
        let field = field.map(|f| f.trim_matches('"').fold_identifier());
        // A table's id (`{app}/table/{number}`) is not its object's id
        // (`{app}/Table/{number}`): match the object by app and number, and
        // its table extensions by the name they extend.
        let mut objects: Vec<&str> = Vec::new();
        for o in &self.model.workspace.objects {
            let is_table = o.object_type == "Table"
                && o.object_number == table.table_number
                && o.app_guid.eq_ignore_ascii_case(&table.app_guid);
            let extends = o.object_type == "TableExtension"
                && o.extends_target_name
                    .as_deref()
                    .is_some_and(|n| n.fold_identifier() == table_name);
            if is_table || extends {
                objects.push(o.id.as_str());
            }
        }
        let mut out: Vec<usize> = objects
            .iter()
            .flat_map(|o| self.rows_of_object.get(o).into_iter().flatten())
            .copied()
            .filter(|&i| {
                let r = self.row(i);
                r.kind == "trigger"
                    && names.contains(&r.name.to_ascii_lowercase())
                    && r.enclosing_member
                        .as_deref()
                        .map(|m| m.trim_matches('"').fold_identifier())
                        == field
            })
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The writes of `rows` that do not (surely) run their table triggers,
    /// with those triggers' rules; only writes whose triggers have rules.
    #[must_use]
    pub fn skipped_triggers(&self, rows: &[usize]) -> Vec<SkippedTriggers> {
        let tables = table_by_id_preferring_real(&self.model.workspace.tables);
        let mut out = Vec::new();
        for &row in rows {
            let r = self.row(row);
            let table_of_var = |var: &str| {
                let key = var.trim_matches('"').fold_identifier();
                r.record_variables
                    .iter()
                    .find(|rv| rv.name.trim_matches('"').fold_identifier() == key)
            };
            let mut found = |op: &str,
                             table_id: &str,
                             field: Option<&str>,
                             reason: &'static str,
                             anchor: &PAnchor| {
                let triggers = self.table_triggers(table_id, field, op);
                let mut rules: Vec<Inh> = triggers
                    .iter()
                    .flat_map(|&t| {
                        self.all(t)
                            .iter()
                            .cloned()
                            .map(|mut i| {
                                i.via.insert(0, t);
                                i
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect();
                dedup(&mut rules);
                if rules.is_empty() {
                    return;
                }
                sort_inh(&mut rules);
                out.push(SkippedTriggers {
                    op: op.to_string(),
                    table: tables
                        .get(table_id)
                        .map_or_else(|| table_id.to_string(), |t| t.name.clone()),
                    field: field.map(str::to_string),
                    reason,
                    triggers,
                    rules,
                    anchor: anchor.clone(),
                });
            };
            for fa in &r.field_accesses {
                if !matches!(
                    fa.access,
                    FieldAccessKind::Write | FieldAccessKind::ReadWrite
                ) {
                    continue;
                }
                let Some(table_id) =
                    table_of_var(&fa.record_variable_name).and_then(|rv| rv.table_id.as_deref())
                else {
                    continue;
                };
                found(
                    "assign",
                    table_id,
                    Some(&fa.field_name),
                    "a plain assignment does not run the field's OnValidate trigger",
                    &fa.source_anchor,
                );
            }
            let sites = self.site_targets.get(&row);
            for op in &r.record_operations {
                let name = op.op.to_ascii_lowercase();
                if !matches!(name.as_str(), "insert" | "modify" | "delete" | "deleteall") {
                    continue;
                }
                let Some(table_id) = op.table_id.as_deref() else {
                    continue;
                };
                let has_edge = sites
                    .and_then(|s| s.get(op.id.as_str()))
                    .is_some_and(|t| !t.is_empty());
                let surely = self
                    .steps
                    .get(&row)
                    .is_some_and(|s| s.iter().any(|x| x.op.is_some_and(|o| o.id == op.id)));
                let reason = match (has_edge, surely) {
                    (_, true) => continue,
                    (true, false) => "the RunTrigger argument is not a literal true",
                    (false, _) => "without RunTrigger = true the table trigger does not run",
                };
                let base = if name == "deleteall" { "delete" } else { &name };
                found(base, table_id, None, reason, &op.source_anchor);
            }
        }
        out.sort_by(|a, b| {
            (pos_start(&a.anchor), &a.op, &a.field).cmp(&(pos_start(&b.anchor), &b.op, &b.field))
        });
        out
    }

    /// The first row of a routine id.
    #[must_use]
    pub fn row_of(&self, id: &str) -> Option<usize> {
        self.first_row.get(id).copied()
    }

    /// The rows the edges at `site` (a call-site or operation id) of `row`
    /// reach.
    #[must_use]
    pub fn targets_at(&self, row: usize, site: &str) -> &[usize] {
        self.site_targets
            .get(&row)
            .and_then(|m| m.get(site))
            .map_or(&[], Vec::as_slice)
    }
}

/// The record-field assignments and mutating operations of `r` (not on a
/// record known temporary).
fn own_writes(r: &ModelRoutine) -> Vec<&PAnchor> {
    let temp_var = |var: &str| {
        let key = var.trim_matches('"').fold_identifier();
        r.record_variables.iter().any(|rv| {
            rv.name.trim_matches('"').fold_identifier() == key
                && known_temp_suppresses(rv.temp_state.known_value())
        })
    };
    let mut out: Vec<&PAnchor> = r
        .field_accesses
        .iter()
        .filter(|fa| {
            matches!(
                fa.access,
                FieldAccessKind::Write | FieldAccessKind::ReadWrite
            ) && !temp_var(&fa.record_variable_name)
        })
        .map(|fa| &fa.source_anchor)
        .collect();
    out.extend(
        r.record_operations
            .iter()
            .filter(|op| {
                WRITE_OPS.contains(&op.op.to_ascii_lowercase().as_str())
                    && !temp_state_suppresses(op.temp_state.as_ref())
                    && !temp_var(&op.record_variable_name)
            })
            .map(|op| &op.source_anchor),
    );
    out
}

fn inh_key(i: &Inh) -> ((&str, u32, u32), &str, usize) {
    let a = &i.rule.anchor;
    (
        (a.source_unit_id.as_str(), a.start_line, a.start_column),
        i.rule.id.as_str(),
        i.via.len(),
    )
}

/// By the write a trigger rule protects (plain rules first), then the
/// check's anchor.
fn sort_inh(v: &mut [Inh]) {
    v.sort_by(|a, b| {
        let ta = a.trigger.as_ref().map(|t| pos_start(&t.anchor));
        let tb = b.trigger.as_ref().map(|t| pos_start(&t.anchor));
        ta.cmp(&tb)
            .then(inh_key(a).cmp(&inh_key(b)))
            .then(a.rule.when_texts.cmp(&b.rule.when_texts))
    });
}
