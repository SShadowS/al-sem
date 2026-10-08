//! The **integration-points report** — a dedicated "who-reacts-to-what" slice of
//! the resolved event wiring (`aldump --integration-points`).
//!
//! Every `EventFlow` edge is a publisher event + its bound subscribers. This
//! projects those into a human/agent-readable report scoped to the **workspace
//! app's integration surface** (events the workspace publishes OR subscribes to),
//! with whole-program totals in the summary:
//!
//! - **inbound**  — the workspace subscribes to an external (dep/platform) event
//!   ("what external changes my app hooks into");
//! - **outbound** — an external app subscribes to a workspace event
//!   ("what extension points my app exposes, and who uses them");
//! - **internal** — both ends in the workspace.
//!
//! This is the consumer-facing counterpart to the graphify `raises_event` edges +
//! event hyperedges: the same wiring, presented as a report.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::Path;

use al_syntax::ir::ObjectKind;
use serde::Serialize;

use crate::program::graph::ProgramGraph;
use crate::program::node::{AppRef, ObjectNodeId, RoutineNodeId};
use crate::program::node_extract::{ObjectNode, RoutineNode};
use crate::program::resolve::edge::{Condition, EdgeKind, Evidence, RouteTarget};
use crate::program::resolve::event::PublisherKind;
use crate::program::resolve::full::ClassifiedEdge;

// ---------------------------------------------------------------------------
// Report schema
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct IntegrationReport {
    pub summary: Summary,
    /// Events touching the workspace app (publisher OR ≥1 subscriber in it),
    /// sorted deterministically by publisher (app, object, event).
    pub events: Vec<EventEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub primary_app: String,
    /// All publisher events in the program (including zero-subscriber ones).
    pub total_events: usize,
    /// Publisher events with ≥1 bound subscriber.
    pub wired_events: usize,
    /// Total (publisher → subscriber) pairs.
    pub total_subscriptions: usize,
    pub by_publisher_kind: BTreeMap<String, usize>,
    /// Subscriptions whose subscriber app differs from the publisher app.
    pub cross_app_subscriptions: usize,
    /// Workspace subscribes to an external event.
    pub workspace_inbound: usize,
    /// External app subscribes to a workspace event.
    pub workspace_outbound: usize,
    /// Both ends in the workspace.
    pub workspace_internal: usize,
    /// Events shown in `events` (the workspace surface slice).
    pub workspace_surface_events: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct EventEntry {
    pub publisher: Publisher,
    pub subscribers: Vec<Subscriber>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Publisher {
    pub app: String,
    pub object_type: String,
    pub object: String,
    pub event: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Subscriber {
    pub app: String,
    pub object: String,
    pub procedure: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<&'static str>,
    pub cross_app: bool,
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Build the integration report for a workspace (resolve, then project).
#[must_use]
pub fn report_workspace(workspace_root: &Path) -> Option<IntegrationReport> {
    let (graph, edges, primary) =
        crate::program::resolve::full::resolve_full_program_for_export(workspace_root)?;
    Some(build_report(&graph, &edges, primary))
}

/// Pure projection: `(ProgramGraph, resolved edges)` → integration report.
#[must_use]
pub fn build_report(
    graph: &ProgramGraph,
    edges: &[ClassifiedEdge],
    primary: AppRef,
) -> IntegrationReport {
    let obj_by_id: HashMap<&ObjectNodeId, &ObjectNode> =
        graph.objects.iter().map(|o| (&o.id, o)).collect();
    let rtn_by_id: HashMap<&RoutineNodeId, &RoutineNode> =
        graph.routines.iter().map(|r| (&r.id, r)).collect();

    let mut total_events = 0usize;
    let mut wired_events = 0usize;
    let mut total_subscriptions = 0usize;
    let mut cross_app_subscriptions = 0usize;
    let mut workspace_inbound = 0usize;
    let mut workspace_outbound = 0usize;
    let mut workspace_internal = 0usize;
    let mut by_publisher_kind: BTreeMap<String, usize> = BTreeMap::new();

    let mut events: Vec<EventEntry> = Vec::new();

    for ce in edges {
        if ce.edge.kind != EdgeKind::EventFlow {
            continue;
        }
        total_events += 1;

        let pub_app = ce.edge.from.object.app;
        let pub_kind = rtn_by_id
            .get(&ce.edge.from)
            .and_then(|r| r.publisher_kind)
            .map_or("Unknown", publisher_kind_str);
        *by_publisher_kind.entry(pub_kind.to_string()).or_insert(0) += 1;

        // Real subscribers (non-Unknown route to a routine).
        let mut subscribers: Vec<Subscriber> = Vec::new();
        for r in &ce.edge.routes {
            if matches!(r.evidence, Evidence::Unknown(_)) {
                continue;
            }
            let RouteTarget::Routine(sub_id) = &r.target else {
                continue;
            };
            let sub_app = sub_id.object.app;
            let cross_app = sub_app != pub_app;
            total_subscriptions += 1;
            if cross_app {
                cross_app_subscriptions += 1;
            }
            // Workspace-surface classification.
            match (pub_app == primary, sub_app == primary) {
                (true, true) => workspace_internal += 1,
                (false, true) => workspace_inbound += 1,
                (true, false) => workspace_outbound += 1,
                (false, false) => {}
            }
            subscribers.push(Subscriber {
                app: app_name(graph, sub_app),
                object: object_name(&sub_id.object, &obj_by_id),
                procedure: rtn_by_id
                    .get(sub_id)
                    .map_or_else(|| sub_id.name_lc.to_string(), |r| r.name.clone()),
                conditions: conditions(&r.conditions),
                cross_app,
            });
        }

        if !subscribers.is_empty() {
            wired_events += 1;
        }

        // Keep only events touching the workspace app (its surface), and only if
        // wired — an unsubscribed event is not an integration point.
        let touches_ws = pub_app == primary
            || subscribers
                .iter()
                .any(|s| s.app == primary_name(graph, primary));
        if subscribers.is_empty() || !touches_ws {
            continue;
        }
        subscribers.sort_by(|a, b| {
            (&a.app, &a.object, &a.procedure).cmp(&(&b.app, &b.object, &b.procedure))
        });
        events.push(EventEntry {
            publisher: Publisher {
                app: app_name(graph, pub_app),
                object_type: object_kind_str(ce.edge.from.object.kind).to_string(),
                object: object_name(&ce.edge.from.object, &obj_by_id),
                event: rtn_by_id
                    .get(&ce.edge.from)
                    .map_or_else(|| ce.edge.from.name_lc.to_string(), |r| r.name.clone()),
                kind: pub_kind.to_string(),
            },
            subscribers,
        });
    }

    events.sort_by(|a, b| {
        (&a.publisher.app, &a.publisher.object, &a.publisher.event).cmp(&(
            &b.publisher.app,
            &b.publisher.object,
            &b.publisher.event,
        ))
    });

    IntegrationReport {
        summary: Summary {
            primary_app: primary_name(graph, primary),
            total_events,
            wired_events,
            total_subscriptions,
            by_publisher_kind,
            cross_app_subscriptions,
            workspace_inbound,
            workspace_outbound,
            workspace_internal,
            workspace_surface_events: events.len(),
        },
        events,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn primary_name(graph: &ProgramGraph, primary: AppRef) -> String {
    app_name(graph, primary)
}

fn app_name(graph: &ProgramGraph, app: AppRef) -> String {
    graph
        .apps
        .try_resolve(app)
        .map_or_else(|| format!("app{}", app.0), |id| id.name.clone())
}

fn object_name(oid: &ObjectNodeId, obj_by_id: &HashMap<&ObjectNodeId, &ObjectNode>) -> String {
    obj_by_id
        .get(oid)
        .map_or_else(|| format!("{:?}", oid.key), |o| o.name.clone())
}

fn publisher_kind_str(pk: PublisherKind) -> &'static str {
    match pk {
        PublisherKind::Integration => "Integration",
        PublisherKind::Business => "Business",
        PublisherKind::Internal => "Internal",
        PublisherKind::Platform => "Platform",
    }
}

fn conditions(conds: &[Condition]) -> Vec<&'static str> {
    let mut out = Vec::new();
    for c in conds {
        out.push(match c {
            Condition::ManualBinding => "manual_binding",
            Condition::SkipOnMissingLicense => "skip_on_missing_license",
            Condition::SkipOnMissingPermission => "skip_on_missing_permission",
            Condition::RunTriggerGuarded => "run_trigger_guarded",
            // Task 3 (sigfp-and-ambiguous-reclassification plan): compiler-forced
            // arm — no producer emits this condition yet (mechanics only).
            Condition::AmbiguousDispatch => "ambiguous_dispatch",
        });
    }
    out
}

fn object_kind_str(k: ObjectKind) -> &'static str {
    match k {
        ObjectKind::Codeunit => "Codeunit",
        ObjectKind::Table => "Table",
        ObjectKind::TableExtension => "TableExtension",
        ObjectKind::Page => "Page",
        ObjectKind::PageExtension => "PageExtension",
        ObjectKind::Report => "Report",
        ObjectKind::ReportExtension => "ReportExtension",
        ObjectKind::Query => "Query",
        ObjectKind::XmlPort => "XmlPort",
        ObjectKind::Enum => "Enum",
        ObjectKind::EnumExtension => "EnumExtension",
        ObjectKind::Interface => "Interface",
        ObjectKind::ControlAddIn => "ControlAddIn",
        ObjectKind::Entitlement => "Entitlement",
        ObjectKind::PermissionSet => "PermissionSet",
        ObjectKind::PermissionSetExtension => "PermissionSetExtension",
        ObjectKind::Profile => "Profile",
        ObjectKind::Other => "Other",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::graph::ObjectIndex;
    use crate::program::node::{AppRegistry, ObjKey};
    use crate::program::node_extract::AbiParams;
    use crate::program::node_extract::Access;
    use crate::program::resolve::edge::{
        CanonicalSpan, DispatchShape, Edge, OpenWorldReason, Route, SetCompleteness, SiteId,
        SourcePos, Witness,
    };
    use crate::program::topology::DependencyGraph;
    use crate::snapshot::{AppId, TrustTier};

    fn app_id(name: &str) -> AppId {
        AppId {
            guid: String::new(),
            name: name.into(),
            publisher: "P".into(),
            version: "1.0.0.0".into(),
        }
    }

    fn obj(app: AppRef, id: i64, name: &str) -> ObjectNode {
        ObjectNode {
            id: ObjectNodeId {
                app,
                kind: ObjectKind::Codeunit,
                key: ObjKey::Id(id),
            },
            name: name.into(),
            declared_id: Some(id),
            extends_target: None,
            implements: vec![],
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

    fn rtn(
        app: AppRef,
        obj_id: i64,
        name: &str,
        params: usize,
        pk: Option<PublisherKind>,
    ) -> RoutineNode {
        RoutineNode {
            id: rid(app, obj_id, name, params),
            name: name.into(),
            is_trigger: false,
            access: Access::Public,
            tier: TrustTier::Workspace,
            event_subscribers: vec![],
            subscriber_instance_manual: false,
            publisher_kind: pk,
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

    fn rid(app: AppRef, obj_id: i64, name: &str, params: usize) -> RoutineNodeId {
        RoutineNodeId {
            object: ObjectNodeId {
                app,
                kind: ObjectKind::Codeunit,
                key: ObjKey::Id(obj_id),
            },
            name_lc: name.to_ascii_lowercase().into(),
            enclosing_member_lc: None,
            params_count: params,
            sig_fp: 0,
        }
    }

    fn src_route(nid: RoutineNodeId) -> Route {
        Route {
            target: RouteTarget::Routine(nid),
            evidence: Evidence::Source,
            conditions: vec![],
            witness: Witness::SourceSpan {
                file: "f.al".into(),
                span: (0, 1),
            },
            receiver_tier: None,
        }
    }

    /// Two apps: `Dep` publishes `OnAfterPost`; `Ws` (primary) subscribes to it
    /// (an inbound integration point).
    #[test]
    fn inbound_workspace_subscription_reported() {
        let mut apps = AppRegistry::default();
        let ws = apps.intern(&app_id("Ws"));
        let dep = apps.intern(&app_id("Dep"));

        let mut objects = vec![obj(dep, 80, "Sales-Post"), obj(ws, 50100, "MySub")];
        objects.sort_by(|a, b| a.id.cmp(&b.id));
        let mut routines = vec![
            rtn(dep, 80, "OnAfterPost", 1, Some(PublisherKind::Integration)),
            rtn(ws, 50100, "HandlePost", 1, None),
        ];
        routines.sort_by(|a, b| a.id.cmp(&b.id));
        let obj_index = ObjectIndex::build(&objects);
        let graph = ProgramGraph {
            apps,
            topology: DependencyGraph::default(),
            objects: objects.into(),
            routines: routines.into(),
            obj_index,
            friends: Default::default(),
            abi_ingest_errors: Default::default(),
            workspace_rows: Default::default(),
        };

        let pubr = rid(dep, 80, "OnAfterPost", 1);
        let edge = Edge {
            from: pubr.clone(),
            site: SiteId {
                caller: pubr.clone(),
                span: CanonicalSpan {
                    unit: "d.al".into(),
                    start: SourcePos { line: 1, col: 1 },
                    end: SourcePos { line: 1, col: 2 },
                },
                callee_fingerprint: 1,
            },
            kind: EdgeKind::EventFlow,
            shape: DispatchShape::Multicast,
            completeness: SetCompleteness::Partial {
                reason: OpenWorldReason::ReverseDependentSubscribers,
            },
            routes: vec![src_route(rid(ws, 50100, "HandlePost", 1))],
        };
        let ce = ClassifiedEdge {
            obligation_id: crate::program::resolve::full::ObligationId::Publisher(pubr),
            edge,
        };

        let rep = build_report(&graph, &[ce], ws);
        assert_eq!(rep.summary.primary_app, "Ws");
        assert_eq!(rep.summary.wired_events, 1);
        assert_eq!(rep.summary.total_subscriptions, 1);
        assert_eq!(rep.summary.workspace_inbound, 1);
        assert_eq!(rep.summary.workspace_outbound, 0);
        assert_eq!(rep.summary.cross_app_subscriptions, 1);
        assert_eq!(rep.events.len(), 1);
        let e = &rep.events[0];
        assert_eq!(e.publisher.app, "Dep");
        assert_eq!(e.publisher.object, "Sales-Post");
        assert_eq!(e.publisher.event, "OnAfterPost");
        assert_eq!(e.publisher.kind, "Integration");
        assert_eq!(e.subscribers.len(), 1);
        assert_eq!(e.subscribers[0].app, "Ws");
        assert_eq!(e.subscribers[0].object, "MySub");
        assert!(e.subscribers[0].cross_app);
    }
}
