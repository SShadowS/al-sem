//! #343: the proofs behind d39's `helperPersists` skip.
//!
//! Is the source record persisted on every path from the dirtying call to an
//! exit? Every fact about a called routine is read from that routine's own
//! body, never from an L4 role summary, and every proof fails closed: when it
//! cannot tell, the finding stays. A body is readable only when it is present,
//! fully parsed, not a `[TryFunction]` (its errors are caught, so its paths are
//! not its tree's paths) and free of nodes `walk_paths` cannot see through
//! (`try`, `asserterror`, a stray `break`). Helpers are followed to
//! [`MAX_HELPER_DEPTH`]; results are memoized per (routine, variable,
//! remaining depth) for one detector run.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::engine::l4::summary_runner::record_flow_role;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::{
    PathQuestion, PathStates, before_anchor, is_table_receiver_rec, walk_paths,
};
use crate::program::body::features::{PAnchor, PCFNNode, PCallArgumentBinding, PCallSite, PCallee};
use crate::program::model::workspace::{ModelRecordOperation, ModelRoutine};

/// How deep a helper's own helpers are followed.
const MAX_HELPER_DEPTH: usize = 3;

/// Path states of the caller walk: before the call; after the latest run of the
/// call with its dirt unsaved (`PENDING`); dirt of an EARLIER run (a loop) still
/// unsaved (`CARRIED`: a clean return of a later run does not clear it); the
/// dirt saved, or never made (`DONE`).
const BEFORE: PathStates = 1;
const PENDING: PathStates = 2;
const DONE: PathStates = 4;
const CARRIED: PathStates = 8;
const UNSAVED: PathStates = PENDING | CARRIED;

/// Path states of the clean-return walk.
const CLEAN: PathStates = 1;
const DIRTY: PathStates = 2;

/// The unsaved states (`mask`) saved.
fn settle(s: PathStates, mask: PathStates) -> PathStates {
    if s & mask != 0 { (s & !mask) | DONE } else { s }
}

fn names_var(name: &str, id: Option<&str>, var_lc: &str, var_id: Option<&str>) -> bool {
    name.eq_ignore_ascii_case(var_lc)
        && match (id, var_id) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        }
}

fn is_event_publisher(r: &ModelRoutine) -> bool {
    use crate::program::attributes::has_attribute;
    ["IntegrationEvent", "BusinessEvent", "InternalEvent"]
        .iter()
        .any(|a| has_attribute(&r.attributes_parsed, a))
}

/// A whole-record argument (not a field or an expression).
fn whole_record_arg(b: &PCallArgumentBinding) -> bool {
    matches!(
        b.source_kind.as_str(),
        "local" | "parameter" | "global" | "implicit-rec"
    )
}

/// How call `cs` takes record `var_lc`: as the receiver (`Var.Proc()`), and the
/// indexes of its whole-record arguments. One definition for the persist and
/// the clean proofs.
fn record_args(cs: &PCallSite, var_lc: &str, var_id: Option<&str>) -> (bool, Vec<usize>) {
    let on_receiver = matches!(&cs.callee, PCallee::Member { receiver, .. }
        if receiver.trim_matches('"').eq_ignore_ascii_case(var_lc));
    let args = cs
        .argument_bindings
        .iter()
        .enumerate()
        .filter(|(_, b)| {
            whole_record_arg(b)
                && b.source_variable_name.as_deref().is_some_and(|n| {
                    names_var(n, b.source_record_variable_id.as_deref(), var_lc, var_id)
                })
        })
        .map(|(i, _)| i)
        .collect();
    (on_receiver, args)
}

/// A bare call (`Proc()`, no receiver) while the record is the implicit `Rec`:
/// in table code it may be a procedure of the same table, working on `Rec`.
fn bare_on_rec(cs: &PCallSite, var_lc: &str) -> bool {
    var_lc.eq_ignore_ascii_case("rec") && matches!(cs.callee, PCallee::Bare { .. })
}

/// A procedure that works on its table's implicit `Rec`.
fn is_table_procedure(callee: &ModelRoutine) -> bool {
    callee.kind != "trigger" && is_table_receiver_rec(callee, "rec")
}

/// Any whole-record assignment to `var_lc` (`Var := <anything>`) in `routine`,
/// after `after` when given. Assignments are not leaves of the tree, so this is
/// by source order and fails closed. `lhs_name` is also the member name of a
/// field assignment, so a field named like the variable counts too (the safe
/// direction). Unlike d37 / d11 (`bare_rec_assignments`, #299 S14), which count
/// only a bare identifier on the right as a reset, any right-hand side counts
/// here: a call result or a member expression replaces the record just the same.
fn record_reassigned(routine: &ModelRoutine, var_lc: &str, after: Option<&PAnchor>) -> bool {
    routine.var_assignments.iter().any(|a| {
        a.lhs_name.eq_ignore_ascii_case(var_lc)
            && after.is_none_or(|x| before_anchor(x, &a.source_anchor))
    })
}

/// What a leaf does to the record for a persist proof.
#[derive(PartialEq)]
enum VarEffect {
    None,
    Persists,
    /// Reloads, inits or copies into it, or may: the unsaved write is lost.
    Resets,
    /// Cannot tell.
    Unknown,
}

fn op_effect(op: &ModelRecordOperation, var_lc: &str, var_id: Option<&str>) -> VarEffect {
    if !names_var(
        &op.record_variable_name,
        op.record_variable_id.as_deref(),
        var_lc,
        var_id,
    ) {
        return VarEffect::None;
    }
    match record_flow_role(&op.op) {
        "persistsCurrent" => VarEffect::Persists,
        "loadsFromDb" | "initialises" | "copiesInto" => VarEffect::Resets,
        _ => VarEffect::None,
    }
}

/// A routine's op and call-site ids, and whether its tree holds a node the walk
/// cannot see through. Per routine row, not from `ctx.call_site_by_id`: rows of a
/// collided id share site ids (#160).
#[derive(Default)]
struct RoutineIndex {
    ops: HashMap<String, usize>,
    sites: HashMap<String, usize>,
    opaque: bool,
}

/// Finds an opaque node anywhere a walk would reach.
#[derive(Default)]
struct OpaqueProbe(bool);

impl PathQuestion for OpaqueProbe {
    fn leaf(&mut self, _leaf: &PCFNNode, s: PathStates) -> PathStates {
        s
    }
    fn exit(&mut self, _value: Option<bool>, _s: PathStates) {}
    fn opaque(&mut self) {
        self.0 = true;
    }
}

type Key = (*const ModelRoutine, String, usize);

/// The `helperPersists` proofs for one d39 run, with their memo tables.
pub(super) struct Proofs<'c> {
    ctx: &'c DetectorContext<'c>,
    index: RefCell<HashMap<*const ModelRoutine, Rc<RoutineIndex>>>,
    persists: RefCell<HashMap<Key, bool>>,
    preserves: RefCell<HashMap<Key, bool>>,
    clean: RefCell<HashMap<Key, bool>>,
    clean_return: RefCell<HashMap<(*const ModelRoutine, u32), Option<bool>>>,
}

impl<'c> Proofs<'c> {
    pub(super) fn new(ctx: &'c DetectorContext<'c>) -> Self {
        Proofs {
            ctx,
            index: RefCell::default(),
            persists: RefCell::default(),
            preserves: RefCell::default(),
            clean: RefCell::default(),
            clean_return: RefCell::default(),
        }
    }

    fn index(&self, r: &ModelRoutine) -> Rc<RoutineIndex> {
        let key = std::ptr::from_ref(r);
        if let Some(ix) = self.index.borrow().get(&key) {
            return Rc::clone(ix);
        }
        let mut ix = RoutineIndex::default();
        for (i, op) in r.record_operations.iter().enumerate() {
            ix.ops.entry(op.id.clone()).or_insert(i);
        }
        for (i, cs) in r.call_sites.iter().enumerate() {
            ix.sites.entry(cs.id.clone()).or_insert(i);
        }
        if let Some(tree) = &r.statement_tree {
            let mut probe = OpaqueProbe::default();
            walk_paths(tree, 1, &mut probe);
            ix.opaque = probe.0;
        }
        let ix = Rc::new(ix);
        self.index.borrow_mut().insert(key, Rc::clone(&ix));
        ix
    }

    /// The body a proof may read (see the module doc), or `None`.
    fn proof_body<'r>(&self, r: &'r ModelRoutine) -> Option<&'r PCFNNode> {
        use crate::program::attributes::has_attribute;
        let tree = r.statement_tree.as_ref()?;
        (r.body_available
            && !r.parse_incomplete
            && !has_attribute(&r.attributes_parsed, "TryFunction")
            && !self.index(r).opaque)
            .then_some(tree)
    }

    fn op<'r>(&self, r: &'r ModelRoutine, id: &str) -> Option<&'r ModelRecordOperation> {
        self.index(r).ops.get(id).map(|&i| &r.record_operations[i])
    }

    fn site<'r>(&self, r: &'r ModelRoutine, id: &str) -> Option<&'r PCallSite> {
        self.index(r).sites.get(id).map(|&i| &r.call_sites[i])
    }

    /// The routine a call site certainly runs: a resolved direct or method call
    /// (not an interface dispatch, whose other implementers are not read).
    fn certain_callee(&self, cs: &PCallSite) -> Option<&'c ModelRoutine> {
        use crate::program::model::taxonomy::{DispatchKind, Resolution};
        let e = self.ctx.resolved_call_edge_by_callsite.get(&cs.id)?;
        if e.resolution != Resolution::Resolved
            || !matches!(e.dispatch_kind, DispatchKind::Direct | DispatchKind::Method)
        {
            return None;
        }
        self.ctx.routine_by_id.get(e.to.as_deref()?).copied()
    }

    /// A call with no target that the resolver did not flag as unresolved: a
    /// platform builtin.
    fn is_builtin_site(&self, routine: &ModelRoutine, cs: &PCallSite) -> bool {
        !self.ctx.resolved_call_edge_by_callsite.contains_key(&cs.id)
            && !self
                .ctx
                .own_uncertainty_edges(&routine.id)
                .any(|u| u.callsite_id.as_deref() == Some(cs.id.as_str()))
    }

    /// What leaf `leaf` of `routine` does to record `var_lc`.
    fn var_effect(
        &self,
        routine: &ModelRoutine,
        leaf: &PCFNNode,
        var_lc: &str,
        var_id: Option<&str>,
        depth: usize,
        global: bool,
    ) -> VarEffect {
        if let Some(op_id) = &leaf.operation_id {
            return self
                .op(routine, op_id)
                .map_or(VarEffect::None, |op| op_effect(op, var_lc, var_id));
        }
        leaf.callsite_id
            .as_ref()
            .and_then(|id| self.site(routine, id))
            .map_or(VarEffect::None, |cs| {
                self.call_effect(routine, cs, var_lc, var_id, depth, global)
            })
    }

    /// What call `cs` of `routine` does to record `var_lc`. A `var` helper
    /// persists it only when its body persists it on every path
    /// ([`Self::helper_persists`]) and leaves it be only when its body provably
    /// does ([`Self::helper_preserves`]). A call on the record to a table
    /// procedure counts only as a proven persist, and never when the record is
    /// also one of its arguments (an alias). For a `global` record, any
    /// non-builtin call may reach it. Anything else that may take the record is
    /// `Unknown`, an event publisher included (#343 F5).
    fn call_effect(
        &self,
        routine: &ModelRoutine,
        cs: &PCallSite,
        var_lc: &str,
        var_id: Option<&str>,
        depth: usize,
        global: bool,
    ) -> VarEffect {
        let (mut on_receiver, args) = record_args(cs, var_lc, var_id);
        let bare_rec = bare_on_rec(cs, var_lc);
        if !on_receiver && args.is_empty() && !bare_rec && !global {
            return VarEffect::None;
        }
        let Some(callee) = self.certain_callee(cs) else {
            // A builtin record method (`Job.TableCaption()`) or a builtin not
            // taking the record leaves it be; a builtin taking it (`Clear(Job)`)
            // or an unresolved call may not.
            return if args.is_empty() && self.is_builtin_site(routine, cs) {
                VarEffect::None
            } else {
                VarEffect::Unknown
            };
        };
        if global {
            return VarEffect::Unknown;
        }
        if bare_rec && is_table_procedure(callee) {
            on_receiver = true;
        } else if bare_rec && args.is_empty() && !on_receiver {
            return VarEffect::None;
        }
        let deeper = depth < MAX_HELPER_DEPTH;
        if on_receiver {
            return if args.is_empty()
                && is_table_procedure(callee)
                && deeper
                && self.helper_persists(callee, "rec", depth + 1)
            {
                VarEffect::Persists
            } else {
                VarEffect::Unknown
            };
        }
        let upgraded = self.ctx.upgraded_bindings_by_callsite.get(&cs.id);
        let mut out = VarEffect::None;
        for i in args {
            let b = &cs.argument_bindings[i];
            let Some(u) = upgraded.and_then(|u| u.get(i)) else {
                return VarEffect::Unknown;
            };
            if u.binding_resolution != "resolved" {
                return VarEffect::Unknown;
            }
            if !u.callee_parameter_is_var {
                continue; // a copy: the callee cannot touch the caller's record
            }
            let Some(p) = callee
                .parameters
                .iter()
                .find(|p| p.index == b.parameter_index)
                .filter(|_| deeper)
            else {
                return VarEffect::Unknown;
            };
            if self.helper_persists(callee, &p.name, depth + 1) {
                out = VarEffect::Persists;
            } else if !self.helper_preserves(callee, &p.name, depth + 1) {
                return VarEffect::Unknown;
            }
        }
        out
    }

    /// Does `routine` leave the record it was handed as `var_lc` as it got it:
    /// no reload / init / copy into it, no whole-record assignment, and every
    /// call that takes it provably persists or preserves it? Path-insensitive.
    /// Never for an event publisher: its subscribers are not read (#343 F5).
    fn helper_preserves(&self, routine: &ModelRoutine, var_lc: &str, depth: usize) -> bool {
        let key = (
            std::ptr::from_ref(routine),
            var_lc.to_ascii_lowercase(),
            depth,
        );
        if let Some(&v) = self.preserves.borrow().get(&key) {
            return v;
        }
        let v = !is_event_publisher(routine)
            && self.proof_body(routine).is_some()
            && !record_reassigned(routine, var_lc, None)
            && routine
                .record_operations
                .iter()
                .all(|op| op_effect(op, var_lc, None) != VarEffect::Resets)
            && routine.call_sites.iter().all(|cs| {
                !matches!(
                    self.call_effect(routine, cs, var_lc, None, depth, false),
                    VarEffect::Resets | VarEffect::Unknown
                )
            });
        self.preserves.borrow_mut().insert(key, v);
        v
    }

    /// Does `routine` persist the record it was handed as `var_lc` on every path
    /// from entry to exit, with nothing reloading, initing, copying into or
    /// replacing it before the persist?
    fn helper_persists(&self, routine: &ModelRoutine, var_lc: &str, depth: usize) -> bool {
        let key = (
            std::ptr::from_ref(routine),
            var_lc.to_ascii_lowercase(),
            depth,
        );
        if let Some(&v) = self.persists.borrow().get(&key) {
            return v;
        }
        let v = match self.proof_body(routine) {
            Some(tree) if !record_reassigned(routine, var_lc, None) => {
                let mut q = HelperPersists {
                    proofs: self,
                    routine,
                    var_lc,
                    depth,
                    failed: false,
                };
                walk_paths(tree, PENDING, &mut q);
                !q.failed
            }
            _ => false,
        };
        self.persists.borrow_mut().insert(key, v);
        v
    }

    /// Can call `cs` of `routine` leave record `var_lc` dirty? It keeps the
    /// record clean only when it does not take it, is a builtin record method
    /// on it, takes it by value, or is a resolved direct / method `var` call
    /// whose own body provably makes no dirt ([`Self::helper_clean`]). A call on
    /// the record to a table procedure (also a bare one on the implicit `Rec`),
    /// an unresolved call, an interface dispatch and an event publisher may
    /// dirty it.
    fn call_keeps_clean(
        &self,
        routine: &ModelRoutine,
        cs: &PCallSite,
        var_lc: &str,
        depth: usize,
    ) -> bool {
        let (on_receiver, args) = record_args(cs, var_lc, None);
        let bare_rec = bare_on_rec(cs, var_lc);
        if !on_receiver && args.is_empty() && !bare_rec {
            return true;
        }
        let Some(callee) = self.certain_callee(cs) else {
            return args.is_empty() && self.is_builtin_site(routine, cs);
        };
        if bare_rec && is_table_procedure(callee) {
            return false;
        }
        if on_receiver || depth >= MAX_HELPER_DEPTH {
            return false;
        }
        let upgraded = self.ctx.upgraded_bindings_by_callsite.get(&cs.id);
        args.into_iter().all(|i| {
            let b = &cs.argument_bindings[i];
            match upgraded.and_then(|u| u.get(i)) {
                Some(u) if u.binding_resolution == "resolved" && !u.callee_parameter_is_var => true,
                Some(u) if u.binding_resolution == "resolved" => callee
                    .parameters
                    .iter()
                    .find(|p| p.index == b.parameter_index)
                    .is_some_and(|p| self.helper_clean(callee, &p.name, depth + 1)),
                _ => false,
            }
        })
    }

    /// Does `routine` make no dirt on the record it was handed as `var_lc`: no
    /// `Validate` or copy into it, no whole-record assignment to it, and every
    /// call that takes it keeps it clean? Path-insensitive. Never for an event
    /// publisher.
    fn helper_clean(&self, routine: &ModelRoutine, var_lc: &str, depth: usize) -> bool {
        let key = (
            std::ptr::from_ref(routine),
            var_lc.to_ascii_lowercase(),
            depth,
        );
        if let Some(&v) = self.clean.borrow().get(&key) {
            return v;
        }
        let v = !is_event_publisher(routine)
            && self.proof_body(routine).is_some()
            && !record_reassigned(routine, var_lc, None)
            && routine.record_operations.iter().all(|op| {
                // A copy counts as dirt here, though `CleanReturn` (path-sensitive,
                // on the callee's own body) treats it as a reset: path-insensitively
                // we cannot tell whether a Validate follows it.
                !op.record_variable_name.eq_ignore_ascii_case(var_lc)
                    || !matches!(record_flow_role(&op.op), "validates" | "copiesInto")
            })
            && routine
                .call_sites
                .iter()
                .all(|cs| self.call_keeps_clean(routine, cs, var_lc, depth));
        self.clean.borrow_mut().insert(key, v);
        v
    }

    /// The value `callee` returns on every path where it leaves its parameter
    /// `param_index` clean, when its dirty exits all return the other Boolean
    /// literal (`exit(false)` before the `Validate`, `exit(true)` after it).
    /// `None` when there is no such value, or when a whole-record assignment to
    /// the parameter (not a tree leaf) could carry dirt in.
    fn clean_return(&self, callee: &ModelRoutine, param_index: u32) -> Option<bool> {
        let key = (std::ptr::from_ref(callee), param_index);
        if let Some(&v) = self.clean_return.borrow().get(&key) {
            return v;
        }
        let v = (|| {
            let tree = self.proof_body(callee)?;
            let p = callee.parameters.iter().find(|p| p.index == param_index)?;
            if record_reassigned(callee, &p.name, None) {
                return None;
            }
            let mut q = CleanReturn {
                proofs: self,
                routine: callee,
                var_lc: &p.name,
                dirty_on: [false, false],
                failed: false,
            };
            walk_paths(tree, CLEAN, &mut q);
            match (q.failed, q.dirty_on) {
                (false, [false, true]) => Some(false),
                (false, [true, false]) => Some(true),
                _ => None,
            }
        })();
        self.clean_return.borrow_mut().insert(key, v);
        v
    }

    /// Is record `var_lc` of `caller` persisted on every path from call `cs` (to
    /// parameter `param_index` of `callee`, given as `(callee, param_index)`) to
    /// an exit, with nothing reloading, initing, copying into or replacing it
    /// first? A path that runs only when the callee returned its clean value (an
    /// arm of `if C(..)` / `if not C(..)`) carries no dirt of that run. `global`:
    /// the record is an object global, which any non-builtin call may reach.
    pub(super) fn persisted_on_every_path_after(
        &self,
        caller: &ModelRoutine,
        cs: &PCallSite,
        (var_lc, var_id): (&str, Option<&str>),
        global: bool,
        (callee, param_index): (&ModelRoutine, u32),
    ) -> bool {
        let Some(tree) = self.proof_body(caller) else {
            return false;
        };
        if record_reassigned(caller, var_lc, Some(&cs.source_anchor)) {
            return false;
        }
        let mut q = PersistAfter {
            proofs: self,
            routine: caller,
            call_id: &cs.id,
            var_lc,
            var_id,
            global,
            callee,
            param_index,
            seen: false,
            failed: false,
        };
        walk_paths(tree, BEFORE, &mut q);
        q.seen && !q.failed
    }
}

/// The caller walk of [`Proofs::persisted_on_every_path_after`].
struct PersistAfter<'p, 'c> {
    proofs: &'p Proofs<'c>,
    routine: &'p ModelRoutine,
    call_id: &'p str,
    var_lc: &'p str,
    var_id: Option<&'p str>,
    global: bool,
    callee: &'p ModelRoutine,
    param_index: u32,
    seen: bool,
    failed: bool,
}

impl PersistAfter<'_, '_> {
    /// The dirtying callee may reload / init / copy into / replace the record
    /// (its body does not provably preserve it): a re-run would drop an earlier
    /// run's unsaved write. Computed only on a re-run.
    fn callee_resets(&self) -> bool {
        !self
            .callee
            .parameters
            .iter()
            .find(|p| p.index == self.param_index)
            .is_some_and(|p| self.proofs.helper_preserves(self.callee, &p.name, 1))
    }
}

impl PathQuestion for PersistAfter<'_, '_> {
    fn leaf(&mut self, leaf: &PCFNNode, s: PathStates) -> PathStates {
        if leaf.callsite_id.as_deref() == Some(self.call_id) {
            self.seen = true;
            if s & UNSAVED == 0 {
                return PENDING;
            }
            // A re-run (a loop): the earlier write is lost if the callee may
            // reset the record, or if a whole-record assignment (not a tree leaf,
            // so anywhere in the routine) may run on the back edge.
            self.failed |=
                record_reassigned(self.routine, self.var_lc, None) || self.callee_resets();
            return PENDING | CARRIED;
        }
        if s & UNSAVED == 0 {
            return s;
        }
        match self
            .proofs
            .var_effect(self.routine, leaf, self.var_lc, self.var_id, 0, self.global)
        {
            VarEffect::None => s,
            VarEffect::Persists => settle(s, UNSAVED),
            VarEffect::Resets | VarEffect::Unknown => {
                self.failed = true;
                s
            }
        }
    }
    fn arm(&mut self, cs_id: &str, value: bool, s: PathStates) -> PathStates {
        // Only this run's dirt: an earlier run's (`CARRIED`) stays unsaved.
        if cs_id == self.call_id
            && self.proofs.clean_return(self.callee, self.param_index) == Some(value)
        {
            settle(s, PENDING)
        } else {
            s
        }
    }
    fn exit(&mut self, _value: Option<bool>, s: PathStates) {
        self.failed |= s & UNSAVED != 0;
    }
    fn opaque(&mut self) {
        self.failed = true;
    }
}

/// The walk of [`Proofs::helper_persists`].
struct HelperPersists<'p, 'c> {
    proofs: &'p Proofs<'c>,
    routine: &'p ModelRoutine,
    var_lc: &'p str,
    depth: usize,
    failed: bool,
}

impl PathQuestion for HelperPersists<'_, '_> {
    fn leaf(&mut self, leaf: &PCFNNode, s: PathStates) -> PathStates {
        if s & PENDING == 0 {
            return s;
        }
        match self
            .proofs
            .var_effect(self.routine, leaf, self.var_lc, None, self.depth, false)
        {
            VarEffect::None => s,
            VarEffect::Persists => settle(s, PENDING),
            VarEffect::Resets | VarEffect::Unknown => {
                self.failed = true;
                s
            }
        }
    }
    fn exit(&mut self, _value: Option<bool>, s: PathStates) {
        self.failed |= s & PENDING != 0;
    }
    fn opaque(&mut self) {
        self.failed = true;
    }
}

/// The walk of [`Proofs::clean_return`].
struct CleanReturn<'p, 'c> {
    proofs: &'p Proofs<'c>,
    routine: &'p ModelRoutine,
    var_lc: &'p str,
    /// Dirty at an exit returning `false` / `true`.
    dirty_on: [bool; 2],
    failed: bool,
}

impl PathQuestion for CleanReturn<'_, '_> {
    fn leaf(&mut self, leaf: &PCFNNode, s: PathStates) -> PathStates {
        let r = self.routine;
        if let Some(op_id) = &leaf.operation_id {
            let Some(op) = self.proofs.op(r, op_id) else {
                return s;
            };
            if !op.record_variable_name.eq_ignore_ascii_case(self.var_lc) {
                return s;
            }
            return match record_flow_role(&op.op) {
                "validates" => DIRTY,
                "persistsCurrent" | "loadsFromDb" | "initialises" | "copiesInto" => CLEAN,
                _ => s,
            };
        }
        // A call that takes the parameter: it may dirty it unless
        // `call_keeps_clean` proves otherwise.
        match leaf
            .callsite_id
            .as_ref()
            .and_then(|id| self.proofs.site(r, id))
        {
            Some(cs) if !self.proofs.call_keeps_clean(r, cs, self.var_lc, 0) => s | DIRTY,
            _ => s,
        }
    }
    fn exit(&mut self, value: Option<bool>, s: PathStates) {
        if s & DIRTY != 0 {
            match value {
                Some(v) => self.dirty_on[usize::from(v)] = true,
                None => self.dirty_on = [true, true],
            }
        }
    }
    fn opaque(&mut self) {
        self.failed = true;
    }
}
