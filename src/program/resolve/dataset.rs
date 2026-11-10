//! B6 (engine limits L20): the record operations the PLATFORM performs when a
//! call runs a report, an XmlPort or a query, with no AL code of its own.
//!
//! - A report's dataitems read their tables in every run, print, save or
//!   execute (a `ProcessingOnly` report too).
//! - A query's dataitems read their tables when the query opens (Learn,
//!   "Query.Read() method": "you must call the Open method before the Read
//!   method") or is saved.
//! - An XmlPort import writes each table element's table: an insert when
//!   `AutoSave` is true (the default), and a modify of an existing record when
//!   `AutoUpdate` or `AutoReplace` is true (Learn, "AutoSave property",
//!   "AutoUpdate property", "AutoReplace property"). It looks the record up
//!   first, so it reads the table too. An export reads each table element's
//!   table. `XmlPort.Run` imports or exports as `Direction` allows (Learn,
//!   "Direction property"; the default is `Both`).
//! - Request pages aside: `RunRequestPage` runs no dataset.
//!
//! Learn does not say whether an XmlPort's own insert or modify runs the
//! table's `OnInsert`/`OnModify` trigger. The answer must never turn a
//! possible effect into a "no", so the trigger is a MAY edge here, and the
//! platform table events (raised whatever the RunTrigger) are followed as for
//! any insert or modify.
//!
//! The trigger code inside the object (`OnAfterGetRecord`, `OnBeforeInsertRecord`,
//! …) is reached through the run edge already; these are only the platform's
//! own operations, so nothing is counted twice.
//!
//! An object whose dataset is not known (symbol-only: the symbol data has no
//! dataitems; or a file that did not parse cleanly) gives operations with no
//! table, which every answer reads as "table not resolved", never as "no".

use al_syntax::IdentifierFoldExt;
use al_syntax::ir::ObjectKind;

use crate::program::node::ObjectNodeId;
use crate::program::node_extract::{DatasetItemKind, ObjectNode};
use crate::program::resolve::edge::Route;
use crate::program::resolve::reads::{GraphReads, IndexReads, SurfaceReads};

/// A call that runs an object with a dataset ([`super::full::SiteFacts::dataset_run`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatasetRun {
    pub object: ObjectNodeId,
    pub mode: DatasetMode,
    /// The platform's operations, in item order ([`dataset_effects`]).
    pub effects: Vec<DatasetEffect>,
}

/// What a [`DatasetRun`] does with its object's dataset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatasetMode {
    /// A report run, print, save or execute; a query's `Open` or save.
    Read,
    /// `XmlPort.Import`.
    Import,
    /// `XmlPort.Export`.
    Export,
    /// `XmlPort.Run`: import, export or either, as `Direction` allows.
    XmlPortRun,
}

/// One operation of a [`DatasetRun`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatasetEffect {
    /// The object declaring the item: the run object, or one of its report
    /// extensions.
    pub owner: ObjectNodeId,
    /// The item, an index into `owner`'s `dataset.items`; `None` when the
    /// owner's dataset is not known (the operation then has no table).
    pub item: Option<usize>,
    /// The record operation, as code would spell it: `FindSet`, `Get`,
    /// `Insert` or `Modify`.
    pub op: &'static str,
    /// The property that makes the platform do it (`AutoSave`, …), if any.
    pub property: Option<&'static str>,
    /// The item's table; `None` when it does not resolve or is not known.
    pub table: Option<ObjectNodeId>,
    /// The item's `UseTemporary`: `None` when builds disagree.
    pub temporary: Option<bool>,
    /// The table's `OnInsert`/`OnModify` triggers this operation may run.
    pub triggers: Vec<Route>,
    /// The table's platform-event publishers this operation raises
    /// ([`super::resolver::resolve_table_events`]).
    pub table_events: Vec<Route>,
}

/// `true` for a report method (folded) that runs the report without its
/// request page, as `Run` does: `Execute`, `Print` and the `SaveAs*` family
/// (Learn, "Report data type"). Such a call runs the report's triggers and
/// reads its dataset, so the resolver dispatches it like `Run`
/// (`extract::classify_call` for `Report.X(Report::Y, …)`, the object-typed
/// receiver arm of `resolver::resolve_member_with_args` for `Rep.X(…)`).
#[must_use]
pub fn is_report_output_method(method_lc: &str) -> bool {
    matches!(
        method_lc,
        "execute"
            | "print"
            | "saveas"
            | "saveasexcel"
            | "saveashtml"
            | "saveaspdf"
            | "saveasword"
            | "saveasxml"
    )
}

/// The object kind a call `receiver.method` (both folded) runs when the
/// receiver is the object keyword itself (`Report.SaveAs(Id, …)`,
/// `Codeunit.Run(…)`, `Xmlport.Import(…)`, `Query.SaveAsCsv(…)`): the
/// platform's static run methods, which the resolver treats as object runs
/// (`extract::classify_call`). `None` for anything else.
///
/// Such a call is a platform method: its Integer, Option and Enum arguments
/// (object number, format, flags) are by value, and it cannot rebind a
/// RecordRef or Variant passed to it. Its routes name the object's triggers,
/// or nothing for a dynamic id, so a flow pass that reads call targets would
/// otherwise take it for an unknown call that may rebind its arguments; the
/// RecordRef flow asks this instead (`l5::recref_flow`).
#[must_use]
pub fn keyword_run_kind(receiver_lc: &str, method_lc: &str) -> Option<ObjectKind> {
    let kind = match receiver_lc {
        "codeunit" => ObjectKind::Codeunit,
        "page" => ObjectKind::Page,
        "report" => ObjectKind::Report,
        "xmlport" => ObjectKind::XmlPort,
        "query" => ObjectKind::Query,
        _ => return None,
    };
    // A query has no static `Open`: only an instance opens.
    let static_form = !(kind == ObjectKind::Query && method_lc == "open");
    (static_form && starts_object(kind, method_lc)).then_some(kind)
}

/// THE list of the methods that start an object of `kind` (`method_lc`
/// folded): the call runs the object's entry triggers (and its dataset)
/// instead of a procedure of it. Learn, the "Codeunit", "Page", "Report",
/// "XmlPort" and "Query" data types: a codeunit's `Run`; a page's `Run` and
/// `RunModal`; a report's `Run`, `RunModal`, `Execute`, `Print` and
/// `SaveAs*`; an XmlPort's `Run`, `Import` and `Export`; a query's `Open` and
/// `SaveAsCsv`/`SaveAsJson`/`SaveAsXml`. The keyword forms
/// ([`keyword_run_kind`], `extract::classify_call`) and the variable forms
/// (`resolver::member_starts_object`) both read it, and the resolver's
/// `SiteFacts::starts_object` tells every consumer (B4's commit scope). A
/// variable form of one of these methods may still bind to the object's own
/// procedure; `resolver::member_starts_object` decides that ([`ObjectStart`]).
#[must_use]
pub fn starts_object(kind: ObjectKind, method_lc: &str) -> bool {
    match kind {
        ObjectKind::Codeunit => method_lc == "run",
        ObjectKind::Page => matches!(method_lc, "run" | "runmodal"),
        ObjectKind::Report => {
            matches!(method_lc, "run" | "runmodal") || is_report_output_method(method_lc)
        }
        ObjectKind::XmlPort => matches!(method_lc, "run" | "import" | "export"),
        ObjectKind::Query => dataset_mode(ObjectKind::Query, method_lc).is_some(),
        _ => false,
    }
}

/// Whether one call starts its object (B4), as decided where the call
/// resolves (`full::SiteFacts::starts_object`, carried on the model's
/// `CallEdge::starts_object`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ObjectStart {
    /// The call starts no object: no method of [`starts_object`], or a call
    /// that binds to the object's own procedure of that name.
    #[default]
    No,
    /// The call starts its object: a keyword run, or a variable's start
    /// method that binds to the platform's method.
    Yes,
    /// A variable's start method that binds to the platform's method or to
    /// the object's own procedure of that name, and the argument types do
    /// not tell which (B6, `resolver::RunBinding::Undecided`). Its route is
    /// unresolved (`OverloadAmbiguous`); no reader may take it as a definite
    /// start or as a definite procedure call (B4's commit scope reads it as
    /// a run whose scope MAY follow).
    Undecided,
}

/// The dataset mode of method `method_lc` (folded) on an object of `kind`: the
/// calls that run the object's dataset (Learn: "Report data type", "XmlPort
/// data type", "Query data type").
#[must_use]
pub fn dataset_mode(kind: ObjectKind, method_lc: &str) -> Option<DatasetMode> {
    match (kind, method_lc) {
        (ObjectKind::Report, "run" | "runmodal") => Some(DatasetMode::Read),
        (ObjectKind::Report, m) if is_report_output_method(m) => Some(DatasetMode::Read),
        (ObjectKind::XmlPort, "import") => Some(DatasetMode::Import),
        (ObjectKind::XmlPort, "export") => Some(DatasetMode::Export),
        (ObjectKind::XmlPort, "run") => Some(DatasetMode::XmlPortRun),
        (ObjectKind::Query, "open" | "saveascsv" | "saveasjson" | "saveasxml") => {
            Some(DatasetMode::Read)
        }
        _ => None,
    }
}

/// The platform's operations when `object` runs in `mode`, with the triggers
/// and table events each insert or modify reaches. See the module doc.
#[must_use]
pub fn dataset_effects(
    object: &ObjectNodeId,
    mode: DatasetMode,
    graph: &dyn GraphReads,
    index: &dyn IndexReads,
    surface: &dyn SurfaceReads,
) -> Vec<DatasetEffect> {
    let Some(obj) = graph.object_first(object) else {
        return Vec::new();
    };
    // The objects whose items run: a report's extensions add dataitems that
    // run with it (as their triggers do, `resolver::dispatch_entry_trigger`).
    let mut owners: Vec<&ObjectNode> = vec![obj];
    if obj.id.kind == ObjectKind::Report {
        owners.extend(
            index
                .report_extensions_of(&obj.name.fold_identifier())
                .iter()
                .filter_map(|e| graph.object_first(e)),
        );
    }
    let (import, export) = match mode {
        DatasetMode::Read => (false, false),
        DatasetMode::Import => (true, false),
        DatasetMode::Export => (false, true),
        // A dataset not known has no `Direction` either: both.
        DatasetMode::XmlPortRun if !obj.dataset.known => (true, true),
        DatasetMode::XmlPortRun => (obj.dataset.may_import, obj.dataset.may_export),
    };
    let mut out = Vec::new();
    for owner in owners {
        let push = |out: &mut Vec<DatasetEffect>,
                    item: Option<usize>,
                    op: &'static str,
                    property: Option<&'static str>| {
            let it = item.map(|i| &owner.dataset.items[i]);
            let table = it.and_then(|it| {
                crate::program::resolve::receiver::resolve_source_table_ref(
                    owner.id.clone(),
                    &it.source_table,
                    graph,
                    index,
                )
            });
            let (triggers, table_events) = match table.as_ref().and_then(|t| graph.object_first(t))
            {
                Some(t) if matches!(op, "Insert" | "Modify") => (
                    crate::program::resolve::resolver::resolve_implicit_trigger(
                        op, t, graph, index, surface,
                    )
                    .2,
                    crate::program::resolve::resolver::resolve_table_events(
                        &op.fold_identifier(),
                        t,
                        graph,
                    ),
                ),
                _ => (Vec::new(), Vec::new()),
            };
            out.push(DatasetEffect {
                owner: owner.id.clone(),
                item,
                op,
                property,
                table,
                temporary: it.and_then(|it| it.use_temporary),
                triggers,
                table_events,
            });
        };
        let ops_of = |kind: DatasetItemKind,
                      auto: (Option<bool>, Option<bool>, Option<bool>)|
         -> Vec<(&'static str, Option<&'static str>)> {
            let may = |v: Option<bool>| v != Some(false);
            match kind {
                DatasetItemKind::ReportDataitem | DatasetItemKind::QueryDataitem => {
                    if mode == DatasetMode::Read {
                        vec![("FindSet", None)]
                    } else {
                        Vec::new()
                    }
                }
                DatasetItemKind::XmlPortTableElement => {
                    let mut ops = Vec::new();
                    if export {
                        ops.push(("FindSet", None));
                    }
                    let (save, update, replace) = auto;
                    if import && may(save) {
                        ops.push(("Get", Some("AutoSave")));
                        ops.push(("Insert", Some("AutoSave")));
                        if may(update) {
                            ops.push(("Modify", Some("AutoUpdate")));
                        } else if may(replace) {
                            ops.push(("Modify", Some("AutoReplace")));
                        }
                    }
                    ops
                }
            }
        };
        // What a dataset not known may hold: the kind the object declares.
        let unknown_kind = match obj.id.kind {
            ObjectKind::XmlPort => DatasetItemKind::XmlPortTableElement,
            ObjectKind::Query => DatasetItemKind::QueryDataitem,
            _ => DatasetItemKind::ReportDataitem,
        };
        if !owner.dataset.known || owner.parse_incomplete {
            // Unknown items: every operation the mode allows, with no table.
            for (op, property) in ops_of(unknown_kind, (None, None, None)) {
                push(&mut out, None, op, property);
            }
        }
        for (i, it) in owner.dataset.items.iter().enumerate() {
            for (op, property) in ops_of(it.kind, (it.auto_save, it.auto_update, it.auto_replace)) {
                push(&mut out, Some(i), op, property);
            }
        }
    }
    out
}
