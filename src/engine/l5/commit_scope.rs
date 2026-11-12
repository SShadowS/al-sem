//! `[CommitBehavior]` scope (engine limits B4/L16): what an explicit
//! `Commit()` does once the routines on its route are taken into account.
//!
//! **The rule** (Microsoft Learn, "CommitBehavior attribute",
//! <https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/attributes/devenv-commitbehavior-attribute>):
//! - `[CommitBehavior(CommitBehavior::Ignore)]` on a method makes every
//!   explicit commit ignored while the method runs; `Error` makes it raise an
//!   error. The behavior lasts for the method's whole scope (every method it
//!   calls, the subscribers of an event it publishes; object runs below) and
//!   returns to the previous one when the method ends.
//! - Only a MORE restrictive behavior can be set: under `Error`, an inner
//!   `Ignore` keeps `Error`. So the behavior a commit runs under is the
//!   strictest one declared on its route (`Normal < Ignore < Error`).
//! - It applies to explicit commits only, not to the implicit commit of
//!   `Codeunit.Run`. A checked run (`if Codeunit.Run(…) then`, the result
//!   used) commits its changes at its end unless an error occurs, and "the
//!   implicit commit by `Codeunit.Run` isn't affected by `CommitBehavior`"
//!   (Learn, "Codeunit.Run(Integer [, var Record]) Method",
//!   <https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/methods-auto/codeunit/codeunit-run-method>).
//!   That implicit commit is a fact of its own since ENG-09: a commit at the
//!   run's call site ([`is_run_commit`]), which no rule here applies to. A
//!   commit inside a checked run is still read as the run's commit too: once a
//!   route crosses a checked run ([`Step::CheckedRun`]) a commit that runs
//!   normally or is ignored is the run's commit ([`P`], pinned). An error the
//!   run's body raises is trapped by the run (it returns `false` and rolls
//!   back), so no error row comes from it: a commit an `Error` scope may turn
//!   into an error inside the run is a commit that may not happen ([`X`]).
//! - Learn does not say whether the scope follows into an object a call
//!   starts (any `Run`, `RunModal`, a report's `SaveAs*`/`Execute`/`Print`,
//!   an XmlPort's `Import`/`Export`, a query's `Open`/`SaveAs*`; keyword or
//!   variable form), checked run included. The engine reads it as MAY apply
//!   ([`Step::Run`], and both readings in [`Step::CheckedRun`]): the started
//!   object runs under the caller's behavior or under its own declarations
//!   alone, so a commit in it stays possible, no definite "no" rests on the
//!   unverified reading, and such a commit is never `unconditional on
//!   success` ([`uncertain`]). What starts an object is decided once, where
//!   the call resolves (`CallEdge::starts_object`, read by `object_runs`); a call
//!   that crosses into a dependency is entered by the same step
//!   ([`CbGraph::beyond`]).
//!
//! An object's `InherentCommitBehavior` property (parsed for source and
//! symbol objects) is not on Microsoft Learn. Its reading is the one that
//! never turns a possible effect into a definite "no": a routine without its
//! own attribute on such an object MAY run under the property, so it adds the
//! property's behavior as a possibility and keeps `Normal` too.
//!
//! **One rule, every reader.** [`declared`] is the routine's own declaration,
//! read by the callers' question (`callers_context`, which lists the
//! declarations above an added commit) and by the forward answers through
//! [`CommitScopes`] and [`CbGraph`], which the digest applies to each COMMIT
//! site it reports (`digest::digest_one_root`): a commit no route runs
//! normally is no commit; a commit some route runs under `Error` is an error
//! site (`ERROR_IMPLICIT` with detail `commitBehavior`).
//!
//! **Sets, not one value.** A route's behavior is a set of possible values
//! (bits [`N`], [`I`], [`E`], and [`P`]/[`X`] for a route pinned by a
//! checked run): a declaration is one value, or two for the object property.
//! [`lift`] composes a set with a declaration by the stricter-of rule;
//! [`norm`] reads the set as the answers do. The graph's backward pass keeps
//! one set per value a routine may be entered under ([`Transfer`]), so a
//! "may" step deep in the graph composes exactly with a declaration above it.
//!
//! **A known over-approximation.** [`CbGraph`] walks every typed edge, a
//! superset of the edges the cone's witness search keeps. An extra edge can
//! add a route the cone cannot take: it may keep a commit as a commit or
//! add a possible error (a spurious `throws-error yes`), never drop one, so
//! no definite "no" comes from it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::engine::l5::snapshot::SnapshotGraphEdge;
use crate::program::model::workspace::{Model, ModelObject, ModelRoutine};

/// The commit runs normally.
pub const N: u8 = 1;
/// The commit is ignored.
pub const I: u8 = 2;
/// The commit raises an error.
pub const E: u8 = 4;
/// The route crossed a checked `Codeunit.Run` and the commit inside runs
/// normally or is ignored: either way the run's implicit commit commits at
/// its end, which no `[CommitBehavior]` changes. Read as [`N`] by every
/// answer ([`norm`]).
pub const P: u8 = 8;
/// The route crossed a checked `Codeunit.Run` and the commit inside MAY
/// raise an error: the run traps that error (it returns `false` and rolls
/// back), so it adds no error row. The run may also commit (the scope may
/// not follow into it, or the code may not reach the commit), so the answers
/// read it as a commit that may not happen ([`norm`]: [`N`] and [`I`]).
pub const X: u8 = 16;

/// Whether `f` is the commit of a checked `Codeunit.Run` (ENG-09,
/// `capability_cone`): its resource is `RUN_COMMIT`, direct or inherited (an
/// inherited fact keeps its source's resource, and its own cone key). Learn: "the
/// implicit commit by `Codeunit.Run` isn't affected by `CommitBehavior`"
/// (measured on BC 28 under `Ignore` and `Error`), so no rule of this module
/// applies to it.
#[must_use]
pub(crate) fn is_run_commit(f: &crate::engine::l5::snapshot::SnapshotCapabilityFact) -> bool {
    f.op() == "commit" && f.resource_id() == Some(crate::engine::l4::capability_cone::RUN_COMMIT)
}

/// The five behavior values a route can carry, in index order.
const VALUES: [u8; 5] = [N, I, E, P, X];

/// The index of the single value `v` in [`VALUES`].
fn vix(v: u8) -> usize {
    VALUES.iter().position(|&x| x == v).expect("a single value")
}

/// One value `x` of the caller's side composed with one declared value `y`
/// (`N`, `I` or `E`) by Learn's stricter-of rule ("It's only possible to
/// assign a more restrictive commit behavior"). Inside a checked run the
/// result stays pinned: `Error` makes the commit an error the run traps
/// ([`X`]), anything else keeps the run's implicit commit ([`P`]).
fn combine(x: u8, y: u8) -> u8 {
    match x {
        P => {
            if y == E {
                X
            } else {
                P
            }
        }
        X => X,
        _ => x.max(y),
    }
}

/// A value as seen inside a checked run (the stricter-of input for the
/// run's own scope): [`P`] behaves as `Normal`, [`X`] as `Error`.
fn unpin(x: u8) -> u8 {
    match x {
        P => N,
        X => E,
        _ => x,
    }
}

/// A value pinned by a checked run: an error is trapped ([`X`]), anything
/// else is the run's commit ([`P`]).
fn pin(x: u8) -> u8 {
    if x == E { X } else { P }
}

/// The stricter-of composition of a behavior set `a` with a declaration
/// set `b` (`N`/`I`/`E` bits): every pair, composed by [`combine`].
#[must_use]
pub fn lift(a: u8, b: u8) -> u8 {
    let mut out = 0;
    for x in VALUES {
        if a & x == 0 {
            continue;
        }
        for y in [N, I, E] {
            if b & y != 0 {
                out |= combine(x, y);
            }
        }
    }
    out
}

/// A behavior set as the answers read it, on the bits [`N`], [`I`], [`E`]:
/// pinned ([`P`]) is normal; trapped ([`X`]) is a commit that may not happen
/// ([`N`] and [`I`]), never an error.
#[must_use]
pub fn norm(s: u8) -> u8 {
    let mut out = s & (N | I | E);
    if s & P != 0 {
        out |= N;
    }
    if s & X != 0 {
        out |= N | I;
    }
    out
}

/// Whether a route with behaviors `s` runs its commit as a commit, but not
/// on every reading: it may also be ignored, raise an error, or be trapped.
/// Such a route does not make the commit `unconditional on success`.
#[must_use]
pub fn uncertain(s: u8) -> bool {
    let n = norm(s);
    n & N != 0 && n != N
}

/// How an edge passes the caller's behavior on to its callee: ONE rule for
/// every way a call can start an object, decided where the call resolves
/// ([`CommitScopes::of_model`] reads the model's
/// [`crate::program::model::calls::CallEdge::starts_object`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Step {
    /// A call, an event dispatch, a trigger: the callee runs under the
    /// caller's behavior (and its own declaration, by the stricter-of rule).
    Plain,
    /// A call that starts an object (any `Run`, `RunModal`, a report's
    /// `SaveAs*`/`Execute`/`Print`, an XmlPort's `Import`/`Export`, a query's
    /// `Open`/`SaveAs*`; keyword or variable form) and is no checked
    /// `Codeunit.Run`: Learn does not say whether the scope follows into the
    /// started object, so it MAY. The callee runs under the caller's behavior
    /// or under its own declaration alone.
    Run,
    /// A checked `Codeunit.Run` (its result used): the run commits at its end
    /// whatever `[CommitBehavior]` says (Learn, "Codeunit.Run"), and traps
    /// any error inside. Whether the scope follows in is not stated (as for
    /// [`Step::Run`]), so both readings are kept: the callee's commits are
    /// pinned ([`P`]) or, where an `Error` may apply, trapped ([`X`]).
    CheckedRun,
}

impl Step {
    /// The behaviors the callee runs under (its own declaration `decl`
    /// applied), entered from a caller running under the single value `cur`.
    #[must_use]
    pub fn enter_one(self, cur: u8, decl: u8) -> u8 {
        let follow = lift(cur, decl);
        match self {
            Step::Plain => follow,
            Step::Run => {
                // The scope does not follow: the callee's own declaration
                // alone, still pinned when the caller is inside a checked run.
                let reset = if matches!(cur, P | X) {
                    lift(P, decl)
                } else {
                    decl
                };
                follow | reset
            }
            Step::CheckedRun => {
                let mut out = 0;
                for y in [N, I, E] {
                    if decl & y != 0 {
                        out |= pin(unpin(cur).max(y)) | pin(y);
                    }
                }
                out
            }
        }
    }

    /// [`Self::enter_one`] over every value of the set `cur`.
    #[must_use]
    pub fn enter(self, cur: u8, decl: u8) -> u8 {
        VALUES
            .iter()
            .filter(|&&v| cur & v != 0)
            .fold(0, |acc, &v| acc | self.enter_one(v, decl))
    }
}

/// Per value a routine can be entered under (its own declaration applied,
/// indexed as [`VALUES`]), the behaviors some commit runs under: a route's
/// transfer function, so that a "may" step deep in the graph composes
/// exactly with any declaration above it.
pub(crate) type Transfer = [u8; 5];

/// The union of `t` over the values of the set `s`.
fn apply(t: &Transfer, s: u8) -> u8 {
    VALUES
        .iter()
        .enumerate()
        .filter(|&(_, &v)| s & v != 0)
        .fold(0, |acc, (i, _)| acc | t[i])
}

/// Where a routine's declaration comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclSource {
    /// The routine's own `[CommitBehavior(…)]`.
    Attribute,
    /// Its object's `InherentCommitBehavior` (the routine has no attribute).
    ObjectProperty,
    /// A `[CommitBehavior(…)]` only some builds of the routine have (it sits
    /// in an `#if` arm the routine is not in, engine limits B2). No build
    /// configuration is chosen, so it MAY apply: the states keep what a build
    /// without it runs under (the object property's, else normal).
    ConditionalAttribute,
}

/// A routine's commit-behavior declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Declared {
    /// The behaviors the routine's scope may set ([`N`]/[`I`]/[`E`] bits).
    pub states: u8,
    pub source: DeclSource,
}

impl Declared {
    /// Two declarations of one id (two `#if` arms, engine limits B2): every
    /// behavior either may set. The source stays the first's unless the
    /// second is a definite attribute.
    #[must_use]
    pub fn union(self, other: Declared) -> Declared {
        Declared {
            states: self.states | other.states,
            source: if other.source == DeclSource::Attribute {
                other.source
            } else {
                self.source
            },
        }
    }

    /// The declared value's word: `Ignore` or `Error`.
    #[must_use]
    pub fn word(self) -> &'static str {
        if self.states & E != 0 {
            "Error"
        } else {
            "Ignore"
        }
    }

    /// `CommitBehavior Error on <routine>` (or the object property's form).
    #[must_use]
    pub fn reason(self, routine_display: &str) -> String {
        match self.source {
            DeclSource::Attribute => format!("CommitBehavior {} on {routine_display}", self.word()),
            DeclSource::ObjectProperty => format!(
                "InherentCommitBehavior {} on the object of {routine_display} (may apply)",
                self.word()
            ),
            DeclSource::ConditionalAttribute => format!(
                "CommitBehavior {} on {routine_display} in some builds only (may apply)",
                self.word()
            ),
        }
    }
}

/// THE rule for one routine: its own `[CommitBehavior(Ignore|Error)]`, else
/// its object's `InherentCommitBehavior` as a possibility; `None` when it
/// declares nothing (it runs its callees under its caller's behavior).
///
/// Engine limits B2: an attribute only some builds have
/// (`AttributeInfo::conditional`) is a possibility too, never a definite
/// declaration. Its values join what a build without it runs under (the
/// object property's reading, else [`N`]), so a commit stays possible and no
/// definite "no" rests on a build nobody chose.
///
/// `r` is ONE declaration (row) of the routine, and this is that row's own
/// declaration: an attribute another declaration lacks
/// (`AttributeInfo::another_declaration_lacks`) still holds in every build in
/// which this row exists, so it is definite here (wave 2 final review). The
/// routine's other rows are joined by the readers that need the routine as a
/// whole ([`CommitScopes`]' per-id union for the routine graph's hub node).
#[must_use]
pub fn declared(r: &ModelRoutine, object: Option<&ModelObject>) -> Option<Declared> {
    use crate::engine::return_summary::commit_behavior_value;
    let without_attribute = match object.and_then(|o| o.inherent_commit_behavior.as_deref()) {
        Some("ignore") => Some(Declared {
            states: N | I,
            source: DeclSource::ObjectProperty,
        }),
        Some("error") => Some(Declared {
            states: N | E,
            source: DeclSource::ObjectProperty,
        }),
        _ => None,
    };
    let own = crate::program::attributes::find_own_definite_attribute(
        &r.attributes_parsed,
        "CommitBehavior",
    )
    .map_or("normal", commit_behavior_value);
    match own {
        "ignore" => Some(Declared {
            states: I,
            source: DeclSource::Attribute,
        }),
        "error" => Some(Declared {
            states: E,
            source: DeclSource::Attribute,
        }),
        _ => {
            let may = r
                .attributes_parsed
                .iter()
                .filter(|a| a.conditional && a.name.eq_ignore_ascii_case("CommitBehavior"))
                .fold(0, |s, a| {
                    s | match commit_behavior_value(a) {
                        "ignore" => I,
                        "error" => E,
                        _ => 0,
                    }
                });
            if may == 0 {
                return without_attribute;
            }
            Some(Declared {
                states: may | without_attribute.map_or(N, |d| d.states),
                source: DeclSource::ConditionalAttribute,
            })
        }
    }
}

/// Every declaring routine of a model, by stable routine id (the snapshot's
/// routine identity). Empty for nearly every workspace: then nothing below
/// runs.
#[derive(Debug, Clone, Default)]
pub struct CommitScopes {
    /// Per routine id: every row's declaration together (a union, with
    /// [`N`] for a row that declares nothing). What a reader of the routine
    /// as a whole sees.
    by_routine: HashMap<String, Declared>,
    /// The object-run call sites (by call-site id): [`Step::CheckedRun`]
    /// for a checked `Codeunit.Run`, else [`Step::Run`]. Filled only when
    /// some routine declares a behavior.
    runs: HashMap<String, Step>,
    /// Engine limits B2 (wave 2 final review): a routine declared more than
    /// once (`#if` arms, two files) whose rows declare different behaviors.
    /// A build runs ONE row's body, so a commit in a row (or below one of its
    /// call sites) runs under THAT row's own declaration, definite for it.
    /// Per such id (stable id): each row's own declaration bits ([`N`] when
    /// it declares nothing). [`CbGraph`] gives each row its own node.
    arms: HashMap<String, Vec<u8>>,
    /// The row ([`Self::arms`] index) that holds each site of those ids,
    /// with its routine id (internal site id → (stable id, row)).
    site_arm: HashMap<String, (String, usize)>,
}

impl CommitScopes {
    /// The declarations of `model`'s routines, and (when there is one) its
    /// object-run call sites: those of `model.calls` and of
    /// `dependency_calls`, the calls of dependency code a cross-app base keeps
    /// apart from the model (empty when the model holds every call).
    #[must_use]
    pub fn of_model(
        model: &Model,
        dependency_calls: &[crate::program::model::calls::CallEdge],
    ) -> CommitScopes {
        let ws = &model.workspace;
        // Engine limits B2 (per-arm ids): an object or a routine declared in
        // two `#if` arms is one row per arm under one id. Each build runs one
        // arm, so the id's declaration is every row's possibilities together
        // (a union of the behavior sets), never the last row's alone.
        let mut objects: HashMap<&str, Vec<&ModelObject>> = HashMap::new();
        for o in ws
            .objects
            .iter()
            .filter(|o| o.inherent_commit_behavior.is_some())
        {
            objects.entry(o.id.as_str()).or_default().push(o);
        }
        let mut by_routine: HashMap<String, Declared> = HashMap::new();
        // The ids with a row that declares nothing: that row's build runs
        // its commits normally.
        let mut plain_row: std::collections::HashSet<&str> = std::collections::HashSet::new();
        // Each row's own declaration bits, by stable id, in model order.
        let mut per_row: HashMap<&str, Vec<(&ModelRoutine, u8)>> = HashMap::new();
        // A row reads its OWN declaring object (#279: an object declared in
        // two `#if` arms is two object rows); one it cannot place reads every
        // object row of its id.
        let declaring = crate::program::model::declaring::DeclaringObjects::new(&ws.objects);
        for r in &ws.routines {
            let d = match (declaring.of(r), objects.get(r.object_id.as_str())) {
                (_, None) => declared(r, None),
                (Some(o), Some(_)) => declared(r, Some(o)),
                (None, Some(rows)) => rows
                    .iter()
                    .filter_map(|o| declared(r, Some(o)))
                    .reduce(Declared::union),
            };
            per_row
                .entry(r.stable_routine_id.as_str())
                .or_default()
                .push((r, d.map_or(N, |d| d.states)));
            match d {
                None => {
                    plain_row.insert(r.stable_routine_id.as_str());
                }
                Some(d) => {
                    let slot = by_routine.entry(r.stable_routine_id.clone()).or_insert(d);
                    *slot = slot.union(d);
                }
            }
        }
        for (id, d) in &mut by_routine {
            if plain_row.contains(id.as_str()) {
                d.states |= N;
            }
        }
        if by_routine.is_empty() {
            return CommitScopes::default();
        }
        // The rows of a declaring id declared more than once, when they
        // disagree: each its own node in the graph.
        let mut arms: HashMap<String, Vec<u8>> = HashMap::new();
        let mut site_arm: HashMap<String, (String, usize)> = HashMap::new();
        for (id, rows) in per_row {
            if rows.len() < 2 || rows.iter().all(|(_, s)| *s == rows[0].1) {
                continue;
            }
            for (k, (r, _)) in rows.iter().enumerate() {
                let sites = r
                    .call_sites
                    .iter()
                    .flat_map(|c| [c.id.as_str(), c.operation_id.as_str()])
                    .chain(r.operation_sites.iter().map(|o| o.id.as_str()))
                    .chain(r.record_operations.iter().map(|o| o.id.as_str()))
                    .chain(r.field_ref_operations.iter().map(|o| o.id.as_str()));
                for s in sites {
                    site_arm.insert(s.to_string(), (id.to_string(), k));
                }
            }
            arms.insert(id.to_string(), rows.iter().map(|(_, s)| *s).collect());
        }
        CommitScopes {
            by_routine,
            runs: object_runs(model, dependency_calls),
            arms,
            site_arm,
        }
    }

    /// The row of routine `routine` that holds `site`, when the routine is
    /// one of [`Self::arms`]'.
    fn arm_of(&self, routine: &str, site: Option<&str>) -> Option<usize> {
        let (id, k) = self.site_arm.get(site?)?;
        (id == routine).then_some(*k)
    }

    /// The declaration bits a route through `routine` runs under when it
    /// leaves the routine at `site` (its next call site, or the commit
    /// itself): that row's own for a routine of [`Self::arms`] whose row
    /// holds the site, else the routine's (every row's together).
    #[must_use]
    pub fn states_leaving(&self, routine: &str, site: Option<&str>) -> u8 {
        if let (Some(rows), Some(k)) = (self.arms.get(routine), self.arm_of(routine, site)) {
            return rows[k];
        }
        self.get(routine).map_or(N, |d| d.states)
    }

    /// How the edge at `site` passes the behavior on: a call site, or the
    /// record operation of a trigger or table-event edge (a dataset
    /// operation of a started object takes its run's step).
    #[must_use]
    pub fn step(&self, site: Option<&str>) -> Step {
        site.and_then(|c| self.runs.get(c).copied())
            .unwrap_or(Step::Plain)
    }

    /// The step of a snapshot edge: by its call site, else by its record
    /// operation.
    #[must_use]
    pub fn step_of(&self, e: &SnapshotGraphEdge) -> Step {
        self.step(e.edge_callsite_id().or_else(|| e.edge_operation_id()))
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_routine.is_empty()
    }

    #[must_use]
    pub fn get(&self, routine: &str) -> Option<Declared> {
        self.by_routine.get(routine).copied()
    }
}

/// Every call site of `model` that starts an object, with its [`Step`]. What
/// starts an object is decided ONCE, where the call resolves: the model's
/// [`crate::program::model::calls::CallEdge::starts_object`] (the
/// resolver's rule, `program::resolve::dataset::starts_object`, for every
/// kind, form and method). Of those, a checked `Codeunit.Run` is told by the
/// one rule the trap and transaction answers use
/// ([`crate::engine::l5::traps::is_checked_run`]). A dataset operation of a
/// started object (its trigger and table-event edges are keyed by the
/// operation) takes its run's step.
///
/// `dependency_calls` are the calls dependency routines make, when the
/// cross-app base moved them out of `model.calls` (the execution relation's
/// split, `capability_cone`): the graph walks them too, so a run inside
/// dependency code needs its step as much as a workspace one (B4 round 3).
fn object_runs(
    model: &Model,
    dependency_calls: &[crate::program::model::calls::CallEdge],
) -> HashMap<String, Step> {
    use crate::engine::l5::traps::is_checked_run;
    use crate::program::resolve::dataset::ObjectStart;
    let ws = &model.workspace;
    let mut runs: HashMap<String, Step> = HashMap::new();
    let edges = || {
        model
            .calls
            .edges
            .iter()
            .chain(dependency_calls)
            .filter(|e| e.starts_object != ObjectStart::No)
    };
    if edges().next().is_none() {
        return runs;
    }
    let by_id: HashMap<&str, &ModelRoutine> =
        ws.routines.iter().map(|r| (r.id.as_str(), r)).collect();
    let sites: HashMap<&str, &crate::program::body::features::PCallSite> = ws
        .routines
        .iter()
        .flat_map(|r| r.call_sites.iter().map(|cs| (cs.id.as_str(), cs)))
        .collect();
    for e in edges() {
        let Some(cs) = sites.get(e.callsite_id.as_str()) else {
            continue;
        };
        let callee = e.to.as_deref().and_then(|t| by_id.get(t)).copied();
        // A call the resolver could not bind (the start or the object's own
        // procedure, B6) is a plain call or a start: `Step::Run` covers both
        // (the scope follows, or MAY not), so no definite answer rests on
        // the binding the engine could not make.
        let step = if e.starts_object == ObjectStart::Yes && is_checked_run(cs, callee) {
            Step::CheckedRun
        } else {
            Step::Run
        };
        // One site, one step; of several routes (a report's triggers) the
        // checked reading wins, which keeps a commit (never a false "no").
        let slot = runs.entry(e.callsite_id.clone()).or_insert(step);
        *slot = (*slot).max(step);
    }
    for r in &ws.routines {
        for op in r
            .record_operations
            .iter()
            .filter(|o| o.dataset_reason.is_some())
        {
            let step = crate::program::model::program_calls::dataset_op_callsite(&op.id)
                .and_then(|cs| runs.get(cs).copied());
            if let Some(step) = step {
                runs.insert(op.id.clone(), step);
            }
        }
    }
    runs
}

/// The snapshot's call graph with the declarations on its nodes, for one
/// digest read (built once per read, shared by every root). Edge set: every
/// typed edge with a target (calls, object runs, event dispatch, triggers,
/// table events), a superset of what the cone follows: an extra route can
/// only keep a commit or add a possible error, never drop one.
pub(crate) struct CbGraph<'a> {
    ix: HashMap<String, u32>,
    names: Vec<String>,
    /// `None`: a hub edge (a routine of [`CommitScopes::arms`] to one of its
    /// row nodes), which no route lists.
    succ: Vec<Vec<(u32, Option<&'a SnapshotGraphEdge>, Step)>>,
    /// A routine of [`CommitScopes::arms`] (its hub node) → its row nodes
    /// and its catch-all node (an edge or a commit no row's site names).
    arm_nodes: HashMap<String, (Vec<u32>, u32)>,
    pred: Vec<Vec<(u32, Step)>>,
    /// Each node's declaration bits ([`N`] when it declares nothing).
    decl: Vec<u8>,
    /// Reachable from a declaring node (the node itself included): a commit
    /// in a node outside it runs normally from every root.
    zone: Vec<bool>,
    /// The declaration bits of the declaring nodes a node can reach (itself
    /// included); 0 = none.
    anc: Vec<u8>,
    /// Whether a node holds a commit of its own.
    carrier: Vec<bool>,
    /// Per node, the behaviors some commit can run under, by the value the
    /// node runs under ([`Transfer`]; all 0: it reaches none).
    any_commit: Vec<Transfer>,
    /// Per commit origin, the same for that origin's commit alone.
    toward: Mutex<HashMap<u32, Arc<Vec<Transfer>>>>,
    /// As `toward`, with a checked run read as an object run whose scope MAY
    /// follow ([`Self::commit_surely_runs`]).
    toward_explicit: Mutex<HashMap<u32, Arc<Vec<Transfer>>>>,
    scopes: &'a CommitScopes,
}

/// A route the graph found: its edges, root side first.
pub(crate) type Route<'a> = Vec<&'a SnapshotGraphEdge>;

/// What each routine one root reaches runs under ([`CbGraph::states_from`]).
pub(crate) struct RootStates(HashMap<u32, u8>);

impl RootStates {
    /// The behaviors node `x` runs under from the root; 0 when the root does
    /// not reach it.
    fn get(&self, x: u32) -> u8 {
        self.0.get(&x).copied().unwrap_or(0)
    }
}

impl<'a> CbGraph<'a> {
    /// The graph of a tree: its typed edges and the routines with a direct
    /// commit fact. `None` when no routine declares a behavior.
    pub(crate) fn of_tree(
        tree: &'a crate::engine::l5::snapshot::CapabilitySnapshot,
    ) -> Option<CbGraph<'a>> {
        CbGraph::build(
            &tree.commit_scopes,
            tree.typed_edges.iter(),
            tree.capability_facts
                .iter()
                .filter(|f| f.op() == "commit" && f.provenance() == "direct" && !is_run_commit(f))
                .map(|f| (f.subject(), f.witness_operation_id())),
        )
    }

    /// `None` when no routine declares a behavior (the common case: nothing
    /// to do). `edges` is the snapshot's typed edges (each routine's in
    /// source order), `carriers` the routines with a commit of their own,
    /// each with the commit's operation id (which names its row).
    ///
    /// Engine limits B2 (wave 2 final review): a routine of
    /// [`CommitScopes::arms`] (declared more than once, its rows declaring
    /// different behaviors) is a HUB node that declares nothing, with an edge
    /// to one node per row (that row's own declaration) and to a catch-all
    /// node (every row's declarations together). A row node holds the
    /// out-edges whose site the row holds and the commits it holds; the
    /// catch-all holds an edge or a commit no row's site names. So a commit
    /// in a row runs under that row's declaration, never under another
    /// row's: `[CommitBehavior(Ignore)]` on the one declaration that commits
    /// ignores that commit in every build that has it.
    pub(crate) fn build(
        scopes: &'a CommitScopes,
        edges: impl IntoIterator<Item = &'a SnapshotGraphEdge>,
        carriers: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
    ) -> Option<CbGraph<'a>> {
        if scopes.is_empty() {
            return None;
        }
        let mut ix: HashMap<String, u32> = HashMap::new();
        let mut names: Vec<String> = Vec::new();
        let mut intern = |s: &str, ix: &mut HashMap<String, u32>| -> u32 {
            if let Some(&i) = ix.get(s) {
                return i;
            }
            names.push(s.to_string());
            let i = (names.len() - 1) as u32;
            ix.insert(s.to_string(), i);
            i
        };
        let arm_key = |id: &str, k: usize| format!("{id}\u{1f}{k}");
        let mixed_key = |id: &str| format!("{id}\u{1f}*");
        // The node a route through `routine` leaving it at `site` is in.
        let leaving = |routine: &str, site: Option<&str>| -> String {
            if !scopes.arms.contains_key(routine) {
                return routine.to_string();
            }
            match scopes.arm_of(routine, site) {
                Some(k) => arm_key(routine, k),
                None => mixed_key(routine),
            }
        };
        // By source routine in id order (a deterministic search order),
        // each routine's edges in their own order.
        let mut outgoing: std::collections::BTreeMap<
            &'a str,
            Vec<(&'a str, &'a SnapshotGraphEdge)>,
        > = std::collections::BTreeMap::new();
        for e in edges {
            if let Some(to) = e.edge_to() {
                outgoing.entry(e.edge_from()).or_default().push((to, e));
            }
        }
        let mut raw: Vec<(u32, u32, Option<&'a SnapshotGraphEdge>)> = Vec::new();
        for (from, out) in outgoing {
            for (to, e) in out {
                let site = e.edge_callsite_id().or_else(|| e.edge_operation_id());
                let f = intern(&leaving(from, site), &mut ix);
                let t = intern(to, &mut ix);
                raw.push((f, t, Some(e)));
            }
        }
        let carriers: Vec<u32> = carriers
            .into_iter()
            .map(|(c, op)| intern(&leaving(c, op), &mut ix))
            .collect();
        // Every hub: its row nodes and catch-all, in id order.
        let mut arm_ids: Vec<&String> = scopes.arms.keys().collect();
        arm_ids.sort();
        let mut arm_nodes: HashMap<String, (Vec<u32>, u32)> = HashMap::new();
        for id in arm_ids {
            let hub = intern(id, &mut ix);
            let rows: Vec<u32> = (0..scopes.arms[id].len())
                .map(|k| intern(&arm_key(id, k), &mut ix))
                .collect();
            let mixed = intern(&mixed_key(id), &mut ix);
            for &x in rows.iter().chain(std::iter::once(&mixed)) {
                raw.push((hub, x, None));
            }
            arm_nodes.insert(id.clone(), (rows, mixed));
        }
        let n = names.len();
        let mut succ: Vec<Vec<(u32, Option<&'a SnapshotGraphEdge>, Step)>> = vec![Vec::new(); n];
        let mut pred: Vec<Vec<(u32, Step)>> = vec![Vec::new(); n];
        for (f, t, e) in raw {
            let step = e.map_or(Step::Plain, |e| scopes.step_of(e));
            succ[f as usize].push((t, e, step));
            pred[t as usize].push((f, step));
        }
        for p in &mut pred {
            p.sort_unstable();
            p.dedup();
        }
        let mut decl: Vec<u8> = names
            .iter()
            .map(|s| scopes.get(s).map_or(N, |d| d.states))
            .collect();
        for (id, (rows, mixed)) in &arm_nodes {
            decl[ix[id.as_str()] as usize] = N;
            for (k, &x) in rows.iter().enumerate() {
                decl[x as usize] = scopes.arms[id][k];
            }
            decl[*mixed as usize] = scopes.get(id).map_or(N, |d| d.states);
        }
        let declaring: Vec<u32> = (0..n as u32).filter(|&i| decl[i as usize] != N).collect();
        // Forward: the zone.
        let mut zone = vec![false; n];
        let mut stack: Vec<u32> = declaring.clone();
        for &d in &declaring {
            zone[d as usize] = true;
        }
        while let Some(x) = stack.pop() {
            for &(y, _, _) in &succ[x as usize] {
                if !zone[y as usize] {
                    zone[y as usize] = true;
                    stack.push(y);
                }
            }
        }
        // Backward: which declarations each node can reach.
        let backward_bits = |seeds: &[u32]| -> Vec<u8> {
            let mut bits = vec![0u8; n];
            let mut stack: Vec<u32> = Vec::new();
            for &d in seeds {
                bits[d as usize] |= decl[d as usize] & (I | E);
                stack.push(d);
            }
            while let Some(x) = stack.pop() {
                let b = bits[x as usize];
                for &(p, _) in &pred[x as usize] {
                    if bits[p as usize] | b != bits[p as usize] {
                        bits[p as usize] |= b;
                        stack.push(p);
                    }
                }
            }
            bits
        };
        let anc = backward_bits(&declaring);
        let mut carrier = vec![false; n];
        for &c in &carriers {
            carrier[c as usize] = true;
        }
        let mut g = CbGraph {
            ix,
            names,
            succ,
            arm_nodes,
            pred,
            decl,
            zone,
            anc,
            carrier,
            any_commit: Vec::new(),
            toward: Mutex::new(HashMap::new()),
            toward_explicit: Mutex::new(HashMap::new()),
            scopes,
        };
        g.any_commit = g.backward_states(&carriers, true);
        Some(g)
    }

    /// The backward fixpoint from `origins`: per node and per value the node
    /// runs under (its own declaration applied), the behaviors a commit at
    /// one of them can run under ([`P`]/[`X`] kept: read them through
    /// [`norm`]). A transfer function rather than one set, so that an edge's
    /// "may" ([`Step::Run`], [`Step::CheckedRun`]) composes exactly with a
    /// declaration above it, wherever the two meet. With `pin` false a
    /// checked run is read as [`Step::Run`] (see
    /// [`Self::commit_surely_runs`]).
    fn backward_states(&self, origins: &[u32], pin: bool) -> Vec<Transfer> {
        let n = self.names.len();
        let mut t: Vec<Transfer> = vec![[0u8; 5]; n];
        let mut stack: Vec<u32> = Vec::new();
        for &o in origins {
            for (i, v) in VALUES.iter().enumerate() {
                t[o as usize][i] |= v;
            }
            stack.push(o);
        }
        while let Some(x) = stack.pop() {
            let tx = t[x as usize];
            let dx = self.decl[x as usize];
            for &(p, step) in &self.pred[x as usize] {
                let step = if !pin && step == Step::CheckedRun {
                    Step::Run
                } else {
                    step
                };
                let mut new = t[p as usize];
                for (i, &v) in VALUES.iter().enumerate() {
                    new[i] |= apply(&tx, step.enter_one(v, dx));
                }
                if new != t[p as usize] {
                    t[p as usize] = new;
                    stack.push(p);
                }
            }
        }
        t
    }

    fn node(&self, id: &str) -> Option<u32> {
        self.ix.get(id).copied()
    }

    /// A routine's declaration bits ([`N`] for an unknown one).
    pub(crate) fn decl_of(&self, id: &str) -> u8 {
        self.node(id).map_or(N, |x| self.decl[x as usize])
    }

    /// Whether nothing on any route from `root` declares a behavior: the
    /// root's commits all run normally.
    pub(crate) fn root_unaffected(&self, root: &str) -> bool {
        self.node(root).is_none_or(|r| self.anc[r as usize] == 0)
    }

    /// The nodes that hold the commits of routine `origin`, and the node that
    /// names it (the cache key): the routine's own node, or for a routine of
    /// [`CommitScopes::arms`] its row nodes (and catch-all) that hold a commit
    /// (every one when none does).
    fn origin_nodes(&self, origin: &str) -> Option<(u32, Vec<u32>)> {
        let key = self.node(origin)?;
        let Some((rows, mixed)) = self.arm_nodes.get(origin) else {
            return Some((key, vec![key]));
        };
        let all: Vec<u32> = rows.iter().copied().chain([*mixed]).collect();
        let carrying: Vec<u32> = all
            .iter()
            .copied()
            .filter(|&x| self.carrier[x as usize])
            .collect();
        Some((key, if carrying.is_empty() { all } else { carrying }))
    }

    /// Whether a commit in `origin` can run under a declared behavior from
    /// any root.
    pub(crate) fn in_zone(&self, origin: &str) -> bool {
        self.origin_nodes(origin)
            .is_some_and(|(_, os)| os.iter().any(|&o| self.zone[o as usize]))
    }

    fn toward(&self, o: u32) -> Arc<Vec<Transfer>> {
        if let Some(t) = self
            .toward
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&o)
        {
            return Arc::clone(t);
        }
        let origins = self
            .origin_nodes(&self.names[o as usize])
            .map_or_else(|| vec![o], |(_, os)| os);
        let t = Arc::new(self.backward_states(&origins, true));
        self.toward
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(o)
            .or_insert(t)
            .clone()
    }

    /// Whether an explicit `Commit()` in `origin` runs normally on EVERY
    /// route from `root` (entered normally): a question that relies on the
    /// commit HAPPENING (B7: a `Commit()` that ends the write a checked
    /// `Codeunit.Run` after it would otherwise run inside). Under `Ignore`
    /// the write stays pending; under `Error` the commit raises an error
    /// and commits nothing. The rule is the one above ([`Step`], [`lift`]),
    /// with one difference in the safe direction for this question: a
    /// checked run is read as [`Step::Run`] (the scope MAY follow into it),
    /// not pinned. The pin stands for the run's IMPLICIT commit, which no
    /// declaration changes; whether a caller's scope reaches an EXPLICIT
    /// commit inside the run is not on Microsoft Learn. `false` when the
    /// graph has no route from `root` to `origin` (a graph mismatch).
    pub(crate) fn commit_surely_runs(&self, root: &str, origin: &str) -> bool {
        let Some((o, origins)) = self.origin_nodes(origin) else {
            // No edge and no commit fact: only its own declaration applies.
            return self.scopes.get(origin).is_none();
        };
        if !origins.iter().any(|&x| self.zone[x as usize]) {
            return true;
        }
        let Some(r) = self.node(root) else {
            return false;
        };
        // The root is entered normally, so it runs under its own
        // declaration alone (`lift(N, decl) == decl`).
        let at_root = |t: &[Transfer]| apply(&t[r as usize], self.decl[r as usize]) == N;
        if let Some(t) = self
            .toward_explicit
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&o)
        {
            return at_root(t);
        }
        let t: Arc<Vec<Transfer>> = Arc::new(self.backward_states(&origins, false));
        let ok = at_root(&t);
        self.toward_explicit
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(o)
            .or_insert(t);
        ok
    }

    /// The behaviors the commit at `origin` can run under on any route from
    /// `root`; `None` when the graph has no route (a graph mismatch: the
    /// caller then keeps the commit as it is).
    pub(crate) fn states_at(&self, root: &str, origin: &str) -> Option<u8> {
        let t = self.raw_states_at(root, origin)?;
        (t != 0).then(|| norm(t))
    }

    /// [`Self::states_at`] with [`P`]/[`X`] kept; `Some(0)` when the root
    /// reaches no such commit.
    fn raw_states_at(&self, root: &str, origin: &str) -> Option<u8> {
        let (r, o) = (self.node(root)?, self.node(origin)?);
        Some(apply(&self.toward(o)[r as usize], self.decl[r as usize]))
    }

    /// How the edge at `site` (a call site or a record operation) passes
    /// the behavior on.
    pub(crate) fn step_at(&self, site: Option<&str>) -> Step {
        self.scopes.step(site)
    }

    /// The behaviors of a listed route, [`P`]/[`X`] kept (read them through
    /// [`norm`], or pass them to [`Self::beyond`]): the root's declaration,
    /// then each hop `(routine, call site)` entered by its edge's [`Step`].
    /// `terminal` is the route's last site (the commit's operation id; `None`
    /// for a route that ends at a crossing). A routine declared more than
    /// once ([`CommitScopes::arms`]) runs under the declaration of the row the
    /// route leaves it through (the next hop's call site, or `terminal`),
    /// else every row's together.
    pub(crate) fn route_states<'s>(
        &self,
        root: &str,
        hops: impl Iterator<Item = (&'s str, Option<&'s str>)>,
        terminal: Option<&str>,
    ) -> u8 {
        let hops: Vec<(&str, Option<&str>)> = hops.collect();
        let leaving = |i: usize| -> Option<&str> {
            match hops.get(i) {
                Some(&(_, cs)) => cs,
                None => terminal,
            }
        };
        let decl = |id: &str, i: usize| self.scopes.states_leaving(id, leaving(i));
        hops.iter()
            .enumerate()
            .fold(decl(root, 0), |s, (i, &(h, cs))| {
                self.scopes.step(cs).enter(s, decl(h, i + 1))
            })
    }

    /// The behaviors `entry` runs under (its own declaration applied) when
    /// it is entered by an edge of `step` from a caller in `prefix`.
    pub(crate) fn entered(&self, prefix: u8, step: Step, entry: &str) -> u8 {
        step.enter(prefix, self.decl_of(entry))
    }

    /// What every routine reachable from `root` runs under ([`P`]/[`X`]
    /// kept), over every route of the graph: one forward pass over the
    /// root's reach. It equals [`Self::raw_states_at`] for each routine (the
    /// steps distribute over a union of values), at the cost of one walk of
    /// the root's reach instead of a whole-graph backward pass per routine
    /// asked. The digest builds it once per root ([`Self::beyond`]).
    pub(crate) fn states_from(&self, root: &str) -> RootStates {
        let mut s: HashMap<u32, u8> = HashMap::new();
        let Some(r) = self.node(root) else {
            return RootStates(s);
        };
        s.insert(r, self.decl[r as usize]);
        let mut stack = vec![r];
        while let Some(x) = stack.pop() {
            let sx = s[&x];
            for &(y, _, step) in &self.succ[x as usize] {
                let add = step.enter(sx, self.decl[y as usize]);
                let slot = s.entry(y).or_insert(0);
                if *slot | add != *slot {
                    *slot |= add;
                    stack.push(y);
                }
            }
        }
        RootStates(s)
    }

    /// The behaviors a commit beyond a crossing can run under: the crossing
    /// edge `from → entry` of `step` is entered from the listed prefix
    /// (`prefix`, from [`Self::route_states`]) and from every other route of
    /// the graph from the root to `from` (`at_root`, the root's
    /// [`Self::states_from`]: a prefix the witness did not list may pass
    /// another declaration), and each commit `entry` reaches is composed
    /// with that. When the graph knows no commit beyond `entry`, the commit
    /// runs under what `entry` itself runs under.
    pub(crate) fn beyond(
        &self,
        at_root: &RootStates,
        prefix: u8,
        from: &str,
        entry: &str,
        step: Step,
    ) -> u8 {
        let pfx = prefix | self.node(from).map_or(0, |f| at_root.get(f));
        let entered = self.entered(pfx, step, entry);
        let reach = self
            .node(entry)
            .map_or(0, |e| apply(&self.any_commit[e as usize], entered));
        if reach == 0 {
            return norm(entered);
        }
        norm(reach)
    }

    /// One route from `root` to the commit at `origin` that runs it under
    /// `target` (a single bit of [`norm`]), shortest first; `None` when there
    /// is none.
    pub(crate) fn route_to(&self, root: &str, origin: &str, target: u8) -> Option<Route<'a>> {
        let r = self.node(root)?;
        let (o, origins) = self.origin_nodes(origin)?;
        let guide = self.toward(o);
        self.guided_from(r, self.decl[r as usize], &guide, target, |x| {
            origins.contains(&x)
        })
    }

    /// One route from `entry` to some commit it reaches that runs under
    /// `target`, once `entry` runs under `start` (its declaration applied,
    /// [`Self::entered`]).
    pub(crate) fn route_beyond(&self, entry: &str, start: u8, target: u8) -> Option<Route<'a>> {
        let e = self.node(entry)?;
        let guide = &self.any_commit;
        let carrier = &self.carrier;
        self.guided_from(e, start, guide, target, |x| carrier[x as usize])
    }

    /// Breadth-first over (node, behavior) from `s` running under the set
    /// `start`: a step is taken only where `target` stays possible
    /// (`guide`), and the walk ends at a goal node reached under a value that
    /// reads as `target` ([`norm`]).
    fn guided_from(
        &self,
        s: u32,
        start: u8,
        guide: &[Transfer],
        target: u8,
        goal: impl Fn(u32) -> bool,
    ) -> Option<Route<'a>> {
        let admissible = |x: u32, cur: u8| norm(guide[x as usize][vix(cur)]) & target != 0;
        // A search state: a node and the behavior it runs under.
        type State = (u32, u8);
        let mut parent: HashMap<State, Option<(State, Option<&'a SnapshotGraphEdge>)>> =
            HashMap::new();
        let mut queue = std::collections::VecDeque::new();
        for a in VALUES {
            if start & a != 0 && admissible(s, a) {
                parent.insert((s, a), None);
                queue.push_back((s, a));
            }
        }
        while let Some((x, cur)) = queue.pop_front() {
            if norm(cur) & target != 0 && goal(x) {
                let mut route = Vec::new();
                let mut at = (x, cur);
                while let Some(Some((prev, e))) = parent.get(&at) {
                    // A hub edge (to a row node) is no edge of the route.
                    route.extend(*e);
                    at = *prev;
                }
                route.reverse();
                return Some(route);
            }
            for &(y, e, step) in &self.succ[x as usize] {
                let entered = step.enter_one(cur, self.decl[y as usize]);
                for next in VALUES {
                    if entered & next == 0 {
                        continue;
                    }
                    if admissible(y, next) && !parent.contains_key(&(y, next)) {
                        parent.insert((y, next), Some(((x, cur), e)));
                        queue.push_back((y, next));
                    }
                }
            }
        }
        None
    }

    /// The routine that sets `target` on a route (`nodes`, root side
    /// first): the first whose declaration holds it, with its declaration.
    pub(crate) fn cause<'s>(
        &self,
        nodes: impl Iterator<Item = &'s str>,
        target: u8,
    ) -> Option<(&'s str, Declared)> {
        nodes
            .filter_map(|id| self.scopes.get(id).map(|d| (id, d)))
            .find(|(_, d)| d.states & target != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lift_takes_the_stricter_of_each_pair() {
        assert_eq!(lift(N, N), N);
        assert_eq!(lift(N, I), I);
        assert_eq!(lift(I, E), E);
        // Learn: under Error an inner Ignore keeps Error.
        assert_eq!(lift(E, I), E);
        assert_eq!(lift(N | I, N), N | I);
        assert_eq!(lift(N | E, I), I | E);
        // Inside a checked run: Ignore keeps the run's commit, Error makes
        // the commit an error the run traps.
        assert_eq!(lift(P, I), P);
        assert_eq!(lift(P, E), X);
        assert_eq!(lift(X, N), X);
        assert_eq!(norm(P | I), N | I);
        // Trapped: may commit, never an error row.
        assert_eq!(norm(X), N | I);
        assert!(uncertain(X) && uncertain(N | I) && !uncertain(P) && !uncertain(I));
    }

    #[test]
    fn a_run_step_may_reset_and_a_checked_run_pins() {
        // An object run: the caller's Ignore may or may not follow.
        assert_eq!(Step::Run.enter(I, N), I | N);
        // A checked run: its commit stays (the implicit commit) under an
        // Ignore that may follow in; under an Error that may follow in it
        // may be trapped instead.
        assert_eq!(Step::CheckedRun.enter(I, N), P);
        assert_eq!(Step::CheckedRun.enter(E, N), P | X);
        assert_eq!(Step::CheckedRun.enter(N, E), X);
        assert_eq!(Step::Plain.enter(I, N), I);
    }

    fn edge(from: &str, to: &str, callsite: &str) -> SnapshotGraphEdge {
        use crate::engine::l5::snapshot::{SnapshotRange, SnapshotSourceAnchor};
        SnapshotGraphEdge::DirectCall {
            kind: "direct-call",
            callsite_id: callsite.to_string(),
            from: from.to_string(),
            to: to.to_string(),
            source_anchor: SnapshotSourceAnchor {
                source_unit_id: "f".to_string(),
                range: SnapshotRange {
                    start_line: 0,
                    start_column: 0,
                    end_line: 0,
                    end_column: 0,
                },
                enclosing_routine_id: from.to_string(),
                syntax_kind: "call".to_string(),
            },
            edge_id: format!("{from}>{to}"),
        }
    }

    /// The precondition is stated by hand: `Root` declares Ignore and calls
    /// `A`; `A` starts `B` by an object run (call site `run`, a [`Step::Run`]
    /// by assignment); `B` commits. The scope may not follow into the run,
    /// so the commit may run normally: a one-set backward pass composed the
    /// run under `A`'s normal entry first and then applied Root's Ignore to
    /// the result, a definite (wrong) "ignored".
    #[test]
    fn a_run_below_a_declaration_keeps_its_may() {
        let mut by_routine = HashMap::new();
        by_routine.insert(
            "Root".to_string(),
            Declared {
                states: I,
                source: DeclSource::Attribute,
            },
        );
        let mut runs = HashMap::new();
        runs.insert("run".to_string(), Step::Run);
        let scopes = CommitScopes {
            by_routine,
            runs,
            ..CommitScopes::default()
        };
        let edges = [edge("Root", "A", "call"), edge("A", "B", "run")];
        let g = CbGraph::build(&scopes, edges.iter(), [("B", None)]).expect("a declaration");
        assert_eq!(g.states_at("Root", "B"), Some(N | I));
        assert_eq!(g.states_at("A", "B"), Some(N));
        // The listed route reads the same.
        let listed = g.route_states(
            "Root",
            [("A", Some("call")), ("B", Some("run"))].into_iter(),
            None,
        );
        assert_eq!(norm(listed), N | I);
        // A route that commits normally is found (it is the may-not-follow
        // reading of the run).
        assert!(g.route_to("Root", "B", N).is_some());
        // The forward pass from the root (the digest's prefix at a crossing)
        // reads every routine as the backward pass does.
        let at = g.states_from("Root");
        for id in ["Root", "A", "B"] {
            let x = g.node(id).expect("a node");
            assert_eq!(at.get(x), g.raw_states_at("Root", id).unwrap(), "{id}");
        }
        assert_eq!(at.get(g.node("B").unwrap()), N | I);
        // Control: the same call as a plain call is ignored for certain.
        let plain = CommitScopes {
            by_routine: scopes.by_routine.clone(),
            ..CommitScopes::default()
        };
        let g = CbGraph::build(&plain, edges.iter(), [("B", None)]).expect("a declaration");
        assert_eq!(g.states_at("Root", "B"), Some(I));
    }

    /// Integration of B4's one start rule with B6's binding rule: a report
    /// variable's `Print()` binds to the report's own `procedure Print()`
    /// (no start, a plain step); `SaveAsWord('x.docx')` next to `procedure
    /// SaveAsWord(T: Code[250])` is a binding the engine cannot decide (an
    /// undecided start, which must be a "may", never a definite step);
    /// `SaveAsPdf('x.pdf')` binds to the built-in (a start). The model's flag
    /// and the scope's step are read from the resolver's one rule
    /// (`resolver::member_starts_object`), built from real source.
    #[test]
    fn a_start_binds_by_the_overload_rule_and_an_undecided_one_is_a_may() {
        use crate::program::resolve::dataset::ObjectStart;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("app.json"),
            r#"{"id":"20202020-0000-4000-8000-0000000004b6","name":"B4B6","publisher":"t","version":"1.0.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("Probe.al"),
            r#"table 50380 "BT" { fields { field(1; K; Integer) { } } }
report 50380 "BR"
{
    ProcessingOnly = true;
    dataset { dataitem(D; "BT") { } }
    trigger OnPreReport()
    begin
        Commit();
    end;

    procedure Print()
    begin
        Commit();
    end;

    procedure SaveAsWord(T: Code[250])
    begin
        Commit();
    end;
}
codeunit 50381 "BC"
{
    [CommitBehavior(CommitBehavior::Ignore)]
    procedure CallPrint()
    var
        R: Report "BR";
    begin
        R.Print();
    end;

    [CommitBehavior(CommitBehavior::Ignore)]
    procedure CallUndecided()
    var
        R: Report "BR";
    begin
        R.SaveAsWord('x.docx');
    end;

    [CommitBehavior(CommitBehavior::Ignore)]
    procedure CallBuiltin()
    var
        R: Report "BR";
    begin
        R.SaveAsPdf('x.pdf');
    end;
}
"#,
        )
        .unwrap();
        let model = crate::program::model::program_calls::assemble_and_resolve_workspace_program(
            &crate::program::inputs::WorkspaceInputs::read(dir.path(), &Default::default()),
            false,
            crate::program::model::workspace::AssemblyNeeds::BASE,
        )
        .expect("a model");
        let scopes = CommitScopes::of_model(&model, &[]);
        let site = |caller: &str, callee: &str| -> String {
            let r = model
                .workspace
                .routines
                .iter()
                .find(|r| r.name == caller)
                .unwrap_or_else(|| panic!("no routine {caller}"));
            r.call_sites
                .iter()
                .find(|cs| cs.callee_text == callee)
                .unwrap_or_else(|| panic!("no site {callee} in {caller}"))
                .id
                .clone()
        };
        let flags = |cs: &str| -> Vec<ObjectStart> {
            model
                .calls
                .edges
                .iter()
                .filter(|e| e.callsite_id == cs)
                .map(|e| e.starts_object)
                .collect()
        };
        let print = site("CallPrint", "R.Print");
        let undecided = site("CallUndecided", "R.SaveAsWord");
        let builtin = site("CallBuiltin", "R.SaveAsPdf");
        assert_eq!(flags(&print), vec![ObjectStart::No], "Print");
        assert_eq!(
            flags(&undecided),
            vec![ObjectStart::Undecided],
            "SaveAsWord"
        );
        assert!(
            !flags(&builtin).is_empty() && flags(&builtin).iter().all(|&f| f == ObjectStart::Yes),
            "SaveAsPdf: {:?}",
            flags(&builtin)
        );
        assert_eq!(scopes.step(Some(&print)), Step::Plain);
        assert_eq!(scopes.step(Some(&undecided)), Step::Run);
        assert_eq!(scopes.step(Some(&builtin)), Step::Run);
    }

    /// Engine limits B2 (per-arm ids) x B4: an id declared in two `#if` arms
    /// is one row per arm, and its declaration is every row's possibilities
    /// together. `Two` declares Ignore in arm A and Error in arm B: each arm's
    /// own builds all have its attribute (wave 2 final review: another
    /// declaration lacking it does not make it a "may" for the row itself),
    /// so every build runs `Two` ignored or as an error, never normally:
    /// `I|E`, and each row its own (`arms`). `Obj` is a whole object in two arms, Ignore in one and
    /// Error in the other (`InherentCommitBehavior`). The last-wins map kept
    /// arm B's `N|E` for both. Break: keep the last row's declaration -> `Two`
    /// and `Obj::Q` read `N|E`. (No CLI answer moves: every row here already
    /// holds `N`, so no definite answer rests on the lost bit; the two-file
    /// case where it does is `cli_b2_arm_readers::a_declaration_one_files_row_lacks_only_may_apply`.)
    #[test]
    fn every_arm_adds_its_declaration() {
        let src = r#"codeunit 50396 "Arms"
{
#if A
    [CommitBehavior(CommitBehavior::Ignore)]
    procedure Two()
    begin
        Commit();
    end;
#else
    [CommitBehavior(CommitBehavior::Error)]
    procedure Two()
    begin
        Commit();
    end;
#endif
}
#if A
codeunit 50397 "Obj"
{
    InherentCommitBehavior = Ignore;
    procedure Q()
    begin
        Commit();
    end;
}
#else
codeunit 50397 "Obj"
{
    InherentCommitBehavior = Error;
    procedure Q()
    begin
        Commit();
    end;
}
#endif
"#;
        let m = crate::program::model::program_calls::assemble_and_resolve_inline_program_default(
            &[("src/A.al".to_string(), src.to_string())],
            "33333333-0000-0000-0000-0000000000b7",
        );
        let scopes = CommitScopes::of_model(&m, &[]);
        for (name, whole, per_row) in [("Two", I | E, [I, E]), ("Q", N | I | E, [N | I, N | E])] {
            let rows: Vec<&ModelRoutine> = m
                .workspace
                .routines
                .iter()
                .filter(|r| r.name == name)
                .collect();
            assert_eq!(rows.len(), 2, "precondition: {name} has two rows");
            let d = scopes
                .get(&rows[0].stable_routine_id)
                .unwrap_or_else(|| panic!("{name} declares"));
            assert_eq!(d.states, whole, "{name}");
            assert_eq!(
                scopes.arms.get(rows[0].stable_routine_id.as_str()),
                Some(&per_row.to_vec()),
                "{name}"
            );
        }
    }
}
