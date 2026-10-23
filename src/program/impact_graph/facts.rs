//! The declaration facts the may-execute rules read (spec §4.4), taken from the
//! owned IR of every source-bearing app: test and handler attributes, variables
//! of a runnable type, `RunObject`/`CardPageId`/`LookupPageId`/`DrillDownPageId`
//! properties and XmlPort table elements. Every object reference is resolved here,
//! from the declaring object's closure, so the projection works on plain ids and
//! its tests can state these facts by hand.

use std::collections::HashMap;

use al_syntax::IdentifierFoldExt;
use al_syntax::ir::{ObjectDecl, ObjectKind};

use crate::program::graph::ProgramGraph;
use crate::program::node::{ObjKey, ObjectNodeId, RoutineNodeId};
use crate::program::node_extract::ObjectRef;
use crate::program::resolve::index::{ObjectRefResolution, ResolveIndex};
use crate::program::resolve::receiver::{ParsedType, classify_type_text, unquote_identifier};
use crate::program::sig_fp::source_routine_node_id;
use crate::snapshot::ParsedUnit;

/// What a declaration may run.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RunTarget {
    /// The one object the reference resolves to.
    Object(ObjectNodeId),
    /// The reference matches more than one object in the closure: any object of
    /// this kind (a run hub).
    AnyOf(ObjectKind),
}

/// Facts of one source routine.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoutineFacts {
    /// Carries `[Test]`.
    pub test: bool,
    /// `[HandlerFunctions('A,B')]` names, folded.
    pub handlers: Vec<String>,
    /// Parameters and locals of a runnable type (`Page`, `Report`, `XmlPort`,
    /// `Query`, `TestPage`, `TestRequestPage`).
    pub runs: Vec<RunTarget>,
}

/// Facts of one source object.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ObjectFacts {
    /// Globals of a runnable type: every routine of the object may run them.
    pub runs: Vec<RunTarget>,
    /// Objects its members open: `RunObject` and `CardPageId`.
    pub opens: Vec<RunTarget>,
    /// A table's `LookupPageId` and `DrillDownPageId` pages.
    pub lookup_pages: Vec<RunTarget>,
    /// An XmlPort's table elements: an import inserts their records.
    pub tables: Vec<ObjectNodeId>,
}

/// [`RoutineFacts`] and [`ObjectFacts`] of every source routine and object.
#[derive(Clone, Debug, Default)]
pub struct SourceFacts {
    pub routines: HashMap<RoutineNodeId, RoutineFacts>,
    pub objects: HashMap<ObjectNodeId, ObjectFacts>,
}

/// Read the facts from every unit in `units` (the workspace and the dependency
/// bodies). Units whose app is not in `graph` are skipped.
#[must_use]
pub fn extract_source_facts(
    graph: &ProgramGraph,
    index: &ResolveIndex,
    units: &[&ParsedUnit],
) -> SourceFacts {
    let mut out = SourceFacts::default();
    for unit in units {
        let Some(app) = graph.apps.find(&unit.app) else {
            continue;
        };
        for pf in &unit.files {
            for obj in &pf.file.objects {
                let obj_id = ObjectNodeId {
                    app,
                    kind: obj.kind,
                    key: match obj.id {
                        Some(n) => ObjKey::Id(n),
                        None => ObjKey::Name(obj.name.fold_identifier().into()),
                    },
                };
                let of = object_facts(obj, &obj_id, graph, index);
                if of != ObjectFacts::default() {
                    let slot = out.objects.entry(obj_id.clone()).or_default();
                    slot.runs.extend(of.runs);
                    slot.opens.extend(of.opens);
                    slot.lookup_pages.extend(of.lookup_pages);
                    slot.tables.extend(of.tables);
                }
                for routine in &obj.routines {
                    let rf = RoutineFacts {
                        test: routine
                            .attributes
                            .iter()
                            .any(|a| a.eq_ignore_ascii_case("test")),
                        handlers: routine
                            .attributes_parsed
                            .iter()
                            .filter(|a| a.name.eq_ignore_ascii_case("handlerfunctions"))
                            .filter_map(|a| first_string_arg(&pf.file, a))
                            .flat_map(|list| handler_names(&list))
                            .collect(),
                        runs: routine
                            .params
                            .iter()
                            .filter_map(|p| p.ty.as_deref())
                            .chain(routine.locals.iter().filter_map(|v| v.ty.as_deref()))
                            .filter_map(|t| declared_runnable(t, &obj_id, graph, index))
                            .collect(),
                    };
                    if rf != RoutineFacts::default() {
                        let id = source_routine_node_id(obj_id.clone(), routine);
                        let slot = out.routines.entry(id).or_default();
                        slot.test |= rf.test;
                        slot.handlers.extend(rf.handlers);
                        slot.runs.extend(rf.runs);
                    }
                }
            }
        }
    }
    for f in out.routines.values_mut() {
        f.handlers.sort();
        f.handlers.dedup();
        f.runs.sort();
        f.runs.dedup();
    }
    for f in out.objects.values_mut() {
        for v in [&mut f.runs, &mut f.opens, &mut f.lookup_pages] {
            v.sort();
            v.dedup();
        }
        f.tables.sort();
        f.tables.dedup();
    }
    out
}

fn object_facts(
    obj: &ObjectDecl,
    obj_id: &ObjectNodeId,
    graph: &ProgramGraph,
    index: &ResolveIndex,
) -> ObjectFacts {
    let prop = |name: &'static str| {
        obj.properties
            .iter()
            .filter(move |p| p.name == name && !p.value.is_empty())
            .map(|p| p.value.as_str())
    };
    let page_ref = |v: &str| {
        resolve(
            obj_id,
            ObjectKind::Page,
            &property_object_ref(v),
            graph,
            index,
        )
    };
    let mut opens: Vec<RunTarget> = obj
        .run_objects
        .iter()
        .filter_map(|v| runnable_type(v, obj_id, graph, index))
        .collect();
    opens.extend(prop("cardpageid").filter_map(page_ref));
    let lookup_pages = if matches!(obj.kind, ObjectKind::Table | ObjectKind::TableExtension) {
        prop("lookuppageid")
            .chain(prop("drilldownpageid"))
            .filter_map(page_ref)
            .collect()
    } else {
        Vec::new()
    };
    let tables = if obj.kind == ObjectKind::XmlPort {
        obj.dataitems
            .iter()
            .filter_map(|(_, table)| {
                match resolve(
                    obj_id,
                    ObjectKind::Table,
                    &property_object_ref(table),
                    graph,
                    index,
                ) {
                    Some(RunTarget::Object(id)) => Some(id),
                    _ => None,
                }
            })
            .collect()
    } else {
        Vec::new()
    };
    ObjectFacts {
        runs: obj
            .globals
            .iter()
            .filter_map(|v| v.ty.as_deref())
            .filter_map(|t| declared_runnable(t, obj_id, graph, index))
            .collect(),
        opens,
        lookup_pages,
        tables,
    }
}

/// The object kind a `TestPage X` / `TestRequestPage X` type drives (`Page` /
/// `Report`) and the rest of the text; `None` for any other type. Shared with
/// `digest --tests` (`l5::test_coverage`), so both read a test page alike.
pub(crate) fn test_page_target(text: &str) -> Option<(&'static str, &str)> {
    let t = text.trim();
    let (head, rest) = t.split_once(char::is_whitespace).unwrap_or((t, ""));
    match head.to_ascii_lowercase().as_str() {
        "testpage" => Some(("Page", rest)),
        "testrequestpage" => Some(("Report", rest)),
        _ => None,
    }
}

/// The names in `[HandlerFunctions('A,B')]`, folded, from the attribute's
/// decoded first string argument `list` (`A,B`). Each reader decodes that
/// argument from its own structured form, never by scanning the raw text, which
/// would read an apostrophe-quoted name inside a comment. Shared with
/// `digest --tests` (`l5::test_coverage`).
pub(crate) fn handler_names(list: &str) -> Vec<String> {
    list.split(',')
        .map(|n| unquote_identifier(n.trim()).fold_identifier())
        .filter(|n| !n.is_empty())
        .collect()
}

/// The decoded first argument of an IR attribute when it is a string literal.
fn first_string_arg(
    file: &al_syntax::ir::AlFile,
    a: &al_syntax::ir::AttributeIr,
) -> Option<String> {
    let e = file.ir.expr(*a.args.first()?);
    match &e.kind {
        al_syntax::ir::ExprKind::Literal(al_syntax::ir::Literal::Text(t)) => {
            let t = t.trim();
            let inner = t
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
                .unwrap_or(t);
            Some(inner.replace("''", "'"))
        }
        _ => None,
    }
}

/// A property's object reference: `"Customer List"`, `22`, `Page::"X"`.
fn property_object_ref(value: &str) -> ObjectRef {
    let v = value.trim();
    let v = match v.split_once("::") {
        Some((_, rest)) => rest.trim(),
        None => v,
    };
    match v.parse::<i64>() {
        Ok(n) => ObjectRef::Id(n),
        Err(_) => {
            let raw = unquote_identifier(v);
            let normalized_lc = raw.fold_identifier();
            ObjectRef::Name { raw, normalized_lc }
        }
    }
}

/// A variable's type as a may-run target (rule 2): any [`runnable_type`] but a
/// codeunit — the engine models `CuVar.Run()` precisely, and codeunit variables
/// mostly serve procedure calls.
fn declared_runnable(
    text: &str,
    from: &ObjectNodeId,
    graph: &ProgramGraph,
    index: &ResolveIndex,
) -> Option<RunTarget> {
    match runnable_type(text, from, graph, index)? {
        RunTarget::Object(id) if id.kind == ObjectKind::Codeunit => None,
        RunTarget::AnyOf(ObjectKind::Codeunit) => None,
        t => Some(t),
    }
}

/// The run target of a type or `RunObject` text naming a runnable object
/// (`Page X`, `Report 50100`, `TestPage "X"`, `TestRequestPage X` -> its report,
/// `Codeunit X` for `RunObject`); `None` for any other type and for an object
/// the program does not contain.
fn runnable_type(
    text: &str,
    from: &ObjectNodeId,
    graph: &ProgramGraph,
    index: &ResolveIndex,
) -> Option<RunTarget> {
    let t = text.trim();
    let rewritten;
    let t = match test_page_target(t) {
        Some((kind, rest)) => {
            rewritten = format!("{kind} {rest}");
            rewritten.as_str()
        }
        None => t,
    };
    match classify_type_text(t) {
        ParsedType::Object { kind, object_ref } => resolve(from, kind, &object_ref, graph, index),
        _ => None,
    }
}

fn resolve(
    from: &ObjectNodeId,
    kind: ObjectKind,
    r: &ObjectRef,
    graph: &ProgramGraph,
    index: &ResolveIndex,
) -> Option<RunTarget> {
    match index.resolve_object_ref(graph, from.clone(), kind, r) {
        ObjectRefResolution::Unique(id) => Some(RunTarget::Object(id)),
        ObjectRefResolution::Ambiguous => Some(RunTarget::AnyOf(kind)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_names_split_and_fold() {
        assert_eq!(
            handler_names("ConfirmYes, \"Modal Handler\""),
            vec!["confirmyes".to_string(), "modal handler".to_string()]
        );
        assert_eq!(handler_names(""), Vec::<String>::new());
    }

    #[test]
    fn property_refs_take_numbers_names_and_qualified_names() {
        assert_eq!(property_object_ref("22"), ObjectRef::Id(22));
        assert!(matches!(
            property_object_ref("Page::\"Customer List\""),
            ObjectRef::Name { ref normalized_lc, .. } if normalized_lc == "customer list"
        ));
    }
}
