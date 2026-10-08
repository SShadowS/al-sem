//! Builds a `ProgramGraph` from an `AppSetSnapshot`.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use crate::program::abi_ingest::AbiCache;
use crate::program::dep_cache::{DepCache, DepKey, DepNodes};
use crate::program::dep_summary::{BuildParse, DepUnitSummary, summarize_file};
use crate::program::graph::{AbiIngestError, ObjectIndex, ProgramGraph};
use crate::program::node::{AppRef, AppRegistry, RoutineNodeId};
use crate::program::node_extract::{AbiParams, Access, ObjectNode, RoutineNode, extract_nodes};
use crate::program::node_set::NodeSet;
use crate::program::profile::BuildProfile;
use crate::program::resolve::decl_surface::DepMetaMap;
use crate::program::resolve::event::{
    PublisherKind, is_platform_page_event, is_platform_table_event, platform_event_display_name,
};
use crate::program::str_pool::{ShareStrings, StrPool};
use crate::program::topology::DependencyGraph;
use crate::snapshot::{AppSetSnapshot, ParsedUnit, TrustTier, parse_snapshot};

/// Immutable-between-dep-changes layer: everything derived from every
/// NON-primary (dependency) app in the snapshot — object/routine nodes,
/// dependency topology, and friend-app wiring. A future incremental LSP
/// updater's "rung 2" (rebuild only the workspace layer, over an UNCHANGED
/// dep layer — see [`assemble_program_graph`]) reuses one `DepLayer` across
/// many workspace edits without re-parsing or re-extracting a single
/// dependency file (T3 Task 3 measured dep-parse alone at ~1.19s on CDO —
/// the cost this primitive exists to stop paying on every keystroke).
///
/// Built by [`build_dep_layer`]; merged with a freshly-extracted primary
/// [`ParsedUnit`] by [`assemble_program_graph`].
pub struct DepLayer {
    /// ALL apps interned (primary included) — `AppRegistry::intern` order
    /// mirrors `snap.apps` order exactly (Step 1 below), so the `AppRef`
    /// `assemble_program_graph` later resolves the primary app to is
    /// IDENTICAL to what a monolithic build would have assigned it.
    pub apps: AppRegistry,
    /// Real dependency-topology wiring for EVERY app (including the
    /// primary's own outbound edges) — pure manifest data
    /// (`AppUnit::declared_deps`), unaffected by which workspace SOURCE
    /// files changed.
    pub topology: DependencyGraph,
    /// `internalsVisibleTo` friend-app wiring for EVERY app — same
    /// manifest-data stability as `topology`. See
    /// [`ProgramGraph::friends`]'s doc for the field's semantics.
    pub friends: HashMap<AppRef, BTreeSet<AppRef>>,
    /// Object nodes from every NON-primary app (parsed source + ABI-ingested
    /// SymbolOnly deps), already sorted + deduped exactly as the original
    /// monolithic Step 4 — scoped to just this population. Every assembled
    /// `ProgramGraph` shares it as its `NodeSet`'s shared part
    /// (`assemble_program_graph`); it is never cloned.
    pub dep_objects: Arc<Vec<ObjectNode>>,
    /// Routine nodes, same population/ordering/dedup contract as
    /// `dep_objects`.
    pub dep_routines: Arc<Vec<RoutineNode>>,
    /// Per-app dependency-ABI ingest diagnostics — see
    /// [`ProgramGraph::abi_ingest_errors`]'s doc. Always non-primary-scoped:
    /// Step 2b below only ever ingests SymbolOnly (source-less) apps, and
    /// the primary/workspace app is always source-bearing.
    pub abi_ingest_errors: Vec<AbiIngestError>,
    /// The [`DepCache`] entry the three fields above came from. Held so the
    /// entry stays live (the cache keeps only a `Weak`) exactly as long as a
    /// snapshot still uses this layer.
    pub dep_nodes: Arc<DepNodes>,
}

/// Build the [`DepLayer`] from every app in `snap` OTHER than
/// `snap.workspace_app` — the primary/workspace app's own nodes are instead
/// extracted fresh, per call, by [`assemble_program_graph`] (that's the
/// whole point: a workspace-file edit never needs to redo this function's
/// work).
///
/// `parsed` must be the FULL parsed snapshot (`parse_snapshot(snap)`,
/// covering every source-bearing app, workspace included) — this function
/// itself filters out the workspace unit's contribution. `build_program_graph`
/// below wires this for a single one-shot build; a future incremental caller
/// instead reuses one `(parsed dep units, DepLayer)` pair across many
/// `assemble_program_graph` calls, re-parsing only the workspace app.
pub fn build_dep_layer(
    snap: &AppSetSnapshot,
    abi_cache: &AbiCache,
    parsed: &[ParsedUnit],
) -> DepLayer {
    // `LIGHT`: a `Parsed` input keeps no bodies, so its key must not claim
    // them (see `build_dep_layer_cached`'s assert).
    build_dep_layer_cached(
        snap,
        abi_cache,
        DepInput::Parsed(parsed),
        BuildProfile::LIGHT,
        &DepCache::default(),
    )
}

/// Where the dependency layer's per-file summaries come from.
pub(crate) enum DepInput<'a> {
    /// Already-parsed units (every source-bearing unit; the workspace's is
    /// skipped): each dependency file is summarized from its tree. The layer
    /// keeps no bodies. For [`build_dep_layer`] and its callers.
    Parsed(&'a [ParsedUnit]),
    /// A profiled build parse ([`crate::program::dep_summary::parse_for_build`]): its summaries are
    /// consumed, and its dependency bodies (if any) kept in the tier.
    Built(BuildParse),
}

/// [`build_dep_layer`], taking the dependency nodes from `dep_cache` when
/// another root already built the same dependency set (see [`DepKey`]).
/// `input` is read only on a miss, so a caller that already holds the live
/// entry may pass a parse without dependencies.
///
/// A `DepInput::Parsed` input builds a tier without bodies, so it must never
/// be keyed with a `Keep` profile: on a shared cache that tier would be
/// published as a `Keep` entry, and the next `FULL` hit would find no bodies
/// and panic in `ProgramContext::all_units`.
pub(crate) fn build_dep_layer_cached(
    snap: &AppSetSnapshot,
    abi_cache: &AbiCache,
    input: DepInput<'_>,
    profile: BuildProfile,
    dep_cache: &DepCache,
) -> DepLayer {
    debug_assert!(
        !(matches!(input, DepInput::Parsed(_))
            && profile.dependency_bodies == crate::program::profile::DependencyBodies::Keep),
        "a Parsed input keeps no dependency bodies; keying it Keep would publish a body-less Keep tier"
    );
    // ── Step 1: intern all app identities (primary included, for AppRef stability) ──
    let mut apps = AppRegistry::default();
    let app_refs: Vec<AppRef> = snap.apps.iter().map(|u| apps.intern(&u.id)).collect();

    let dep_nodes = dep_cache.get_or_build(DepKey::of(snap, profile), || {
        build_dep_nodes(snap, abi_cache, input, &mut apps)
    });

    // ── Step 3 / 3b: wire topology + friends for the WHOLE app set ───────────
    // Both are pure manifest data (declared_deps / internalsVisibleTo), so
    // they belong on the immutable dep layer even though the wiring loops
    // below also touch the primary app's OWN outbound edges/grants.
    let topology = wire_dependency_topology(snap, &app_refs);
    let friends = wire_friend_authorizations(snap, &app_refs);

    DepLayer {
        apps,
        topology,
        friends,
        dep_objects: Arc::clone(&dep_nodes.objects),
        dep_routines: Arc::clone(&dep_nodes.routines),
        abi_ingest_errors: dep_nodes.abi_ingest_errors.clone(),
        dep_nodes,
    }
}

/// Steps 2, 2b and 4 of [`build_dep_layer_cached`]: the root-independent
/// dependency nodes. `apps` must already hold every app of `snap` (Step 1).
fn build_dep_nodes(
    snap: &AppSetSnapshot,
    abi_cache: &AbiCache,
    input: DepInput<'_>,
    apps: &mut AppRegistry,
) -> DepNodes {
    // ── Step 2: the NON-primary per-file summaries ───────────────────────────
    let (summaries, bodies) = match input {
        DepInput::Parsed(parsed) => {
            let summaries = parsed
                .iter()
                // primary — extracted fresh per call by `assemble_program_graph`.
                .filter(|unit| unit.app != snap.workspace_app)
                .map(|unit| {
                    // `intern` is idempotent — the `AppRef` assigned in step 1.
                    let app_ref = apps.intern(&unit.app);
                    DepUnitSummary {
                        app: unit.app.clone(),
                        files: unit
                            .files
                            .iter()
                            .map(|pf| {
                                summarize_file(
                                    app_ref,
                                    pf.provenance.tier,
                                    &pf.virtual_path,
                                    &pf.file,
                                )
                            })
                            .collect(),
                    }
                })
                .collect();
            (summaries, None)
        }
        DepInput::Built(parse) => (parse.dep_summaries, parse.dep_bodies.map(Arc::new)),
    };

    // Each summary is consumed: its nodes move into the layer, its
    // `RoutineMeta` into `dep_meta` (in parse order, so a true same-key
    // collision keeps the last one, as `DeclSurface::build` does), and its
    // recovered flag into `recovered`. Nothing summary-shaped survives.
    //
    // Equal strings become one allocation as they arrive (engine-switch
    // S10.2), so every later clone of a tier value shares it; `dep_meta`'s
    // keys are shared before insertion, since a map key cannot be changed.
    let mut pool = StrPool::default();
    let mut objects: Vec<ObjectNode> = Vec::new();
    let mut routines: Vec<RoutineNode> = Vec::new();
    let mut dep_meta = DepMetaMap::new();
    let mut recovered: Vec<String> = Vec::new();
    for unit in summaries {
        for mut file in unit.files {
            file.objects.share_strings(&mut pool);
            file.routines.share_strings(&mut pool);
            objects.extend(file.objects);
            routines.extend(file.routines);
            dep_meta.extend(file.routine_meta.into_iter().map(|(mut id, mut meta)| {
                id.share_strings(&mut pool);
                meta.share_strings(&mut pool);
                (id, meta)
            }));
            if file.parse_status_recovered {
                recovered.push(crate::snapshot::parse::recovered_path(
                    &unit.app.name,
                    &file.virtual_path,
                ));
            }
        }
    }
    recovered.sort();

    // ── Step 2b: ingest SymbolOnly dep ABI nodes ─────────────────────────────
    let mut abi_ingest_errors: Vec<AbiIngestError> = Vec::new();
    for unit in &snap.apps {
        if unit.source.is_some() {
            continue;
        }
        let app_ref = apps.intern(&unit.id);
        let result = crate::program::abi_ingest::ingest_abi(unit, app_ref, abi_cache);
        if let Some(message) = result.error {
            // H-3: a read/parse failure on this dep's SymbolReference.json —
            // previously silently swallowed into an indistinguishable-from-
            // genuinely-empty ABI. Ingestion still proceeds (fields default
            // empty), but the failure is now observable.
            abi_ingest_errors.push(AbiIngestError {
                app: app_ref,
                message,
            });
        }
        let (mut abi_objects, mut abi_routines) = (result.objects, result.routines);
        abi_objects.share_strings(&mut pool);
        abi_routines.share_strings(&mut pool);
        objects.extend(abi_objects);
        routines.extend(abi_routines);
    }

    // ── Step 4: sort for determinism, then dedup this (non-primary) population ──
    // Same non-primary app can appear as both a workspace-multi-app source
    // AND an embedded dep (e.g. sibling apps in a multi-app workspace whose
    // compiled .app also lands in .alpackages). extract_nodes would process
    // the source twice, producing duplicate ObjectNode/RoutineNode entries
    // with identical ids. Dedup after sort keeps the first occurrence
    // (arbitrary but stable) — see `dedup_routines_preserving_genuine_overloads`'s
    // doc for why routines need content-aware, not blanket, dedup.
    objects.sort_by(|a, b| a.id.cmp(&b.id));
    objects.dedup_by(|a, b| a.id == b.id);
    routines.sort_by(|a, b| a.id.cmp(&b.id));
    dedup_routines_preserving_genuine_overloads(&mut routines);

    DepNodes {
        objects: Arc::new(objects),
        routines: Arc::new(routines),
        abi_ingest_errors,
        dep_meta: Arc::new(dep_meta),
        recovered,
        bodies,
        lsp: Default::default(),
    }
}

/// Merge a [`DepLayer`] with a freshly-extracted PRIMARY (workspace)
/// [`ParsedUnit`] into a full [`ProgramGraph`] — the assembly half of the
/// layered split.
///
/// Sorts + dedups only the WORKSPACE nodes (catches any workspace-internal
/// duplicate, e.g. a `#if`/`#else` union-read producing two
/// textually-identical `RoutineDecl`s for the same procedure — see
/// `dedup_routines_preserving_genuine_overloads`'s doc), then layers them
/// over the dep layer's already-sorted-and-deduped part as a `NodeSet`
/// (shared, not copied). No cross-part dedup is needed:
/// `ObjectNodeId`/`RoutineNodeId` are namespaced by `AppRef`, and the
/// primary app's `AppRef` is disjoint from every dependency's, so a
/// dep-layer entry can never collide with a workspace one.
pub fn assemble_program_graph(
    dep: &DepLayer,
    ws_unit: &ParsedUnit,
    snap: &AppSetSnapshot,
) -> ProgramGraph {
    let ws_app_ref = dep
        .apps
        .find(&snap.workspace_app)
        .expect("workspace app must already be interned by build_dep_layer's Step 1");

    // Workspace nodes only; the dependency part is shared, not copied.
    let mut objects: Vec<ObjectNode> = Vec::new();
    let mut routines: Vec<RoutineNode> = Vec::new();
    let mut workspace_rows = crate::program::physical::PhysicalIndex::default();

    for pf in &ws_unit.files {
        let (o0, r0) = (objects.len(), routines.len());
        extract_nodes(
            ws_app_ref,
            &pf.file,
            pf.provenance.tier,
            &mut objects,
            &mut routines,
        );
        // Before the sort + dedup below: one row per occurrence (S2b.3).
        workspace_rows.record_file(&pf.virtual_path, &pf.file, &objects[o0..], &routines[r0..]);
    }

    objects.sort_by(|a, b| a.id.cmp(&b.id));
    objects.dedup_by(|a, b| a.id == b.id);
    routines.sort_by(|a, b| a.id.cmp(&b.id));
    dedup_routines_preserving_genuine_overloads(&mut routines);

    let objects = NodeSet::layered(Arc::clone(&dep.dep_objects), objects);
    let routines = NodeSet::layered(Arc::clone(&dep.dep_routines), routines);
    let obj_index = ObjectIndex::build(&objects);

    let mut graph = ProgramGraph {
        apps: dep.apps.clone(),
        topology: dep.topology.clone(),
        objects,
        routines,
        obj_index,
        friends: dep.friends.clone(),
        abi_ingest_errors: dep.abi_ingest_errors.clone(),
        workspace_rows,
    };

    // ── Inject synthetic platform-event publishers ───────────────────────────
    // Binds subscribers to the table's implicit DB-trigger / validate events,
    // which have no publisher routine in source (see below). Must run AFTER
    // the full dep+workspace merge: a subscriber in one app can target a
    // publisher object living in another.
    inject_platform_event_publishers(&mut graph);

    graph
}

/// As [`build_program_graph`], but takes an ALREADY-parsed snapshot — kills
/// the production double-parse T3 Task 3 measured (`resolve::full::
/// build_context` used to call `build_program_graph` [parses internally]
/// AND run its OWN standalone `parse_snapshot` for the resolver's body-walk).
/// Locates the primary/workspace [`ParsedUnit`] in `parsed` (or synthesizes
/// an empty one if the workspace app genuinely has no source — e.g. a
/// symbol-only "workspace", which never occurs in practice but is handled
/// rather than panicking), builds the dep layer, and assembles.
pub fn build_program_graph_from_parsed(
    snap: &AppSetSnapshot,
    abi_cache: &AbiCache,
    parsed: &[ParsedUnit],
) -> ProgramGraph {
    let dep = build_dep_layer(snap, abi_cache, parsed);

    // `snap.apps` is GUID-deduped upstream (H-2), so at most one parsed unit
    // can match the workspace identity.
    let empty_ws_unit;
    let ws_unit: &ParsedUnit = match parsed.iter().find(|u| u.app == snap.workspace_app) {
        Some(u) => u,
        None => {
            empty_ws_unit = ParsedUnit {
                app: snap.workspace_app.clone(),
                files: vec![],
            };
            &empty_ws_unit
        }
    };

    assemble_program_graph(&dep, ws_unit, snap)
}

/// Assemble a `ProgramGraph` from a fully-resolved `AppSetSnapshot` — thin
/// wrapper: parse once, then delegate to the layered split
/// (`build_dep_layer` plus `assemble_program_graph`) via
/// [`build_program_graph_from_parsed`]. Kept as the PUBLIC,
/// source-compatible entry point every existing caller (aldump,
/// `engine/l4`/`l5`/`gate`, tests) already uses unchanged; `resolve::full::
/// build_context` instead inlines `build_dep_layer` + `assemble_program_graph`
/// itself (T3 Task 8) so it can keep the `DepLayer` for reuse (previously,
/// through T3 Task 5, it called `build_program_graph_from_parsed` directly so
/// the whole `ProgramContext` build parsed the snapshot only ONCE — that part
/// is unchanged, only the dep-layer's lifetime is).
pub fn build_program_graph(snap: &AppSetSnapshot, abi_cache: &AbiCache) -> ProgramGraph {
    let parsed = parse_snapshot(snap);
    build_program_graph_from_parsed(snap, abi_cache, &parsed)
}

/// Wire the real dependency topology from each unit's `declared_deps`
/// (GUID-match preferred; name+version fallback; deps absent from the
/// snapshot are silently skipped — open-world assumption). Shared by
/// [`build_dep_layer`] — pure manifest data, unaffected by which workspace
/// source files changed.
fn wire_dependency_topology(snap: &AppSetSnapshot, app_refs: &[AppRef]) -> DependencyGraph {
    let mut topology = DependencyGraph::default();

    for (i, unit) in snap.apps.iter().enumerate() {
        let from_ref = app_refs[i];
        for dep in &unit.declared_deps {
            // Match the dep to a snapshot app: GUID is globally unique and tried
            // first; fall through to name (case-insensitive) + version when no
            // GUID match (e.g. a snapshot unit whose manifest GUID was unavailable).
            //
            // H-2 note (Tier-1 remediation, Task T1.2): `snap.apps` is now
            // GUID-deduped upstream — `dependencies::load_all_apps` collapses
            // every physically-discovered `.app` to at most one survivor per
            // non-empty GUID (the highest available version) BEFORE the
            // snapshot is built (see `SnapshotBuilder::build_with_diagnostics`
            // and `dependencies::dedup_by_guid_keep_highest_version`'s doc).
            // So `.find(...)` below can match at most one candidate per GUID —
            // the prior "first match in a version-lexicographic-sorted list"
            // stale-wins hazard is closed at the source. `dep.version` (the
            // MinVersion this specific edge requires) is still NOT consulted
            // here: if the one surviving version undercuts it, this still
            // binds anyway rather than declining. Left as-is deliberately —
            // enforcing it would mean FALLING BACK to "not in closure" for a
            // real dependency purely because the newest AVAILABLE `.app`
            // happens to be older than a MinVersion string, which is a
            // distinct, unconfirmed-on-real-data soundness/coverage
            // trade-off, not part of this fix's scope.
            let by_guid = (!dep.app_id.is_empty())
                .then(|| {
                    snap.apps
                        .iter()
                        .zip(app_refs.iter())
                        .find(|(u, _)| !u.id.guid.is_empty() && u.id.guid == dep.app_id)
                        .map(|(_, r)| *r)
                })
                .flatten();
            let dep_ref = by_guid.or_else(|| {
                snap.apps
                    .iter()
                    .zip(app_refs.iter())
                    .find(|(u, _)| {
                        u.id.name.eq_ignore_ascii_case(&dep.name) && u.id.version == dep.version
                    })
                    .map(|(_, r)| *r)
            });

            if let Some(dep_ref) = dep_ref {
                topology.add_dependency(from_ref, dep_ref);
            }
            // Deps not present in the snapshot are silently skipped (open-world).
        }
    }

    topology
}

/// Wire `internalsVisibleTo` friend-app authorizations. AL: a member declared
/// `internal` is visible within its declaring app AND to any app the
/// declaring app's manifest lists in `<InternalsVisibleTo>` (a "friend" app)
/// — one-directional per the app EXPOSING the internals, never the reverse.
/// The returned map is keyed by the exposing app's `AppRef` so
/// `resolver.rs`'s `Access::Internal` visibility rule can do an O(1)
/// `friends.get(&declaring_app).is_some_and(|f| f.contains(&caller_app))`
/// check alongside the existing same-app check (Task 1.5). Shared by
/// [`build_dep_layer`] — pure manifest data, unaffected by which workspace
/// source files changed.
fn wire_friend_authorizations(
    snap: &AppSetSnapshot,
    app_refs: &[AppRef],
) -> HashMap<AppRef, BTreeSet<AppRef>> {
    let mut friends: HashMap<AppRef, BTreeSet<AppRef>> = HashMap::new();
    for (i, unit) in snap.apps.iter().enumerate() {
        let exposing_ref = app_refs[i];
        for friend in &unit.internals_visible_to {
            // Same GUID-first, name+publisher-fallback resolution as the
            // dependency wiring above — a `<Module>` friend entry carries no
            // version, so the fallback compares publisher instead.
            let by_guid = (!friend.app_id.is_empty())
                .then(|| {
                    snap.apps
                        .iter()
                        .zip(app_refs.iter())
                        .find(|(u, _)| {
                            !u.id.guid.is_empty() && u.id.guid.eq_ignore_ascii_case(&friend.app_id)
                        })
                        .map(|(_, r)| *r)
                })
                .flatten();
            let friend_ref = by_guid.or_else(|| {
                snap.apps
                    .iter()
                    .zip(app_refs.iter())
                    .find(|(u, _)| {
                        u.id.name.eq_ignore_ascii_case(&friend.name)
                            && u.id.publisher.eq_ignore_ascii_case(&friend.publisher)
                    })
                    .map(|(_, r)| *r)
            });

            if let Some(friend_ref) = friend_ref {
                friends.entry(exposing_ref).or_default().insert(friend_ref);
            }
            // Friends not present in the snapshot are silently skipped (open-world).
        }
    }
    friends
}

/// Synthetic platform-publisher arity: a generous upper bound so the resolve
/// index's `params_count >= sub_params` candidate filter admits every real
/// subscriber. Platform events top out at ~3 params and are never overloaded,
/// so the exact value only needs to dominate any subscriber's arity.
const PLATFORM_EVENT_PUBLISHER_ARITY: usize = 8;

/// Inject synthetic [`PublisherKind::Platform`] publisher routines for the
/// platform-generated TABLE events (`OnAfter*Event` / `OnBefore*Event` + field
/// validate) that a subscriber targets but which have NO publisher routine in
/// source.
///
/// Without these, a `[EventSubscriber(ObjectType::Table, Database::X,
/// 'OnAfterDeleteEvent', …)]` resolves to no publisher (the resolve index needs
/// a `publisher_kind`-bearing routine), so its integration edge — "this
/// subscriber fires when X is deleted", the charter's data-is-control-flow
/// wiring — is silently lost. On real BC apps this orphans ~27% of subscribers.
///
/// One synthetic per distinct `(table, event_name)`; field-level (`element`)
/// granularity is intentionally collapsed so the index's `(object, name)`
/// candidate model resolves each to exactly one publisher (per-field precision
/// is a later refinement). Events already served by a real publisher routine on
/// the table are skipped — a synthetic never shadows source.
pub(crate) fn inject_platform_event_publishers(graph: &mut ProgramGraph) {
    let mut synth: Vec<RoutineNode> = Vec::new();
    let mut seen: std::collections::HashSet<RoutineNodeId> = std::collections::HashSet::new();

    for sub in &graph.routines {
        for args in &sub.event_subscribers {
            // Recognize the platform TABLE and PAGE events (implicit DB triggers /
            // field validate / page lifecycle / record / action) that have no
            // publisher routine in source. Everything else resolves through the
            // normal `[IntegrationEvent]` publisher path.
            let is_platform = match args.publisher_object_type.as_str() {
                "table" => is_platform_table_event(&args.event_name),
                "page" => is_platform_page_event(&args.event_name),
                _ => false,
            };
            if !is_platform {
                continue;
            }
            // Resolve the publisher object from the subscriber's app (fail-closed),
            // by name or number, as the subscriber wiring does.
            let Some(pub_obj) = crate::program::resolve::index::resolve_subscription_publisher(
                graph,
                sub.id.object.app,
                args,
            ) else {
                continue;
            };
            let synth_id = RoutineNodeId {
                object: pub_obj.id.clone(),
                name_lc: args.event_name.as_str().into(),
                enclosing_member_lc: None,
                params_count: PLATFORM_EVENT_PUBLISHER_ARITY,
                sig_fp: 0,
            };
            if !seen.insert(synth_id.clone()) {
                continue; // already injected for this (table, event)
            }
            // Never shadow a real source publisher of the same name on the table.
            let has_real_publisher = graph.routines.iter().any(|r| {
                r.id.object == pub_obj.id
                    && r.id.name_lc == args.event_name
                    && r.publisher_kind.is_some()
            });
            if has_real_publisher {
                continue;
            }
            synth.push(RoutineNode {
                id: synth_id,
                name: platform_event_display_name(&args.event_name).into(),
                is_trigger: false,
                access: Access::Public,
                tier: pub_obj.tier,
                event_subscribers: vec![],
                subscriber_instance_manual: false,
                publisher_kind: Some(PublisherKind::Platform),
                // No real `[IntegrationEvent]` attribute to read (this
                // publisher is synthesized, not parsed) — and platform
                // DB-trigger/lifecycle events never legally prepend a Sender
                // anyway, so `None` correctly yields no `+1` tolerance via
                // `event::subscriber_arity_bound`.
                include_sender: None,
                abi_routine_kind: None,
                abi_event_kind: None,
                param_sig_key: Default::default(),
                return_type: None,
                return_type_id: None,
                abi_overload_collapsed: false,
                source_overload_aliased: false,
                preproc_context: Box::default(),
                // Synthesized, not ingested from any `SymbolReference.json`
                // — no ABI parameter metadata exists to retain.
                abi_params: AbiParams::Missing,
            });
        }
    }

    if synth.is_empty() {
        return;
    }
    graph.routines.extend(synth);
    graph.routines.sort_by(|a, b| a.id.cmp(&b.id));
}

/// Collapse a SORTED `routines` vec's runs of equal `RoutineNodeId` down to
/// one canonical entry PER DISTINCT parameter-type signature.
///
/// Two SOURCE routines collide onto the same `RoutineNodeId` whenever they
/// share `(object, name_lc, enclosing_member_lc, params_count)` AND `sig_fp`
/// (see node.rs). Post-Task-2 (sigfp-and-ambiguous-reclassification plan)
/// `sig_fp` is a REAL fingerprint of the parameter-type tuple, so a genuine
/// same-name/same-arity SOURCE overload PAIR (two textually distinct
/// declarations differing only by parameter type) now almost always gets
/// DISTINCT ids and never reaches this run-grouping at all. The id alone
/// still cannot tell a re-parsed DUPLICATE of one routine (e.g. its owning
/// object embedded both as workspace source and as an embedded dep; see the
/// Step 4 comment above) apart from the RARE case where two genuinely
/// different overloads' normalized parameter tuples happen to hash to the
/// same `sig_fp` (a residual fnv1a collision) — this function's
/// `param_sig_key` text-based grouping (below) resolves both cases without
/// relying on `sig_fp` alone.
///
/// Within a run, entries are grouped by [`RoutineNode::param_sig_key`] — the
/// lowercased, `|`-joined parameter-type-text sequence (mirrors
/// `abi_ingest::param_type_fp`'s normalization, computed for source params).
/// Each distinct key collapses to its first occurrence (arbitrary but
/// stable); a run with N distinct keys yields exactly N canonical entries.
///
/// This is correct independent of how many times the enclosing object itself
/// was duplicated: two re-parses of the SAME declaration always share a
/// param signature and collapse together, while two genuinely distinct
/// overloads always differ in param signature and are both preserved — even
/// in the COMPOUND case where an object is both duplicated AND declares a
/// genuine overload pair (beyond-1B.3b Task 2 review fix: the previous
/// dup-factor heuristic under-collapsed that case, e.g. 2 overloads × 2
/// object copies = 4 raw entries kept instead of the canonical 2). Pre-Task-2
/// (source `sig_fp` was always `0`) the two canonical entries for a genuine
/// overload pair always shared one `RoutineNodeId`; post-Task-2 they only
/// still share one when a residual `sig_fp` collision applies (the common
/// case now gets distinct ids straight from sorting — see the doc comment
/// above). Either way `resolve_in_object`'s `>1` arm still returns an honest
/// `Unresolved` rather than guessing when a shared id survives with >1
/// canonical entry; only the CANONICAL COUNT is fixed here, not distinct node
/// identity (deferred overload-dispatch work). Never drops a genuine
/// collision silently for a SOURCE routine.
///
/// **ABI routines are a narrower case (Task 3 review fix; fp fidelity fixed
/// Task 2).** An ABI routine's `param_sig_key` is hardcoded `String::new()`
/// (see that field's doc on [`RoutineNode`]) — every ABI entry in a run
/// shares the SAME empty key, so a run of ≥2 raw ABI entries always
/// collapses to exactly ONE survivor here. Task 2 made
/// `abi_ingest::param_type_fp` (hence `RoutineNodeId::sig_fp`) fold a
/// length-delimited canonical tuple of EVERY parameter's outer kind +
/// Subtype id + raw Subtype name + a degradation tag (previously: only the
/// OUTER keyword, never a `Subtype` — the gap that let two genuinely
/// DIFFERENT ABI overloads differing only by an object-typed parameter's
/// Subtype silently share one `RoutineNodeId`). Post-fix, two entries reach
/// this function's SAME run only when their ENTIRE canonical tuple matched
/// — i.e. they are either a true re-parse duplicate, or a residual
/// fingerprint collision this engine genuinely cannot distinguish further
/// (round-2 addendum: "any residual same-key multi-entry group is
/// collapse-marked so collisions OVER-DECLINE, never select"). Either way,
/// collapsing to ONE survivor and marking it is the correct fail-closed
/// outcome: this function marks that survivor
/// [`RoutineNode::abi_overload_collapsed`] whenever ≥2 raw
/// `TrustTier::SymbolOnly` entries shared a node id, so a downstream
/// type-query (`resolver::routine_node_for_type_query` /
/// `receiver::receiver_from_routine_node`, Task 3's cross-object call-result
/// chain typing) AND plain dispatch (`resolver::resolve_in_object`'s
/// collapse-marker guard, Task 2 round-2) both decline rather than trust a
/// possibly-wrong candidate. `abi_overload_collapsed` is never set for a
/// SOURCE routine: its `param_sig_key` is real parsed param-type content, so
/// a genuine same-id/same-key collapse there is always a true re-parse
/// duplicate of the identical declaration.
///
/// **Source-overload alias marking (sigfp-and-ambiguous-reclassification
/// plan, Task 1; reframed by Task 2).** A run with ≥2 DISTINCT
/// `param_sig_key`s surviving under one `RoutineNodeId` is the source-tier
/// mirror of the ABI case above. Pre-Task-2 (source `sig_fp` was always `0`)
/// this was the GENERAL case for every same-name/same-arity overload pair;
/// post-Task-2 (`sig_fp` is a real parameter-type fingerprint) a genuine
/// overload pair almost always gets distinct ids and never reaches this
/// function at all — a run surviving here with ≥2 distinct keys now means
/// the two overloads' `sig_fp`s themselves collided (a residual fnv1a
/// fingerprint collision `source_param_sig_fp` cannot rule out; see
/// `sig_fp.rs`'s module doc). Neither survivor collapses (both are real, both
/// are kept — this function's whole point), but EVERY survivor in such a run
/// is marked [`RoutineNode::source_overload_aliased`] — a same-id/different-key
/// COLLISION GUARD consumed by
/// `resolver::emit_event_flow_edges`. A TRUE re-parse duplicate (one
/// distinct key in the run) collapses to a single unmarked survivor, same as
/// always. The two marker fields are mutually exclusive by construction:
/// this branch only ever fires for `r.tier != TrustTier::SymbolOnly`, whose
/// `param_sig_key` is never the ABI-only empty-key sentinel this function
/// collapses on.
fn dedup_routines_preserving_genuine_overloads(routines: &mut Vec<RoutineNode>) {
    // Pass 1 decides, per entry, whether it survives and how it is marked
    // (borrowing); pass 2 MOVES the survivors out, so nothing is cloned.
    #[derive(Clone, Copy)]
    enum Verdict {
        Drop,
        Keep,
        KeepCollapsed,
        KeepAliased,
    }
    let mut verdicts: Vec<Verdict> = Vec::with_capacity(routines.len());
    let mut contexts = Vec::with_capacity(routines.len());
    let mut i = 0;
    while i < routines.len() {
        let mut j = i + 1;
        while j < routines.len() && routines[j].id == routines[i].id {
            j += 1;
        }
        // Count raw entries per param-signature key within this run FIRST —
        // a survivor is only markable once the true raw count sharing its
        // key is known (needed for the ABI empty-key case above). The
        // number of DISTINCT keys in the run (`sig_counts.len()`) is what
        // the source-overload-alias marking below needs: >=2 distinct keys
        // surviving under one id is a genuine aliased overload pair.
        let mut sig_counts: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        for r in &routines[i..j] {
            *sig_counts.entry(r.param_sig_key.as_str()).or_insert(0) += 1;
        }
        let distinct_key_count = sig_counts.len();
        // Preserve first-occurrence order for determinism; collapse every
        // later entry in the run that repeats an already-seen param signature.
        let mut seen_sigs: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for r in &routines[i..j] {
            // A collapsed duplicate (the same declaration in two `#if` arms)
            // exists under its own build conditions, so the survivor exists
            // under either: it keeps only the symbols every collapsed copy
            // decides alike (`RoutineNode::preproc_context`).
            contexts.push((!seen_sigs.contains(r.param_sig_key.as_str())).then(|| {
                r.preproc_context
                    .iter()
                    .filter(|e| {
                        routines[i..j]
                            .iter()
                            .filter(|o| o.param_sig_key == r.param_sig_key)
                            .all(|o| o.preproc_context.contains(e))
                    })
                    .cloned()
                    .collect::<Box<[_]>>()
            }));
            verdicts.push(if !seen_sigs.insert(r.param_sig_key.as_str()) {
                Verdict::Drop
            } else if r.tier == TrustTier::SymbolOnly && sig_counts[r.param_sig_key.as_str()] >= 2 {
                Verdict::KeepCollapsed
            } else if r.tier != TrustTier::SymbolOnly && distinct_key_count >= 2 {
                Verdict::KeepAliased
            } else {
                Verdict::Keep
            });
        }
        i = j;
    }
    let mut out: Vec<RoutineNode> = Vec::with_capacity(routines.len());
    for ((mut survivor, verdict), context) in routines.drain(..).zip(verdicts).zip(contexts) {
        if let Some(context) = context {
            survivor.preproc_context = context;
        }
        match verdict {
            Verdict::Drop => continue,
            Verdict::Keep => {}
            Verdict::KeepCollapsed => {
                survivor.abi_overload_collapsed = true;
                // Task 2 (roadmap-closure plan): demote the retained ABI
                // parameter list in LOCKSTEP with the collapse marker —
                // the SAME survivor, the SAME "≥2 raw entries
                // fingerprint-collided" condition. `abi_params` on this
                // survivor belongs to only ONE of the ≥2 real
                // declarations (arbitrary raw-JSON-order choice, same as
                // `return_type`/`return_type_id` above) — the structural
                // guard (`AbiParams::CollapsedUntrusted`) makes reading
                // it for arg-type dispatch impossible by type, never
                // merely a convention a future call site could forget.
                survivor.abi_params = AbiParams::CollapsedUntrusted;
            }
            Verdict::KeepAliased => survivor.source_overload_aliased = true,
        }
        out.push(survivor);
    }
    *routines = out;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use al_syntax::ir::ObjectKind;

    // -----------------------------------------------------------------------
    // T3 (LSP-migration arc) Task 5: layered dep/workspace graph split.
    // -----------------------------------------------------------------------

    use crate::snapshot::compilation::CompilationContext;
    use crate::snapshot::provider::SourceRoot;
    use crate::snapshot::{AppId, AppUnit, Provenance, World};

    fn layer_split_app_id(name: &str) -> AppId {
        AppId {
            guid: String::new(),
            name: name.to_string(),
            publisher: "Test".into(),
            version: "1.0.0.0".into(),
        }
    }

    fn layer_split_source_unit(id: &AppId, tier: TrustTier, files: Vec<(&str, &str)>) -> AppUnit {
        AppUnit {
            id: id.clone(),
            provenance: Provenance {
                app: id.clone(),
                tier,
                content_hash: String::new(),
            },
            source: Some(SourceRoot {
                files: files
                    .into_iter()
                    .map(|(path, text)| crate::snapshot::embedded::SourceFile {
                        virtual_path: path.to_string(),
                        text: text.into(),
                    })
                    .collect::<Vec<_>>()
                    .into(),
                tier,
                content_hash: String::new(),
            }),
            compilation: CompilationContext::default(),
            declared_deps: vec![],
            internals_visible_to: vec![],
            abi: None,
            app_path: None,
            app_stamp: None,
        }
    }

    /// Characterization test (T3 Task 5, Step 1): a hand-built two-app
    /// fixture (a workspace app declaring a dependency on, and granting
    /// `internalsVisibleTo` friendship to, a source-bearing dep app) proves
    /// that manually composing `build_dep_layer` + `assemble_program_graph`
    /// produces the EXACT SAME graph — field by field — as the (now-wrapper)
    /// `build_program_graph` entry point every existing caller already uses.
    #[test]
    fn assemble_program_graph_matches_build_program_graph_field_by_field() {
        let ws_id = layer_split_app_id("Ws");
        let dep_id = layer_split_app_id("Dep");

        let ws_src = r#"
codeunit 50000 "Ws Cu"
{
    procedure Foo()
    begin
    end;
}
"#;
        let dep_src = r#"
codeunit 60000 "Dep Cu"
{
    procedure Bar()
    begin
    end;
}
"#;

        let mut ws_unit_snap =
            layer_split_source_unit(&ws_id, TrustTier::Workspace, vec![("Ws.al", ws_src)]);
        ws_unit_snap.declared_deps = vec![crate::dependencies::AppDependency {
            app_id: String::new(),
            name: dep_id.name.clone(),
            publisher: dep_id.publisher.clone(),
            version: dep_id.version.clone(),
        }];
        ws_unit_snap.internals_visible_to = vec![crate::app_package::FriendApp {
            app_id: String::new(),
            name: dep_id.name.clone(),
            publisher: dep_id.publisher.clone(),
        }];

        let dep_unit_snap = layer_split_source_unit(
            &dep_id,
            TrustTier::EmbeddedSource,
            vec![("Dep.al", dep_src)],
        );

        let snap = AppSetSnapshot {
            apps: vec![ws_unit_snap, dep_unit_snap],
            workspace_app: ws_id.clone(),
            world: World::Closed,
        };

        let cache = AbiCache::new();

        // Reference: the (now-wrapper) production entry point.
        let graph_direct = build_program_graph(&snap, &cache);

        // Split path: parse once, build the dep layer, assemble over the
        // workspace unit — exactly what `build_program_graph_from_parsed`
        // does internally, done here by hand to prove the pieces compose.
        let parsed = parse_snapshot(&snap);
        let dep_layer = build_dep_layer(&snap, &cache, &parsed);
        let ws_unit = parsed
            .iter()
            .find(|u| u.app == snap.workspace_app)
            .expect("workspace ParsedUnit must exist (Ws.al has source)");
        let graph_split = assemble_program_graph(&dep_layer, ws_unit, &snap);

        // Objects: same ids, same order.
        let direct_obj_ids: Vec<_> = graph_direct.objects.iter().map(|o| o.id.clone()).collect();
        let split_obj_ids: Vec<_> = graph_split.objects.iter().map(|o| o.id.clone()).collect();
        assert_eq!(
            direct_obj_ids, split_obj_ids,
            "objects must match id-for-id, in order"
        );
        assert!(
            !direct_obj_ids.is_empty(),
            "fixture must produce real objects"
        );

        // Routines: same ids, same order.
        let direct_routine_ids: Vec<_> =
            graph_direct.routines.iter().map(|r| r.id.clone()).collect();
        let split_routine_ids: Vec<_> = graph_split.routines.iter().map(|r| r.id.clone()).collect();
        assert_eq!(
            direct_routine_ids, split_routine_ids,
            "routines must match id-for-id, in order"
        );
        assert!(
            !direct_routine_ids.is_empty(),
            "fixture must produce real routines"
        );

        // obj_index (private field): compared behaviorally via resolve_object.
        let ws_ref = graph_direct.apps.find(&ws_id).expect("ws app interned");
        assert_eq!(
            graph_direct
                .resolve_object(ws_ref, ObjectKind::Codeunit, "Ws Cu")
                .map(|o| o.id.clone()),
            graph_split
                .resolve_object(ws_ref, ObjectKind::Codeunit, "Ws Cu")
                .map(|o| o.id.clone())
        );

        // apps: identical identity-per-AppRef, in the same interning order.
        for i in 0..snap.apps.len() {
            let r = AppRef(i as u32);
            assert_eq!(
                graph_direct.apps.try_resolve(r),
                graph_split.apps.try_resolve(r),
                "AppRef({i}) must resolve to the same identity in both paths"
            );
        }

        // topology: identical dependency closure for every app.
        for i in 0..snap.apps.len() {
            let r = AppRef(i as u32);
            assert_eq!(
                graph_direct.topology.closure(r),
                graph_split.topology.closure(r),
                "AppRef({i})'s dependency closure must match"
            );
        }

        // friends: identical map (order-independent — sort by key first).
        let mut fd: Vec<_> = graph_direct
            .friends
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect();
        let mut fs: Vec<_> = graph_split
            .friends
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect();
        fd.sort_by_key(|(k, _)| k.0);
        fs.sort_by_key(|(k, _)| k.0);
        assert_eq!(fd, fs, "friends wiring must match");
        assert!(!fd.is_empty(), "fixture must exercise friends wiring");

        assert_eq!(
            graph_direct.abi_ingest_errors.len(),
            graph_split.abi_ingest_errors.len(),
            "abi_ingest_errors count must match"
        );
    }

    /// Lever A: the assembled graph SHARES the dep layer's node lists (no
    /// copy), and a second assembly shares the very same allocation.
    #[test]
    fn assemble_program_graph_shares_dep_nodes_without_copying() {
        let ws_id = layer_split_app_id("Ws3");
        let dep_id = layer_split_app_id("Dep3");
        let dep_src =
            "codeunit 60100 \"Dep3 Cu\"\n{\n    procedure Baz()\n    begin\n    end;\n}\n";
        let ws_src = "codeunit 50100 \"Ws3 Cu\"\n{\n    procedure One()\n    begin\n    end;\n}\n";
        let snap = AppSetSnapshot {
            apps: vec![
                layer_split_source_unit(&ws_id, TrustTier::Workspace, vec![]),
                layer_split_source_unit(
                    &dep_id,
                    TrustTier::EmbeddedSource,
                    vec![("Dep3.al", dep_src)],
                ),
            ],
            workspace_app: ws_id.clone(),
            world: World::Closed,
        };
        let parsed = parse_snapshot(&snap);
        let dep = build_dep_layer(&snap, &AbiCache::new(), &parsed);
        let ws = ParsedUnit {
            app: ws_id.clone(),
            files: vec![crate::snapshot::ParsedFile {
                virtual_path: "Ws3.al".to_string(),
                file: std::sync::Arc::new(al_syntax::parse(ws_src)),
                provenance: Provenance {
                    app: ws_id.clone(),
                    tier: TrustTier::Workspace,
                    content_hash: String::new(),
                },
                text: ws_src.into(),
            }],
        };
        let g1 = assemble_program_graph(&dep, &ws, &snap);
        let g2 = assemble_program_graph(&dep, &ws, &snap);
        assert!(!dep.dep_objects.is_empty() && !dep.dep_routines.is_empty());
        assert!(std::sync::Arc::ptr_eq(
            g1.objects.shared(),
            &dep.dep_objects
        ));
        assert!(std::sync::Arc::ptr_eq(
            g1.routines.shared(),
            &dep.dep_routines
        ));
        assert!(std::sync::Arc::ptr_eq(
            g1.routines.shared(),
            g2.routines.shared()
        ));
        let ws_ref = dep.apps.find(&ws_id).unwrap();
        assert!(!g1.objects.own().is_empty());
        assert!(g1.objects.own().iter().all(|o| o.id.app == ws_ref));
    }

    /// Rung-2 shape (the reason this split exists): build ONE `DepLayer`,
    /// then assemble TWICE with two DIFFERENT workspace `ParsedUnit`s — the
    /// dep-derived population must stay byte-identical while only the
    /// workspace-derived population changes, proving `assemble_program_graph`
    /// genuinely REUSES the dep layer rather than silently re-deriving it.
    #[test]
    fn assemble_program_graph_reuses_dep_layer_across_two_workspace_edits() {
        let ws_id = layer_split_app_id("Ws2");
        let dep_id = layer_split_app_id("Dep2");

        let dep_src = r#"
codeunit 60100 "Dep2 Cu"
{
    procedure Baz()
    begin
    end;
}
"#;
        let ws_unit_snap = layer_split_source_unit(&ws_id, TrustTier::Workspace, vec![]);
        let dep_unit_snap = layer_split_source_unit(
            &dep_id,
            TrustTier::EmbeddedSource,
            vec![("Dep2.al", dep_src)],
        );

        let snap = AppSetSnapshot {
            apps: vec![ws_unit_snap, dep_unit_snap],
            workspace_app: ws_id.clone(),
            world: World::Closed,
        };

        let cache = AbiCache::new();
        let parsed = parse_snapshot(&snap);
        let dep_layer = build_dep_layer(&snap, &cache, &parsed);
        let ws_ref = dep_layer.apps.find(&ws_id).expect("ws app interned");

        let ws_src_v1 = r#"
codeunit 50100 "Ws2 Cu"
{
    procedure One()
    begin
    end;
}
"#;
        let ws_src_v2 = r#"
codeunit 50100 "Ws2 Cu"
{
    procedure One()
    begin
    end;

    procedure Two()
    begin
    end;
}
"#;

        fn ws_parsed_unit(ws_id: &AppId, src: &str) -> ParsedUnit {
            ParsedUnit {
                app: ws_id.clone(),
                files: vec![crate::snapshot::ParsedFile {
                    virtual_path: "Ws2.al".to_string(),
                    file: std::sync::Arc::new(al_syntax::parse(src)),
                    provenance: Provenance {
                        app: ws_id.clone(),
                        tier: TrustTier::Workspace,
                        content_hash: String::new(),
                    },
                    text: src.into(),
                }],
            }
        }

        let ws_unit_v1 = ws_parsed_unit(&ws_id, ws_src_v1);
        let ws_unit_v2 = ws_parsed_unit(&ws_id, ws_src_v2);

        let graph_v1 = assemble_program_graph(&dep_layer, &ws_unit_v1, &snap);
        let graph_v2 = assemble_program_graph(&dep_layer, &ws_unit_v2, &snap);

        // Dep-derived population is byte-identical across both assemblies.
        let dep_obj_ids = |g: &ProgramGraph| {
            g.objects
                .iter()
                .filter(|o| o.id.app != ws_ref)
                .map(|o| o.id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(dep_obj_ids(&graph_v1), dep_obj_ids(&graph_v2));
        assert!(
            !dep_obj_ids(&graph_v1).is_empty(),
            "fixture must produce a real dep object"
        );

        // Workspace-derived population reflects the edit: v2 has an extra routine.
        let ws_routine_names = |g: &ProgramGraph| {
            let mut names: Vec<String> = g
                .routines
                .iter()
                .filter(|r| r.id.object.app == ws_ref)
                .map(|r| r.name.to_string())
                .collect();
            names.sort();
            names
        };
        assert_eq!(ws_routine_names(&graph_v1), vec!["One".to_string()]);
        assert_eq!(
            ws_routine_names(&graph_v2),
            vec!["One".to_string(), "Two".to_string()]
        );
    }

    #[test]
    fn build_program_graph_over_cdo_workspace() {
        let Some(ws) = std::env::var_os("CDO_WS")
            .map(std::path::PathBuf::from)
            .filter(|p| p.exists())
        else {
            return;
        };

        let snap = crate::snapshot::SnapshotBuilder {
            workspace_root: ws,
            local_providers: vec![],
        }
        .build()
        .expect("snapshot");

        let cache = AbiCache::new();
        let g = build_program_graph(&snap, &cache);

        assert!(!g.objects.is_empty(), "expected objects from CDO workspace");

        // Workspace app should have a non-trivial dependency closure.
        let ws_ref = g
            .apps
            .find_by_name(&snap.workspace_app.name)
            .expect("workspace app must be interned");
        let closure = g.topology.closure(ws_ref);
        assert!(closure.len() > 1, "workspace should have ≥1 dependency");
    }

    /// Pin the post-Task-2 `source_overload_aliased` collision-guard-marked
    /// GROUP count on CDO (sigfp-and-ambiguous-reclassification plan, Task 2,
    /// T1-review fold-in). Pre-Task-2 (source `sig_fp` always `0`), CDO
    /// measured 6 primary / 313 whole-program ALIASED groups — every genuine
    /// same-name/same-arity overload pair, since none had distinct ids.
    /// Post-Task-2, a real overload pair gets a real distinct `sig_fp` and no
    /// longer reaches the guard at all; a NONZERO count here now means a
    /// genuine `sig_fp` NORMALIZATION COLLISION survived — a threshold
    /// alert to investigate, never to silently mask (round-1 addendum,
    /// collision-guard observability). `routines` is sorted by
    /// `RoutineNodeId`, so entries sharing one id are adjacent; a "group" is
    /// one maximal run of adjacent marked entries sharing the SAME id.
    #[test]
    fn source_overload_alias_collision_guard_group_count_pinned_on_cdo() {
        let Some(ws) = std::env::var_os("CDO_WS")
            .map(std::path::PathBuf::from)
            .filter(|p| p.exists())
        else {
            return;
        };

        let snap = crate::snapshot::SnapshotBuilder {
            workspace_root: ws,
            local_providers: vec![],
        }
        .build()
        .expect("snapshot");

        let cache = AbiCache::new();
        let g = build_program_graph(&snap, &cache);

        fn count_groups(marked: &[&RoutineNode]) -> usize {
            let mut groups = 0usize;
            let mut i = 0;
            while i < marked.len() {
                let mut j = i + 1;
                while j < marked.len() && marked[j].id == marked[i].id {
                    j += 1;
                }
                groups += 1;
                i = j;
            }
            groups
        }

        let primary_app_ref = g
            .apps
            .find_by_name(&snap.workspace_app.name)
            .expect("workspace app must be interned");

        let all_marked: Vec<&RoutineNode> = g
            .routines
            .iter()
            .filter(|r| r.source_overload_aliased)
            .collect();
        let primary_marked: Vec<&RoutineNode> = all_marked
            .iter()
            .copied()
            .filter(|r| r.id.object.app == primary_app_ref)
            .collect();

        let whole_program_groups = count_groups(&all_marked);
        let primary_groups = count_groups(&primary_marked);

        // Measured on CDO_WS 2026-07-03 (Task 2 landing): both `0` — every
        // real overload pair now gets a distinct sig_fp; zero residual
        // normalization collisions. Re-derive (never silently loosen) if
        // this ever moves — see the doc above.
        const EXPECTED_WHOLE_PROGRAM_GROUPS: usize = 0;
        const EXPECTED_PRIMARY_GROUPS: usize = 0;
        assert_eq!(
            whole_program_groups,
            EXPECTED_WHOLE_PROGRAM_GROUPS,
            "whole-program collision-guard-marked group count moved from the \
             pinned CDO baseline — investigate before re-pinning; marked: {:?}",
            all_marked
                .iter()
                .map(|r| (r.name.clone(), r.param_sig_key.clone()))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            primary_groups,
            EXPECTED_PRIMARY_GROUPS,
            "primary-scoped collision-guard-marked group count moved from the \
             pinned CDO baseline — investigate before re-pinning; marked: {:?}",
            primary_marked
                .iter()
                .map(|r| (r.name.clone(), r.param_sig_key.clone()))
                .collect::<Vec<_>>()
        );
    }

    // -----------------------------------------------------------------------
    // Task 3 review fix: `abi_overload_collapsed` marking
    // -----------------------------------------------------------------------

    use crate::program::node::{ObjKey, ObjectNodeId};

    fn dep_obj_id(app: AppRef, number: i64) -> ObjectNodeId {
        ObjectNodeId {
            app,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(number),
        }
    }

    /// A minimal ABI-tier (`SymbolOnly`) `RoutineNode` — `param_sig_key`
    /// hardcoded empty exactly as `abi_ingest::ingest_abi` produces it.
    fn abi_routine(
        obj: &ObjectNodeId,
        name_lc: &str,
        params_count: usize,
        sig_fp: u64,
    ) -> RoutineNode {
        RoutineNode {
            id: RoutineNodeId {
                object: obj.clone(),
                name_lc: name_lc.into(),
                enclosing_member_lc: None,
                params_count,
                sig_fp,
            },
            name: name_lc.into(),
            is_trigger: false,
            access: Access::Public,
            tier: TrustTier::SymbolOnly,
            event_subscribers: vec![],
            subscriber_instance_manual: false,
            publisher_kind: None,
            include_sender: None,
            abi_routine_kind: None,
            abi_event_kind: None,
            param_sig_key: Default::default(),
            return_type: None,
            return_type_id: None,
            abi_overload_collapsed: false,
            source_overload_aliased: false,
            preproc_context: Box::default(),
            abi_params: AbiParams::Missing,
        }
    }

    /// A minimal SOURCE (`Workspace`) `RoutineNode` carrying a real
    /// `param_sig_key` (content, never hardcoded empty for source) AND an
    /// explicit `sig_fp` — since sigfp-and-ambiguous-reclassification plan
    /// Task 2, SOURCE `sig_fp` is a real (non-zero for non-empty params)
    /// fingerprint, so tests that need to construct a SPECIFIC id relationship
    /// (distinct ids for a normal overload pair; a residual same-id
    /// collision) must control it explicitly rather than hardcoding `0`.
    fn source_routine(
        obj: &ObjectNodeId,
        name_lc: &str,
        params_count: usize,
        param_sig_key: &str,
        sig_fp: u64,
    ) -> RoutineNode {
        RoutineNode {
            id: RoutineNodeId {
                object: obj.clone(),
                name_lc: name_lc.into(),
                enclosing_member_lc: None,
                params_count,
                sig_fp,
            },
            name: name_lc.into(),
            is_trigger: false,
            access: Access::Public,
            tier: TrustTier::Workspace,
            event_subscribers: vec![],
            subscriber_instance_manual: false,
            publisher_kind: None,
            include_sender: None,
            abi_routine_kind: None,
            abi_event_kind: None,
            param_sig_key: param_sig_key.into(),
            return_type: None,
            return_type_id: None,
            abi_overload_collapsed: false,
            source_overload_aliased: false,
            preproc_context: Box::default(),
            abi_params: AbiParams::Missing,
        }
    }

    /// Two raw ABI entries sharing one `RoutineNodeId` (a genuine `sig_fp`
    /// collision — the degraded-Subtype scenario) collapse to ONE survivor,
    /// marked `abi_overload_collapsed`.
    #[test]
    fn abi_sig_fp_collision_marks_survivor_collapsed() {
        let app = AppRef(0);
        let obj = dep_obj_id(app, 60104);
        let mut routines = vec![
            abi_routine(&obj, "get", 1, 777),
            abi_routine(&obj, "get", 1, 777),
        ];
        dedup_routines_preserving_genuine_overloads(&mut routines);

        assert_eq!(
            routines.len(),
            1,
            "two raw entries sharing (object, name, arity, sig_fp) must collapse to one node"
        );
        assert!(
            routines[0].abi_overload_collapsed,
            "the survivor of a ≥2-raw-ABI-entry collapse must be flagged \
             abi_overload_collapsed so a chain type-query declines rather \
             than trust its return type"
        );
        assert_eq!(
            routines[0].abi_params,
            AbiParams::CollapsedUntrusted,
            "Task 2 (roadmap-closure plan): abi_params must demote to \
             CollapsedUntrusted in LOCKSTEP with abi_overload_collapsed — \
             the survivor's retained parameter list is exactly as \
             untrustworthy as its return type"
        );
    }

    /// Whole-graph invariant (Task 2 review fix, Finding 2):
    /// `abi_overload_collapsed` and `abi_params == CollapsedUntrusted` must
    /// stay IN LOCKSTEP for EVERY routine that survives a dedup pass mixing
    /// collapsed and non-collapsed, ABI and SOURCE routines across MULTIPLE
    /// objects — not just the single hand-picked case
    /// `abi_sig_fp_collision_marks_survivor_collapsed` exercises in
    /// isolation. `arg_dispatch::candidate_param_infos_abi`'s structural
    /// `AbiParams::Complete`-only guard is only as sound as this invariant
    /// holding for every routine that reaches it.
    #[test]
    fn abi_overload_collapsed_and_abi_params_stay_in_lockstep_whole_graph() {
        let app = AppRef(0);
        let obj_a = dep_obj_id(app, 60200);
        let obj_b = dep_obj_id(app, 60201);
        let mut routines = vec![
            // A genuine ABI sig_fp collision on obj_a -> must collapse AND demote.
            abi_routine(&obj_a, "get", 1, 777),
            abi_routine(&obj_a, "get", 1, 777),
            // A lone ABI routine on obj_a -> never marked.
            abi_routine(&obj_a, "getcontent", 0, 0),
            // Two distinct-sig_fp ABI routines on obj_b -> neither marked.
            abi_routine(&obj_b, "set", 1, 111),
            abi_routine(&obj_b, "set", 1, 222),
            // A SOURCE re-parse duplicate on obj_b -> collapses but is NEVER
            // marked (the guard is ABI-tier-only); abi_params stays Missing.
            source_routine(&obj_b, "name", 0, "", 0),
            source_routine(&obj_b, "name", 0, "", 0),
        ];
        dedup_routines_preserving_genuine_overloads(&mut routines);

        for r in &routines {
            if r.abi_overload_collapsed {
                assert_eq!(
                    r.abi_params,
                    AbiParams::CollapsedUntrusted,
                    "routine {:?} is abi_overload_collapsed but abi_params is \
                     {:?}, not CollapsedUntrusted — arg_dispatch::candidate_param_infos_abi \
                     could read a collapsed survivor's parameter list as trustworthy",
                    r.id,
                    r.abi_params
                );
            } else {
                assert_ne!(
                    r.abi_params,
                    AbiParams::CollapsedUntrusted,
                    "routine {:?} is NOT abi_overload_collapsed but abi_params \
                     is CollapsedUntrusted anyway — the two markers must move \
                     together",
                    r.id
                );
            }
        }
    }

    /// A SINGLE ABI routine (no collision at all) must NEVER be marked —
    /// this is the `GetContent`-shaped real-world case (CDO's 2 real
    /// resolved chain edges) that must keep resolving after this fix.
    #[test]
    fn abi_single_routine_never_marked_collapsed() {
        let app = AppRef(0);
        let obj = dep_obj_id(app, 60100);
        let mut routines = vec![abi_routine(&obj, "getcontent", 0, 0)];
        dedup_routines_preserving_genuine_overloads(&mut routines);

        assert_eq!(routines.len(), 1);
        assert!(
            !routines[0].abi_overload_collapsed,
            "a lone ABI routine with no id collision must never be marked collapsed"
        );
    }

    /// Two DISTINCT `sig_fp`s (a genuinely non-degenerate ABI overload pair,
    /// e.g. differing OUTER param kind) never collide onto one
    /// `RoutineNodeId` in the first place — both survive as separate nodes,
    /// neither marked (there was no collapse to mark).
    #[test]
    fn abi_distinct_sig_fp_both_survive_unmarked() {
        let app = AppRef(0);
        let obj = dep_obj_id(app, 60103);
        let mut routines = vec![
            abi_routine(&obj, "get", 1, 111),
            abi_routine(&obj, "get", 1, 222),
        ];
        dedup_routines_preserving_genuine_overloads(&mut routines);

        assert_eq!(
            routines.len(),
            2,
            "distinct sig_fp means distinct RoutineNodeId — no collapse at all"
        );
        assert!(routines.iter().all(|r| !r.abi_overload_collapsed));
    }

    /// A genuine SOURCE re-parse duplicate (same declaration, non-empty
    /// matching `param_sig_key`) collapses exactly like before — but is
    /// NEVER marked `abi_overload_collapsed` (only `TrustTier::SymbolOnly`
    /// entries are eligible), since it is content-identical and trustworthy.
    #[test]
    fn source_duplicate_collapses_but_is_never_marked() {
        let app = AppRef(0);
        let obj = dep_obj_id(app, 51200);
        let mut routines = vec![
            source_routine(&obj, "name", 0, "", 0),
            source_routine(&obj, "name", 0, "", 0),
        ];
        dedup_routines_preserving_genuine_overloads(&mut routines);

        assert_eq!(routines.len(), 1);
        assert!(
            !routines[0].abi_overload_collapsed,
            "a SOURCE routine must never be marked, even when it collapses \
             on an empty param_sig_key (a genuine 0-arg re-parse duplicate)"
        );
    }

    // -----------------------------------------------------------------------
    // sigfp-and-ambiguous-reclassification plan, Task 2: the
    // `source_overload_aliased` marker's REFRAMED post-Task-2 role —
    // a same-id/different-`param_sig_key` COLLISION GUARD (T2 Step-1(d)).
    // -----------------------------------------------------------------------

    /// Normal case: two genuine same-name/same-arity SOURCE overloads with
    /// DISTINCT `sig_fp` (the ordinary Task 2 outcome — real overloads never
    /// share an id in the first place) never even reach the same dedup run —
    /// both survive UNMARKED. Mirrors `abi_distinct_sig_fp_both_survive_
    /// unmarked` for the source tier.
    #[test]
    fn source_distinct_sig_fp_overloads_survive_unmarked() {
        let app = AppRef(0);
        let obj = dep_obj_id(app, 51201);
        let mut routines = vec![
            source_routine(&obj, "resolve", 1, "integer", 111),
            source_routine(&obj, "resolve", 1, "text", 222),
        ];
        dedup_routines_preserving_genuine_overloads(&mut routines);

        assert_eq!(
            routines.len(),
            2,
            "distinct sig_fp means distinct RoutineNodeId — no shared run at all"
        );
        assert!(
            routines.iter().all(|r| !r.source_overload_aliased),
            "a normal distinct-id overload pair must NOT be marked \
             source_overload_aliased; got {:?}",
            routines
                .iter()
                .map(|r| r.source_overload_aliased)
                .collect::<Vec<_>>()
        );
    }

    /// Residual collision case: two entries whose `param_sig_key` CONTENT
    /// genuinely differs but whose `sig_fp` nonetheless collides (the
    /// normalization-collision scenario `sig_fp::source_param_sig_fp`'s doc
    /// warns is possible for two SPELLINGS of what a compiler would treat as
    /// the SAME type, e.g. differing internal whitespace — reachable in
    /// practice, see `sig_fp::tests::case_and_whitespace_variants_of_same_
    /// type_collide` for the fingerprint-level proof). Both survive (neither
    /// is a true duplicate), but BOTH are marked `source_overload_aliased`
    /// as a fail-closed COLLISION GUARD — the post-Task-2 role this field
    /// exists for.
    #[test]
    fn source_normalization_collision_marks_both_survivors_collision_guard() {
        let app = AppRef(0);
        let obj = dep_obj_id(app, 51202);
        let mut routines = vec![
            source_routine(&obj, "resolve", 1, "record customer", 999),
            source_routine(&obj, "resolve", 1, "record  customer", 999),
        ];
        dedup_routines_preserving_genuine_overloads(&mut routines);

        assert_eq!(
            routines.len(),
            2,
            "a same-id/different-param_sig_key run is NOT a true duplicate — \
             both entries must survive"
        );
        assert!(
            routines.iter().all(|r| r.source_overload_aliased),
            "every survivor of a same-id/different-param_sig_key collision \
             run must be marked source_overload_aliased (fail-closed); got {:?}",
            routines
                .iter()
                .map(|r| r.source_overload_aliased)
                .collect::<Vec<_>>()
        );
        assert!(
            routines.iter().all(|r| !r.abi_overload_collapsed),
            "the SOURCE collision guard is distinct from the ABI collapse \
             marker — a source routine must never be abi_overload_collapsed"
        );
    }

    // -----------------------------------------------------------------------
    // Task 3 (preprocessor foundations plan): the dedup interplay half of the
    // both-arms union-read pin. `al_syntax::lower` always emits TWO distinct
    // `RoutineDecl`s for a `#if`/`#else`-split procedure (see
    // `al_syntax::lower::tests::preproc_both_arms_distinct_signature_yield_
    // two_routine_decls`); THIS module's `dedup_routines_preserving_genuine_
    // overloads` is what decides whether they survive as two nodes (distinct
    // param types → distinct `sig_fp` → distinct `RoutineNodeId`, never even
    // reach a shared dedup run) or collapse to one (identical param types →
    // identical `sig_fp` → a true re-parse-shaped duplicate).
    // -----------------------------------------------------------------------

    /// Real AL text (not a hand-built fixture) through the FULL
    /// parse → extract_nodes → dedup pipeline: a `#if`/`#else` procedure pair
    /// with DIFFERING parameter types must survive as two distinct, UNMARKED
    /// `RoutineNode`s.
    #[test]
    fn preproc_both_arms_distinct_signature_yield_two_unmarked_source_overloads() {
        let src = r#"
codeunit 50300 "Preproc Overloads"
{
#if SOME_UNDEFINED_SYMBOL
    procedure Bar(X: Integer)
    begin
    end;
#else
    procedure Bar(Y: Text)
    begin
    end;
#endif
}
"#;
        let file = al_syntax::parse(src);
        let mut objects = Vec::new();
        let mut routines = Vec::new();
        extract_nodes(
            AppRef(0),
            &file,
            TrustTier::Workspace,
            &mut objects,
            &mut routines,
        );
        routines.sort_by(|a, b| a.id.cmp(&b.id));
        dedup_routines_preserving_genuine_overloads(&mut routines);

        let bar: Vec<_> = routines.iter().filter(|r| r.id.name_lc == "bar").collect();
        assert_eq!(
            bar.len(),
            2,
            "both #if/#else Bar arms are genuinely different-signature \
             overloads — both must survive; got: {:?}",
            bar.iter().map(|r| &r.param_sig_key).collect::<Vec<_>>()
        );
        assert_ne!(
            bar[0].id.sig_fp, bar[1].id.sig_fp,
            "Integer vs Text params must fingerprint distinctly"
        );
        assert!(
            bar.iter().all(|r| !r.source_overload_aliased),
            "distinct sig_fp means neither shares a dedup run — never marked"
        );
    }

    /// The other half: IDENTICAL parameter types across both `#if`/`#else`
    /// arms is the union-read's honest "same text, duplicated" case — the two
    /// `RoutineDecl`s share one `RoutineNodeId` (same `sig_fp`) and must
    /// collapse to ONE canonical, UNMARKED survivor (a true duplicate, not an
    /// overload — `source_overload_aliased` must never fire for it).
    #[test]
    fn preproc_same_signature_arms_collapse_to_one_unmarked_survivor() {
        let src = r#"
codeunit 50301 "Preproc Dup Sig"
{
#if SOME_UNDEFINED_SYMBOL
    procedure Baz(X: Integer)
    begin
    end;
#else
    procedure Baz(X: Integer)
    begin
    end;
#endif
}
"#;
        let file = al_syntax::parse(src);
        let mut objects = Vec::new();
        let mut routines = Vec::new();
        extract_nodes(
            AppRef(0),
            &file,
            TrustTier::Workspace,
            &mut objects,
            &mut routines,
        );
        routines.sort_by(|a, b| a.id.cmp(&b.id));
        dedup_routines_preserving_genuine_overloads(&mut routines);

        let baz: Vec<_> = routines.iter().filter(|r| r.id.name_lc == "baz").collect();
        assert_eq!(
            baz.len(),
            1,
            "identical-signature #if/#else arms are the SAME procedure \
             textually duplicated by the union-read — must collapse to one \
             canonical entry, not survive as two"
        );
        assert!(
            !baz[0].source_overload_aliased,
            "a true re-parse-shaped duplicate must never be marked \
             source_overload_aliased (that marker is for genuine overload \
             collisions only)"
        );
    }

    /// Review Focus 3: the same non-primary app present TWICE (workspace
    /// multi-app source AND embedded dependency) must reduce, through the
    /// summaries + Step 4's sort/dedup, to exactly `build_dep_layer`'s nodes,
    /// and the summaries' meta must equal the frozen map taken from the trees.
    #[test]
    fn sibling_app_summaries_reduce_to_the_layer_nodes_and_frozen_meta() {
        use crate::program::dep_summary::tests::{
            DEP_FILES, app_id, old_frozen_tier, unit, ws_unit,
        };
        let (ws, dep) = (app_id("Ws"), app_id("Dep"));
        let snap = AppSetSnapshot {
            apps: vec![
                ws_unit(&ws),
                unit(&dep, TrustTier::EmbeddedSource, &DEP_FILES),
                unit(&dep, TrustTier::Workspace, &DEP_FILES),
            ],
            workspace_app: ws.clone(),
            world: World::Closed,
        };
        let cache = AbiCache::new();
        let parsed = parse_snapshot(&snap);
        let layer = build_dep_layer(&snap, &cache, &parsed);

        let units: Vec<DepUnitSummary> = parsed
            .iter()
            .filter(|u| u.app != snap.workspace_app)
            .map(|u| {
                let app = layer.apps.find(&u.app).unwrap();
                DepUnitSummary {
                    app: u.app.clone(),
                    files: u
                        .files
                        .iter()
                        .map(|pf| {
                            summarize_file(app, pf.provenance.tier, &pf.virtual_path, &pf.file)
                        })
                        .collect(),
                }
            })
            .collect();
        assert_eq!(units.len(), 2, "the sibling must really appear twice");

        let mut objects = Vec::new();
        let mut routines = Vec::new();
        let mut dep_meta = DepMetaMap::new();
        for f in units.iter().flat_map(|u| &u.files) {
            objects.extend(f.objects.iter().cloned());
            routines.extend(f.routines.iter().cloned());
            dep_meta.extend(f.routine_meta.iter().cloned());
        }
        let raw_routines = routines.len();
        objects.sort_by(|a, b| a.id.cmp(&b.id));
        objects.dedup_by(|a, b| a.id == b.id);
        routines.sort_by(|a, b| a.id.cmp(&b.id));
        dedup_routines_preserving_genuine_overloads(&mut routines);

        assert!(routines.len() < raw_routines, "dedup must have fired");
        assert_eq!(objects, *layer.dep_objects);
        assert_eq!(routines, *layer.dep_routines);

        let graph = build_program_graph_from_parsed(&snap, &cache, &parsed);
        let primary = graph.apps.find(&ws).unwrap();
        assert_eq!(dep_meta, old_frozen_tier(&graph, &parsed, primary));
    }
}
