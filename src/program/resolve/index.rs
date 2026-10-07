//! `ResolveIndex` and `WorldMode`: topology-scoped lookup indexes built from a
//! [`ProgramGraph`].
//!
//! ## Scoping model
//!
//! Two scoping modes govern which objects are visible:
//!
//! - **[`WorldMode::CallerClosure`]**: the caller's compile-time view —
//!   `from` itself plus its transitive dependency closure.  Used for
//!   name/id-based object resolution (`object_by_number`).  Mirrors the
//!   semantics of [`ProgramGraph::resolve_object`] but keys on numeric id
//!   rather than name.
//!
//! - **[`WorldMode::AnalyzedSnapshot`]**: whole-program, all-apps view.  Used
//!   for reverse-dependency queries whose answers depend on apps that live
//!   *outside* a caller's closure (extension targets, interface implementers,
//!   event subscribers).
//!
//! The mode is baked into each method signature rather than passed as a
//! runtime parameter, making the scoping visible and compiler-checkable at
//! each call site.

use std::collections::HashMap;

use al_syntax::IdentifierFoldExt;
use al_syntax::ir::ObjectKind;

use crate::program::graph::ProgramGraph;
use crate::program::node::{AppRef, ObjectNodeId, RoutineNodeId};
use crate::program::node_extract::{FieldNode, ObjectNode, ObjectRef};
use crate::program::resolve::edge::Condition;
use crate::program::resolve::event::{ParsedSubscriberArgs, subscriber_arity_bound};

// ---------------------------------------------------------------------------
// WorldMode
// ---------------------------------------------------------------------------

/// Which slice of the world a lookup is scoped to.
///
/// Callers may carry this value to dispatch between lookup strategies; the
/// `ResolveIndex` methods themselves have the mode baked into their signatures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorldMode {
    /// Resolve as seen **from** one specific app: self first, then its
    /// transitive dependency closure.  Objects outside the closure are
    /// invisible — same rule the AL compiler applies.
    CallerClosure(AppRef),
    /// Whole-snapshot view: all apps, no scoping.  Required for queries whose
    /// answer depends on reverse-dependency relationships (extension targets,
    /// interface implementers, event subscribers).
    AnalyzedSnapshot,
}

// ---------------------------------------------------------------------------
// ObjectRefResolution — result of `ResolveIndex::resolve_object_ref`
// ---------------------------------------------------------------------------

/// Result of resolving an [`ObjectRef`] (a `SourceTable`/`TableNo`/page-control
/// target) against the whole-program graph, as seen from one object.
///
/// Fail-closed by construction: only [`Self::Unique`] carries an id. Every
/// other variant is a deliberate decline — callers (Tasks 5–7) must treat a
/// non-`Unique` result as "no table"/"unknown", never fabricate a guess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectRefResolution {
    /// Exactly one object in `from`'s AL-visible dependency closure matches.
    Unique(ObjectNodeId),
    /// More than one object in the closure matches and no AL shadowing rule
    /// (own-app-wins) picks a single winner — declared uniqueness could not be
    /// proven from the data this index holds.
    Ambiguous,
    /// The reference matches a declared object somewhere in the whole
    /// snapshot, but that object's app is not in `from`'s dependency closure
    /// (unreachable, not a resolution — distinct from never having been
    /// declared at all).
    OutOfClosure,
    /// No declared object anywhere in the snapshot matches (wrong/absent kind,
    /// or the id/name was never declared).
    Unresolved,
}

// ---------------------------------------------------------------------------
// SubscriberEntry / Subscription — public types produced by the event index
// ---------------------------------------------------------------------------

/// A resolved event-subscriber for one publisher routine.
pub struct SubscriberEntry {
    /// The subscriber routine that will fire when the publisher fires.
    pub subscriber: RoutineNodeId,
    /// Dispatch conditions on this subscription (empty = unconditional).
    pub conditions: Vec<Condition>,
    /// Element filter from the `[EventSubscriber]` attribute, if present.
    pub element: Option<String>,
}

/// One parsed `[EventSubscriber]` attribute and what it bound to (engine-switch
/// S4, spec G7). [`SubscriberIndex`] keeps every one of them, bound or not: an
/// unbound subscription still drives event fan-out coverage, so it must never
/// vanish.
pub struct Subscription {
    /// The subscriber routine carrying the attribute.
    pub subscriber: RoutineNodeId,
    /// The attribute's position among the routine's parsed `[EventSubscriber]`
    /// attributes (a routine may subscribe to several events).
    pub ordinal: usize,
    /// Publisher object type as written, lowercased (e.g. `"codeunit"`).
    pub publisher_object_type: String,
    /// Publisher object name as written, unquoted and lowercased; empty when the
    /// attribute names the publisher by number (`publisher_id`).
    pub publisher_name: String,
    /// The publisher's object number, when the attribute names it that way.
    pub publisher_id: Option<i64>,
    /// Lowercased event name from the attribute.
    pub event_name_lc: String,
    /// Element filter from the attribute, if present.
    pub element: Option<String>,
    /// Dispatch conditions (manual binding, skip-on-missing-license/permission).
    pub conditions: Vec<Condition>,
    pub outcome: SubscriptionOutcome,
}

/// What a [`Subscription`] bound to.
pub enum SubscriptionOutcome {
    /// Exactly one publisher routine (after the exact-arity / Sender-arity
    /// preference).
    Bound(RoutineNodeId),
    /// The publisher object was found but no single overload could be chosen.
    Ambiguous {
        publisher_object: ObjectNodeId,
        /// Number of candidate overloads that matched the arity filter.
        candidate_count: usize,
    },
    /// The publisher OBJECT was resolved but no publisher-kind routine
    /// candidate existed for the event name + arity bound (Tier-1 remediation,
    /// H-1 fix: this used to be a silent `0 => continue`). Not necessarily a
    /// bug: the attribute can name an event that does not exist (a real AL
    /// compile error) or an arity the publisher never declares — but it must
    /// never be invisible.
    Orphaned { publisher_object: ObjectNodeId },
    /// No object of that type and name is visible from the subscriber's app, or
    /// the object type is not one AL events are published on. Until S4 these
    /// were dropped without a record.
    ObjectUnresolved,
}

impl Subscription {
    /// The publisher object, when one was resolved.
    pub fn publisher_object(&self) -> Option<&ObjectNodeId> {
        match &self.outcome {
            SubscriptionOutcome::Bound(id) => Some(&id.object),
            SubscriptionOutcome::Ambiguous {
                publisher_object, ..
            }
            | SubscriptionOutcome::Orphaned { publisher_object } => Some(publisher_object),
            SubscriptionOutcome::ObjectUnresolved => None,
        }
    }
}

// ---------------------------------------------------------------------------
// ResolveIndex
// ---------------------------------------------------------------------------

/// Pre-built lookup indexes over a [`ProgramGraph`].
///
/// All internal `Vec`s are populated by iterating `graph.objects` in its
/// already-sorted (by `NodeId`) order, so every returned list is
/// deterministic without a secondary sort. Routines are not copied in:
/// [`Self::routines_in_object`] reads `graph.routines` directly.
pub struct ResolveIndex {
    /// `(app, kind, declared_id)` → `ObjectNodeId` (first in sorted order for
    /// that app; duplicates within one app silently ignored). Feeds
    /// [`Self::object_by_number`] — self-preferred (own-app shadow), and
    /// fail-closed (I1) on a genuine cross-app collision: more than one app
    /// in the closure matching `(kind, declared_id)` DECLINES (`None`)
    /// instead of picking the lowest `ObjectNodeId`.
    objs_by_number: HashMap<(AppRef, ObjectKind, i64), ObjectNodeId>,
    /// `(kind, declared_id)` → every `ObjectNodeId` across ALL apps sharing
    /// that (kind, id), in `graph.objects` sort order. GLOBAL (whole-snapshot,
    /// not closure-scoped) — unlike `objs_by_number`, which is grouped by
    /// `app` for an O(1) per-app probe, this is grouped by `(kind, id)` alone
    /// so [`Self::resolve_object_ref`] can answer "does this id exist ANYWHERE"
    /// in O(1) without enumerating apps, and can detect a genuine cross-app id
    /// collision (`ObjectRefResolution::Ambiguous`) that `objs_by_number`'s
    /// single-slot-per-app shape cannot represent.
    objects_by_id: HashMap<(ObjectKind, i64), Vec<ObjectNodeId>>,
    /// `(kind, name_lc)` → every `ObjectNodeId` across ALL apps sharing that
    /// (kind, name), in `graph.objects` sort order. The Name-arm counterpart
    /// to `objects_by_id`, used only by [`Self::resolve_object_ref`].
    objects_by_name: HashMap<(ObjectKind, String), Vec<ObjectNodeId>>,
    /// Lowercased `extends_target` of a `TableExtension` → all extension ids.
    table_extensions: HashMap<String, Vec<ObjectNodeId>>,
    /// Lowercased `extends_target` of a `PageExtension` → all extension ids
    /// (pageext-merge-and-final-residual plan, Task 1 — the `Page` analog of
    /// `table_extensions`, needed to close the engine gap: a `PageExtension`'s
    /// routines are indexed under the EXTENSION's own `ObjectNodeId`, so a
    /// base-Page-typed receiver could never reach them without this reverse
    /// lookup; see [`Self::page_extensions_of`]).
    page_extensions: HashMap<String, Vec<ObjectNodeId>>,
    /// Lowercased `extends_target` of a `ReportExtension` → all extension ids
    /// (roadmap-closure plan, Task 1 — the `Report` analog of
    /// `table_extensions`/`page_extensions`; `extends_target` is populated
    /// for `ReportExtension` identically to `PageExtension` —
    /// `node_extract::extract_nodes` does not kind-gate that field — so this
    /// index is the mechanical third copy of the same reverse lookup; see
    /// [`Self::report_extensions_of`]).
    report_extensions: HashMap<String, Vec<ObjectNodeId>>,
    /// Lowercased interface name → all object ids that implement it.
    implementers: HashMap<String, Vec<ObjectNodeId>>,
}

/// Event subscriptions resolved over a [`ProgramGraph`]: which subscribers
/// each publisher routine has, plus every subscription with its outcome
/// (bound or not).
///
/// Kept apart from [`ResolveIndex`] on purpose. Only
/// `resolver::emit_event_flow_edges` reads it, and it builds one for the
/// length of that call. The LSP updater keeps a `ResolveIndex` alive while
/// idle (rung 1) and never reads subscribers, so holding them there cost
/// memory for nothing (compact-graph step 2, Task 4).
pub struct SubscriberIndex {
    /// Publisher `RoutineNodeId` → ordered list of resolved subscribers.
    subscribers_map: HashMap<RoutineNodeId, Vec<SubscriberEntry>>,
    /// Every parsed subscription, in `graph.routines` order then attribute
    /// order (engine-switch S4, G7).
    subscriptions: Vec<Subscription>,
}

impl SubscriberIndex {
    /// Resolve every `[EventSubscriber]` in `graph` to its publisher routine.
    pub fn build(graph: &ProgramGraph) -> Self {
        // `routine_indices_by_obj_name` groups `graph.routines` INDICES (not
        // ids) by `(object, name_lc)`. Indices — not a `RoutineNodeId`-keyed
        // map — are required here: a genuine same-name/same-arity SOURCE
        // overload pair legitimately shares one `RoutineNodeId` (source
        // `sig_fp` is always `0`; see node.rs and
        // `build::dedup_routines_preserving_genuine_overloads`), so a
        // `HashMap<RoutineNodeId, usize>` can hold only ONE of the two
        // physical routines and silently loses the other's `publisher_kind`
        // on the second `insert` — corrupting the subscriber-candidate
        // filter below into either double-counting or dropping a legitimate
        // publisher (beyond-1B.3b Task 2 review fix).
        let mut routine_indices_by_obj_name: HashMap<(ObjectNodeId, String), Vec<usize>> =
            HashMap::new();
        for (i, r) in graph.routines.iter().enumerate() {
            let key = (r.id.object.clone(), r.id.name_lc.clone());
            routine_indices_by_obj_name.entry(key).or_default().push(i);
        }

        // ── Event subscriber index ────────────────────────────────────────────
        let mut subscribers_map: HashMap<RoutineNodeId, Vec<SubscriberEntry>> = HashMap::new();
        let mut subscriptions: Vec<Subscription> = Vec::new();

        for sub_routine in &graph.routines {
            if sub_routine.event_subscribers.is_empty() {
                continue;
            }
            let sub_app = sub_routine.id.object.app;
            let sub_params = sub_routine.id.params_count;

            for (ordinal, args) in sub_routine.event_subscribers.iter().enumerate() {
                let entry = build_entry(sub_routine, args);
                let mut record = |outcome: SubscriptionOutcome| {
                    subscriptions.push(Subscription {
                        subscriber: sub_routine.id.clone(),
                        ordinal,
                        publisher_object_type: args.publisher_object_type.clone(),
                        publisher_name: args.publisher_name.clone(),
                        publisher_id: args.publisher_id,
                        event_name_lc: args.event_name.fold_identifier(),
                        element: entry.element.clone(),
                        conditions: entry.conditions.clone(),
                        outcome,
                    });
                };
                // (a) + (b) The publisher object, by name or number. An unknown
                // object type or an unresolvable object is recorded, not dropped.
                let Some(pub_obj) = resolve_subscription_publisher(graph, sub_app, args) else {
                    record(SubscriptionOutcome::ObjectUnresolved);
                    continue;
                };
                let pub_obj_id = pub_obj.id.clone();
                let event_name_lc = args.event_name.fold_identifier();

                // (c) Candidates: PHYSICAL routines (by index, not id — see
                //     the comment on `routine_indices_by_obj_name` above) in
                //     that object matching name + publisher_kind.is_some() +
                //     params_count >= sub_params. Counts every matching
                //     routine even when two share a `RoutineNodeId`.
                let candidates: Vec<RoutineNodeId> = routine_indices_by_obj_name
                    .get(&(pub_obj_id.clone(), event_name_lc.clone()))
                    .map(Vec::as_slice)
                    .unwrap_or(&[])
                    .iter()
                    .filter_map(|&i| {
                        let r = &graph.routines[i];
                        // Sender-tolerant arity bound (Task 1, round-2: CONDITIONAL, never
                        // blanket): an `[IntegrationEvent(IncludeSender: true, …)]` (also
                        // Business/Internal — all three carry `IncludeSender` at arg
                        // position 0) prepends an implicit `Sender` parameter that a
                        // subscriber may capture, so a valid subscriber's arity is at most
                        // the publisher's EXPLICIT arity + 1 — but ONLY when the publisher
                        // actually declares `IncludeSender: true`. A blanket `+1`
                        // regardless of the flag would be SYNCHRONIZED WRONGNESS (the extra
                        // param is illegal AL otherwise). `r.include_sender` is the single
                        // source of truth (populated at ingestion — see
                        // `RoutineNode::include_sender`'s doc); `subscriber_arity_bound` is
                        // the SAME shared helper `differential::verify_event_subscriber_
                        // route`'s independent checker uses, so the two can never drift.
                        // The pre-1B.3b-Task-1 `params_count >= sub_params` bound (no
                        // tolerance at all) dropped every IncludeSender subscriber
                        // (publisher explicit arity 0 vs subscriber arity 1) — a large
                        // orphan class; the disambiguation below still prefers an
                        // exact-arity match over a Sender-tolerant one.
                        let max_arity = subscriber_arity_bound(r.id.params_count, r.include_sender);
                        (sub_params <= max_arity && r.publisher_kind.is_some())
                            .then(|| r.id.clone())
                    })
                    .collect();

                // (d) Dispatch on candidate count.
                let chosen: Option<RoutineNodeId> = match candidates.len() {
                    0 => {
                        // H-1 fix: this used to be a bare `continue` — the
                        // subscription vanished with no record at all (see
                        // `SubscriptionOutcome::Orphaned`'s doc); resolution
                        // behavior (no edge emitted) is unchanged.
                        record(SubscriptionOutcome::Orphaned {
                            publisher_object: pub_obj_id,
                        });
                        continue;
                    }
                    1 => Some(candidates[0].clone()),
                    _ => {
                        // MORE THAN ONE: prefer exactly one EXACT-arity match; else fall
                        // back to exactly one Sender-arity (`+1`) match. Only genuine
                        // ambiguity (0 or >1 at the chosen precision) is recorded.
                        let exact: Vec<&RoutineNodeId> = candidates
                            .iter()
                            .filter(|rid| rid.params_count == sub_params)
                            .collect();
                        let picked: Option<&RoutineNodeId> = if exact.len() == 1 {
                            Some(exact[0])
                        } else if exact.is_empty() {
                            let sender: Vec<&RoutineNodeId> = candidates
                                .iter()
                                .filter(|rid| rid.params_count + 1 == sub_params)
                                .collect();
                            (sender.len() == 1).then(|| sender[0])
                        } else {
                            None
                        };
                        picked.cloned()
                    }
                };
                match chosen {
                    Some(rid) => {
                        record(SubscriptionOutcome::Bound(rid.clone()));
                        subscribers_map.entry(rid).or_default().push(entry);
                    }
                    None => record(SubscriptionOutcome::Ambiguous {
                        publisher_object: pub_obj_id,
                        candidate_count: candidates.len(),
                    }),
                }
            }
        }

        // Sort each entry list by subscriber RoutineNodeId for determinism.
        for entries in subscribers_map.values_mut() {
            entries.sort_by(|a, b| a.subscriber.cmp(&b.subscriber));
        }

        SubscriberIndex {
            subscribers_map,
            subscriptions,
        }
    }

    /// All resolved event subscribers of `publisher` — [`WorldMode::AnalyzedSnapshot`].
    ///
    /// Returns a deterministically sorted (by `subscriber` `RoutineNodeId`) slice.
    /// Empty when `publisher` is not a publisher routine or has no subscribers.
    pub fn subscribers_of(&self, publisher: &RoutineNodeId) -> &[SubscriberEntry] {
        self.subscribers_map
            .get(publisher)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Every parsed subscription with its outcome, in `graph.routines` order
    /// then attribute order (engine-switch S4, G7).
    pub fn subscriptions(&self) -> &[Subscription] {
        &self.subscriptions
    }

    /// Subscriptions that matched a publisher object but could not be resolved
    /// to a single overload (multiple candidates, no unique strict arity match).
    pub fn ambiguous_subscriptions(&self) -> Vec<&Subscription> {
        self.subscriptions
            .iter()
            .filter(|s| matches!(s.outcome, SubscriptionOutcome::Ambiguous { .. }))
            .collect()
    }

    /// Subscriptions whose publisher object resolved but had zero eligible
    /// routine candidates (see [`SubscriptionOutcome::Orphaned`]).
    pub fn orphaned_subscriptions(&self) -> Vec<&Subscription> {
        self.subscriptions
            .iter()
            .filter(|s| matches!(s.outcome, SubscriptionOutcome::Orphaned { .. }))
            .collect()
    }
}

impl ResolveIndex {
    /// Build all indexes from `graph`.
    ///
    /// `graph.objects` and `graph.routines` are already sorted by `NodeId`;
    /// the index preserves that order so every returned `Vec` is deterministic.
    /// Event subscriptions live in [`SubscriberIndex`], not here.
    pub fn build(graph: &ProgramGraph) -> Self {
        let mut objs_by_number: HashMap<(AppRef, ObjectKind, i64), ObjectNodeId> = HashMap::new();
        let mut objects_by_id: HashMap<(ObjectKind, i64), Vec<ObjectNodeId>> = HashMap::new();
        let mut objects_by_name: HashMap<(ObjectKind, String), Vec<ObjectNodeId>> = HashMap::new();
        let mut table_extensions: HashMap<String, Vec<ObjectNodeId>> = HashMap::new();
        let mut page_extensions: HashMap<String, Vec<ObjectNodeId>> = HashMap::new();
        let mut report_extensions: HashMap<String, Vec<ObjectNodeId>> = HashMap::new();
        let mut implementers: HashMap<String, Vec<ObjectNodeId>> = HashMap::new();

        for obj in &graph.objects {
            // By-number: first sorted entry wins for a given (app, kind, id).
            if let Some(n) = obj.declared_id {
                objs_by_number
                    .entry((obj.id.app, obj.id.kind, n))
                    .or_insert_with(|| obj.id.clone());
                objects_by_id
                    .entry((obj.id.kind, n))
                    .or_default()
                    .push(obj.id.clone());
            }
            objects_by_name
                .entry((obj.id.kind, obj.name.fold_identifier()))
                .or_default()
                .push(obj.id.clone());

            // TableExtension → base table name (lowercased).
            if obj.id.kind == ObjectKind::TableExtension
                && let Some(ref target) = obj.extends_target
            {
                table_extensions
                    .entry(target.fold_identifier())
                    .or_default()
                    .push(obj.id.clone());
            }

            // PageExtension → base page name (lowercased) — the Task 1 analog
            // of the TableExtension index above.
            if obj.id.kind == ObjectKind::PageExtension
                && let Some(ref target) = obj.extends_target
            {
                page_extensions
                    .entry(target.fold_identifier())
                    .or_default()
                    .push(obj.id.clone());
            }

            // ReportExtension → base report name (lowercased) — the
            // roadmap-closure plan Task 1 analog of the Table/Page indexes
            // above.
            if obj.id.kind == ObjectKind::ReportExtension
                && let Some(ref target) = obj.extends_target
            {
                report_extensions
                    .entry(target.fold_identifier())
                    .or_default()
                    .push(obj.id.clone());
            }

            // Interface implementers: codeunits only. An enum's `implements`
            // maps its VALUES to implementing codeunits (which declare the
            // interface themselves); the enum has no methods to dispatch to
            // (S9.0e). A header split across `#if` arms lists an interface once
            // per arm, so each object is recorded once per interface.
            if obj.id.kind == ObjectKind::Codeunit {
                for iface in &obj.implements {
                    let list = implementers.entry(iface.fold_identifier()).or_default();
                    if !list.contains(&obj.id) {
                        list.push(obj.id.clone());
                    }
                }
            }
        }

        ResolveIndex {
            objs_by_number,
            objects_by_id,
            objects_by_name,
            table_extensions,
            page_extensions,
            report_extensions,
            implementers,
        }
    }

    /// Census only: move every field out, boxed and named, so a probe can drop
    /// them one at a time and read the heap delta. Production never calls it.
    /// A new field must be added here; `census_parts_names_every_field` fails
    /// until it is.
    #[doc(hidden)]
    pub fn census_parts(self) -> Vec<(&'static str, Box<dyn std::any::Any + Send>)> {
        let ResolveIndex {
            objs_by_number,
            objects_by_id,
            objects_by_name,
            table_extensions,
            page_extensions,
            report_extensions,
            implementers,
        } = self;
        vec![
            ("objs_by_number", Box::new(objs_by_number)),
            ("objects_by_id", Box::new(objects_by_id)),
            ("objects_by_name", Box::new(objects_by_name)),
            ("table_extensions", Box::new(table_extensions)),
            ("page_extensions", Box::new(page_extensions)),
            ("report_extensions", Box::new(report_extensions)),
            ("implementers", Box::new(implementers)),
        ]
    }

    /// All overloads of `name_lc` declared in `obj` — [`WorldMode::CallerClosure`]
    /// or [`WorldMode::AnalyzedSnapshot`] (no scoping needed; the object id is
    /// already fully-qualified).
    ///
    /// Yields every `graph.routines` row of `(obj, name_lc)` in merged order,
    /// multiplicity kept (physical duplicate rows included). Empty when
    /// nothing is found.
    ///
    /// `RoutineNodeId` sorts by `(object, name_lc, …)` first, so these rows
    /// are one contiguous run of the sorted list: a binary search finds it,
    /// with no per-call allocation and no per-routine map (the map this
    /// replaced was 50.03 of 56.81 MiB per idle updater on the CG corpus,
    /// embedded mode, root 1; see `docs/2026-10-04-step2-index-census.md`).
    /// `graph.routines` must be sorted (the `NodeSet` invariant) and consistent
    /// with the object maps the caller also uses; the index holds no routines.
    pub fn routines_in_object<'g>(
        &self,
        graph: &'g ProgramGraph,
        obj: &ObjectNodeId,
        name_lc: &str,
    ) -> impl ExactSizeIterator<Item = &'g RoutineNodeId> + Clone + use<'g> {
        graph
            .routines
            .run_by(|r| {
                r.id.object
                    .cmp(obj)
                    .then_with(|| r.id.name_lc.as_str().cmp(name_lc))
            })
            .map(|r| &r.id)
    }

    /// Resolve an object by its **numeric AL id** as seen from `from`
    /// ([`WorldMode::CallerClosure`]) — fail-closed (I1).
    ///
    /// Search order mirrors [`ProgramGraph::resolve_object`]:
    /// 1. `from` itself always wins (own-app shadow), short-circuiting before
    ///    computing the closure.
    /// 2. Otherwise, exactly ONE `(kind, declared_id)` match among `from`'s
    ///    transitive dependency closure resolves; more than one VISIBLE
    ///    dependency match is an unprovable cross-app collision and DECLINES
    ///    (`None`) rather than guessing via the lowest-`NodeId` tiebreak (a
    ///    confident WRONG pick is the cardinal sin — I1).
    ///
    /// Objects whose declaring app is NOT in the closure are invisible.
    ///
    /// `graph` is required to compute the transitive closure on demand;
    /// the closure is NOT cached here (Phase 1 — if call-hot, cache in a later
    /// phase or pre-expand into a per-app index).
    pub fn object_by_number(
        &self,
        graph: &ProgramGraph,
        from: AppRef,
        kind: ObjectKind,
        declared_id: i64,
    ) -> Option<ObjectNodeId> {
        // Prefer `from` itself (avoids building the closure set in the common case).
        if let Some(oid) = self.objs_by_number.get(&(from, kind, declared_id)) {
            return Some(oid.clone());
        }

        // No own-app declaration: search the rest of the closure (cycle-safe;
        // `from` is skipped below). `objs_by_number` already holds at most
        // one entry per `(app, kind, declared_id)` (first/lowest-id wins on a
        // same-app duplicate), so at most one match can come from any single
        // app — more than one app matching is a genuine cross-app collision.
        let closure = graph.topology.closure(from);
        let mut found: Option<&ObjectNodeId> = None;
        for &app in &closure {
            if app == from {
                continue;
            }
            if let Some(oid) = self.objs_by_number.get(&(app, kind, declared_id)) {
                if found.is_some() {
                    return None; // >1 dependency declares this (kind, id) — decline.
                }
                found = Some(oid);
            }
        }
        found.cloned()
    }

    /// Resolve an [`ObjectRef`] (a `SourceTable`/`TableNo`/page-control target)
    /// of the given `kind`, as seen from `from` — the ONE shared, fail-closed
    /// helper Tasks 5–7 call.
    ///
    /// Unlike [`Self::object_by_number`]/[`ProgramGraph::resolve_object`]
    /// (which silently pick the lowest-`ObjectNodeId` "best" match across the
    /// closure and never signal ambiguity), this is deliberately stricter:
    /// more than one distinct in-closure declaration is an unprovable case and
    /// this DECLINES ([`ObjectRefResolution::Ambiguous`]) rather than guessing
    /// — "a guessed id is the cardinal sin". Only [`ObjectRefResolution::Unique`]
    /// ever carries an id.
    ///
    /// - [`ObjectRef::Id`] matches a declared numeric object id of the SAME
    ///   `kind` only, via [`Self::objects_by_id`] filtered to `from`'s
    ///   dependency closure (self included). An object declared in `from`'s
    ///   OWN app always wins over any dependency's same-`(kind, id)` object
    ///   (own-app shadow — mirrors the Name arm below and matches
    ///   `object_by_number`'s existing self-shortcut), so two DEPENDENCIES
    ///   sharing an id — an anomaly that a merged whole-program snapshot can
    ///   surface even though a real compile never would — is `Ambiguous` only
    ///   when NEITHER is `from` itself.
    /// - [`ObjectRef::Name`] matches by `kind` + lowercased name within the
    ///   closure via [`Self::objects_by_name`]. An object declared in `from`'s
    ///   OWN app always wins over any dependency's same-named object (AL's
    ///   own-app lookup priority — mirrors the self-preference already applied
    ///   by `object_by_number`/`resolve_object`), so two dependencies sharing a
    ///   name is `Ambiguous` only when NEITHER is `from` itself.
    /// - When the ref matches no declared object anywhere in the whole
    ///   snapshot, the result is [`ObjectRefResolution::Unresolved`]. When it
    ///   matches a declared object whose app is outside `from`'s closure, the
    ///   result is [`ObjectRefResolution::OutOfClosure`] — a distinct, more
    ///   informative decline than `Unresolved` (the reference is real, just
    ///   unreachable from here).
    ///
    /// No namespace data exists on `ObjectNode` today, so a namespace-qualified
    /// name is matched purely as literal text via `normalized_lc` (whatever the
    /// caller wrote); when namespace data lands on the graph, a qualified and
    /// an unqualified reference to the same object will naturally compare
    /// unequal here without any change to this function.
    pub fn resolve_object_ref(
        &self,
        graph: &ProgramGraph,
        from: ObjectNodeId,
        kind: ObjectKind,
        r: &ObjectRef,
    ) -> ObjectRefResolution {
        let closure = graph.topology.closure(from.app);
        match r {
            ObjectRef::Id(n) => {
                let candidates = self
                    .objects_by_id
                    .get(&(kind, *n))
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                if candidates.is_empty() {
                    return ObjectRefResolution::Unresolved;
                }
                // Own-app shadow: `from`'s own declaration always wins over
                // any dependency's same-`(kind, id)` object — mirrors the
                // Name arm below, and matches `object_by_number`'s existing
                // self-shortcut, so numeric-id resolution stays
                // behavior-preserving for the already-covered self-declared
                // case.
                if let Some(own) = candidates.iter().find(|oid| oid.app == from.app) {
                    return ObjectRefResolution::Unique(own.clone());
                }
                let in_closure: Vec<&ObjectNodeId> = candidates
                    .iter()
                    .filter(|oid| closure.contains(&oid.app))
                    .collect();
                match in_closure.len() {
                    0 => ObjectRefResolution::OutOfClosure,
                    1 => ObjectRefResolution::Unique(in_closure[0].clone()),
                    _ => ObjectRefResolution::Ambiguous,
                }
            }
            ObjectRef::Name { normalized_lc, .. } => {
                let candidates = self
                    .objects_by_name
                    .get(&(kind, normalized_lc.clone()))
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                if candidates.is_empty() {
                    return ObjectRefResolution::Unresolved;
                }
                // Own-app shadow: `from`'s own declaration always wins over any
                // dependency's same-named object — short-circuits before
                // ambiguity among dependencies is even considered.
                if let Some(own) = candidates.iter().find(|oid| oid.app == from.app) {
                    return ObjectRefResolution::Unique(own.clone());
                }
                let in_closure: Vec<&ObjectNodeId> = candidates
                    .iter()
                    .filter(|oid| closure.contains(&oid.app))
                    .collect();
                match in_closure.len() {
                    0 => ObjectRefResolution::OutOfClosure,
                    1 => ObjectRefResolution::Unique(in_closure[0].clone()),
                    _ => ObjectRefResolution::Ambiguous,
                }
            }
        }
    }

    /// All `TableExtension` objects whose `extends_target` (lowercased) equals
    /// `base_table_name_lc` — [`WorldMode::AnalyzedSnapshot`], whole-program
    /// view (extensions live in reverse-dependent apps, outside the base
    /// table's own closure).
    pub fn table_extensions_of(&self, base_table_name_lc: &str) -> &[ObjectNodeId] {
        self.table_extensions
            .get(base_table_name_lc)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// All `PageExtension` objects whose `extends_target` (lowercased) equals
    /// `base_page_name_lc` — [`WorldMode::AnalyzedSnapshot`], whole-program
    /// view (extensions live in reverse-dependent apps, outside the base
    /// page's own closure). The `Page` analog of [`Self::table_extensions_of`]
    /// (pageext-merge-and-final-residual plan, Task 1) — see
    /// `resolver::resolve_in_page_scope` for the closure- and access-filtered
    /// consumer.
    pub fn page_extensions_of(&self, base_page_name_lc: &str) -> &[ObjectNodeId] {
        self.page_extensions
            .get(base_page_name_lc)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// All `ReportExtension` objects whose `extends_target` (lowercased)
    /// equals `base_report_name_lc` — [`WorldMode::AnalyzedSnapshot`],
    /// whole-program view (extensions live in reverse-dependent apps,
    /// outside the base report's own closure). The `Report` analog of
    /// [`Self::table_extensions_of`]/[`Self::page_extensions_of`]
    /// (roadmap-closure plan, Task 1) — see
    /// `resolver::resolve_in_report_scope` for the closure- and
    /// access-filtered consumer.
    pub fn report_extensions_of(&self, base_report_name_lc: &str) -> &[ObjectNodeId] {
        self.report_extensions
            .get(base_report_name_lc)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// All objects whose `implements` list (lowercased) contains
    /// `interface_name_lc` — [`WorldMode::AnalyzedSnapshot`], whole-program
    /// view (implementers live in reverse-dependent apps).
    pub fn implementers_of(&self, interface_name_lc: &str) -> &[ObjectNodeId] {
        self.implementers
            .get(interface_name_lc)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Resolve field `field_lc` against the VISIBILITY-SCOPED table field
    /// surface for `base` — the base table's own fields plus every
    /// `TableExtension` field visible in `from_object`'s compile-time app
    /// dependency closure (Task 3; mirrors `resolve_in_table_scope`'s
    /// identical base+extension, closure-filtered scoping for ROUTINES —
    /// see that function's doc for the full visibility rationale, which
    /// applies identically here to FIELDS).
    ///
    /// # Visibility scoping (fail-closed, same discipline as routines)
    ///
    /// [`Self::table_extensions_of`] is whole-snapshot
    /// (`WorldMode::AnalyzedSnapshot` — no app-scoping). A `TableExtension`
    /// declared in an app OUTSIDE `from_object`'s transitive dependency
    /// closure is a symbol `from_object`'s own app never imported — the real
    /// AL compiler could never resolve a field access against it, so it is
    /// dropped from the candidate scope entirely, never merely deprioritized.
    /// `base` itself is gated the same way, defense-in-depth.
    ///
    /// # Cardinality
    ///
    /// A UNIQUE match across base + visible extensions resolves
    /// (`Some(FieldNode)`); zero or more-than-one visible declaration
    /// declines (`None`) — never guess among ambiguous same-name field
    /// declarations (the cardinal sin).
    ///
    /// # Provenance dedup (round-2 addendum)
    ///
    /// Before the cardinality check, matches are deduped BY PROVENANCE: an
    /// IDENTICAL `(declaring object, name, type text)` triple counted more
    /// than once — e.g. a field declared inside BOTH branches of a source
    /// `#if`/`#else` (`FieldDecl` extraction, like `globals`/`locals`,
    /// collects from both branches — see `al_syntax::ir::decl::ObjectDecl`'s
    /// doc) — collapses to ONE, so harmless re-parse duplication never
    /// manufactures an artificial ambiguity. Only a genuinely DIFFERENT
    /// declaration (a different declaring object, or the SAME object
    /// declaring two DIFFERENT types under the same field name) counts as a
    /// real duplicate. Every real duplicate-decline is logged (declaring
    /// object id + field name) for measurement, per the round-2 addendum.
    pub fn field_in_table(
        &self,
        graph: &ProgramGraph,
        from_object: &ObjectNode,
        base: &ObjectNodeId,
        field_lc: &str,
    ) -> Option<FieldNode> {
        let closure = graph.topology.closure(from_object.id.app);
        if !closure.contains(&base.app) {
            return None;
        }
        let base_obj = Self::find_object(graph, base)?;

        let mut matches: Vec<(&ObjectNodeId, &FieldNode)> = base_obj
            .fields
            .iter()
            .filter(|f| f.name_lc == field_lc)
            .map(|f| (base, f))
            .collect();

        let base_name_lc = base_obj.name.fold_identifier();
        for ext_id in self.table_extensions_of(&base_name_lc) {
            if !closure.contains(&ext_id.app) {
                // Outside from_object's dependency closure: invisible, not a
                // candidate (fail-closed — mirrors `resolve_in_table_scope`).
                continue;
            }
            let Some(ext_obj) = Self::find_object(graph, ext_id) else {
                continue;
            };
            matches.extend(
                ext_obj
                    .fields
                    .iter()
                    .filter(|f| f.name_lc == field_lc)
                    .map(|f| (ext_id, f)),
            );
        }

        // Dedupe identical (declaring object, name, type) triples — see this
        // function's doc for why (harmless #if/#else re-parse duplication
        // must never manufacture an artificial ambiguity).
        let mut deduped: Vec<(&ObjectNodeId, &FieldNode)> = Vec::new();
        for m in matches {
            if !deduped
                .iter()
                .any(|(oid, f): &(&ObjectNodeId, &FieldNode)| {
                    **oid == *m.0 && f.type_text == m.1.type_text
                })
            {
                deduped.push(m);
            }
        }

        match deduped.as_slice() {
            [(_, f)] => Some((*f).clone()),
            [] => None,
            _ => {
                log::debug!(
                    "field_in_table: duplicate field {field_lc:?} on table {base:?} declines \
                     (candidates: {:?})",
                    deduped
                        .iter()
                        .map(|(oid, _)| (*oid).clone())
                        .collect::<Vec<_>>()
                );
                None
            }
        }
    }

    /// Whether a routine (ANY arity, ANY access level) named `name_lc`
    /// exists anywhere in the VISIBILITY-SCOPED table surface for `base` —
    /// the SAME base + closure-visible-`TableExtension` scope
    /// [`Self::field_in_table`] itself consults for fields (round-2
    /// soundness correction, record-field chains plan Task 4).
    ///
    /// # Why this exists: the non-method `Member` AST shape is AMBIGUOUS
    ///
    /// AL's parens are OPTIONAL on a zero-argument procedure call
    /// (`Rec.Insert;` compiles — the Code Cop AA0008 flags the missing
    /// parens as a STYLE issue, not a compile error). A parens-less call
    /// therefore parses to the EXACT SAME AST shape as a plain field/
    /// property access: `ExprKind::Member{object, member}` with no
    /// enclosing `Call` node at all. Every consumer that types a
    /// non-method `Member` as a RECORD FIELD (`infer_compound_member_
    /// receiver`'s field arm, and the bare implicit-Rec quoted-field arm in
    /// `infer_receiver_type`) MUST check this FIRST — if a same-named
    /// routine exists anywhere a parens-less call to it could legally
    /// target, typing `member`/the bare identifier as a field instead would
    /// risk a false `Source`/`Catalog` edge, the cardinal sin. This
    /// function is the fail-closed gate: it only ever WIDENS the decline
    /// set (a `true` result blocks field-typing; it never causes one), so a
    /// caller composing `!table_scope_has_routine(..) && field_in_table(..)`
    /// stays fail-closed by construction.
    ///
    /// # Scope, deliberately NOT access-filtered
    ///
    /// Unlike [`crate::program::resolve::resolver::resolve_in_table_scope`]
    /// (which additionally filters candidates by `Access` — `Local`/
    /// `Internal`/`Protected`/`Public` visibility from the caller), this
    /// function checks EXISTENCE alone across the closure-visible scope. A
    /// routine that would be `Access`-excluded from an actual call still
    /// signals a real symbol collision worth declining over — over-
    /// declining is always the safe direction, and skipping access
    /// filtering keeps this check simple and keeps it in lockstep with
    /// `field_in_table`'s own (access-agnostic — fields have no `Access`
    /// modifier) visibility discipline.
    pub fn table_scope_has_routine(
        &self,
        graph: &ProgramGraph,
        from_object: &ObjectNode,
        base: &ObjectNodeId,
        name_lc: &str,
    ) -> bool {
        let closure = graph.topology.closure(from_object.id.app);
        if !closure.contains(&base.app) {
            return false;
        }
        if self
            .routines_in_object(graph, base, name_lc)
            .next()
            .is_some()
        {
            return true;
        }
        let Some(base_obj) = Self::find_object(graph, base) else {
            return false;
        };
        let base_name_lc = base_obj.name.fold_identifier();
        for ext_id in self.table_extensions_of(&base_name_lc) {
            if !closure.contains(&ext_id.app) {
                // Outside from_object's dependency closure: invisible, not a
                // candidate — mirrors `field_in_table`'s identical filter.
                continue;
            }
            if self
                .routines_in_object(graph, ext_id, name_lc)
                .next()
                .is_some()
            {
                return true;
            }
        }
        false
    }

    /// Look up an `ObjectNode` by id via binary search — `graph.objects` is
    /// sorted by `ObjectNodeId` at construction (`build_program_graph` Step 4
    /// / every in-memory test fixture), mirroring `object_extends`'s
    /// identical `graph.objects.binary_search_by` lookup pattern.
    fn find_object<'g>(graph: &'g ProgramGraph, id: &ObjectNodeId) -> Option<&'g ObjectNode> {
        graph
            .objects
            .binary_search_by(|probe| probe.id.cmp(id))
            .ok()
            .map(|i| &graph.objects[i])
    }

    /// Whether `from` is an extension object that DIRECTLY extends `target`,
    /// by RESOLVED OBJECT IDENTITY (never a lowercased-name comparison) —
    /// the visibility test `object_has_visible_member_candidate`
    /// (`resolver.rs`) uses for `Access::Protected` (beyond-1B.3b Task 1),
    /// generalized across every AL extension kind (not hardcoded to
    /// `TableExtension`).
    ///
    /// Three independent guards, ALL required:
    /// 1. **Kind-compatible.** `from.kind.is_extension_kind()` AND
    ///    `from.kind.extension_base_kind() == Some(target.kind)` — a
    ///    `TableExtension` can only extend a `Table`, a `PageExtension` only
    ///    a `Page`, etc. Rules out a same-named-but-wrong-kind object by
    ///    construction, not by luck.
    /// 2. **Direct, never transitive.** Only `from`'s OWN `extends_target` is
    ///    consulted — "extension of an extension" is not an AL construct, so
    ///    there is nothing to walk transitively.
    /// 3. **Identity-resolved.** `from`'s `extends_target` (a raw name) is
    ///    resolved via [`Self::resolve_object_ref`] (fail-closed, dependency-
    ///    closure-scoped from `from`) and the result must be EXACTLY
    ///    `target`'s `ObjectNodeId` — `Ambiguous`/`OutOfClosure`/`Unresolved`
    ///    all decline (`false`), never guessed.
    ///
    /// Deliberately **NOT reverse**: a base object does not "extend" its own
    /// extension, so `object_extends(base, extension)` is always `false` — a
    /// base object's `Protected` members stay invisible from a caller that is
    /// itself the extension only via the symmetric self/extends check at the
    /// visibility call site, never via this function returning `true`
    /// backwards. And **NEVER peer**: a sibling extension `ExtB`'s
    /// `extends_target` resolves to the shared BASE, never to a co-extension
    /// `ExtA` — so `object_extends(ExtB, ExtA)` is always `false`, closing the
    /// peer-extension `Protected`-bleed gap (the biggest latent false-`Source`
    /// this task closes).
    pub fn object_extends(
        &self,
        graph: &ProgramGraph,
        from: &ObjectNodeId,
        target: &ObjectNodeId,
    ) -> bool {
        if !from.kind.is_extension_kind() || from.kind.extension_base_kind() != Some(target.kind) {
            return false;
        }
        // `graph.objects` is sorted by `ObjectNodeId` at construction
        // (`build_program_graph` Step 4 / every in-memory test fixture) —
        // binary-searchable, mirroring `lookup_routine_access`'s identical
        // `graph.routines.binary_search_by` lookup pattern (resolver.rs).
        let Some(from_obj) = graph
            .objects
            .binary_search_by(|probe| probe.id.cmp(from))
            .ok()
            .map(|i| &graph.objects[i])
        else {
            return false;
        };
        let Some(extends) = from_obj.extends_target.as_deref() else {
            return false;
        };
        let base_ref = ObjectRef::Name {
            raw: extends.to_string(),
            normalized_lc: extends.fold_identifier(),
        };
        matches!(
            self.resolve_object_ref(graph, from.clone(), target.kind, &base_ref),
            ObjectRefResolution::Unique(id) if &id == target
        )
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Build a [`SubscriberEntry`] from a resolved subscriber routine + parsed args.
fn build_entry(
    sub_routine: &crate::program::node_extract::RoutineNode,
    args: &ParsedSubscriberArgs,
) -> SubscriberEntry {
    let mut conditions = Vec::new();
    if sub_routine.subscriber_instance_manual {
        conditions.push(Condition::ManualBinding);
    }
    if args.skip_on_missing_license {
        conditions.push(Condition::SkipOnMissingLicense);
    }
    if args.skip_on_missing_permission {
        conditions.push(Condition::SkipOnMissingPermission);
    }
    SubscriberEntry {
        subscriber: sub_routine.id.clone(),
        conditions,
        element: args.element.clone(),
    }
}

/// The publisher object an `[EventSubscriber]` names, as seen from the
/// subscriber's app `from`: by name, or by number (`Codeunit, 50`;
/// engine-switch S4.3b). `None` when the type is not one events are published
/// on, or no single object matches (both lookups fail closed on a cross-app
/// collision). The one resolver for subscriber wiring, the platform-publisher
/// injection and the IncludeSender preflight.
pub(crate) fn resolve_subscription_publisher<'g>(
    graph: &'g ProgramGraph,
    from: AppRef,
    args: &ParsedSubscriberArgs,
) -> Option<&'g ObjectNode> {
    let kind = kind_from_object_type_str(&args.publisher_object_type)?;
    match args.publisher_id {
        Some(n) => graph.resolve_object_by_number(from, kind, n),
        None => graph.resolve_object(from, kind, &args.publisher_name),
    }
}

/// Map a lowercased publisher-object-type string (as written in an
/// `[EventSubscriber]` attribute) to the corresponding [`ObjectKind`].
/// Returns `None` for unrecognised strings.
fn kind_from_object_type_str(s: &str) -> Option<ObjectKind> {
    match s {
        "codeunit" => Some(ObjectKind::Codeunit),
        "table" => Some(ObjectKind::Table),
        "tableextension" => Some(ObjectKind::TableExtension),
        "page" => Some(ObjectKind::Page),
        "pageextension" => Some(ObjectKind::PageExtension),
        "report" => Some(ObjectKind::Report),
        "reportextension" => Some(ObjectKind::ReportExtension),
        "query" => Some(ObjectKind::Query),
        "xmlport" => Some(ObjectKind::XmlPort),
        "enum" => Some(ObjectKind::Enum),
        "enumextension" => Some(ObjectKind::EnumExtension),
        "interface" => Some(ObjectKind::Interface),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// PREFLIGHT DIAGNOSTIC — Task 1 round-2 addendum, folded in by Task 2
// ---------------------------------------------------------------------------

/// Count event-subscriber routines whose declared arity is EXACTLY the
/// resolved publisher's explicit arity **+1** (i.e. a shape that WOULD wire
/// under the Sender-tolerant `+1` bound, `event::subscriber_arity_bound`)
/// where the publisher's [`RoutineNode::include_sender`] is UNKNOWN
/// (`None`) — the exact population Task 1's fail-closed policy ("no `+1`
/// tolerance without positive evidence") silently declines to wire.
///
/// Task 1's own commit narrative reported a 13,581-entry probe of a real
/// Microsoft Base Application `SymbolReference.json` finding 100%
/// `IncludeSender` coverage (zero `None`s), but never landed that as a CODE
/// diagnostic — this closes that gap (round-2 addendum: "Emit the preflight
/// diagnostic: count of unknown-IncludeSender publishers with +1-arity
/// subscribers"). A CDO gate asserts this is `0` — confirming the
/// fail-closed policy is not silently orphaning a legitimate wiring
/// population on a real workspace. A nonzero count elsewhere is not itself
/// a bug (it may be a genuinely unparseable/absent attribute); it is the
/// exact signal the round-2 addendum asked to surface for adjudication
/// rather than letting the policy discard it silently.
///
/// Deliberately INDEPENDENT of [`ResolveIndex::build`]'s own subscriber
/// wiring loop (rather than instrumenting it) — a diagnostic that shares no
/// code path with the mechanism it audits cannot be silently defeated by a
/// future change to that mechanism.
pub fn count_unknown_include_sender_plus1_subscribers(graph: &ProgramGraph) -> usize {
    let mut count = 0usize;
    for sub_routine in &graph.routines {
        if sub_routine.event_subscribers.is_empty() {
            continue;
        }
        let sub_app = sub_routine.id.object.app;
        let sub_params = sub_routine.id.params_count;

        for args in &sub_routine.event_subscribers {
            let Some(pub_obj) = resolve_subscription_publisher(graph, sub_app, args) else {
                continue;
            };
            let event_name_lc = args.event_name.fold_identifier();

            for pr in &graph.routines {
                if pr.id.object == pub_obj.id
                    && pr.id.name_lc == event_name_lc
                    && pr.publisher_kind.is_some()
                    && pr.include_sender.is_none()
                    && pr.id.params_count + 1 == sub_params
                {
                    count += 1;
                }
            }
        }
    }
    count
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::graph::{ObjectIndex, ProgramGraph};
    use crate::program::node::{AppRef, AppRegistry, ObjKey, ObjectNodeId, RoutineNodeId};
    use crate::program::node_extract::{AbiParams, Access, ObjectNode, RoutineNode};
    use crate::program::resolve::edge::Condition;
    use crate::program::resolve::event::{ParsedSubscriberArgs, PublisherKind};
    use crate::program::topology::DependencyGraph;
    use crate::snapshot::{AppId, TrustTier};
    use al_syntax::ir::ObjectKind;

    fn make_app_id(name: &str) -> AppId {
        AppId {
            guid: String::new(),
            name: name.into(),
            publisher: "Test".into(),
            version: "1.0.0.0".into(),
        }
    }

    fn make_obj(
        app: AppRef,
        kind: ObjectKind,
        declared_id: Option<i64>,
        name: &str,
        extends_target: Option<&str>,
        implements: Vec<&str>,
    ) -> ObjectNode {
        let key = match declared_id {
            Some(n) => ObjKey::Id(n),
            None => ObjKey::Name(name.to_ascii_lowercase()),
        };
        ObjectNode {
            id: ObjectNodeId { app, kind, key },
            name: name.to_string(),
            declared_id,
            extends_target: extends_target.map(str::to_string),
            implements: implements.into_iter().map(str::to_string).collect(),
            tier: TrustTier::Workspace,
            source_table: None,
            table_no: None,
            source_table_temporary: false,
            page_controls: vec![],
            fields: vec![],
            dataitems: vec![],
            query_columns: Vec::new(),
            protected_vars: Vec::new(),
            parse_incomplete: false,
        }
    }

    fn make_routine(obj_id: ObjectNodeId, name: &str) -> RoutineNode {
        RoutineNode {
            id: RoutineNodeId {
                object: obj_id,
                name_lc: name.to_ascii_lowercase(),
                enclosing_member_lc: None,
                params_count: 0,
                sig_fp: 0,
            },
            name: name.to_string(),
            is_trigger: false,
            access: Access::Public,
            tier: TrustTier::Workspace,
            event_subscribers: vec![],
            subscriber_instance_manual: false,
            publisher_kind: None,
            include_sender: None,
            abi_routine_kind: None,
            abi_event_kind: None,
            param_sig_key: String::new(),
            return_type: None,
            return_type_id: None,
            abi_overload_collapsed: false,
            source_overload_aliased: false,
            preproc_context: Box::default(),
            abi_params: AbiParams::Missing,
        }
    }

    fn make_publisher(
        obj_id: ObjectNodeId,
        name: &str,
        params: usize,
        kind: PublisherKind,
        include_sender: Option<bool>,
    ) -> RoutineNode {
        RoutineNode {
            id: RoutineNodeId {
                object: obj_id,
                name_lc: name.to_ascii_lowercase(),
                enclosing_member_lc: None,
                params_count: params,
                sig_fp: 0,
            },
            name: name.to_string(),
            is_trigger: false,
            access: Access::Public,
            tier: TrustTier::Workspace,
            event_subscribers: vec![],
            subscriber_instance_manual: false,
            publisher_kind: Some(kind),
            include_sender,
            abi_routine_kind: None,
            abi_event_kind: None,
            param_sig_key: String::new(),
            return_type: None,
            return_type_id: None,
            abi_overload_collapsed: false,
            source_overload_aliased: false,
            preproc_context: Box::default(),
            abi_params: AbiParams::Missing,
        }
    }

    fn make_subscriber(
        obj_id: ObjectNodeId,
        name: &str,
        params: usize,
        subs: Vec<ParsedSubscriberArgs>,
        manual: bool,
    ) -> RoutineNode {
        RoutineNode {
            id: RoutineNodeId {
                object: obj_id,
                name_lc: name.to_ascii_lowercase(),
                enclosing_member_lc: None,
                params_count: params,
                sig_fp: 0,
            },
            name: name.to_string(),
            is_trigger: false,
            access: Access::Public,
            tier: TrustTier::Workspace,
            event_subscribers: subs,
            subscriber_instance_manual: manual,
            publisher_kind: None,
            include_sender: None,
            abi_routine_kind: None,
            abi_event_kind: None,
            param_sig_key: String::new(),
            return_type: None,
            return_type_id: None,
            abi_overload_collapsed: false,
            source_overload_aliased: false,
            preproc_context: Box::default(),
            abi_params: AbiParams::Missing,
        }
    }

    fn sub_args(pub_name: &str, event: &str) -> ParsedSubscriberArgs {
        ParsedSubscriberArgs {
            publisher_object_type: "codeunit".to_string(),
            publisher_name: pub_name.to_string(),
            publisher_id: None,
            event_name: event.to_string(),
            element: None,
            skip_on_missing_license: false,
            skip_on_missing_permission: false,
        }
    }

    /// Single-app fixture with Codeunit 1 "Pub" and Codeunit 2 "Sub".
    fn build_event_fixture(
        pub_routines: Vec<RoutineNode>,
        sub_routines: Vec<RoutineNode>,
    ) -> (ProgramGraph, ObjectNodeId, ObjectNodeId) {
        let mut apps = AppRegistry::default();
        let app = apps.intern(&make_app_id("App"));
        let topology = DependencyGraph::default();

        let pub_id = ObjectNodeId {
            app,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(1),
        };
        let sub_id = ObjectNodeId {
            app,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(2),
        };

        let pub_obj = make_obj(app, ObjectKind::Codeunit, Some(1), "Pub", None, vec![]);
        let sub_obj = make_obj(app, ObjectKind::Codeunit, Some(2), "Sub", None, vec![]);

        let mut objects = vec![pub_obj, sub_obj];
        objects.sort_by(|x, y| x.id.cmp(&y.id));

        let mut routines: Vec<RoutineNode> = [pub_routines, sub_routines].concat();
        routines.sort_by(|x, y| x.id.cmp(&y.id));

        let obj_index = ObjectIndex::build(&objects);
        (
            ProgramGraph {
                apps,
                topology,
                objects: objects.into(),
                routines: routines.into(),
                obj_index,
                ..Default::default()
            },
            pub_id,
            sub_id,
        )
    }

    /// Builds a two-app fixture:
    ///
    /// - AppA (`a`, AppRef 0) depends on AppB (`b`, AppRef 1).
    /// - AppB has Table 18 "Customer" and Codeunit 50201 "TheirCU" with routine "Do".
    /// - AppA has TableExtension 50100 extending "Customer" and Codeunit 50200
    ///   "SomeImpl" implementing interface "IFoo".
    fn build_fixture() -> (ProgramGraph, AppRef, AppRef) {
        let mut apps = AppRegistry::default();
        let a = apps.intern(&make_app_id("AppA")); // AppRef(0)
        let b = apps.intern(&make_app_id("AppB")); // AppRef(1)

        let mut topology = DependencyGraph::default();
        topology.add_dependency(a, b); // A sees B's objects; B does not see A's.

        let their_cu_id = ObjectNodeId {
            app: b,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(50201),
        };

        let mut objects = vec![
            make_obj(b, ObjectKind::Table, Some(18), "Customer", None, vec![]),
            make_obj(
                a,
                ObjectKind::TableExtension,
                Some(50100),
                "CustomerExt",
                Some("Customer"),
                vec![],
            ),
            make_obj(
                a,
                ObjectKind::Codeunit,
                Some(50200),
                "SomeImpl",
                None,
                vec!["IFoo"],
            ),
            make_obj(
                b,
                ObjectKind::Codeunit,
                Some(50201),
                "TheirCU",
                None,
                vec![],
            ),
        ];
        objects.sort_by(|x, y| x.id.cmp(&y.id));

        let mut routines = vec![make_routine(their_cu_id, "Do")];
        routines.sort_by(|x, y| x.id.cmp(&y.id));

        let obj_index = ObjectIndex::build(&objects);
        (
            ProgramGraph {
                apps,
                topology,
                objects: objects.into(),
                routines: routines.into(),
                obj_index,
                ..Default::default()
            },
            a,
            b,
        )
    }

    /// The census must name every `ResolveIndex` field, no more, no fewer. The
    /// field list is read from this file's own struct text, so a new field
    /// fails here even if the destructure in `census_parts` is loosened.
    #[test]
    fn census_parts_names_every_field() {
        let src = include_str!("index.rs");
        let start = src.find("pub struct ResolveIndex {").unwrap();
        let body = &src[start..][..src[start..].find("\n}\n").unwrap()];
        let mut declared: Vec<&str> = body
            .lines()
            .skip(1)
            .filter(|l| !l.trim_start().starts_with("//"))
            .filter_map(|l| l.trim().split_once(':').map(|(n, _)| n))
            .collect();
        declared.sort_unstable();
        let (graph, _a, _b) = build_fixture();
        let mut named: Vec<&str> = ResolveIndex::build(&graph)
            .census_parts()
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        named.sort_unstable();
        assert_eq!(named, declared);
    }

    // -- object_by_number tests -----------------------------------------------

    #[test]
    fn object_by_number_finds_dep_in_closure() {
        let (graph, a, b) = build_fixture();
        let idx = ResolveIndex::build(&graph);

        // A depends on B; Customer (Table 18) is in B → visible from A.
        let found = idx.object_by_number(&graph, a, ObjectKind::Table, 18);
        assert!(found.is_some(), "Table 18 must be visible from AppA");
        let oid = found.unwrap();
        assert_eq!(oid.app, b, "must resolve to AppB's Customer");
        assert_eq!(oid.key, ObjKey::Id(18));
    }

    #[test]
    fn object_by_number_prefers_self() {
        // Add a codeunit 50201 to AppA as well — from AppA, it should win.
        let (mut graph, a, _b) = build_fixture();
        let extra = make_obj(a, ObjectKind::Codeunit, Some(50201), "OurCU", None, vec![]);
        graph.objects.push(extra);
        graph.objects.sort_by(|x, y| x.id.cmp(&y.id));
        graph.obj_index = ObjectIndex::build(&graph.objects);

        let idx = ResolveIndex::build(&graph);
        let found = idx.object_by_number(&graph, a, ObjectKind::Codeunit, 50201);
        assert!(found.is_some());
        assert_eq!(found.unwrap().app, a, "own app must be preferred over dep");
    }

    #[test]
    fn object_by_number_outside_closure_returns_none() {
        let (graph, _a, b) = build_fixture();
        let idx = ResolveIndex::build(&graph);

        // B does not depend on A; TableExtension 50100 is in A → invisible from B.
        let found = idx.object_by_number(&graph, b, ObjectKind::TableExtension, 50100);
        assert!(
            found.is_none(),
            "TableExtension 50100 (AppA) must not be visible from AppB"
        );

        // Verify A itself also doesn't have it when queried by a completely unknown AppRef.
        let unknown = AppRef(99);
        let found2 = idx.object_by_number(&graph, unknown, ObjectKind::Table, 18);
        assert!(found2.is_none(), "unknown app has empty closure");
    }

    /// Three-app fixture for `object_by_number`'s I1 root-fix tests:
    /// - AppA (`a`, the `from` app) depends on AppB and AppC.
    /// - AppB and AppC both declare Table 900 (a genuine cross-app id
    ///   collision, neither app is `a` itself).
    /// - AppA and AppB both declare Table 950 (own-app-shadow case: A's own
    ///   declaration must still win over B's colliding one).
    fn build_three_app_fixture() -> (ProgramGraph, AppRef, AppRef, AppRef) {
        let mut apps = AppRegistry::default();
        let a = apps.intern(&make_app_id("AppA"));
        let b = apps.intern(&make_app_id("AppB"));
        let c = apps.intern(&make_app_id("AppC"));

        let mut topology = DependencyGraph::default();
        topology.add_dependency(a, b);
        topology.add_dependency(a, c);

        let mut objects = vec![
            make_obj(b, ObjectKind::Table, Some(900), "SharedB", None, vec![]),
            make_obj(c, ObjectKind::Table, Some(900), "SharedC", None, vec![]),
            make_obj(a, ObjectKind::Table, Some(950), "OwnShadow", None, vec![]),
            make_obj(b, ObjectKind::Table, Some(950), "OwnShadow", None, vec![]),
        ];
        objects.sort_by(|x, y| x.id.cmp(&y.id));

        let obj_index = ObjectIndex::build(&objects);
        (
            ProgramGraph {
                apps,
                topology,
                objects: objects.into(),
                routines: vec![].into(),
                obj_index,
                ..Default::default()
            },
            a,
            b,
            c,
        )
    }

    #[test]
    fn object_by_number_declines_on_cross_app_dependency_collision_never_lowest_id() {
        let (graph, a, _b, _c) = build_three_app_fixture();
        let idx = ResolveIndex::build(&graph);

        // AppB and AppC both declare Table 900 — neither is A's own app. The
        // pre-fix behavior silently picked the lowest ObjectNodeId (I1); the
        // fixed behavior must decline instead of guessing which dep "wins".
        let found = idx.object_by_number(&graph, a, ObjectKind::Table, 900);
        assert!(
            found.is_none(),
            "cross-app dependency collision on id 900 must decline (None), \
             never silently pick the lowest id"
        );
    }

    #[test]
    fn object_by_number_own_app_shadow_survives_dependency_collision() {
        let (graph, a, b, _c) = build_three_app_fixture();
        let idx = ResolveIndex::build(&graph);

        // A declares its own Table 950; B ALSO declares a same-id table — A's
        // own declaration must still win outright (shadow preserved).
        let found = idx.object_by_number(&graph, a, ObjectKind::Table, 950);
        assert!(found.is_some());
        let oid = found.unwrap();
        assert_eq!(
            oid.app, a,
            "own-app declaration must shadow a colliding dependency"
        );
        assert_ne!(oid.app, b);
    }

    // -- resolve_object_ref tests ----------------------------------------------

    /// Fixture for `resolve_object_ref`:
    /// - AppA (the `from` app) depends on AppB and AppC. AppD is interned but
    ///   never added as a dependency of A — out of A's closure.
    /// - AppA: Table 500 "Shared" (own declaration).
    /// - AppB: Table 501 "Shared" (name collides with A's own — A must shadow
    ///   it); Table 100 "Item"; Table 200 "OnlyInB".
    /// - AppC: Table 100 "Item" (id AND name collide with AppB's — neither is
    ///   `from`'s own app, so both the id- and name-arm see a genuine
    ///   cross-app ambiguity).
    /// - AppD: Table 900 "Foreign" — declared, but AppD is unreachable from A.
    /// - AppA: Table 600 "OwnIdShadow" (own declaration); AppB: Table 600
    ///   "TheirIdShadow" (dep, SAME id, DIFFERENT name — proves the Id arm's
    ///   own-app-shadow matches purely on `(kind, id)`, ignoring name; A's own
    ///   declaration must win outright, never `Ambiguous`).
    fn build_ref_fixture() -> (ProgramGraph, ObjectNodeId) {
        let mut apps = AppRegistry::default();
        let a = apps.intern(&make_app_id("AppA"));
        let b = apps.intern(&make_app_id("AppB"));
        let c = apps.intern(&make_app_id("AppC"));
        let d = apps.intern(&make_app_id("AppD"));

        let mut topology = DependencyGraph::default();
        topology.add_dependency(a, b);
        topology.add_dependency(a, c);
        // `d` is intentionally never wired in — out of A's closure.

        let mut objects = vec![
            make_obj(a, ObjectKind::Table, Some(500), "Shared", None, vec![]),
            make_obj(b, ObjectKind::Table, Some(501), "Shared", None, vec![]),
            make_obj(b, ObjectKind::Table, Some(100), "Item", None, vec![]),
            make_obj(b, ObjectKind::Table, Some(200), "OnlyInB", None, vec![]),
            make_obj(c, ObjectKind::Table, Some(100), "Item", None, vec![]),
            make_obj(d, ObjectKind::Table, Some(900), "Foreign", None, vec![]),
            make_obj(a, ObjectKind::Table, Some(600), "OwnIdShadow", None, vec![]),
            make_obj(
                b,
                ObjectKind::Table,
                Some(600),
                "TheirIdShadow",
                None,
                vec![],
            ),
        ];
        objects.sort_by(|x, y| x.id.cmp(&y.id));

        let obj_index = ObjectIndex::build(&objects);
        let from = ObjectNodeId {
            app: a,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(1),
        };
        (
            ProgramGraph {
                apps,
                topology,
                objects: objects.into(),
                routines: vec![].into(),
                obj_index,
                ..Default::default()
            },
            from,
        )
    }

    fn name_ref(s: &str) -> ObjectRef {
        ObjectRef::Name {
            raw: s.to_string(),
            normalized_lc: s.to_ascii_lowercase(),
        }
    }

    #[test]
    fn resolve_object_ref_id_unique_same_kind() {
        let (graph, from) = build_ref_fixture();
        let idx = ResolveIndex::build(&graph);

        let res = idx.resolve_object_ref(&graph, from, ObjectKind::Table, &ObjectRef::Id(200));
        match res {
            ObjectRefResolution::Unique(oid) => {
                assert!(oid.id_equals_number(200));
                assert_eq!(oid.kind, ObjectKind::Table);
            }
            other => panic!("expected Unique, got {other:?}"),
        }
    }

    #[test]
    fn resolve_object_ref_id_wrong_kind_is_unresolved() {
        let (graph, from) = build_ref_fixture();
        let idx = ResolveIndex::build(&graph);

        // 500 is declared as a Table, never as a Codeunit.
        let res = idx.resolve_object_ref(&graph, from, ObjectKind::Codeunit, &ObjectRef::Id(500));
        assert_eq!(res, ObjectRefResolution::Unresolved);
    }

    #[test]
    fn resolve_object_ref_id_absent_is_unresolved() {
        let (graph, from) = build_ref_fixture();
        let idx = ResolveIndex::build(&graph);

        let res = idx.resolve_object_ref(&graph, from, ObjectKind::Table, &ObjectRef::Id(999_999));
        assert_eq!(res, ObjectRefResolution::Unresolved);
    }

    #[test]
    fn resolve_object_ref_id_out_of_closure() {
        let (graph, from) = build_ref_fixture();
        let idx = ResolveIndex::build(&graph);

        // 900 is declared in AppD, which A does not depend on.
        let res = idx.resolve_object_ref(&graph, from, ObjectKind::Table, &ObjectRef::Id(900));
        assert_eq!(res, ObjectRefResolution::OutOfClosure);
    }

    #[test]
    fn resolve_object_ref_id_ambiguous_cross_app_collision() {
        let (graph, from) = build_ref_fixture();
        let idx = ResolveIndex::build(&graph);

        // 100 is declared as Table "Item" in BOTH AppB and AppC — neither is A.
        let res = idx.resolve_object_ref(&graph, from, ObjectKind::Table, &ObjectRef::Id(100));
        assert_eq!(res, ObjectRefResolution::Ambiguous);
    }

    #[test]
    fn resolve_object_ref_id_own_app_shadows_dependency() {
        let (graph, from) = build_ref_fixture();
        let idx = ResolveIndex::build(&graph);
        let from_app = from.app;

        // 600 is declared as a Table in BOTH A (own app) and B (a dep) — A's
        // own declaration must win outright, mirroring the Name arm's
        // own-app-shadow and matching `object_by_number`'s existing
        // self-shortcut. Never `Ambiguous` when `from`'s own app is one of
        // the colliding declarations.
        let res = idx.resolve_object_ref(&graph, from, ObjectKind::Table, &ObjectRef::Id(600));
        match res {
            ObjectRefResolution::Unique(oid) => {
                assert_eq!(
                    oid.app, from_app,
                    "A's own Table 600 must win over B's same-id dep"
                );
                assert!(oid.id_equals_number(600));
            }
            other => panic!("expected Unique(A's Table 600), got {other:?}"),
        }
    }

    #[test]
    fn resolve_object_ref_name_unique_in_closure() {
        let (graph, from) = build_ref_fixture();
        let idx = ResolveIndex::build(&graph);

        let res = idx.resolve_object_ref(&graph, from, ObjectKind::Table, &name_ref("OnlyInB"));
        match res {
            ObjectRefResolution::Unique(oid) => assert!(oid.id_equals_number(200)),
            other => panic!("expected Unique, got {other:?}"),
        }
    }

    #[test]
    fn resolve_object_ref_name_ambiguous_two_apps() {
        let (graph, from) = build_ref_fixture();
        let idx = ResolveIndex::build(&graph);

        // "Item" is declared in both AppB and AppC — neither is A itself.
        let res = idx.resolve_object_ref(&graph, from, ObjectKind::Table, &name_ref("Item"));
        assert_eq!(res, ObjectRefResolution::Ambiguous);
    }

    #[test]
    fn resolve_object_ref_name_own_app_shadows_dependency() {
        let (graph, from) = build_ref_fixture();
        let idx = ResolveIndex::build(&graph);
        let from_app = from.app;

        // "Shared" is declared in BOTH A (id 500) and B (id 501) — A's own
        // declaration must win outright, never Ambiguous.
        let res = idx.resolve_object_ref(&graph, from, ObjectKind::Table, &name_ref("Shared"));
        match res {
            ObjectRefResolution::Unique(oid) => {
                assert!(oid.id_equals_number(500), "A's own Shared must win");
                assert_eq!(oid.app, from_app);
            }
            other => panic!("expected Unique(A's Shared), got {other:?}"),
        }
    }

    #[test]
    fn resolve_object_ref_name_out_of_closure() {
        let (graph, from) = build_ref_fixture();
        let idx = ResolveIndex::build(&graph);

        let res = idx.resolve_object_ref(&graph, from, ObjectKind::Table, &name_ref("Foreign"));
        assert_eq!(res, ObjectRefResolution::OutOfClosure);
    }

    #[test]
    fn resolve_object_ref_name_unresolved() {
        let (graph, from) = build_ref_fixture();
        let idx = ResolveIndex::build(&graph);

        let res = idx.resolve_object_ref(
            &graph,
            from,
            ObjectKind::Table,
            &name_ref("NoSuchTableAnywhere"),
        );
        assert_eq!(res, ObjectRefResolution::Unresolved);
    }

    #[test]
    fn resolve_object_ref_is_deterministic_across_two_builds() {
        let (graph1, from1) = build_ref_fixture();
        let idx1 = ResolveIndex::build(&graph1);
        let (graph2, from2) = build_ref_fixture();
        let idx2 = ResolveIndex::build(&graph2);

        let ambiguous1 =
            idx1.resolve_object_ref(&graph1, from1.clone(), ObjectKind::Table, &name_ref("Item"));
        let ambiguous2 =
            idx2.resolve_object_ref(&graph2, from2.clone(), ObjectKind::Table, &name_ref("Item"));
        assert_eq!(ambiguous1, ambiguous2);
        assert_eq!(ambiguous1, ObjectRefResolution::Ambiguous);

        let unique1 =
            idx1.resolve_object_ref(&graph1, from1, ObjectKind::Table, &ObjectRef::Id(200));
        let unique2 =
            idx2.resolve_object_ref(&graph2, from2, ObjectKind::Table, &ObjectRef::Id(200));
        assert_eq!(unique1, unique2);
    }

    // -- table_extensions_of tests --------------------------------------------

    #[test]
    fn table_extensions_of_returns_extension() {
        let (graph, a, _b) = build_fixture();
        let idx = ResolveIndex::build(&graph);

        let exts = idx.table_extensions_of("customer");
        assert_eq!(exts.len(), 1, "expected exactly one extension of Customer");
        assert_eq!(exts[0].app, a);
        assert_eq!(exts[0].kind, ObjectKind::TableExtension);
    }

    #[test]
    fn table_extensions_of_missing_returns_empty() {
        let (graph, _, _) = build_fixture();
        let idx = ResolveIndex::build(&graph);

        let exts = idx.table_extensions_of("nosuchtable");
        assert!(exts.is_empty());
    }

    // -- page_extensions_of tests (pageext-merge-and-final-residual plan, Task 1) --

    /// Single-app fixture: Page 50400 "PxBase" + PageExtension 50401
    /// "PxBaseExt" extending it.
    fn build_page_ext_fixture() -> (ProgramGraph, AppRef) {
        let mut apps = AppRegistry::default();
        let a = apps.intern(&make_app_id("PxApp"));
        let topology = DependencyGraph::default();

        let mut objects = vec![
            make_obj(a, ObjectKind::Page, Some(50400), "PxBase", None, vec![]),
            make_obj(
                a,
                ObjectKind::PageExtension,
                Some(50401),
                "PxBaseExt",
                Some("PxBase"),
                vec![],
            ),
        ];
        objects.sort_by(|x, y| x.id.cmp(&y.id));

        let obj_index = ObjectIndex::build(&objects);
        (
            ProgramGraph {
                apps,
                topology,
                objects: objects.into(),
                routines: vec![].into(),
                obj_index,
                ..Default::default()
            },
            a,
        )
    }

    #[test]
    fn page_extensions_of_returns_extension() {
        let (graph, a) = build_page_ext_fixture();
        let idx = ResolveIndex::build(&graph);

        let exts = idx.page_extensions_of("pxbase");
        assert_eq!(exts.len(), 1, "expected exactly one extension of PxBase");
        assert_eq!(exts[0].app, a);
        assert_eq!(exts[0].kind, ObjectKind::PageExtension);
    }

    #[test]
    fn page_extensions_of_missing_returns_empty() {
        let (graph, _a) = build_page_ext_fixture();
        let idx = ResolveIndex::build(&graph);

        let exts = idx.page_extensions_of("nosuchpage");
        assert!(exts.is_empty());
    }

    #[test]
    fn page_extensions_of_does_not_leak_table_extensions() {
        // A TableExtension named the same shape must never satisfy a
        // page_extensions_of lookup — the two indexes are kind-partitioned
        // at build time (each `if obj.id.kind == ...` guard is independent),
        // never merged into one name-keyed map.
        let (graph, _a, _b) = build_fixture(); // has a TableExtension "CustomerExt" extending "Customer"
        let idx = ResolveIndex::build(&graph);

        assert!(idx.page_extensions_of("customer").is_empty());
    }

    // -- report_extensions_of tests (roadmap-closure plan, Task 1) -------------

    /// Single-app fixture: Report 50700 "RxBase" + ReportExtension 50701
    /// "RxBaseExt" extending it — mirrors `build_page_ext_fixture` exactly.
    fn build_report_ext_fixture() -> (ProgramGraph, AppRef) {
        let mut apps = AppRegistry::default();
        let a = apps.intern(&make_app_id("RxApp"));
        let topology = DependencyGraph::default();

        let mut objects = vec![
            make_obj(a, ObjectKind::Report, Some(50700), "RxBase", None, vec![]),
            make_obj(
                a,
                ObjectKind::ReportExtension,
                Some(50701),
                "RxBaseExt",
                Some("RxBase"),
                vec![],
            ),
        ];
        objects.sort_by(|x, y| x.id.cmp(&y.id));

        let obj_index = ObjectIndex::build(&objects);
        (
            ProgramGraph {
                apps,
                topology,
                objects: objects.into(),
                routines: vec![].into(),
                obj_index,
                ..Default::default()
            },
            a,
        )
    }

    #[test]
    fn report_extensions_of_returns_extension() {
        let (graph, a) = build_report_ext_fixture();
        let idx = ResolveIndex::build(&graph);

        let exts = idx.report_extensions_of("rxbase");
        assert_eq!(exts.len(), 1, "expected exactly one extension of RxBase");
        assert_eq!(exts[0].app, a);
        assert_eq!(exts[0].kind, ObjectKind::ReportExtension);
    }

    #[test]
    fn report_extensions_of_missing_returns_empty() {
        let (graph, _a) = build_report_ext_fixture();
        let idx = ResolveIndex::build(&graph);

        let exts = idx.report_extensions_of("nosuchreport");
        assert!(exts.is_empty());
    }

    #[test]
    fn report_extensions_of_does_not_leak_page_or_table_extensions() {
        // A TableExtension/PageExtension named the same shape must never
        // satisfy a report_extensions_of lookup — all three indexes are
        // kind-partitioned at build time (each `if obj.id.kind == ...` guard
        // is independent), never merged into one name-keyed map.
        let (graph_t, _a, _b) = build_fixture(); // TableExtension "CustomerExt" extending "Customer"
        let idx_t = ResolveIndex::build(&graph_t);
        assert!(idx_t.report_extensions_of("customer").is_empty());

        let (graph_p, _a2) = build_page_ext_fixture(); // PageExtension "PxBaseExt" extending "PxBase"
        let idx_p = ResolveIndex::build(&graph_p);
        assert!(idx_p.report_extensions_of("pxbase").is_empty());
    }

    // -- field_in_table tests (Task 3) -----------------------------------------
    //
    // Reuses `build_fixture`'s topology: App A (`a`) depends on App B (`b`);
    // App B does NOT depend on App A. Table 18 "Customer" lives in App B;
    // TableExtension 50100 "CustomerExt" (extends "Customer") lives in App A.
    // So from App A's perspective, BOTH the base table and its extension are
    // visible; from App B's perspective, the base table is visible (it's
    // B's own) but the extension is OUT OF CLOSURE (B never imported A).

    fn customer_table_id(b: AppRef) -> ObjectNodeId {
        ObjectNodeId {
            app: b,
            kind: ObjectKind::Table,
            key: ObjKey::Id(18),
        }
    }

    fn find_obj_mut<'g>(graph: &'g mut ProgramGraph, id: &ObjectNodeId) -> &'g mut ObjectNode {
        graph
            .objects
            .iter_mut()
            .find(|o| &o.id == id)
            .expect("object must exist in fixture")
    }

    #[test]
    fn field_in_table_unique_base_field_resolves() {
        let (mut graph, _a, b) = build_fixture();
        let customer_id = customer_table_id(b);
        find_obj_mut(&mut graph, &customer_id)
            .fields
            .push(FieldNode {
                name_lc: "no.".to_string(),
                type_text: "Code[20]".to_string(),
            });
        let idx = ResolveIndex::build(&graph);
        let from_obj = graph
            .objects
            .iter()
            .find(|o| o.name == "SomeImpl")
            .unwrap()
            .clone();

        let found = idx.field_in_table(&graph, &from_obj, &customer_id, "no.");
        assert_eq!(
            found,
            Some(FieldNode {
                name_lc: "no.".to_string(),
                type_text: "Code[20]".to_string(),
            })
        );
    }

    #[test]
    fn field_in_table_unknown_field_name_declines() {
        let (mut graph, _a, b) = build_fixture();
        let customer_id = customer_table_id(b);
        find_obj_mut(&mut graph, &customer_id)
            .fields
            .push(FieldNode {
                name_lc: "no.".to_string(),
                type_text: "Code[20]".to_string(),
            });
        let idx = ResolveIndex::build(&graph);
        let from_obj = graph
            .objects
            .iter()
            .find(|o| o.name == "SomeImpl")
            .unwrap()
            .clone();

        assert_eq!(
            idx.field_in_table(&graph, &from_obj, &customer_id, "no such field"),
            None
        );
    }

    #[test]
    fn field_in_table_extension_field_folds_into_base_scope() {
        let (mut graph, a, b) = build_fixture();
        let ext_id = ObjectNodeId {
            app: a,
            kind: ObjectKind::TableExtension,
            key: ObjKey::Id(50100),
        };
        find_obj_mut(&mut graph, &ext_id).fields.push(FieldNode {
            name_lc: "ext blob".to_string(),
            type_text: "Blob".to_string(),
        });
        let idx = ResolveIndex::build(&graph);
        // Referencing object lives in App A — sees both Customer (its dep,
        // App B) and CustomerExt (its own app).
        let from_obj = graph
            .objects
            .iter()
            .find(|o| o.name == "SomeImpl")
            .unwrap()
            .clone();
        let customer_id = customer_table_id(b);

        let found = idx.field_in_table(&graph, &from_obj, &customer_id, "ext blob");
        assert_eq!(
            found,
            Some(FieldNode {
                name_lc: "ext blob".to_string(),
                type_text: "Blob".to_string(),
            }),
            "an extension field visible in from_object's closure must fold into the base scope"
        );
    }

    #[test]
    fn field_in_table_out_of_closure_extension_field_declines() {
        let (mut graph, a, b) = build_fixture();
        let ext_id = ObjectNodeId {
            app: a,
            kind: ObjectKind::TableExtension,
            key: ObjKey::Id(50100),
        };
        find_obj_mut(&mut graph, &ext_id).fields.push(FieldNode {
            name_lc: "ext blob".to_string(),
            type_text: "Blob".to_string(),
        });
        let idx = ResolveIndex::build(&graph);
        // Referencing object lives in App B — App B does NOT depend on App A
        // (build_fixture wires only `a -> b`), so CustomerExt (App A) is
        // outside B's closure — fail-closed: must NOT resolve, even though
        // the base table itself (Customer, App B) is B's own object.
        let from_obj = graph
            .objects
            .iter()
            .find(|o| o.name == "TheirCU")
            .unwrap()
            .clone();
        let customer_id = customer_table_id(b);

        let found = idx.field_in_table(&graph, &from_obj, &customer_id, "ext blob");
        assert_eq!(
            found, None,
            "an extension field OUTSIDE from_object's dependency closure must never resolve"
        );
    }

    #[test]
    fn field_in_table_duplicate_across_base_and_extension_declines() {
        let (mut graph, a, b) = build_fixture();
        let customer_id = customer_table_id(b);
        let ext_id = ObjectNodeId {
            app: a,
            kind: ObjectKind::TableExtension,
            key: ObjKey::Id(50100),
        };
        find_obj_mut(&mut graph, &customer_id)
            .fields
            .push(FieldNode {
                name_lc: "dup field".to_string(),
                type_text: "Blob".to_string(),
            });
        find_obj_mut(&mut graph, &ext_id).fields.push(FieldNode {
            name_lc: "dup field".to_string(),
            type_text: "Text[50]".to_string(),
        });
        let idx = ResolveIndex::build(&graph);
        let from_obj = graph
            .objects
            .iter()
            .find(|o| o.name == "SomeImpl")
            .unwrap()
            .clone();

        let found = idx.field_in_table(&graph, &from_obj, &customer_id, "dup field");
        assert_eq!(
            found, None,
            "a same-name field declared by BOTH the base and a visible extension \
             must decline (fail-closed ambiguity), never guess"
        );
    }

    #[test]
    fn field_in_table_identical_duplicate_by_provenance_dedupes_and_resolves() {
        // Mirrors a source field declared inside BOTH branches of a `#if`/
        // `#else` (`FieldDecl` extraction collects from both — see
        // `al_syntax::ir::decl::ObjectDecl`'s doc): the SAME declaring
        // object ends up with two IDENTICAL (name, type) entries. This must
        // NOT manufacture an artificial ambiguity.
        let (mut graph, _a, b) = build_fixture();
        let customer_id = customer_table_id(b);
        let obj = find_obj_mut(&mut graph, &customer_id);
        obj.fields.push(FieldNode {
            name_lc: "no.".to_string(),
            type_text: "Code[20]".to_string(),
        });
        obj.fields.push(FieldNode {
            name_lc: "no.".to_string(),
            type_text: "Code[20]".to_string(),
        });
        let idx = ResolveIndex::build(&graph);
        let from_obj = graph
            .objects
            .iter()
            .find(|o| o.name == "SomeImpl")
            .unwrap()
            .clone();

        let found = idx.field_in_table(&graph, &from_obj, &customer_id, "no.");
        assert_eq!(
            found,
            Some(FieldNode {
                name_lc: "no.".to_string(),
                type_text: "Code[20]".to_string(),
            }),
            "an identical (object, name, type) duplicate must dedupe to one candidate, not decline"
        );
    }

    // -- table_scope_has_routine tests (round-2, Task 4) -----------------------

    #[test]
    fn table_scope_has_routine_finds_base_routine() {
        let (mut graph, _a, b) = build_fixture();
        let customer_id = customer_table_id(b);
        graph
            .routines
            .push(make_routine(customer_id.clone(), "GetThing"));
        graph.routines.sort_by(|x, y| x.id.cmp(&y.id));
        let idx = ResolveIndex::build(&graph);
        let from_obj = graph
            .objects
            .iter()
            .find(|o| o.name == "SomeImpl")
            .unwrap()
            .clone();

        assert!(idx.table_scope_has_routine(&graph, &from_obj, &customer_id, "getthing"));
    }

    #[test]
    fn table_scope_has_routine_finds_extension_routine_in_closure() {
        let (mut graph, a, b) = build_fixture();
        let ext_id = ObjectNodeId {
            app: a,
            kind: ObjectKind::TableExtension,
            key: ObjKey::Id(50100),
        };
        graph.routines.push(make_routine(ext_id, "GetThing"));
        graph.routines.sort_by(|x, y| x.id.cmp(&y.id));
        let idx = ResolveIndex::build(&graph);
        // Referencing object lives in App A — sees CustomerExt (its own app).
        let from_obj = graph
            .objects
            .iter()
            .find(|o| o.name == "SomeImpl")
            .unwrap()
            .clone();
        let customer_id = customer_table_id(b);

        assert!(
            idx.table_scope_has_routine(&graph, &from_obj, &customer_id, "getthing"),
            "an extension routine visible in from_object's closure must count"
        );
    }

    #[test]
    fn table_scope_has_routine_out_of_closure_extension_routine_declines() {
        let (mut graph, a, b) = build_fixture();
        let ext_id = ObjectNodeId {
            app: a,
            kind: ObjectKind::TableExtension,
            key: ObjKey::Id(50100),
        };
        graph.routines.push(make_routine(ext_id, "GetThing"));
        graph.routines.sort_by(|x, y| x.id.cmp(&y.id));
        let idx = ResolveIndex::build(&graph);
        // Referencing object lives in App B — App B does NOT depend on App A,
        // so CustomerExt (App A) is outside B's closure.
        let from_obj = graph
            .objects
            .iter()
            .find(|o| o.name == "TheirCU")
            .unwrap()
            .clone();
        let customer_id = customer_table_id(b);

        assert!(
            !idx.table_scope_has_routine(&graph, &from_obj, &customer_id, "getthing"),
            "an extension routine OUTSIDE from_object's dependency closure must not count"
        );
    }

    #[test]
    fn table_scope_has_routine_absent_returns_false() {
        let (graph, _a, b) = build_fixture();
        let idx = ResolveIndex::build(&graph);
        let from_obj = graph
            .objects
            .iter()
            .find(|o| o.name == "SomeImpl")
            .unwrap()
            .clone();
        let customer_id = customer_table_id(b);

        assert!(!idx.table_scope_has_routine(&graph, &from_obj, &customer_id, "nosuchroutine"));
    }

    // -- implementers_of tests ------------------------------------------------

    #[test]
    fn implementers_of_returns_codeunit() {
        let (graph, a, _b) = build_fixture();
        let idx = ResolveIndex::build(&graph);

        let impls = idx.implementers_of("ifoo");
        assert_eq!(impls.len(), 1, "expected exactly one implementer of IFoo");
        assert_eq!(impls[0].app, a);
        assert_eq!(impls[0].kind, ObjectKind::Codeunit);
    }

    /// S9.0e: an enum that `implements` the interface is not a method
    /// implementer, and a codeunit listing the interface twice (one per `#if`
    /// arm of a split header) is one implementer.
    #[test]
    fn implementers_of_is_codeunits_once_each() {
        let (mut graph, _a, _b) = build_fixture();
        let cu = graph
            .objects
            .iter_mut()
            .find(|o| o.implements.iter().any(|i| i.eq_ignore_ascii_case("ifoo")))
            .expect("fixture implementer");
        cu.implements.push("IFoo".to_string());
        let mut en = cu.clone();
        en.id.kind = ObjectKind::Enum;
        en.name = "IFooEnum".to_string();
        graph.objects.push(en);
        let idx = ResolveIndex::build(&graph);

        let impls = idx.implementers_of("ifoo");
        assert_eq!(impls.len(), 1, "{impls:?}");
        assert_eq!(impls[0].kind, ObjectKind::Codeunit);
    }

    #[test]
    fn implementers_of_missing_returns_empty() {
        let (graph, _, _) = build_fixture();
        let idx = ResolveIndex::build(&graph);

        assert!(idx.implementers_of("ibar").is_empty());
    }

    // -- routines_in_object test ----------------------------------------------

    #[test]
    fn routines_in_object_finds_routine() {
        let (graph, _a, b) = build_fixture();
        let idx = ResolveIndex::build(&graph);

        let their_cu = ObjectNodeId {
            app: b,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(50201),
        };
        let rids: Vec<_> = idx.routines_in_object(&graph, &their_cu, "do").collect();
        assert_eq!(rids.len(), 1);
        assert_eq!(rids[0].name_lc, "do");
    }

    #[test]
    fn routines_in_object_absent_returns_empty() {
        let (graph, _a, b) = build_fixture();
        let idx = ResolveIndex::build(&graph);

        let their_cu = ObjectNodeId {
            app: b,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(50201),
        };
        assert_eq!(
            idx.routines_in_object(&graph, &their_cu, "notexist").len(),
            0
        );
    }

    // -- subscribers_of tests -------------------------------------------------

    #[test]
    fn subscribers_of_stub_returns_empty() {
        let (graph, a, _b) = build_fixture();
        let idx = SubscriberIndex::build(&graph);

        let fake_pub = RoutineNodeId {
            object: ObjectNodeId {
                app: a,
                kind: ObjectKind::Codeunit,
                key: ObjKey::Id(50200),
            },
            name_lc: "publisher".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        assert!(
            idx.subscribers_of(&fake_pub).is_empty(),
            "unknown publisher must return empty"
        );
    }

    // (a) Basic manual subscriber --------------------------------------------

    #[test]
    fn subscribers_of_basic_manual() {
        let app = AppRef(0); // deterministic: first intern in a fresh registry
        let pub_id = ObjectNodeId {
            app,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(1),
        };
        let sub_id = ObjectNodeId {
            app,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(2),
        };
        let pub_onafterx_id = RoutineNodeId {
            object: pub_id.clone(),
            name_lc: "onafterx".to_string(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };

        let (graph, _, _) = build_event_fixture(
            vec![make_publisher(
                pub_id.clone(),
                "OnAfterX",
                0,
                PublisherKind::Integration,
                Some(false),
            )],
            vec![make_subscriber(
                sub_id,
                "Handler",
                0,
                vec![sub_args("pub", "onafterx")],
                true,
            )],
        );

        let idx = SubscriberIndex::build(&graph);
        let subs = idx.subscribers_of(&pub_onafterx_id);
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].conditions, vec![Condition::ManualBinding]);
        assert_eq!(subs[0].element, None);
    }

    // (b) One handler subscribing to two different events --------------------

    #[test]
    fn subscribers_of_handler_with_two_event_subscriber_attrs() {
        let app = AppRef(0);
        let pub_id = ObjectNodeId {
            app,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(1),
        };
        let sub_id = ObjectNodeId {
            app,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(2),
        };
        let pub_onafterx_id = RoutineNodeId {
            object: pub_id.clone(),
            name_lc: "onafterx".to_string(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        let pub_onbeforex_id = RoutineNodeId {
            object: pub_id.clone(),
            name_lc: "onbeforex".to_string(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };

        let (graph, _, _) = build_event_fixture(
            vec![
                make_publisher(
                    pub_id.clone(),
                    "OnAfterX",
                    0,
                    PublisherKind::Integration,
                    Some(false),
                ),
                make_publisher(
                    pub_id.clone(),
                    "OnBeforeX",
                    0,
                    PublisherKind::Integration,
                    Some(false),
                ),
            ],
            vec![make_subscriber(
                sub_id,
                "Handler",
                0,
                vec![sub_args("pub", "onafterx"), sub_args("pub", "onbeforex")],
                false,
            )],
        );

        let idx = SubscriberIndex::build(&graph);
        assert_eq!(idx.subscribers_of(&pub_onafterx_id).len(), 1);
        assert_eq!(idx.subscribers_of(&pub_onbeforex_id).len(), 1);
    }

    // (c) SkipOnMissingLicense condition -------------------------------------

    #[test]
    fn subscribers_of_skip_on_missing_license() {
        let app = AppRef(0);
        let pub_id = ObjectNodeId {
            app,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(1),
        };
        let sub_id = ObjectNodeId {
            app,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(2),
        };
        let pub_onafterx_id = RoutineNodeId {
            object: pub_id.clone(),
            name_lc: "onafterx".to_string(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };

        let mut sa = sub_args("pub", "onafterx");
        sa.skip_on_missing_license = true;

        let (graph, _, _) = build_event_fixture(
            vec![make_publisher(
                pub_id.clone(),
                "OnAfterX",
                0,
                PublisherKind::Integration,
                Some(false),
            )],
            vec![make_subscriber(sub_id, "Handler", 0, vec![sa], false)],
        );

        let idx = SubscriberIndex::build(&graph);
        let subs = idx.subscribers_of(&pub_onafterx_id);
        assert_eq!(subs.len(), 1);
        assert!(
            subs[0]
                .conditions
                .contains(&Condition::SkipOnMissingLicense)
        );
        assert!(!subs[0].conditions.contains(&Condition::ManualBinding));
    }

    // (d) Ambiguous overloads — no strict arity match → AmbiguousSub --------

    #[test]
    fn subscribers_of_ambiguous_overloads_no_strict_match() {
        let app = AppRef(0);
        let pub_id = ObjectNodeId {
            app,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(1),
        };
        let sub_id = ObjectNodeId {
            app,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(2),
        };
        let pub_onafterx_1param_id = RoutineNodeId {
            object: pub_id.clone(),
            name_lc: "onafterx".to_string(),
            enclosing_member_lc: None,
            params_count: 1,
            sig_fp: 0,
        };
        let pub_onafterx_2param_id = RoutineNodeId {
            object: pub_id.clone(),
            name_lc: "onafterx".to_string(),
            enclosing_member_lc: None,
            params_count: 2,
            sig_fp: 0,
        };

        // Subscriber params=0: both overloads satisfy >=0 but neither equals 0.
        let (graph, _, _) = build_event_fixture(
            vec![
                make_publisher(
                    pub_id.clone(),
                    "OnAfterX",
                    1,
                    PublisherKind::Integration,
                    Some(false),
                ),
                make_publisher(
                    pub_id.clone(),
                    "OnAfterX",
                    2,
                    PublisherKind::Integration,
                    Some(false),
                ),
            ],
            vec![make_subscriber(
                sub_id,
                "Handler",
                0,
                vec![sub_args("pub", "onafterx")],
                false,
            )],
        );

        let idx = SubscriberIndex::build(&graph);
        assert!(idx.subscribers_of(&pub_onafterx_1param_id).is_empty());
        assert!(idx.subscribers_of(&pub_onafterx_2param_id).is_empty());
        assert_eq!(idx.ambiguous_subscriptions().len(), 1);
        assert!(matches!(
            idx.ambiguous_subscriptions()[0].outcome,
            SubscriptionOutcome::Ambiguous {
                candidate_count: 2,
                ..
            }
        ));
    }

    // (e0) IncludeSender: publisher explicit arity 0, subscriber arity 1 (captures
    // the implicit `Sender`) — the single candidate must bind. Pre-fix, the
    // `params_count >= sub_params` filter dropped it (`0 >= 1` false), orphaning a
    // large class of real subscribers (e.g. `OnRegisterManualSetup`).
    #[test]
    fn subscribers_of_include_sender_publisher_binds_arity_one_subscriber() {
        let app = AppRef(0);
        let pub_id = ObjectNodeId {
            app,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(1),
        };
        let sub_id = ObjectNodeId {
            app,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(2),
        };
        let pub_arity0 = RoutineNodeId {
            object: pub_id.clone(),
            name_lc: "onafterx".to_string(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        let (graph, _, _) = build_event_fixture(
            vec![make_publisher(
                pub_id.clone(),
                "OnAfterX",
                0,
                PublisherKind::Integration,
                Some(true),
            )],
            vec![make_subscriber(
                sub_id,
                "Handler",
                1,
                vec![sub_args("pub", "onafterx")],
                false,
            )],
        );
        let idx = SubscriberIndex::build(&graph);
        assert_eq!(
            idx.subscribers_of(&pub_arity0).len(),
            1,
            "arity-1 (Sender) subscriber must bind to arity-0 IncludeSender publisher \
             when the publisher declares IncludeSender=true"
        );
        assert!(idx.ambiguous_subscriptions().is_empty());
    }

    // (e0-neg) NEGATIVE: IncludeSender=false, 0-arity publisher + 1-arity
    // subscriber — the conditional bound must NOT tolerate the extra param; the
    // subscriber orphans (no candidate), proving the fix is CONDITIONAL, not a
    // blanket re-introduction of the pre-fix strict bound's opposite extreme.
    #[test]
    fn subscribers_of_include_sender_false_publisher_orphans_arity_one_subscriber() {
        let app = AppRef(0);
        let pub_id = ObjectNodeId {
            app,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(1),
        };
        let sub_id = ObjectNodeId {
            app,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(2),
        };
        let pub_arity0 = RoutineNodeId {
            object: pub_id.clone(),
            name_lc: "onafterx".to_string(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        let (graph, _, _) = build_event_fixture(
            vec![make_publisher(
                pub_id.clone(),
                "OnAfterX",
                0,
                PublisherKind::Integration,
                Some(false),
            )],
            vec![make_subscriber(
                sub_id,
                "Handler",
                1,
                vec![sub_args("pub", "onafterx")],
                false,
            )],
        );
        let idx = SubscriberIndex::build(&graph);
        assert!(
            idx.subscribers_of(&pub_arity0).is_empty(),
            "arity-1 subscriber must NOT bind to an arity-0 publisher whose \
             IncludeSender is explicitly false — the +1 tolerance is conditional"
        );
        assert!(
            idx.ambiguous_subscriptions().is_empty(),
            "an orphaned subscriber (no candidate at all) is not ambiguity"
        );
        // H-1 fix: a 0-candidate subscription must surface as an orphan
        // diagnostic instead of vanishing via a bare `continue` with no record.
        assert_eq!(
            idx.orphaned_subscriptions().len(),
            1,
            "a 0-candidate subscription must be recorded as an orphan, not silently dropped"
        );
        assert_eq!(
            idx.orphaned_subscriptions()[0].subscriber.name_lc,
            "handler"
        );
        assert_eq!(
            idx.orphaned_subscriptions()[0].publisher_object(),
            Some(&pub_id)
        );
        assert_eq!(idx.orphaned_subscriptions()[0].event_name_lc, "onafterx");
    }

    // Engine-switch S4 (G7): a subscription whose publisher object cannot be
    // found, or whose object type is not one events are published on, is kept
    // in the inventory with its attribute order. It used to be dropped.
    #[test]
    fn subscriptions_keep_unresolved_publisher_objects_in_attribute_order() {
        let app = AppRef(0);
        let sub_id = ObjectNodeId {
            app,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(2),
        };
        let mut bad_type = sub_args("pub", "onafterx");
        // Not a kind `kind_from_object_type_str` maps (`xmlport` would be, and
        // would then fail at the object lookup instead).
        bad_type.publisher_object_type = "controladdin".to_string();
        let (graph, _, _) = build_event_fixture(
            vec![make_publisher(
                ObjectNodeId {
                    app,
                    kind: ObjectKind::Codeunit,
                    key: ObjKey::Id(1),
                },
                "OnAfterX",
                0,
                PublisherKind::Integration,
                Some(false),
            )],
            vec![make_subscriber(
                sub_id,
                "Handler",
                0,
                vec![
                    sub_args("pub", "onafterx"),
                    sub_args("no such codeunit", "onafterx"),
                    bad_type,
                ],
                false,
            )],
        );
        let idx = SubscriberIndex::build(&graph);
        let rows: Vec<(usize, &str, bool)> = idx
            .subscriptions()
            .iter()
            .map(|s| {
                (
                    s.ordinal,
                    s.publisher_name.as_str(),
                    matches!(s.outcome, SubscriptionOutcome::ObjectUnresolved),
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                (0, "pub", false),
                (1, "no such codeunit", true),
                (2, "pub", true),
            ]
        );
    }

    // -------------------------------------------------------------------
    // Task 1 round-2 / Task 2 fold-in: unknown-IncludeSender preflight
    // -------------------------------------------------------------------

    /// A publisher with `include_sender: None` (UNKNOWN) whose sole
    /// subscriber sits at EXACTLY `publisher_arity + 1` must be counted —
    /// this is the population the fail-closed policy silently orphans.
    #[test]
    fn count_unknown_include_sender_plus1_subscribers_counts_the_orphaned_shape() {
        let app = AppRef(0);
        let pub_id = ObjectNodeId {
            app,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(1),
        };
        let sub_id = ObjectNodeId {
            app,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(2),
        };
        let (graph, _, _) = build_event_fixture(
            vec![make_publisher(
                pub_id,
                "OnAfterX",
                0,
                PublisherKind::Integration,
                None, // UNKNOWN IncludeSender
            )],
            vec![make_subscriber(
                sub_id,
                "Handler",
                1, // publisher_arity(0) + 1
                vec![sub_args("pub", "onafterx")],
                false,
            )],
        );
        assert_eq!(count_unknown_include_sender_plus1_subscribers(&graph), 1);
    }

    /// Control (a): `include_sender: Some(true)` — known, not unknown — must
    /// NOT be counted even though the arity shape is identical.
    #[test]
    fn count_unknown_include_sender_plus1_subscribers_excludes_known_true() {
        let app = AppRef(0);
        let pub_id = ObjectNodeId {
            app,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(1),
        };
        let sub_id = ObjectNodeId {
            app,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(2),
        };
        let (graph, _, _) = build_event_fixture(
            vec![make_publisher(
                pub_id,
                "OnAfterX",
                0,
                PublisherKind::Integration,
                Some(true),
            )],
            vec![make_subscriber(
                sub_id,
                "Handler",
                1,
                vec![sub_args("pub", "onafterx")],
                false,
            )],
        );
        assert_eq!(count_unknown_include_sender_plus1_subscribers(&graph), 0);
    }

    /// Control (b): `include_sender: None` but the subscriber sits at the
    /// EXACT (non-`+1`) arity — must NOT be counted; this diagnostic is
    /// specifically about the `+1`-shaped population, not every unknown
    /// publisher.
    #[test]
    fn count_unknown_include_sender_plus1_subscribers_excludes_exact_arity() {
        let app = AppRef(0);
        let pub_id = ObjectNodeId {
            app,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(1),
        };
        let sub_id = ObjectNodeId {
            app,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(2),
        };
        let (graph, _, _) = build_event_fixture(
            vec![make_publisher(
                pub_id,
                "OnAfterX",
                1,
                PublisherKind::Integration,
                None,
            )],
            vec![make_subscriber(
                sub_id,
                "Handler",
                1, // exact match, not +1
                vec![sub_args("pub", "onafterx")],
                false,
            )],
        );
        assert_eq!(count_unknown_include_sender_plus1_subscribers(&graph), 0);
    }

    // (e) Unresolvable publisher — no panic ----------------------------------

    #[test]
    fn subscribers_of_unresolvable_publisher_no_panic() {
        let app = AppRef(0);
        let sub_id = ObjectNodeId {
            app,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(2),
        };

        let (graph, _, _) = build_event_fixture(
            vec![], // no publishers at all
            vec![make_subscriber(
                sub_id,
                "Handler",
                0,
                vec![sub_args("nonexistent", "onevent")],
                false,
            )],
        );

        let idx = SubscriberIndex::build(&graph);
        // Publisher not found → silently dropped, no panic.
        assert!(idx.ambiguous_subscriptions().is_empty());
    }

    // (f) Two overloads, exactly one strict arity match → resolved -----------

    #[test]
    fn subscribers_of_unique_strict_arity_match_resolves() {
        let app = AppRef(0);
        let pub_id = ObjectNodeId {
            app,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(1),
        };
        let sub_id = ObjectNodeId {
            app,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(2),
        };
        let pub_onafterx_0param_id = RoutineNodeId {
            object: pub_id.clone(),
            name_lc: "onafterx".to_string(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        let pub_onafterx_1param_id = RoutineNodeId {
            object: pub_id.clone(),
            name_lc: "onafterx".to_string(),
            enclosing_member_lc: None,
            params_count: 1,
            sig_fp: 0,
        };

        // Subscriber params=0: both >=0, but exactly ONE has params==0.
        let (graph, _, _) = build_event_fixture(
            vec![
                make_publisher(
                    pub_id.clone(),
                    "OnAfterX",
                    0,
                    PublisherKind::Integration,
                    Some(false),
                ),
                make_publisher(
                    pub_id.clone(),
                    "OnAfterX",
                    1,
                    PublisherKind::Integration,
                    Some(false),
                ),
            ],
            vec![make_subscriber(
                sub_id,
                "Handler",
                0,
                vec![sub_args("pub", "onafterx")],
                false,
            )],
        );

        let idx = SubscriberIndex::build(&graph);
        assert_eq!(idx.subscribers_of(&pub_onafterx_0param_id).len(), 1);
        assert!(idx.subscribers_of(&pub_onafterx_1param_id).is_empty());
        assert!(idx.ambiguous_subscriptions().is_empty());
    }

    // (g) Pinned subscriber output --------------------------------------------

    /// The subscriber lists, ambiguous list and orphan list for one fixture,
    /// pinned as literal text. The expected text was captured from the build
    /// BEFORE the subscriber maps moved out of `ResolveIndex` (compact-graph
    /// step 2, Task 4), so the move is proven to change nothing: same entries,
    /// same order, same conditions.
    #[test]
    fn subscriber_lists_match_the_pre_move_capture() {
        let app = AppRef(0);
        let pub_id = ObjectNodeId {
            app,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(1),
        };
        let sub_id = ObjectNodeId {
            app,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(2),
        };
        let mut license = sub_args("pub", "onafterx");
        license.skip_on_missing_license = true;
        let mut element = sub_args("pub", "onafterx");
        element.element = Some("Field1".to_string());
        let (graph, _, _) = build_event_fixture(
            vec![
                make_publisher(
                    pub_id.clone(),
                    "OnAfterX",
                    0,
                    PublisherKind::Integration,
                    Some(false),
                ),
                make_publisher(
                    pub_id.clone(),
                    "OnAmb",
                    1,
                    PublisherKind::Integration,
                    Some(false),
                ),
                make_publisher(
                    pub_id.clone(),
                    "OnAmb",
                    2,
                    PublisherKind::Integration,
                    Some(false),
                ),
            ],
            vec![
                // Declared out of name order on purpose; the graph sorts them.
                make_subscriber(sub_id.clone(), "HandlerC", 0, vec![element], false),
                make_subscriber(
                    sub_id.clone(),
                    "HandlerA",
                    0,
                    vec![sub_args("pub", "onafterx")],
                    true,
                ),
                make_subscriber(sub_id.clone(), "HandlerB", 0, vec![license], false),
                make_subscriber(
                    sub_id.clone(),
                    "HandlerD",
                    0,
                    vec![sub_args("pub", "onamb")],
                    false,
                ),
                make_subscriber(
                    sub_id.clone(),
                    "HandlerE",
                    0,
                    vec![sub_args("pub", "onmissing")],
                    false,
                ),
            ],
        );
        let publisher = |name: &str, params: usize| RoutineNodeId {
            object: pub_id.clone(),
            name_lc: name.to_string(),
            enclosing_member_lc: None,
            params_count: params,
            sig_fp: 0,
        };

        let idx = SubscriberIndex::build(&graph);
        let mut out = String::new();
        for (name, params) in [("onafterx", 0), ("onamb", 1), ("onamb", 2)] {
            for s in idx.subscribers_of(&publisher(name, params)) {
                out.push_str(&format!(
                    "sub {name}/{params} <- {} {:?} {:?}\n",
                    s.subscriber.name_lc, s.conditions, s.element
                ));
            }
        }
        for a in idx.ambiguous_subscriptions() {
            let SubscriptionOutcome::Ambiguous {
                candidate_count, ..
            } = a.outcome
            else {
                unreachable!()
            };
            out.push_str(&format!(
                "ambiguous {} -> {}/{} x{}\n",
                a.subscriber.name_lc,
                a.publisher_object() == Some(&pub_id),
                a.event_name_lc,
                candidate_count
            ));
        }
        for o in idx.orphaned_subscriptions() {
            out.push_str(&format!(
                "orphan {} -> {}/{}\n",
                o.subscriber.name_lc,
                o.publisher_object() == Some(&pub_id),
                o.event_name_lc
            ));
        }
        assert_eq!(
            out,
            "sub onafterx/0 <- handlera [ManualBinding] None\n\
             sub onafterx/0 <- handlerb [SkipOnMissingLicense] None\n\
             sub onafterx/0 <- handlerc [] Some(\"Field1\")\n\
             ambiguous handlerd -> true/onamb x2\n\
             orphan handlere -> true/onmissing\n"
        );
    }

    // -- WorldMode is a value type test ---------------------------------------

    #[test]
    fn world_mode_variants_constructible() {
        let cc = WorldMode::CallerClosure(AppRef(0));
        let snap = WorldMode::AnalyzedSnapshot;
        assert_ne!(cc, snap);
        // CallerClosure equality is by contained AppRef.
        assert_eq!(cc, WorldMode::CallerClosure(AppRef(0)));
        assert_ne!(cc, WorldMode::CallerClosure(AppRef(1)));
    }
}
